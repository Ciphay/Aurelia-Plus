pub mod client;
mod proto;
mod server;
mod transport;

use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::RwLock as StdRwLock;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use aurelia::core::error::ErrorKind;
use aurelia::steam_client::{Friend, Roster, SteamClient};

pub use server::run_server;

pub fn endpoint_path() -> String {
    transport::endpoint()
}

pub fn systemd_managed() -> bool {
    transport::systemd_managed()
}

#[derive(Debug, Serialize, Deserialize)]
struct Header {
    argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    config_dir: Option<std::path::PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cwd: Option<std::path::PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    env: Vec<(String, String)>,
}

fn own_config_dir() -> Option<std::path::PathBuf> {
    aurelia::core::config::config_dir()
        .ok()
        .map(|d| transport::canonical(&d))
}

fn config_dir_mismatch(
    client: Option<&std::path::Path>,
    own: Option<&std::path::Path>,
) -> Option<String> {
    let (client, own) = (client?, own?);
    if client == own {
        return None;
    }
    Some(format!(
        "this session daemon serves the config directory {}, not {}; set \
         AURELIA_DAEMON_SOCKET to a separate socket or AURELIA_NO_DAEMON=1",
        own.display(),
        client.display()
    ))
}

#[derive(Debug, Serialize, Deserialize)]
struct DaemonInfo {
    version: String,
    pid: u32,
    #[serde(default)]
    build_id: Option<String>,
}

pub(crate) fn current_build_id() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let meta = std::fs::metadata(exe).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(format!("{mtime}-{}", meta.len()))
}

fn write_daemon_info() {
    let info = DaemonInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        pid: std::process::id(),
        build_id: current_build_id(),
    };
    let path = transport::version_marker_path();
    match serde_json::to_vec(&info) {
        Ok(bytes) => {
            if let Err(e) = aurelia::core::atomic::write_atomic(&path, &bytes) {
                tracing::warn!(
                    "daemon: could not write version marker {}: {e}",
                    path.display()
                );
            }
        }
        Err(e) => tracing::warn!("daemon: could not serialize version marker: {e}"),
    }
}

