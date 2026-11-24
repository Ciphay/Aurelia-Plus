use std::collections::HashSet;

use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};

use crate::core::models::OwnedGame;
use crate::steam_client::SteamClient;

pub const SCHEMA: u32 = 1;

pub type TokenFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>>> + Send>>;

const FRIENDS_WAIT: std::time::Duration = std::time::Duration::from_secs(3);
const NAMES_WAIT: std::time::Duration = std::time::Duration::from_millis(2500);
const MAX_PUBLIC_NAMES: usize = 60;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub steamid: u64,
    pub persona: String,
    pub country: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OwnedApp {
    pub appid: u32,
    pub name: String,
    pub installed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TicketData {
    pub appid: u32,
    pub ownership_ticket_b64: String,
    pub game_connect_token_b64: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DlcEntry {
    pub appid: u32,
    pub owned: bool,
    pub installed: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FriendEntry {
    pub steamid: u64,
    pub persona: String,
    pub state: u32,
    pub game_appid: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppInfo {
    pub appid: u32,
    pub install_dir: String,
    pub cloud_dir: String,
    pub cloud_quota: u64,
    pub build_id: u64,
    pub beta_name: String,
    pub dlc: Vec<DlcEntry>,
    #[serde(default)]
    pub installed_depots: Vec<u32>,
    #[serde(default)]
    pub betas: Vec<BetaBranch>,
    #[serde(default)]
    pub purchase_time: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BetaBranch {
    pub name: String,
    pub description: String,
    pub build_id: u64,
    pub time_updated: u32,
    pub private: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotV1 {
    pub schema: u32,
    pub account: Account,
    pub apps: Vec<OwnedApp>,
    pub ticket: Option<TicketData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<AppInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub friends: Vec<FriendEntry>,
    #[serde(default = "online_default")]
    pub online: bool,
}

fn online_default() -> bool {
    true
}

impl SnapshotV1 {
    pub fn with_friends(mut self, friends: Vec<FriendEntry>) -> Self {
        self.friends = friends;
        self
    }

    pub fn with_app(mut self, app: AppInfo) -> Self {
        self.app = Some(app);
        self
    }
}

pub fn encode_ticket(appid: u32, ownership: &[u8], token: &[u8]) -> TicketData {
    TicketData {
        appid,
        ownership_ticket_b64: STANDARD.encode(ownership),
        game_connect_token_b64: STANDARD.encode(token),
    }
}

pub fn assemble(
    account: Account,
    games: &[OwnedGame],
    installed: &HashSet<u32>,
    ticket: Option<TicketData>,
) -> SnapshotV1 {
    SnapshotV1 {
        schema: SCHEMA,
        account,
        apps: games
            .iter()
            .map(|g| OwnedApp { appid: g.app_id, name: g.name.clone(), installed: installed.contains(&g.app_id) })
            .collect(),
        ticket,
        app: None,
        friends: Vec::new(),
        online: true,
    }
}

pub async fn prewarm_app_now(client: &SteamClient, appid: u32) -> bool {
    let steamid = client.steam_id().unwrap_or(0);
    let warm = super::warm::global();
    if matches!(warm.app_net_stale_ok(steamid, appid), Some((_, true))) {
        return false;
    }
    let Some(guard) = super::warm::begin_refresh(steamid, appid) else { return false };
    let net = fetch_app_net(client, appid).await;
    warm.put_app_net(steamid, appid, net);
    drop(guard);
    true
}

pub async fn prewarm_app(client: &SteamClient, appid: u32) -> bool {
    let steamid = client.steam_id().unwrap_or(0);
    let warm = super::warm::global();
    let recent = warm.app_net_stale_ok(steamid, appid).is_some_and(|(n, _)| super::warm::now_secs().saturating_sub(n.at) < super::warm::PREWARM_FRESH_SECS);
    if recent {
        return false;
    }
    let Some(guard) = super::warm::begin_refresh(steamid, appid) else { return false };
    let net = fetch_app_net(client, appid).await;
    warm.put_app_net(steamid, appid, net);
    drop(guard);
    true
}

async fn app_net(client: &SteamClient, steamid: u64, appid: u32) -> super::warm::AppNet {
    let warm = super::warm::global();
    match warm.app_net_stale_ok(steamid, appid) {
        Some((net, true)) => net,
        Some((net, false)) => {
            if let Some(guard) = super::warm::begin_refresh(steamid, appid) {
                let client = client.clone();
                tokio::spawn(async move {
                    let fresh = fetch_app_net(&client, appid).await;
                    super::warm::global().put_app_net(steamid, appid, fresh);
                    drop(guard);
                });
            }
            net
        }
        None => {
            let net = fetch_app_net(client, appid).await;
            warm.put_app_net(steamid, appid, net.clone());
            net
        }
    }
}

async fn fetch_app_net(client: &SteamClient, appid: u32) -> super::warm::AppNet {
    let _ = client.ensure_purchase_data().await;
    let dlc = async {
        let ids = client.get_extended_app_info(appid).await.map(|e| e.dlcs).unwrap_or_default();
        let (states, names) = tokio::join!(client.dlc_states(appid, &ids), client.app_names(&ids));
        let steamid = client.steam_id().unwrap_or(0);
        let warm = super::warm::global();
        states
            .map(|states| {
                states
                    .into_iter()
                    .map(|d| {
                        let owned = d.owned || warm.package_gives(steamid, d.app_id);
                        DlcEntry { appid: d.app_id, owned, installed: d.installed && !d.disabled, name: names.get(&d.app_id).cloned().unwrap_or_default() }
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let quota = async { client.fetch_ufs_config_cached(appid).await.map(|u| u.quota).unwrap_or(0) };
    let betas = async {
        client
            .fetch_branch_details(appid)
            .await
            .map(|all| all.into_iter().map(|b| BetaBranch { name: b.name, description: b.description, build_id: b.build_id, time_updated: b.time_updated, private: b.private }).collect())
            .unwrap_or_default()
    };
    let (dlc, cloud_quota, betas) = tokio::join!(dlc, quota, betas);
    super::warm::AppNet { dlc, cloud_quota, betas, at: super::warm::now_secs() }
}

async fn build_app_info(client: &SteamClient, appid: u32) -> AppInfo {
    let steamid = client.steam_id().unwrap_or(0);
    let net = app_net(client, steamid, appid).await;
    app_info_with(client, steamid, appid, net).await
}

async fn app_info_with(client: &SteamClient, steamid: u64, appid: u32, net: super::warm::AppNet) -> AppInfo {
    let install_dir = async { client.install_root_for_app(appid).await.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default() };
    let manifest = async {
        match client.appmanifest_path(appid).await {
            Ok(path) => std::fs::read_to_string(path).map(|raw| crate::core::acf::parse_app_manifest(&raw)).ok(),
            Err(_) => None,
        }
    };
    let (install_dir, manifest) = tokio::join!(install_dir, manifest);
    let cloud_dir = crate::library::cloud_sync::default_cloud_root(steamid, appid)
        .map(|p| p.join("remote").to_string_lossy().into_owned())
        .unwrap_or_default();
    let installed_depots = manifest.as_ref().map(|m| m.installed_depots.clone()).unwrap_or_default();
    let (build_id, beta_name) = manifest.map(|m| (m.build_id, m.active_branch)).unwrap_or((0, "public".to_string()));
    let warm = super::warm::global();
    let mut purchase_time = warm.purchase_time(steamid, appid);
    if purchase_time.is_none() {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), client.ensure_purchase_data()).await;
        purchase_time = warm.purchase_time(steamid, appid);
    }
    let purchase_time = purchase_time.unwrap_or(0);
    AppInfo { appid, install_dir, cloud_dir, cloud_quota: net.cloud_quota, build_id, beta_name, dlc: net.dlc, installed_depots, betas: net.betas, purchase_time }
}

pub fn assemble_offline(account: Account, games: &[OwnedGame], installed: &HashSet<u32>, app: Option<AppInfo>) -> SnapshotV1 {
    let mut snapshot = assemble(account, games, installed, None);
    snapshot.app = app;
    snapshot.online = false;
    snapshot
}

pub async fn offline_snapshot(app_id: u32) -> Option<(u64, SnapshotV1)> {
    let saved = crate::core::config::load_session().await.ok()?;
    saved.refresh_token.as_ref()?;
    let steamid = saved.steam_id?;
    let games = crate::core::config::load_library_cache().await.ok()?;
    let installed: HashSet<u32> = crate::library::scan_installed_app_info().await.ok()?.keys().copied().collect();
    if !installed.contains(&app_id) || !games.iter().any(|g| g.app_id == app_id) {
        return None;
    }
    let client = SteamClient::new().ok()?;
    let net = super::warm::global()
        .app_net_stale_ok(steamid, app_id)
        .map(|(n, _)| n)
        .unwrap_or(super::warm::AppNet { dlc: Vec::new(), cloud_quota: 0, betas: Vec::new(), at: 0 });
    let app = app_info_with(&client, steamid, app_id, net).await;
    let persona = saved.persona_name.clone().or(saved.account_name.clone()).unwrap_or_default();
    let account = Account { steamid, persona, country: String::new() };
    Some((steamid, assemble_offline(account, &games, &installed, Some(app))))
}

pub fn friend_entries(list: Vec<crate::steam_client::Friend>) -> Vec<FriendEntry> {
    list.into_iter()
        .filter(|f| f.relationship == crate::steam_client::RELATIONSHIP_FRIEND)
        .map(|f| FriendEntry {
            steamid: f.steam_id,
            persona: f.persona_name.unwrap_or_default(),
            state: f.persona_state.unwrap_or(0),
            game_appid: f.game_app_id.unwrap_or(0),
        })
        .collect()
}

pub async fn read_friends(client: &SteamClient, shared: Option<Vec<crate::steam_client::Friend>>) -> Vec<FriendEntry> {
    friend_entries(read_friend_list(client, shared).await)
}

pub async fn read_friend_list(client: &SteamClient, shared: Option<Vec<crate::steam_client::Friend>>) -> Vec<crate::steam_client::Friend> {
    match shared {
        Some(list) => list,
        None => client.collect_friends(FRIENDS_WAIT).await.unwrap_or_default(),
    }
}

pub async fn public_names(ids: &[u64]) -> std::collections::HashMap<u64, String> {
    let mut out = std::collections::HashMap::new();
    if ids.is_empty() {
        return out;
    }
    let Ok(http) = reqwest::Client::builder().timeout(NAMES_WAIT).build() else { return out };
    let mut set = tokio::task::JoinSet::new();
    for &id in ids.iter().take(MAX_PUBLIC_NAMES) {
        let http = http.clone();
        set.spawn(async move {
            let body = http.get(format!("https://steamcommunity.com/profiles/{id}?xml=1")).send().await.ok()?.text().await.ok()?;
            if let Some(name) = profile_name(&body) {
                return Some((id, name));
            }
            let account = id.checked_sub(76_561_197_960_265_728)?;
            let mini = http.get(format!("https://steamcommunity.com/miniprofile/{account}")).send().await.ok()?.text().await.ok()?;
            Some((id, miniprofile_name(&mini)?))
        });
    }
    let _ = tokio::time::timeout(NAMES_WAIT, async {
        while let Some(done) = set.join_next().await {
            if let Ok(Some((id, name))) = done {
                out.insert(id, name);
            }
        }
    })
    .await;
    out
}

pub fn profile_name(xml: &str) -> Option<String> {
    let start = xml.find("<steamID>")? + "<steamID>".len();
    let end = start + xml[start..].find("</steamID>")?;
    let name = xml[start..end].trim().trim_start_matches("<![CDATA[").trim_end_matches("]]>").trim();
    (!name.is_empty()).then(|| name.to_string())
}

pub fn miniprofile_name(html: &str) -> Option<String> {
    let at = html.find("class=\"persona")?;
    let start = at + html[at..].find('>')? + 1;
    let end = start + html[start..].find('<')?;
    let name = html[start..end].trim().replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&");
    (!name.is_empty()).then_some(name)
}

pub async fn build_snapshot(client: &mut SteamClient, ticket_for: Option<(u32, TokenFuture)>, friends: Vec<FriendEntry>) -> Result<SnapshotV1> {
    let started = std::time::Instant::now();
    let (app_id, token_future) = match ticket_for {
        Some((id, token)) => (Some(id), Some(token)),
        None => (None, None),
    };
    let data = client.get_account_data().await;
    let warm = super::warm::global();
    let steamid = data.steam_id;
    let mut friends = friends;
    let mut wanted: Vec<u64> = friends.iter().filter(|f| f.persona.is_empty()).map(|f| f.steamid).collect();
    if data.persona_name.is_none() {
        wanted.push(steamid);
    }
    let mut names = warm.names(steamid, &wanted);
    let missing: Vec<u64> = wanted.iter().filter(|id| !names.contains_key(id)).copied().collect();
    let names_job = async {
        let fetched = public_names(&missing).await;
        warm.put_names(steamid, &fetched);
        fetched
    };
    let games_job = async {
        let mut c = client.clone();
        match c.fetch_owned_games().await {
            Ok(games) => Ok(games),
            Err(e) => match crate::core::config::load_library_cache().await {
                Ok(cached) if !cached.is_empty() => Ok(cached),
                _ => Err(e),
            },
        }
    };
    let installed_job = async { crate::library::scan_installed_app_info().await.map(|apps| apps.keys().copied().collect::<HashSet<u32>>()).unwrap_or_default() };
    let ticket_job = async {
        match (app_id, token_future) {
            (Some(id), Some(token)) => {
                let (ownership, token) = tokio::try_join!(client.get_app_ticket(id), token)?;
                Ok::<_, anyhow::Error>(Some(encode_ticket(id, &ownership, &token)))
            }
            _ => Ok(None),
        }
    };
    let app_job = async {
        match app_id {
            Some(id) => Some(build_app_info(client, id).await),
            None => None,
        }
    };
    let (fetched, games, installed, ticket, app) = tokio::join!(names_job, games_job, installed_job, ticket_job, app_job);
    names.extend(fetched);
    for f in friends.iter_mut().filter(|f| f.persona.is_empty()) {
        if let Some(name) = names.get(&f.steamid) {
            f.persona = name.clone();
        }
    }
    let persona = data.persona_name.or_else(|| names.get(&steamid).cloned()).unwrap_or(data.account_name);
    let account = Account { steamid, persona, country: data.country };
    let mut snapshot = assemble(account, &games?, &installed, ticket?);
    if let Some(app) = app {
        snapshot = snapshot.with_app(app);
    }
    snapshot = snapshot.with_friends(friends);
    super::log::bridge_log(&format!("bridge: snapshot built in {} ms ({} cached names, {} fetched)", started.elapsed().as_millis(), names.len() - missing.len().min(names.len()), missing.len()));
    Ok(snapshot)
}

#[cfg(test)]
#[path = "snapshot_tests.rs"]
mod tests;
