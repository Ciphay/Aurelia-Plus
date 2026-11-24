use anyhow::{Context, Result, anyhow};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use sha1::{Digest, Sha1};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};
use steam_vent::ConnectionTrait;
use steam_vent::connection::Connection;
use steam_vent_proto::steammessages_cloud_steamclient::{
    CCloud_ClientBeginFileUpload_Request, CCloud_ClientBeginFileUpload_Response,
    CCloud_ClientCommitFileUpload_Request, CCloud_ClientCommitFileUpload_Response,
    CCloud_ClientFileDownload_Request, CCloud_ClientFileDownload_Response,
    CCloud_EnumerateUserFiles_Request, CCloud_EnumerateUserFiles_Response,
    ClientCloudFileUploadBlockDetails,
};

#[derive(Debug, Clone)]
pub struct CloudFileEntry {
    pub filename: String,
    pub timestamp: u64,
    pub size: u64,
    pub sha_hash: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConflictPolicy {
    #[default]
    Detect,
    TakeCloud,
    TakeLocal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncDirection {
    Down,
    Up,
    Both,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SyncConflict {
    pub filename: String,
    pub local_path: String,
    pub local_hash: String,
    pub local_size: u64,
    pub local_timestamp: u64,
    pub cloud_hash: String,
    pub cloud_size: u64,
    pub cloud_timestamp: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SyncSkip {
    pub filename: String,
    pub root_token: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SyncFailure {
    pub filename: String,
    pub direction: String,
    pub error: String,
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct SyncOutcome {
    pub downloaded: Vec<String>,
    pub uploaded: Vec<String>,
    pub conflicts: Vec<SyncConflict>,
    pub skipped: Vec<SyncSkip>,
    pub failed: Vec<SyncFailure>,
}

impl SyncOutcome {
    pub fn has_conflicts(&self) -> bool {
        !self.conflicts.is_empty()
    }

    pub fn has_skips(&self) -> bool {
        !self.skipped.is_empty()
    }

    pub fn has_failures(&self) -> bool {
        !self.failed.is_empty()
    }

    pub fn is_incomplete(&self) -> bool {
        self.has_failures() || self.has_skips()
    }

    fn record_failure(&mut self, filename: &str, direction: &str, error: &anyhow::Error) {
        tracing::warn!("cloud sync {direction} failed for '{filename}': {error:#}");
        self.failed.push(SyncFailure {
            filename: filename.to_string(),
            direction: direction.to_string(),
            error: format!("{error:#}"),
        });
    }

    pub fn skipped_tokens(&self) -> Vec<String> {
        let mut tokens: Vec<String> = self
            .skipped
            .iter()
            .filter_map(|s| s.root_token.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        tokens.sort();
        tokens
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct BaselineEntry {
    hash: String,
    timestamp: u64,
    size: u64,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct SyncBaseline {
    files: HashMap<String, BaselineEntry>,
}

impl SyncBaseline {
    fn path_for(appid: u32) -> Result<PathBuf> {
        Ok(crate::core::config::config_dir()?
            .join("cloud_sync")
            .join(format!("{appid}.json")))
    }

    fn load(appid: u32) -> Self {
        let Ok(path) = Self::path_for(appid) else {
            return Self::default();
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn save(&self, appid: u32) {
        let Ok(path) = Self::path_for(appid) else {
            return;
        };
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!("could not create cloud-sync state dir: {e}");
                return;
            }
        }
        match serde_json::to_string_pretty(self) {
            Ok(raw) => {
                if let Err(e) = crate::core::atomic::write_atomic(&path, raw.as_bytes()) {
                    tracing::warn!("could not persist cloud-sync state {}: {e}", path.display());
                }
            }
            Err(e) => tracing::warn!("could not serialize cloud-sync state: {e}"),
        }
    }
}

fn sha1_hex_of_file(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let mut hasher = Sha1::new();
    hasher.update(&bytes);
    Some(hex_lower(&hasher.finalize()))
}

fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

fn hash_eq(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

#[derive(Debug, Clone)]
pub struct UfsSaveSpec {
    pub root: String,
    pub path: String,
    pub pattern: String,
    pub recursive: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct UfsRootOverride {
    pub root: String,
    pub os: String,
    pub os_compare: String,
    pub use_instead: String,
    pub add_path: String,
    pub path_transforms: Vec<(String, String)>,
}

impl UfsRootOverride {
    pub fn applies_to(&self, target_os: &str) -> bool {
        let same = self.os.is_empty() || self.os.eq_ignore_ascii_case(target_os);
        if self.os_compare.trim() == "!=" {
            !same
        } else {
            same
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct UfsConfig {
    pub savefiles: Vec<UfsSaveSpec>,
    pub root_overrides: Vec<UfsRootOverride>,
    pub quota: u64,
    pub max_files: u32,
}

pub struct CloudPathResolver {
    remote_root: PathBuf,
    install_dir: Option<PathBuf>,
    wine_prefix: Option<PathBuf>,
    overrides: Vec<UfsRootOverride>,
    steam_id: Option<u64>,
}

impl CloudPathResolver {
    pub fn new(remote_root: PathBuf, install_dir: Option<PathBuf>) -> Self {
        Self {
            remote_root,
            install_dir,
            wine_prefix: None,
            overrides: Vec::new(),
            steam_id: None,
        }
    }

    #[must_use]
    pub fn with_root_overrides(mut self, overrides: &[UfsRootOverride], target_os: &str) -> Self {
        self.overrides = overrides
            .iter()
            .filter(|o| o.applies_to(target_os))
            .cloned()
            .collect();
        self
    }

    #[must_use]
    pub fn with_steam_id(mut self, steam_id: u64) -> Self {
        self.steam_id = Some(steam_id);
        self
    }

    fn override_for(&self, token: &str) -> Option<&UfsRootOverride> {
        self.overrides
            .iter()
            .find(|o| o.root.eq_ignore_ascii_case(token))
    }

    pub fn transform_path(&self, token: &str, path: &str) -> String {
        let mut out = path.to_string();
        if let Some(o) = self.override_for(token) {
            for (find, replace) in &o.path_transforms {
                if out.contains(find.as_str()) {
                    out = out.replacen(find.as_str(), replace, 1);
                }
            }
        }
        out
    }

    pub fn substitute_ids(&self, path: &str) -> String {
        let Some(id) = self.steam_id else {
            return path.to_string();
        };
        path.replace("{64BitSteamID}", &id.to_string())
            .replace("{Steam3AccountID}", &(id as u32).to_string())
    }

    #[must_use]
    pub fn with_wine_prefix(mut self, wine_prefix: Option<PathBuf>) -> Self {
        self.wine_prefix = wine_prefix;
        self
    }

    pub fn resolve(&self, filename: &str) -> Result<PathBuf> {
        match split_root_token(filename) {
            Some((token, rest)) => {
                let base = self.resolve_root(token).with_context(|| {
                    format!("unsupported Steam Cloud root token '%{token}%' on this platform")
                })?;
                Ok(join_relative(&base, &self.transform_path(token, rest)))
            }
            None => Ok(join_relative(&self.remote_root, filename)),
        }
    }

    pub fn resolve_root(&self, root_token: &str) -> Option<PathBuf> {
        let lookup = |token: &str| {
            resolve_root_token(
                token,
                self.install_dir.as_deref(),
                self.wine_prefix.as_deref(),
            )
        };
        match self.override_for(root_token) {
            Some(o) => {
                let target = if o.use_instead.is_empty() {
                    o.root.as_str()
                } else {
                    o.use_instead.as_str()
                };
                let base = lookup(target)?;
                Some(join_relative(&base, &self.substitute_ids(&o.add_path)))
            }
            None => lookup(root_token),
        }
    }
}

fn discover_local_saves(
    specs: &[UfsSaveSpec],
    resolver: &CloudPathResolver,
) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for spec in specs {
        let Some(base) = resolver.resolve_root(&spec.root) else {
            continue;
        };
        let spec_path = normalize_rel(&resolver.substitute_ids(&spec.path));
        let search_root = join_relative(&base, &resolver.transform_path(&spec.root, &spec_path));
        if !search_root.is_dir() {
            continue;
        }

        let walker = walkdir::WalkDir::new(&search_root).min_depth(1);
        let walker = if spec.recursive {
            walker
        } else {
            walker.max_depth(1)
        };
        for entry in walker
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_type().is_file())
        {
            let name = entry.file_name().to_string_lossy();
            if !glob_matches(&spec.pattern, &name) {
                continue;
            }
            let Ok(rel) = entry.path().strip_prefix(&search_root) else {
                continue;
            };
            let rel = rel.to_string_lossy().replace('\\', "/");
            let cloud_name = if spec_path.is_empty() {
                format!("%{}%{rel}", spec.root)
            } else {
                format!("%{}%{spec_path}/{rel}", spec.root)
            };
            out.push((cloud_name, entry.path().to_path_buf()));
        }
    }
    out
}

/// Match a Steam UFS filename glob (`*`, `?`) against a filename, case-insensitively
/// (Windows save files are matched case-insensitively). An empty/`*` pattern matches
/// everything; an invalid pattern is treated as a match rather than dropping files.
fn glob_matches(pattern: &str, name: &str) -> bool {
    if pattern.is_empty() || pattern == "*" {
        return true;
    }
    let mut re = String::from("(?i)^");
    for ch in pattern.chars() {
        match ch {
            '*' => re.push_str(".*"),
            '?' => re.push('.'),
            c => re.push_str(&regex::escape(&c.to_string())),
        }
    }
    re.push('$');
    regex::Regex::new(&re)
        .map(|r| r.is_match(name))
        .unwrap_or(true)
}

fn split_root_token(filename: &str) -> Option<(&str, &str)> {
    let after = filename.strip_prefix('%')?;
    let end = after.find('%')?;
    Some((&after[..end], &after[end + 1..]))
}

fn normalize_rel(rel: &str) -> String {
    rel.split(['/', '\\'])
        .filter(|c| !c.is_empty() && *c != "." && *c != "..")
        .collect::<Vec<_>>()
        .join("/")
}

/// Join a `/`- or `\`-separated cloud-relative path onto `base`, skipping empty,
/// `.` and `..` components so a crafted filename can't escape the base directory.
fn join_relative(base: &Path, rel: &str) -> PathBuf {
    let mut out = base.to_path_buf();
    for comp in rel.split(['/', '\\']) {
        if !comp.is_empty() && comp != "." && comp != ".." {
            out.push(comp);
        }
    }
    out
}

/// Resolve a Steam Cloud root token (without the surrounding `%`) to an OS directory.
/// `%GameInstall%` needs the game's install directory; the rest derive from the
#[cfg(windows)]
fn resolve_root_token(
    token: &str,
    install_dir: Option<&Path>,
    _wine_prefix: Option<&Path>,
) -> Option<PathBuf> {
    let user = || std::env::var_os("USERPROFILE").map(PathBuf::from);
    match token {
        "GameInstall" => install_dir.map(Path::to_path_buf),
        "WinMyDocuments" | "WinDocuments" => user().map(|u| u.join("Documents")),
        "WinAppDataLocal" => std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| user().map(|u| u.join("AppData").join("Local"))),
        "WinAppDataLocalLow" => user().map(|u| u.join("AppData").join("LocalLow")),
        "WinAppDataRoaming" => std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| user().map(|u| u.join("AppData").join("Roaming"))),
        "WinSavedGames" => user().map(|u| u.join("Saved Games")),
        _ => None,
    }
}

#[cfg(not(windows))]
fn resolve_root_token(
    token: &str,
    install_dir: Option<&Path>,
    wine_prefix: Option<&Path>,
) -> Option<PathBuf> {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    let win_user = || wine_prefix.map(|p| p.join("drive_c/users/steamuser"));
    match token {
        "GameInstall" => install_dir.map(Path::to_path_buf),
        "LinuxHome" => home(),
        "LinuxXdgDataHome" => std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home().map(|h| h.join(".local/share"))),
        "LinuxXdgConfigHome" => std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home().map(|h| h.join(".config"))),
        "WinMyDocuments" | "WinDocuments" => win_user().map(|u| u.join("Documents")),
        "WinAppDataLocal" => win_user().map(|u| u.join("AppData/Local")),
        "WinAppDataLocalLow" => win_user().map(|u| u.join("AppData/LocalLow")),
        "WinAppDataRoaming" => win_user().map(|u| u.join("AppData/Roaming")),
        "WinSavedGames" => win_user().map(|u| u.join("Saved Games")),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlannedAction {
    Skip,
    Download,
    Upload,
    Conflict,
}

struct LocalInfo {
    hash: String,
    size: u64,
    timestamp: u64,
}

impl LocalInfo {
    fn as_baseline(&self) -> BaselineEntry {
        BaselineEntry {
            hash: self.hash.clone(),
            timestamp: self.timestamp,
            size: self.size,
        }
    }
}

fn read_local_info(path: &Path) -> Option<LocalInfo> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let timestamp = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let hash = sha1_hex_of_file(path)?;
    Some(LocalInfo {
        hash,
        size: meta.len(),
        timestamp,
    })
}

fn cloud_baseline(cloud: &CloudFileEntry, local_path: &Path) -> BaselineEntry {
    let hash = cloud
        .sha_hash
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| sha1_hex_of_file(local_path))
        .unwrap_or_default();
    BaselineEntry {
        hash,
        timestamp: cloud.timestamp,
        size: cloud.size,
    }
}

fn plan_action(
    baseline: Option<&BaselineEntry>,
    local: Option<&LocalInfo>,
    cloud: Option<&CloudFileEntry>,
) -> PlannedAction {
    match (local, cloud) {
        (Some(l), Some(c)) => match c.sha_hash.as_deref() {
            Some(ch) if hash_eq(&l.hash, ch) => PlannedAction::Skip,
            Some(ch) => {
                let local_changed = baseline.is_none_or(|b| !hash_eq(&b.hash, &l.hash));
                let cloud_changed = baseline.is_none_or(|b| !hash_eq(&b.hash, ch));
                match (local_changed, cloud_changed) {
                    (true, true) => PlannedAction::Conflict,
                    (true, false) => PlannedAction::Upload,
                    (false, true) => PlannedAction::Download,
                    (false, false) => PlannedAction::Skip,
                }
            }
            None => {
                if l.size == c.size && l.timestamp == c.timestamp {
                    PlannedAction::Skip
                } else if l.timestamp >= c.timestamp {
                    PlannedAction::Upload
                } else {
                    PlannedAction::Download
                }
            }
        },
        (Some(_), None) => PlannedAction::Upload,
        (None, Some(_)) => PlannedAction::Download,
        (None, None) => PlannedAction::Skip,
    }
}

fn set_file_mtime_secs(path: &Path, secs: u64) {
    let mtime = filetime::FileTime::from_unix_time(secs as i64, 0);
    if let Err(e) = filetime::set_file_mtime(path, mtime) {
        tracing::warn!("could not set mtime on {}: {e}", path.display());
    }
}

pub struct CloudClient {
    connection: Connection,
    steam_id: u64,
    http: reqwest::Client,
}

const CLOUD_RPC_TIMEOUT: Duration = Duration::from_secs(30);
const CLOUD_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const CLOUD_READ_TIMEOUT: Duration = Duration::from_secs(60);
const TRANSFER_BACKOFF: [Duration; 3] = [
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
];
const UPLOAD_FLOOR_RATE: u64 = 32 * 1024;
const UPLOAD_MIN_DEADLINE: Duration = Duration::from_secs(60);

fn cloud_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("aurelia")
        .connect_timeout(CLOUD_CONNECT_TIMEOUT)
        .read_timeout(CLOUD_READ_TIMEOUT)
        .build()
        .unwrap_or_else(|e| {
            tracing::warn!("cloud HTTP client fallback: {e}");
            reqwest::Client::new()
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransferFailure {
    Connect,
    Timeout,
    Body,
    Status(u16),
    Other,
}

fn transfer_failure(err: &anyhow::Error) -> TransferFailure {
    for cause in err.chain() {
        if let Some(e) = cause.downcast_ref::<reqwest::Error>() {
            return if e.is_timeout() {
                TransferFailure::Timeout
            } else if e.is_connect() {
                TransferFailure::Connect
            } else if let Some(status) = e.status() {
                TransferFailure::Status(status.as_u16())
            } else if e.is_body() || e.is_decode() {
                TransferFailure::Body
            } else {
                TransferFailure::Other
            };
        }
    }
    TransferFailure::Other
}

fn is_transient(failure: TransferFailure) -> bool {
    match failure {
        TransferFailure::Connect | TransferFailure::Timeout | TransferFailure::Body => true,
        TransferFailure::Status(code) => code == 429 || (500..600).contains(&code),
        TransferFailure::Other => false,
    }
}

fn http_error(e: reqwest::Error) -> anyhow::Error {
    anyhow::Error::new(e.without_url())
}

fn upload_deadline(len: usize) -> Duration {
    let secs = (len as u64).div_ceil(UPLOAD_FLOOR_RATE);
    Duration::from_secs(secs).max(UPLOAD_MIN_DEADLINE)
}

async fn retry_transfer<T, F, Fut>(what: &str, delays: &[Duration], mut op: F) -> Result<T>
where
    F: FnMut(usize) -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut attempt = 0usize;
    loop {
        attempt += 1;
        match op(attempt).await {
            Ok(v) => return Ok(v),
            Err(e) => {
                let Some(delay) = delays.get(attempt - 1) else {
                    return Err(e);
                };
                if !is_transient(transfer_failure(&e)) {
                    return Err(e);
                }
                tracing::warn!("{what}: attempt {attempt} failed ({e:#}); retrying in {delay:?}");
                tokio::time::sleep(*delay).await;
            }
        }
    }
}

impl CloudClient {
    pub fn new(mut connection: Connection) -> Self {
        let steam_id = u64::from(connection.steam_id());
        connection.set_timeout(CLOUD_RPC_TIMEOUT);
        Self {
            connection,
            steam_id,
            http: cloud_http_client(),
        }
    }

    pub fn steam_id(&self) -> u64 {
        self.steam_id
    }

    pub async fn get_file_list(&self, appid: u32) -> Result<Vec<CloudFileEntry>> {
        let request = CCloud_EnumerateUserFiles_Request {
            appid: Some(appid),
            extended_details: Some(true),
            ..Default::default()
        };

        let response: CCloud_EnumerateUserFiles_Response = self
            .connection
            .service_method(request)
            .await
            .context("failed calling Cloud.EnumerateUserFiles")?;

        Ok(response
            .files
            .into_iter()
            .map(|file| CloudFileEntry {
                filename: file.filename().to_string(),
                timestamp: file.timestamp(),
                size: u64::from(file.file_size()),
                sha_hash: file.file_sha,
            })
            .collect())
    }

    pub async fn sync_down(&self, appid: u32, resolver: &CloudPathResolver) -> Result<SyncOutcome> {
        self.sync(
            appid,
            resolver,
            &[],
            SyncDirection::Down,
            ConflictPolicy::Detect,
        )
        .await
    }

    pub async fn sync_up(
        &self,
        appid: u32,
        resolver: &CloudPathResolver,
        specs: &[UfsSaveSpec],
    ) -> Result<SyncOutcome> {
        self.sync(
            appid,
            resolver,
            specs,
            SyncDirection::Up,
            ConflictPolicy::Detect,
        )
        .await
    }

    pub async fn sync(
        &self,
        appid: u32,
        resolver: &CloudPathResolver,
        specs: &[UfsSaveSpec],
        direction: SyncDirection,
        policy: ConflictPolicy,
    ) -> Result<SyncOutcome> {
        let remote_map: HashMap<String, CloudFileEntry> = self
            .get_file_list(appid)
            .await?
            .into_iter()
            .map(|e| (e.filename.clone(), e))
            .collect();

        let mut names: BTreeSet<String> = remote_map.keys().cloned().collect();
        let mut local_only: HashMap<String, PathBuf> = HashMap::new();
        for (cloud_name, local_path) in discover_local_saves(specs, resolver) {
            if !remote_map.contains_key(&cloud_name) {
                names.insert(cloud_name.clone());
                local_only.insert(cloud_name, local_path);
            }
        }

        let mut baseline = SyncBaseline::load(appid);
        let mut outcome = SyncOutcome::default();

        for name in names {
            let cloud = remote_map.get(&name);
            let local_path = match local_only.get(&name) {
                Some(p) => p.clone(),
                None => match resolver.resolve(&name) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!("skipping cloud file '{name}': {e:#}");
                        outcome.skipped.push(SyncSkip {
                            filename: name.clone(),
                            root_token: split_root_token(&name).map(|(t, _)| t.to_string()),
                            reason: format!("{e:#}"),
                        });
                        continue;
                    }
                },
            };
            let local = read_local_info(&local_path);

            match plan_action(baseline.files.get(&name), local.as_ref(), cloud) {
                PlannedAction::Skip => {
                    if let (Some(l), Some(_)) = (&local, cloud) {
                        baseline.files.insert(name.clone(), l.as_baseline());
                    }
                }
                PlannedAction::Download => {
                    if direction == SyncDirection::Up {
                        continue;
                    }
                    let Some(c) = cloud else { continue };
                    self.apply_download(appid, &name, &local_path, c, &mut baseline, &mut outcome)
                        .await;
                }
                PlannedAction::Upload => {
                    if direction == SyncDirection::Down {
                        continue;
                    }
                    let Some(l) = &local else { continue };
                    self.apply_upload(appid, &name, &local_path, l, &mut baseline, &mut outcome)
                        .await;
                }
                PlannedAction::Conflict => {
                    let (Some(l), Some(c)) = (&local, cloud) else {
                        continue;
                    };
                    match policy {
                        ConflictPolicy::Detect => {
                            outcome.conflicts.push(SyncConflict {
                                filename: name.clone(),
                                local_path: local_path.to_string_lossy().into_owned(),
                                local_hash: l.hash.clone(),
                                local_size: l.size,
                                local_timestamp: l.timestamp,
                                cloud_hash: c.sha_hash.clone().unwrap_or_default(),
                                cloud_size: c.size,
                                cloud_timestamp: c.timestamp,
                            });
                        }
                        ConflictPolicy::TakeCloud => {
                            self.apply_download(
                                appid,
                                &name,
                                &local_path,
                                c,
                                &mut baseline,
                                &mut outcome,
                            )
                            .await;
                        }
                        ConflictPolicy::TakeLocal => {
                            self.apply_upload(
                                appid,
                                &name,
                                &local_path,
                                l,
                                &mut baseline,
                                &mut outcome,
                            )
                            .await;
                        }
                    }
                }
            }
        }

        baseline.save(appid);
        Ok(outcome)
    }

    async fn apply_download(
        &self,
        appid: u32,
        name: &str,
        local_path: &Path,
        cloud: &CloudFileEntry,
        baseline: &mut SyncBaseline,
        outcome: &mut SyncOutcome,
    ) {
        match self.download_to(appid, name, local_path, cloud).await {
            Ok(()) => {
                baseline
                    .files
                    .insert(name.to_string(), cloud_baseline(cloud, local_path));
                outcome.downloaded.push(name.to_string());
            }
            Err(e) => outcome.record_failure(name, "down", &e),
        }
    }

    async fn apply_upload(
        &self,
        appid: u32,
        name: &str,
        local_path: &Path,
        local: &LocalInfo,
        baseline: &mut SyncBaseline,
        outcome: &mut SyncOutcome,
    ) {
        match self
            .upload_from(appid, name, local_path, local.timestamp)
            .await
        {
            Ok(()) => {
                baseline.files.insert(name.to_string(), local.as_baseline());
                outcome.uploaded.push(name.to_string());
            }
            Err(e) => outcome.record_failure(name, "up", &e),
        }
    }

    async fn download_to(
        &self,
        appid: u32,
        filename: &str,
        local_path: &Path,
        cloud: &CloudFileEntry,
    ) -> Result<()> {
        if let Some(parent) = local_path.parent() {
            tokio::fs::create_dir_all(parent).await.with_context(|| {
                format!("failed to create parent directory {}", parent.display())
            })?;
        }
        let what = format!("cloud download '{filename}'");
        let body = retry_transfer(&what, &TRANSFER_BACKOFF, |attempt| {
            tracing::info!("{what} for app {appid} (attempt {attempt})");
            self.download_file(appid, filename, cloud.sha_hash.as_deref())
        })
        .await
        .with_context(|| format!("failed downloading cloud file '{filename}' for app {appid}"))?;
        tokio::fs::write(local_path, &body)
            .await
            .with_context(|| format!("failed writing {}", local_path.display()))?;
        set_file_mtime_secs(local_path, cloud.timestamp);
        Ok(())
    }

    async fn upload_from(
        &self,
        appid: u32,
        filename: &str,
        local_path: &Path,
        local_timestamp: u64,
    ) -> Result<()> {
        let data = tokio::fs::read(local_path)
            .await
            .with_context(|| format!("failed reading {}", local_path.display()))?;
        self.upload_file(appid, filename, local_timestamp, data)
            .await
            .with_context(|| format!("failed uploading cloud file '{filename}' for app {appid}"))
    }

    pub async fn read_cloud_file(&self, appid: u32, filename: &str) -> Result<Option<Vec<u8>>> {
        let files = self.get_file_list(appid).await?;
        let Some(entry) = files.iter().find(|f| f.filename == filename) else { return Ok(None) };
        let what = format!("cloud download '{filename}'");
        let body = retry_transfer(&what, &TRANSFER_BACKOFF, |_| self.download_file(appid, filename, entry.sha_hash.as_deref())).await?;
        Ok(Some(body))
    }

    pub async fn write_cloud_file(&self, appid: u32, filename: &str, data: Vec<u8>) -> Result<()> {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        self.upload_file(appid, filename, now, data).await
    }

    async fn download_file(
        &self,
        appid: u32,
        filename: &str,
        expected_sha: Option<&str>,
    ) -> Result<Vec<u8>> {
        let request = CCloud_ClientFileDownload_Request {
            appid: Some(appid),
            filename: Some(filename.to_string()),
            ..Default::default()
        };

        let response: CCloud_ClientFileDownload_Response = self
            .connection
            .service_method(request)
            .await
            .context("failed calling Cloud.ClientFileDownload")?;

        let url = cloud_transfer_url(
            response.use_https(),
            response.url_host(),
            response.url_path(),
            "ClientFileDownload",
        )?;
        let headers = build_header_map(response.request_headers.iter().map(|h| {
            (
                h.name.as_deref().unwrap_or_default(),
                h.value.as_deref().unwrap_or_default(),
            )
        }))?;

        let raw_file_size = response.raw_file_size();
        let stored_size = response.file_size();

        let http_response = self
            .http
            .get(url)
            .headers(headers)
            .send()
            .await
            .map_err(http_error)
            .context("failed cloud HTTP GET")?
            .error_for_status()
            .map_err(http_error)
            .context("cloud HTTP GET returned failure status")?;

        let body = http_response
            .bytes()
            .await
            .map_err(http_error)
            .context("failed reading cloud download body")?
            .to_vec();

        Ok(maybe_decompress(
            body,
            stored_size,
            raw_file_size,
            expected_sha,
            filename,
        ))
    }

    async fn upload_file(
        &self,
        appid: u32,
        filename: &str,
        timestamp: u64,
        data: Vec<u8>,
    ) -> Result<()> {
        let mut sha = Sha1::new();
        sha.update(&data);
        let file_sha = sha.finalize().to_vec();
        let file_size = u32::try_from(data.len()).context("cloud upload larger than u32")?;

        let begin_request = CCloud_ClientBeginFileUpload_Request {
            appid: Some(appid),
            file_size: Some(file_size),
            raw_file_size: Some(file_size),
            file_sha: Some(file_sha.clone()),
            time_stamp: Some(timestamp),
            filename: Some(filename.to_string()),
            ..Default::default()
        };

        let begin_response: CCloud_ClientBeginFileUpload_Response = self
            .connection
            .service_method(begin_request)
            .await
            .context("failed calling Cloud.ClientBeginFileUpload")?;

        let blocks = self
            .upload_blocks(filename, &data, begin_response.block_requests)
            .await;
        let succeeded = blocks.is_ok();

        let commit_request = CCloud_ClientCommitFileUpload_Request {
            transfer_succeeded: Some(succeeded),
            appid: Some(appid),
            file_sha: Some(file_sha),
            filename: Some(filename.to_string()),
            ..Default::default()
        };

        let commit: Result<CCloud_ClientCommitFileUpload_Response> = self
            .connection
            .service_method(commit_request)
            .await
            .context("failed calling Cloud.ClientCommitFileUpload");
        blocks?;
        let commit_response = commit?;

        if !commit_response.file_committed() {
            return Err(anyhow!(
                "Cloud.ClientCommitFileUpload returned file_committed=false"
            ));
        }

        Ok(())
    }

    async fn upload_blocks(
        &self,
        filename: &str,
        data: &[u8],
        block_requests: Vec<ClientCloudFileUploadBlockDetails>,
    ) -> Result<()> {
        for mut block in block_requests {
            let url = cloud_transfer_url(
                block.use_https(),
                block.url_host(),
                block.url_path(),
                "ClientBeginFileUpload",
            )?;

            let block_offset = usize::try_from(block.block_offset()).unwrap_or(0);
            let block_length = usize::try_from(block.block_length()).unwrap_or(data.len());
            let end = block_offset.saturating_add(block_length).min(data.len());
            let payload = if block.explicit_body_data.is_some() {
                block.take_explicit_body_data()
            } else if block_offset < data.len() {
                data[block_offset..end].to_vec()
            } else {
                data.to_vec()
            };

            let method = match block.http_method() {
                1 => reqwest::Method::PUT,
                2 => reqwest::Method::POST,
                _ => reqwest::Method::PUT,
            };
            let headers = build_header_map(block.request_headers.iter().map(|h| {
                (
                    h.name.as_deref().unwrap_or_default(),
                    h.value.as_deref().unwrap_or_default(),
                )
            }))?;

            let deadline = upload_deadline(payload.len());
            let what = format!("cloud upload block of '{filename}'");
            retry_transfer(&what, &TRANSFER_BACKOFF, |attempt| {
                tracing::info!("{what} ({} bytes, attempt {attempt})", payload.len());
                let request = self
                    .http
                    .request(method.clone(), &url)
                    .headers(headers.clone())
                    .timeout(deadline)
                    .body(payload.clone());
                async move {
                    request
                        .send()
                        .await
                        .map_err(http_error)
                        .context("failed cloud HTTP upload")?
                        .error_for_status()
                        .map_err(http_error)
                        .context("cloud HTTP upload returned failure status")?;
                    Ok(())
                }
            })
            .await?;
        }
        Ok(())
    }
}

const MAX_CLOUD_UNZIPPED: u64 = 1024 * 1024 * 1024;
const MAX_CLOUD_PREALLOC: u32 = 64 * 1024 * 1024;

const ZIP_MAGIC: &[u8; 4] = b"PK\x03\x04";

fn unzip_single_entry(body: &[u8], expected_len: Option<u32>) -> Option<Vec<u8>> {
    unzip_capped(body, expected_len, MAX_CLOUD_UNZIPPED)
}

fn unzip_capped(body: &[u8], expected_len: Option<u32>, max: u64) -> Option<Vec<u8>> {
    if !body.starts_with(ZIP_MAGIC) {
        return None;
    }
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(body)).ok()?;
    if archive.is_empty() {
        return None;
    }
    let entry = archive.by_index(0).ok()?;
    let hint = expected_len.unwrap_or(0).min(MAX_CLOUD_PREALLOC) as usize;
    let mut out = Vec::with_capacity(hint);
    let limit = max.saturating_add(1);
    std::io::Read::read_to_end(&mut std::io::Read::take(entry, limit), &mut out).ok()?;
    if out.len() as u64 > max {
        tracing::warn!("Cloud file unzips past {max} bytes; keeping raw body");
        return None;
    }
    Some(out)
}

fn maybe_decompress(
    body: Vec<u8>,
    stored_size: u32,
    raw_file_size: u32,
    expected_sha: Option<&str>,
    filename: &str,
) -> Vec<u8> {
    let raw_len = (raw_file_size > 0).then_some(raw_file_size);

    if let Some(sha) = expected_sha.map(str::trim).filter(|s| !s.is_empty()) {
        if hash_eq(&hex_lower(&Sha1::digest(&body)), sha) {
            return body;
        }
        match unzip_single_entry(&body, raw_len) {
            Some(inner) if hash_eq(&hex_lower(&Sha1::digest(&inner)), sha) => return inner,
            _ => {
                tracing::warn!(
                    "cloud file '{filename}' did not match its expected hash {sha} either as \
                     received or decompressed; writing it as received"
                );
                return body;
            }
        }
    }

    if raw_file_size > 0 && raw_file_size != stored_size {
        if let Some(inner) = unzip_single_entry(&body, raw_len) {
            if inner.len() as u32 != raw_file_size {
                tracing::warn!(
                    "cloud file '{filename}' decompressed to {} bytes, expected {raw_file_size}",
                    inner.len()
                );
            }
            return inner;
        }
        tracing::warn!("cloud file '{filename}' looked compressed but is not a readable zip");
    }
    body
}

fn cloud_transfer_url(use_https: bool, host: &str, path: &str, what: &str) -> Result<String> {
    if host.is_empty() || path.is_empty() {
        return Err(anyhow!("{what} returned empty URL host/path"));
    }
    let scheme = if use_https { "https" } else { "http" };
    Ok(format!("{scheme}://{host}{path}"))
}

fn build_header_map<'a>(headers: impl Iterator<Item = (&'a str, &'a str)>) -> Result<HeaderMap> {
    let mut map = HeaderMap::new();
    for (name, value) in headers {
        if name.is_empty() {
            continue;
        }

        let header_name = HeaderName::from_bytes(name.as_bytes())
            .with_context(|| format!("invalid header name '{name}'"))?;
        let header_value = HeaderValue::from_str(value)
            .with_context(|| format!("invalid header value for '{name}'"))?;
        map.insert(header_name, header_value);
    }
    Ok(map)
}

pub fn default_cloud_root(steam_id: u64, appid: u32) -> Result<PathBuf> {
    let home = crate::core::config::home_dir()?;
    let account_id = steam_id as u32;
    Ok(home
        .join(".local/share/Steam/userdata")
        .join(account_id.to_string())
        .join(appid.to_string()))
}

#[cfg(test)]
#[path = "cloud_sync_tests.rs"]
mod tests;