fn read_daemon_info() -> Option<DaemonInfo> {
    if !transport::location_trusted() {
        return None;
    }
    let bytes = std::fs::read(transport::version_marker_path()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn clear_daemon_info() {
    let _ = std::fs::remove_file(transport::version_marker_path());
}

static DAEMON: OnceLock<DaemonState> = OnceLock::new();

const RESTORE_RETRY_BACKOFF: Duration = Duration::from_secs(30);

const OFFLINE_RETRY_FIRST: Duration = Duration::from_secs(2);

const LIVENESS_PROBE_INTERVAL: Duration = Duration::from_secs(60);

const LIVENESS_RECHECK_DELAY: Duration = Duration::from_secs(2);

const ANNOUNCE_NAME_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

struct DaemonState {
    slot: RwLock<Slot>,
    roster: Arc<StdRwLock<Roster>>,
    watcher: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    reconnecting: std::sync::atomic::AtomicBool,
}

struct Slot {
    client: Option<SteamClient>,
    session_mtime: Option<SystemTime>,
    last_failure: Option<Instant>,
    last_error: Option<RestoreFailure>,
    failures: u32,
}

#[derive(Clone)]
struct RestoreFailure {
    kind: ErrorKind,
    retry_after: Option<Duration>,
    reason: String,
}

impl Slot {
    fn record_failure(&mut self, kind: ErrorKind, retry_after: Option<Duration>, reason: String) {
        self.client = None;
        self.failures = self.failures.saturating_add(1);
        self.last_failure = Some(Instant::now());
        self.last_error = Some(RestoreFailure {
            kind,
            retry_after,
            reason,
        });
    }

    fn is_current(&self, mtime: Option<SystemTime>) -> bool {
        if self.session_mtime != mtime {
            return false;
        }
        if self.client.is_some() {
            return true;
        }
        self.last_failure.is_some_and(|at| at.elapsed() < self.backoff())
    }

    fn backoff(&self) -> Duration {
        if self.last_error.as_ref().is_some_and(|e| e.kind == ErrorKind::Offline) {
            return OFFLINE_RETRY_FIRST
                .saturating_mul(1u32 << self.failures.saturating_sub(1).min(8))
                .min(RESTORE_RETRY_BACKOFF);
        }
        RESTORE_RETRY_BACKOFF
    }
}

pub fn report_dead() {
    if let Some(state) = DAEMON.get() {
        tokio::spawn(async move {
            state.invalidate().await;
            state.ensure_session().await;
        });
    }
}

pub fn in_daemon() -> bool {
    DAEMON.get().is_some()
}

fn init_state() -> &'static DaemonState {
    DAEMON.get_or_init(|| DaemonState {
        slot: RwLock::new(Slot {
            client: None,
            session_mtime: None,
            last_failure: None,
            last_error: None,
            failures: 0,
        }),
        reconnecting: std::sync::atomic::AtomicBool::new(false),
        roster: Arc::new(StdRwLock::new(Roster::new())),
        watcher: std::sync::Mutex::new(None),
    })
}

async fn session_mtime() -> Option<SystemTime> {
    let path = aurelia::core::config::session_path().ok()?;
    tokio::fs::metadata(&path).await.ok()?.modified().ok()
}

impl DaemonState {
    fn spawn_reconnect(&self) {
        use std::sync::atomic::Ordering;
        if self.reconnecting.swap(true, Ordering::AcqRel) {
            return;
        }
        tokio::spawn(async {
            loop {
                let Some(state) = DAEMON.get() else { break };
                let wait = state.slot.read().await.backoff();
                tokio::time::sleep(wait).await;
                state.ensure_session().await;
                let s = state.slot.read().await;
                let still_offline = s.client.is_none() && s.last_error.as_ref().is_some_and(|e| e.kind == ErrorKind::Offline);
                drop(s);
                if !still_offline {
                    break;
                }
            }
            if let Some(state) = DAEMON.get() {
                state.reconnecting.store(false, Ordering::Release);
            }
        });
    }

    async fn forget_session(&self) {
        let mut s = self.slot.write().await;
        s.client = None;
        s.last_failure = None;
    }

    async fn ensure_session(&self) {
        let mtime = session_mtime().await;
        {
            let s = self.slot.read().await;
            if s.is_current(mtime) {
                return;
            }
        }
        let mut s = self.slot.write().await;
        if s.is_current(mtime) {
            return;
        }

        if mtime.is_none() {
            s.client = None;
            s.last_failure = None;
            s.last_error = None;
            s.failures = 0;
            s.session_mtime = None;
            return;
        }

        match SteamClient::new() {
            Ok(mut client) => match client.restore_session().await {
                Ok(_) if client.is_authenticated() => {
                    tracing::info!("daemon: shared Steam session established");
                    s.client = Some(client);
                    s.last_failure = None;
                    s.last_error = None;
                    s.failures = 0;
                    self.spawn_watcher(s.client.as_ref().unwrap());
                    s.client.as_ref().unwrap().redeclare_playing().await;
                }
                Ok(_) => {
                    tracing::warn!("daemon: session restore did not authenticate");
                    s.record_failure(
                        ErrorKind::AuthRequired,
                        None,
                        "session restore did not authenticate".to_string(),
                    );
                }
                Err(e) => {
                    tracing::warn!("daemon: could not restore shared session: {e:#}");
                    let c = aurelia::core::error::classify(&e);
                    let offline = c.kind == ErrorKind::Offline;
                    s.record_failure(c.kind, c.retry_after, format!("{e:#}"));
                    if offline {
                        self.spawn_reconnect();
                    }
                }
            },
            Err(e) => {
                tracing::warn!("daemon: could not build Steam client: {e:#}");
                s.record_failure(
                    ErrorKind::Unknown,
                    None,
                    format!("could not build Steam client: {e:#}"),
                );
            }
        }
        s.session_mtime = session_mtime().await.or(mtime);
    }

    fn spawn_watcher(&self, client: &SteamClient) {
        let mut guard = self.watcher.lock().unwrap();
        if let Some(old) = guard.take() {
            old.abort();
        }
        self.roster.write().unwrap().clear();
        let client = client.clone();
        let roster = Arc::clone(&self.roster);
        let handle = tokio::spawn(async move {
            match client.run_friends_watcher(roster).await {
                Ok(aurelia::steam_client::WatcherEnd::LoggedOff(eresult)) => {
                    tracing::warn!(
                        eresult,
                        "daemon: Steam ended the shared session; reconnecting on next command"
                    );
                    if let Some(session) = client.connection().map(|c| c.session_id()) {
                        init_state().drop_session(session).await;
                    }
                }
                Ok(aurelia::steam_client::WatcherEnd::StreamsClosed) => {}
                Err(e) => tracing::warn!("daemon: friends watcher exited: {e:#}"),
            }
        });
        tracing::info!("daemon: friends watcher started");
        *guard = Some(handle);
    }

    async fn announce_and_await_names(&self) {
        let client = self.slot.read().await.client.clone();
        let Some(client) = client else { return };
        let fresh = client.announced_persona().is_none();
        if let Err(e) = client.announce_configured_presence().await {
            tracing::warn!("could not announce presence: {e:#}");
            return;
        }
        if !fresh || client.announced_persona().is_none() {
            return;
        }
        let deadline = tokio::time::Instant::now() + ANNOUNCE_NAME_WAIT;
        while tokio::time::Instant::now() < deadline {
            let unnamed = self
                .roster
                .read()
                .unwrap()
                .values()
                .any(|f| f.relationship == 3 && f.persona_name.is_none());
            if !unnamed {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }

    async fn drop_session(&self, session_id: i32) {
        let current = {
            let s = self.slot.read().await;
            s.client
                .as_ref()
                .and_then(|c| c.connection())
                .map(|c| c.session_id())
        };
        if current == Some(session_id) {
            self.invalidate().await;
        }
    }

    async fn invalidate(&self) {
        let mut s = self.slot.write().await;
        s.client = None;
        s.last_failure = None;
        s.last_error = None;
        s.session_mtime = None;
        if let Some(handle) = self.watcher.lock().unwrap().take() {
            handle.abort();
        }
        self.roster.write().unwrap().clear();
    }

    async fn liveness_loop(&self) {
        loop {
            tokio::time::sleep(LIVENESS_PROBE_INTERVAL).await;

            if self.probe_once().await {
                continue;
            }
            tokio::time::sleep(LIVENESS_RECHECK_DELAY).await;
            if self.probe_once().await {
                continue;
            }

            tracing::warn!("daemon: shared session failed liveness probe; reconnecting");
            self.invalidate().await;
            self.ensure_session().await;
        }
    }

    async fn probe_once(&self) -> bool {
        let client = self.slot.read().await.client.clone();
        match client {
            Some(client) => client.probe_alive().await.is_ok(),
            None => true,
        }
    }
}

fn warmup_delay() -> Duration {
    Duration::from_secs(std::env::var("AURELIA_WARMUP_DELAY").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(20))
}

const WARM_PACE: Duration = Duration::from_millis(300);
const WARM_REPEAT: Duration = Duration::from_secs(30 * 60);

pub async fn warm_account_forever() {
    tokio::time::sleep(warmup_delay()).await;
    let mut failures = 0u32;
    loop {
        let state = init_state();
        state.ensure_session().await;
        let client = state.slot.read().await.client.clone();
        let wait = match client {
            Some(mut client) => {
                failures = 0;
                warm_account_once(state, &mut client).await;
                WARM_REPEAT
            }
            None => {
                failures = failures.saturating_add(1);
                warm_retry_delay(failures)
            }
        };
        tokio::time::sleep(wait).await;
    }
}

const KEEPALIVE_EVERY: Duration = Duration::from_secs(60);
const KEEPALIVE_PROBE: Duration = Duration::from_secs(8);
const KEEPALIVE_FAILURES: u32 = 2;

pub(crate) fn session_is_dead(consecutive_failures: u32) -> bool {
    consecutive_failures >= KEEPALIVE_FAILURES
}

pub async fn keep_session_forever() {
    tokio::time::sleep(warmup_delay()).await;
    let mut failures = 0u32;
    loop {
        tokio::time::sleep(KEEPALIVE_EVERY).await;
        let state = init_state();
        let client = state.slot.read().await.client.clone();
        if let Some(client) = client {
            match tokio::time::timeout(KEEPALIVE_PROBE, client.current_players(480)).await {
                Ok(Ok(_)) => failures = 0,
                _ => {
                    failures += 1;
                    if session_is_dead(failures) {
                        aurelia::bridge::log::bridge_log("daemon: shared session stopped answering, restoring it");
                        state.forget_session().await;
                        failures = 0;
                    }
                }
            }
        }
        state.ensure_session().await;
    }
}

pub(crate) fn warm_retry_delay(failures: u32) -> Duration {
    Duration::from_secs(10).saturating_mul(1u32 << failures.saturating_sub(1).min(8)).min(Duration::from_secs(300))
}

async fn warm_account_once(state: &DaemonState, client: &mut SteamClient) {
    let started = Instant::now();
    let data = client.get_account_data().await;
    let steamid = data.steam_id;
    for _ in 0..40 {
        if !state.roster.read().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let mut wanted: Vec<u64> = state.roster.read().unwrap().values().filter(|f| f.relationship == 3 && f.persona_name.is_none()).map(|f| f.steam_id).collect();
    if data.persona_name.is_none() {
        wanted.push(steamid);
    }
    let warm = aurelia::bridge::warm::global();
    let known = warm.names(steamid, &wanted);
    let missing: Vec<u64> = wanted.iter().copied().filter(|id| !known.contains_key(id)).collect();
    let names = aurelia::bridge::snapshot::public_names(&missing).await;
    warm.put_names(steamid, &names);
    let games = client.fetch_owned_games().await.map(|g| g.len()).unwrap_or(0);
    let mut apps = 0;
    if let Ok(installed) = aurelia::library::scan_installed_app_info().await {
        let mut by_recent: Vec<(u32, u64)> = installed.iter().map(|(id, info)| (*id, info.last_updated)).collect();
        by_recent.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
        for (id, _) in by_recent {
            if aurelia::bridge::snapshot::prewarm_app(client, id).await {
                apps += 1;
                tokio::time::sleep(WARM_PACE).await;
            }
            if !aurelia::steam_client::pics_cache::exists(id) {
                let _ = client.get_product_info(id).await;
            }
            let _ = client.fetch_ufs_config_cached(id).await;
            crate::commands::prewarm_cloud_os(client, id).await;
        }
    }
    aurelia::bridge::log::bridge_log(&format!(
        "daemon: account warmed in {} ms ({} names, {games} games, {apps} apps)",
        started.elapsed().as_millis(),
        names.len()
    ));
}

pub async fn last_restore_error() -> Option<(ErrorKind, Option<Duration>, String)> {
    let state = DAEMON.get()?;
    let failure = state.slot.read().await.last_error.clone()?;
    Some((failure.kind, failure.retry_after, failure.reason))
}

pub async fn shared_restored_client() -> SteamClient {
    let state = init_state();
    state.ensure_session().await;
    let slot = state.slot.read().await;
    match slot.client.as_ref().and_then(|c| c.connection().cloned()) {
        Some(connection) => SteamClient::from_shared(connection),
        None => SteamClient::new().expect("SteamClient::new is infallible"),
    }
}

pub async fn shared_roster() -> Vec<Friend> {
    let state = init_state();
    state.ensure_session().await;
    state.announce_and_await_names().await;
    let mut friends: Vec<Friend> = {
        let roster = state.roster.read().unwrap();
        roster
            .values()
            .filter(|f| f.relationship == 3)
            .cloned()
            .collect()
    };
    friends.sort_by(|a, b| match (&a.persona_name, &b.persona_name) {
        (Some(x), Some(y)) => x.cmp(y).then(a.steam_id.cmp(&b.steam_id)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.steam_id.cmp(&b.steam_id),
    });
    friends
}

pub async fn shared_roster_nowait() -> Vec<Friend> {
    let state = init_state();
    state.ensure_session().await;
    let mut friends: Vec<Friend> = state.roster.read().unwrap().values().filter(|f| f.relationship == 3).cloned().collect();
    friends.sort_by(|a, b| match (&a.persona_name, &b.persona_name) {
        (Some(x), Some(y)) => x.cmp(y).then(a.steam_id.cmp(&b.steam_id)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.steam_id.cmp(&b.steam_id),
    });
    friends
}

pub async fn shared_roster_with_requests() -> Vec<Friend> {
    let state = init_state();
    state.ensure_session().await;
    state.announce_and_await_names().await;
    let roster = state.roster.read().unwrap();
    roster
        .values()
        .filter(|f| f.relationship == 3 || f.relationship == 2)
        .cloned()
        .collect()
}

#[derive(Debug, Default)]
pub struct SessionStatus {
    pub authenticated: bool,
    pub account: Option<String>,
    pub steam_id: Option<u64>,
    pub web_token: bool,
}

async fn status_from_slot(slot: &Slot) -> SessionStatus {
    let persisted = aurelia::core::config::load_session().await.ok();
    let account = persisted.as_ref().and_then(|s| s.account_name.clone());
    let persisted_steam_id = persisted.as_ref().and_then(|s| s.steam_id);
    let web_token = persisted
        .as_ref()
        .is_some_and(|s| s.web_token.as_ref().is_some_and(|t| !t.is_empty()));

    match &slot.client {
        Some(client) => SessionStatus {
            authenticated: true,
            account,
            steam_id: client.steam_id().or(persisted_steam_id),
            web_token,
        },
        None => SessionStatus {
            authenticated: false,
            account,
            steam_id: persisted_steam_id,
            web_token,
        },
    }
}

pub async fn session_status() -> SessionStatus {
    status_after(false).await
}

pub async fn force_reconnect() -> SessionStatus {
    status_after(true).await
}

async fn status_after(invalidate: bool) -> SessionStatus {
    let state = init_state();
    if invalidate {
        state.invalidate().await;
    }
    state.ensure_session().await;
    let slot = state.slot.read().await;
    status_from_slot(&slot).await
}

async fn maybe_refresh_after(argv: &[String]) {
    if argv.iter().any(|a| a == "--health" || a == "--reconnect") {
        return;
    }
    let touches_session = argv.iter().any(|a| a == "login" || a == "logout");
    if let Some(state) = DAEMON.get().filter(|_| touches_session) {
        state.invalidate().await;
        state.ensure_session().await;
    }
}

#[cfg(test)]
#[path = "daemon_tests.rs"]
mod tests;
