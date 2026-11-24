use super::*;
use std::sync::RwLock;
use steam_vent::ReceivableMessage;
use steam_vent_proto::steammessages_clientserver_2::CMsgClientPlayingSessionState;
use steam_vent_proto::steammessages_clientserver_friends::{
    CMsgClientAddFriend, CMsgClientAddFriendResponse, CMsgClientChangeStatus,
    CMsgClientFriendsList, CMsgClientPersonaState, CMsgClientRemoveFriend,
    CMsgClientRequestFriendData,
};
use steam_vent_proto::steammessages_clientserver_login::CMsgClientLoggedOff;
use tokio_stream::StreamExt;

pub const RELATIONSHIP_FRIEND: u32 = 3;

pub const PERSONA_REQUEST_FLAGS: u32 = 1 | 2 | 16 | 256 | 512 | 4096;

pub const RELATIONSHIP_REQUEST_RECEIVED: u32 = 2;

const PERSONA_REQUEST_CHUNK: usize = 8;

const UNNAMED_RETRY_EVERY: std::time::Duration = std::time::Duration::from_secs(5);
const UNNAMED_RETRIES: u32 = 6;

static ANNOUNCED: std::sync::Mutex<Option<(i32, u32)>> = std::sync::Mutex::new(None);

const PERSONA_DRAIN_IDLE: std::time::Duration = std::time::Duration::from_millis(500);

#[derive(Debug, Clone, serde::Serialize)]
pub struct Friend {
    pub steam_id: u64,
    pub relationship: u32,
    pub persona_name: Option<String>,
    pub persona_state: Option<u32>,
    pub game_app_id: Option<u32>,
    pub game_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_lobby_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_server: Option<String>,
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub rich_presence: std::collections::BTreeMap<String, String>,
}

impl Friend {
    fn new(steam_id: u64, relationship: u32) -> Self {
        Friend {
            steam_id,
            relationship,
            persona_name: None,
            persona_state: None,
            game_app_id: None,
            game_name: None,
            game_lobby_id: None,
            game_server: None,
            rich_presence: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatcherEnd {
    StreamsClosed,
    LoggedOff(i32),
}

pub type Roster = HashMap<u64, Friend>;

#[derive(Debug, Clone, serde::Serialize)]
pub struct AddedFriend {
    pub steam_id: u64,
    pub persona_name: Option<String>,
}

fn nonempty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

fn add_friend_error(eresult: i32) -> String {
    match eresult {
        15 => "access denied — the account may have blocked you or restricts who can add it"
            .to_string(),
        25 => "your friends list is full (Steam friend limit reached)".to_string(),
        84 => "Steam is rate-limiting friend requests — wait a while and try again".to_string(),
        other => format!("Steam rejected the friend request (EResult {other})"),
    }
}

fn friend_ids(roster: &Roster) -> Vec<u64> {
    roster
        .values()
        .filter(|f| f.relationship == RELATIONSHIP_FRIEND || f.relationship == RELATIONSHIP_REQUEST_RECEIVED)
        .map(|f| f.steam_id)
        .collect()
}

fn unnamed_friend_ids(roster: &Roster) -> Vec<u64> {
    roster
        .values()
        .filter(|f| (f.relationship == RELATIONSHIP_FRIEND || f.relationship == RELATIONSHIP_REQUEST_RECEIVED) && f.persona_name.is_none())
        .map(|f| f.steam_id)
        .collect()
}

pub fn apply_friends_list(roster: &mut Roster, msg: &CMsgClientFriendsList) {
    if !msg.bincremental() {
        let keep: std::collections::HashSet<u64> =
            msg.friends.iter().map(|f| f.ulfriendid()).collect();
        roster.retain(|id, _| keep.contains(id));
    }
    for f in &msg.friends {
        let id = f.ulfriendid();
        let rel = f.efriendrelationship();
        if rel == 0 {
            roster.remove(&id);
            continue;
        }
        match roster.get_mut(&id) {
            Some(existing) => existing.relationship = rel,
            None => {
                roster.insert(id, Friend::new(id, rel));
            }
        }
    }
}

pub fn apply_persona_state(roster: &mut Roster, msg: &CMsgClientPersonaState) {
    for fr in &msg.friends {
        let id = fr.friendid();
        let entry = roster.entry(id).or_insert_with(|| Friend::new(id, 0));

        let name = fr.player_name();
        if !name.is_empty() {
            entry.persona_name = Some(name.to_string());
        }
        if fr.has_persona_state() {
            entry.persona_state = Some(fr.persona_state());
        }
        if fr.has_game_played_app_id() {
            entry.game_app_id = match fr.game_played_app_id() {
                0 => None,
                app_id => Some(app_id),
            };
            if entry.game_app_id.is_none() {
                entry.game_name = None;
                entry.game_lobby_id = None;
                entry.game_server = None;
                entry.rich_presence.clear();
            }
        }
        if fr.has_game_lobby_id() {
            entry.game_lobby_id = Some(fr.game_lobby_id()).filter(|&l| l != 0);
        }
        if fr.has_game_server_ip() {
            entry.game_server = match (fr.game_server_ip(), fr.game_server_port()) {
                (0, _) => None,
                (ip, port) => Some(format!("{}:{port}", std::net::Ipv4Addr::from(ip))),
            };
        }
        if !fr.rich_presence.is_empty() {
            entry.rich_presence = fr.rich_presence.iter().filter(|kv| !kv.key().is_empty()).map(|kv| (kv.key().to_string(), kv.value().to_string())).collect();
        }
        if fr.has_game_name() && entry.game_app_id.is_some() {
            entry.game_name = nonempty(fr.game_name());
        }
    }
}

pub fn host_steam_owns_presence() -> bool {
    crate::core::utils::is_steam_running()
}

static LOGON_BURST: std::sync::Mutex<Vec<(i32, bool, Vec<u8>)>> = std::sync::Mutex::new(Vec::new());

pub fn logon_burst() -> Vec<(i32, bool, Vec<u8>)> {
    LOGON_BURST.lock().expect("logon burst lock poisoned").clone()
}

fn drain_unprocessed_into(connection: &Connection, roster: &mut Roster) {
    let mut lists = Vec::new();
    let mut personas = Vec::new();
    let pending = connection.take_unprocessed();
    {
        let mut kept = LOGON_BURST.lock().expect("logon burst lock poisoned");
        for raw in &pending {
            if kept.len() < 256 {
                kept.push((raw.kind.0, raw.is_protobuf, raw.data.to_vec()));
            }
        }
    }
    tracing::debug!("friends: {} mensagens pendentes no logon", pending.len());
    for raw in pending {
        if raw.kind == <CMsgClientFriendsList as ReceivableMessage>::KIND {
            if let Ok(msg) = raw.into_message::<CMsgClientFriendsList>() {
                lists.push(msg);
            }
        } else if raw.kind == <CMsgClientPersonaState as ReceivableMessage>::KIND {
            if let Ok(msg) = raw.into_message::<CMsgClientPersonaState>() {
                personas.push(msg);
            }
        } else if raw.kind == <CMsgClientPlayingSessionState as ReceivableMessage>::KIND {
            if let Ok(msg) = raw.into_message::<CMsgClientPlayingSessionState>() {
                super::chat::record_playing_session(connection.session_id(), &msg);
            }
        }
    }
    for list in &lists {
        apply_friends_list(roster, list);
    }
    for persona in &personas {
        apply_persona_state(roster, persona);
    }
}

impl SteamClient {
    pub async fn announce_persona(&self, persona_state: u32) -> Result<()> {
        let connection = self.require_connection()?;
        if self.announced_persona() == Some(persona_state) {
            return Ok(());
        }
        let mut status = CMsgClientChangeStatus::new();
        status.set_persona_state(persona_state);
        status.set_need_persona_response(true);
        connection
            .send(status)
            .await
            .map_err(|e| anyhow!("failed to announce persona status: {e}"))?;
        *ANNOUNCED.lock().expect("announce lock poisoned") =
            Some((connection.session_id(), persona_state));
        Ok(())
    }

    pub fn announced_persona(&self) -> Option<u32> {
        let session = self.connection.as_ref()?.session_id();
        match *ANNOUNCED.lock().expect("announce lock poisoned") {
            Some((s, state)) if s == session => Some(state),
            _ => None,
        }
    }

    pub async fn announce_configured_presence(&self) -> Result<()> {
        if host_steam_owns_presence() {
            tracing::debug!("host Steam client running; not announcing presence");
            return Ok(());
        }
        let persona_state = crate::core::config::load_launcher_config()
            .await
            .map(|c| c.chat_presence.persona_state())
            .unwrap_or(7);
        self.announce_persona(persona_state).await
    }

    pub async fn request_friend_data(&self, ids: &[u64]) -> Result<()> {
        let connection = self.require_connection()?;
        if ids.is_empty() || self.announced_persona().is_none() {
            return Ok(());
        }
        let mut req = CMsgClientRequestFriendData::new();
        req.set_persona_state_requested(PERSONA_REQUEST_FLAGS);
        req.friends = ids.to_vec();
        connection
            .send(req)
            .await
            .map_err(|e| anyhow!("failed to request friend data: {e}"))?;
        Ok(())
    }

    async fn drain_persona_data<S, E>(
        &self,
        persona_stream: &mut S,
        ids: &[u64],
        deadline: Option<tokio::time::Instant>,
        log_request_errors: bool,
        mut apply: impl FnMut(&CMsgClientPersonaState),
    ) where
        S: tokio_stream::Stream<Item = std::result::Result<CMsgClientPersonaState, E>> + Unpin,
    {
        for chunk in ids.chunks(PERSONA_REQUEST_CHUNK) {
            if deadline.is_some_and(|d| tokio::time::Instant::now() >= d) {
                break;
            }
            if let Err(e) = self.request_friend_data(chunk).await {
                if log_request_errors {
                    tracing::warn!("failed to request friend data: {e}");
                }
                continue;
            }
            while let Ok(item) =
                tokio::time::timeout(PERSONA_DRAIN_IDLE, persona_stream.next()).await
            {
                match item {
                    Some(Ok(state)) => apply(&state),
                    Some(Err(_)) => continue,
                    None => break,
                }
            }
        }
    }

    pub async fn run_friends_watcher(&self, roster: Arc<RwLock<Roster>>) -> Result<WatcherEnd> {
        let connection = self.require_connection_owned()?;
        let mut friends_stream = connection.on::<CMsgClientFriendsList>();
        let mut persona_stream = connection.on::<CMsgClientPersonaState>();
        let mut playing_stream = connection.on::<CMsgClientPlayingSessionState>();
        let mut logoff_stream = connection.on::<CMsgClientLoggedOff>();

        let initial_ids: Vec<u64> = {
            let mut guard = roster.write().expect("roster lock poisoned");
            drain_unprocessed_into(&connection, &mut guard);
            friend_ids(&guard)
        };

        self.drain_persona_data(&mut persona_stream, &initial_ids, None, true, |state| {
            let mut guard = roster.write().expect("roster lock poisoned");
            apply_persona_state(&mut guard, state);
        })
        .await;

        tracing::info!(
            "friends watcher started ({} friend(s) so far)",
            initial_ids.len()
        );
        let mut retry = tokio::time::interval(UNNAMED_RETRY_EVERY);
        retry.tick().await;
        let mut retries_left = UNNAMED_RETRIES;
        loop {
            tokio::select! {
                _ = retry.tick(), if retries_left > 0 => {
                    let ids = unnamed_friend_ids(&roster.read().expect("roster lock poisoned"));
                    if ids.is_empty() {
                        retries_left = 0;
                        continue;
                    }
                    retries_left -= 1;
                    tracing::debug!("re-requesting {} unnamed friend(s)", ids.len());
                    self.drain_persona_data(&mut persona_stream, &ids, None, true, |state| {
                        let mut guard = roster.write().expect("roster lock poisoned");
                        apply_persona_state(&mut guard, state);
                    })
                    .await;
                }
                list = friends_stream.next() => {
                    let list = match list {
                        Some(Ok(list)) => list,
                        Some(Err(_)) => continue,
                        None => break,
                    };
                    let ids: Vec<u64> = {
                        let mut guard = roster.write().expect("roster lock poisoned");
                        apply_friends_list(&mut guard, &list);
                        friend_ids(&guard)
                    };
                    self.drain_persona_data(&mut persona_stream, &ids, None, true, |state| {
                        let mut guard = roster.write().expect("roster lock poisoned");
                        apply_persona_state(&mut guard, state);
                    })
                    .await;
                }
                Some(Ok(logoff)) = logoff_stream.next() => {
                    tracing::warn!(eresult = logoff.eresult(), "Steam logged this session off");
                    return Ok(WatcherEnd::LoggedOff(logoff.eresult()));
                }
                Some(Ok(state)) = playing_stream.next() => {
                    super::chat::record_playing_session(connection.session_id(), &state);
                }
                state = persona_stream.next() => {
                    match state {
                        Some(Ok(state)) => {
                            let mut guard = roster.write().expect("roster lock poisoned");
                            apply_persona_state(&mut guard, &state);
                        }
                        Some(Err(_)) => continue,
                        None => break,
                    }
                }
            }
        }
        tracing::info!("friends watcher stopped");
        Ok(WatcherEnd::StreamsClosed)
    }

    pub async fn collect_friends(&self, wait: std::time::Duration) -> Result<Vec<Friend>> {
        let connection = self.require_connection_owned()?;
        let mut persona_stream = connection.on::<CMsgClientPersonaState>();

        let _ = self.announce_configured_presence().await;

        let mut roster: Roster = HashMap::new();
        let deadline = tokio::time::Instant::now() + wait;

        drain_unprocessed_into(&connection, &mut roster);
        let ids = friend_ids(&roster);
        self.drain_persona_data(&mut persona_stream, &ids, Some(deadline), false, |state| {
            apply_persona_state(&mut roster, state);
        })
        .await;

        let mut friends: Vec<Friend> = roster.into_values().collect();
        friends.sort_by(|a, b| match (&a.persona_name, &b.persona_name) {
            (Some(x), Some(y)) => x.cmp(y).then(a.steam_id.cmp(&b.steam_id)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.steam_id.cmp(&b.steam_id),
        });
        Ok(friends)
    }
}

impl SteamClient {
    pub async fn add_friend(&self, steam_id: u64) -> Result<AddedFriend> {
        let connection = self.require_connection()?;
        let mut req = CMsgClientAddFriend::new();
        req.set_steamid_to_add(steam_id);

        let resp: CMsgClientAddFriendResponse =
            tokio::time::timeout(std::time::Duration::from_secs(15), connection.job(req))
                .await
                .map_err(|_| anyhow!("timed out waiting for Steam to confirm the friend request"))?
                .map_err(|e| anyhow!("friend request failed: {e}"))?;

        if resp.eresult() != 1 {
            bail!("{}", add_friend_error(resp.eresult()));
        }
        let added_id = match resp.steam_id_added() {
            0 => steam_id,
            id => id,
        };
        Ok(AddedFriend {
            steam_id: added_id,
            persona_name: nonempty(resp.persona_name_added()),
        })
    }

    pub async fn remove_friend(&self, steam_id: u64) -> Result<()> {
        let connection = self.require_connection()?;
        let mut req = CMsgClientRemoveFriend::new();
        req.set_friendid(steam_id);
        connection
            .send(req)
            .await
            .map_err(|e| anyhow!("failed to remove friend: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "friends_tests.rs"]
mod tests;
