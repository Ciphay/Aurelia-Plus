use std::sync::Arc;

use serde_json::Value;
use tokio::sync::mpsc::UnboundedReceiver;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyReason {
    User,
    Game,
    Steam,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Approved,
    Denied(DenyReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    Answer { id: String, answer: Answer },
    Shown { id: String },
    Hidden { id: String },
    JoinLobby { id: String, lobby: u64, friend: u64 },
    Text { id: String, submitted: bool, text: String },
    Keys { id: String, text: String, done: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayError {
    UnknownId,
    NotPending,
    NotMicrotxn,
    NotInvite,
    NotTextInput,
}

impl std::fmt::Display for OverlayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            OverlayError::UnknownId => "unknown_id",
            OverlayError::NotPending => "not_pending",
            OverlayError::NotMicrotxn => "not_microtxn",
            OverlayError::NotInvite => "not_invite",
            OverlayError::NotTextInput => "not_text_input",
        })
    }
}

impl std::error::Error for OverlayError {}

const MICROTXN: &str = "microtxn";
const ACHIEVEMENT: &str = "achievement";
const PLAYED_WITH: &str = "played_with";
const TIMELINE: &str = "timeline";
const LOBBY_INVITE: &str = "lobby_invite";
const GAMEPAD_TEXT: &str = "gamepad_text";
const GAMEPAD_TEXT_CLOSE: &str = "gamepad_text_close";
pub const MAX_TEXT: usize = 2048;

struct Entry {
    app_id: u32,
    kind: String,
    pending: bool,
    invite: Option<(u64, u64)>,
    floating: bool,
}

#[derive(Default)]
struct Inner {
    next: u64,
    entries: std::collections::BTreeMap<u64, Entry>,
    updates: Vec<Update>,
    capable: bool,
    dlc: Vec<DlcState>,
    recent: crate::bridge::social::RecentPlayers,
    recent_path: Option<std::path::PathBuf>,
    steam_coplay: Option<std::path::PathBuf>,
    friend_lobbies: Vec<FriendLobby>,
    cloud_changes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FriendLobby {
    pub friend: u64,
    pub lobby: u64,
    pub app_id: u32,
}

pub struct OverlayHub {
    inner: std::sync::Mutex<Inner>,
    events: tokio::sync::mpsc::UnboundedSender<Value>,
}

fn finish(mut base: serde_json::Map<String, Value>, id: u64, app_id: u32, extra: Value) -> Value {
    base.insert("id".into(), id.to_string().into());
    base.insert("app_id".into(), app_id.into());
    if let Value::Object(fields) = extra {
        for (k, v) in fields {
            base.entry(k).or_insert(v);
        }
    }
    Value::Object(base)
}

fn closed(id: u64, reason: &str) -> Value {
    serde_json::json!({ "event": "overlay_closed", "id": id.to_string(), "reason": reason })
}

fn parse_id(id: &str) -> Option<u64> {
    id.parse().ok()
}

impl OverlayHub {
    pub fn new() -> (Arc<OverlayHub>, UnboundedReceiver<Value>) {
        let (events, rx) = tokio::sync::mpsc::unbounded_channel();
        (Arc::new(OverlayHub { inner: std::sync::Mutex::new(Inner::default()), events }), rx)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("overlay hub lock poisoned")
    }

    fn add(&self, app_id: u32, kind: &str, base: Value, payload: Value, pending: bool) -> String {
        let mut inner = self.lock();
        inner.next += 1;
        let id = inner.next;
        inner.entries.insert(id, Entry { app_id, kind: kind.to_string(), pending, invite: None, floating: false });
        let Value::Object(base) = base else { unreachable!("the base event is always an object") };
        let _ = self.events.send(finish(base, id, app_id, payload));
        id.to_string()
    }

    pub fn request(&self, app_id: u32, kind: &str, payload: Value) -> String {
        let floating = kind == GAMEPAD_TEXT && payload.get("floating").and_then(Value::as_bool).unwrap_or(false);
        let id = self.add(app_id, kind, serde_json::json!({ "event": "overlay_request", "kind": kind }), payload, false);
        if floating {
            if let Some(key) = parse_id(&id) {
                if let Some(entry) = self.lock().entries.get_mut(&key) {
                    entry.floating = true;
                }
            }
        }
        id
    }

    pub fn microtxn(&self, app_id: u32, info: Value) -> String {
        self.add(app_id, MICROTXN, serde_json::json!({ "event": "microtxn_request" }), info, true)
    }

    pub fn respond(&self, id: &str, approve: bool) -> Result<(), OverlayError> {
        let key = parse_id(id).ok_or(OverlayError::UnknownId)?;
        let mut inner = self.lock();
        let entry = inner.entries.get_mut(&key).ok_or(OverlayError::UnknownId)?;
        if entry.kind != MICROTXN {
            return Err(OverlayError::NotMicrotxn);
        }
        if !entry.pending {
            return Err(OverlayError::NotPending);
        }
        entry.pending = false;
        let answer = if approve { Answer::Approved } else { Answer::Denied(DenyReason::User) };
        inner.updates.push(Update::Answer { id: id.to_string(), answer });
        let _ = self.events.send(closed(key, "answered"));
        Ok(())
    }

    fn state_change(&self, id: &str, update: fn(String) -> Update) -> Result<(), OverlayError> {
        let key = parse_id(id).ok_or(OverlayError::UnknownId)?;
        let mut inner = self.lock();
        if !inner.entries.contains_key(&key) {
            return Err(OverlayError::UnknownId);
        }
        inner.updates.push(update(id.to_string()));
        Ok(())
    }

    pub fn shown(&self, id: &str) -> Result<(), OverlayError> {
        self.state_change(id, |id| Update::Shown { id })
    }

    pub fn hidden(&self, id: &str) -> Result<(), OverlayError> {
        self.state_change(id, |id| Update::Hidden { id })
    }

    fn deny(&self, inner: &mut Inner, key: u64, reason: DenyReason, why: &str) {
        inner.updates.push(Update::Answer { id: key.to_string(), answer: Answer::Denied(reason) });
        let _ = self.events.send(closed(key, why));
    }

    pub fn dismiss(&self, id: &str) {
        let Some(key) = parse_id(id) else { return };
        let mut inner = self.lock();
        let pending = inner.entries.get_mut(&key).map(|e| std::mem::replace(&mut e.pending, false)).unwrap_or(false);
        if pending {
            self.deny(&mut inner, key, DenyReason::Steam, "dismissed_by_steam");
        }
    }

    pub fn close_all(&self, reason: &str) {
        let mut inner = self.lock();
        let keys: Vec<u64> = inner.entries.iter().filter(|(_, e)| e.pending).map(|(k, _)| *k).collect();
        for key in keys {
            if let Some(e) = inner.entries.get_mut(&key) {
                e.pending = false;
            }
            self.deny(&mut inner, key, DenyReason::Game, reason);
        }
        let invites: Vec<u64> = inner.entries.iter().filter(|(_, e)| e.invite.is_some()).map(|(k, _)| *k).collect();
        for key in invites {
            inner.entries.remove(&key);
            let _ = self.events.send(closed(key, reason));
        }
    }

    pub fn lobby_invite(&self, app_id: u32, json: &Value) -> Option<String> {
        let lobby = json.get("lobby").and_then(Value::as_u64).filter(|&l| l != 0)?;
        let inviter = json.get("inviter").and_then(Value::as_u64).unwrap_or(0);
        let game = json.get("game").and_then(Value::as_u64).unwrap_or(0);
        let id = self.add(
            app_id,
            LOBBY_INVITE,
            serde_json::json!({ "event": "lobby_invite" }),
            serde_json::json!({ "lobby": lobby.to_string(), "inviter": inviter.to_string(), "game": game.to_string() }),
            false,
        );
        if let Some(e) = self.lock().entries.get_mut(&parse_id(&id)?) {
            e.invite = Some((lobby, inviter));
        }
        Some(id)
    }

    fn take_invite(&self, id: &str) -> Result<(u64, (u64, u64)), OverlayError> {
        let key = parse_id(id).ok_or(OverlayError::UnknownId)?;
        let mut inner = self.lock();
        let entry = inner.entries.get(&key).ok_or(OverlayError::UnknownId)?;
        let invite = entry.invite.ok_or(OverlayError::NotInvite)?;
        inner.entries.remove(&key);
        Ok((key, invite))
    }

    pub fn accept_invite(&self, id: &str) -> Result<(), OverlayError> {
        let (key, (lobby, friend)) = self.take_invite(id)?;
        let mut inner = self.lock();
        inner.updates.push(Update::JoinLobby { id: id.to_string(), lobby, friend });
        let _ = self.events.send(closed(key, "accepted"));
        Ok(())
    }

    pub fn respond_text(&self, id: &str, submitted: bool, text: &str) -> Result<(), OverlayError> {
        let key = parse_id(id).ok_or(OverlayError::UnknownId)?;
        let mut inner = self.lock();
        let entry = inner.entries.get(&key).ok_or(OverlayError::UnknownId)?;
        if entry.kind != GAMEPAD_TEXT {
            return Err(OverlayError::NotTextInput);
        }
        let floating = entry.floating;
        let mut end = text.len().min(MAX_TEXT);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if floating {
            let done = !submitted || text.is_empty();
            inner.updates.push(Update::Keys { id: id.to_string(), text: text[..end].to_string(), done });
            if !done {
                return Ok(());
            }
            inner.entries.remove(&key);
            let _ = self.events.send(closed(key, if submitted { "answered" } else { "cancelled" }));
            return Ok(());
        }
        inner.entries.remove(&key);
        inner.updates.push(Update::Text { id: id.to_string(), submitted, text: text[..end].to_string() });
        let _ = self.events.send(closed(key, if submitted { "answered" } else { "cancelled" }));
        Ok(())
    }

    pub fn close_text(&self) {
        let mut inner = self.lock();
        let open: Vec<u64> = inner.entries.iter().filter(|(_, e)| e.kind == GAMEPAD_TEXT).map(|(k, _)| *k).collect();
        for key in open {
            inner.entries.remove(&key);
            let _ = self.events.send(closed(key, "dismissed_by_game"));
        }
    }

    pub fn decline_invite(&self, id: &str) -> Result<(), OverlayError> {
        let (key, _) = self.take_invite(id)?;
        let _ = self.events.send(closed(key, "declined"));
        Ok(())
    }

    pub fn achievement(&self, app_id: u32, json: &Value) {
        let Some(api_name) = json.get("api_name").and_then(Value::as_str) else { return };
        let _ = self.events.send(serde_json::json!({ "event": "achievement_unlocked", "app_id": app_id, "api_name": api_name }));
    }

    pub fn timeline(&self, app_id: u32, json: &Value) {
        let mut event = serde_json::json!({ "event": "timeline_event", "app_id": app_id });
        if let (Some(dst), Some(src)) = (event.as_object_mut(), json.as_object()) {
            for (k, v) in src {
                dst.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        let _ = self.events.send(event);
    }

    pub fn load_recent(&self, path: std::path::PathBuf) {
        let mut inner = self.lock();
        inner.recent = crate::bridge::social::RecentPlayers::load(&path);
        inner.recent_path = Some(path);
    }

    pub fn use_steam_coplay(&self, path: std::path::PathBuf) {
        let mut inner = self.lock();
        if let Ok(text) = std::fs::read_to_string(&path) {
            inner.recent.merge(crate::bridge::social::parse_steam_coplay(&text));
        }
        inner.steam_coplay = Some(path);
    }

    pub fn played_with(&self, app_id: u32, json: &Value) {
        let Some(steam_id) = json.get("steam_id").and_then(Value::as_u64).filter(|&id| id != 0) else { return };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let mut inner = self.lock();
        inner.recent.record(steam_id, app_id, now);
        if let Some(path) = &inner.recent_path {
            let _ = inner.recent.save(path);
        }
        if let Some(path) = &inner.steam_coplay {
            let _ = inner.recent.save_steam(path);
        }
    }

    pub fn set_friend_lobbies(&self, lobbies: Vec<FriendLobby>) {
        self.lock().friend_lobbies = lobbies;
    }

    pub fn set_cloud_changes(&self, names: Vec<String>) {
        self.lock().cloud_changes = names;
    }

    pub fn encode_recent(&self) -> Vec<u8> {
        let inner = self.lock();
        let list = inner.recent.entries();
        let mut out = (list.len() as u32).to_le_bytes().to_vec();
        for p in list {
            out.extend_from_slice(&p.steam_id.to_le_bytes());
            out.extend_from_slice(&p.app_id.to_le_bytes());
            out.extend_from_slice(&(p.last_played.min(u64::from(u32::MAX)) as u32).to_le_bytes());
        }
        out
    }

    pub fn encode_friend_lobbies(&self, app_id: u32) -> Vec<u8> {
        let inner = self.lock();
        let list: Vec<&FriendLobby> = inner.friend_lobbies.iter().filter(|l| l.app_id == app_id).collect();
        let mut out = (list.len() as u32).to_le_bytes().to_vec();
        for l in list {
            out.extend_from_slice(&l.friend.to_le_bytes());
            out.extend_from_slice(&l.lobby.to_le_bytes());
        }
        out
    }

    pub fn encode_cloud_changes(&self) -> Vec<u8> {
        let inner = self.lock();
        let mut out = (inner.cloud_changes.len() as u32).to_le_bytes().to_vec();
        for name in &inner.cloud_changes {
            let bytes = name.as_bytes();
            let n = bytes.len().min(u16::MAX as usize);
            out.extend_from_slice(&(n as u16).to_le_bytes());
            out.extend_from_slice(&bytes[..n]);
        }
        out
    }

    pub fn recent_event(&self) -> Value {
        self.lock().recent.event()
    }

    pub fn emit(&self, event: Value) {
        let _ = self.events.send(event);
    }

    pub fn dlc_changed(&self, app_id: u32, dlc_id: u32, owned: bool, installed: bool) {
        let _ = self.events.send(serde_json::json!({ "event": "dlc_changed", "app_id": app_id, "dlc_id": dlc_id, "owned": owned, "installed": installed }));
    }

    pub fn init_dlc(&self, states: Vec<DlcState>) {
        self.lock().dlc = states;
    }

    pub fn set_dlc(&self, app_id: u32, states: Vec<DlcState>) {
        let changed = {
            let mut inner = self.lock();
            let changed = diff_dlc(&inner.dlc, &states);
            inner.dlc = states;
            changed
        };
        for d in changed {
            self.dlc_changed(app_id, d.dlc_id, d.owned, d.installed);
        }
    }

    pub fn dlc(&self) -> Vec<DlcState> {
        self.lock().dlc.clone()
    }

    pub fn is_known(&self, id: &str) -> bool {
        parse_id(id).map(|k| self.lock().entries.contains_key(&k)).unwrap_or(false)
    }

    pub fn set_capable(&self, on: bool) {
        self.lock().capable = on;
    }

    pub fn capable(&self) -> bool {
        self.lock().capable
    }

    pub fn drain_updates(&self) -> Vec<Update> {
        std::mem::take(&mut self.lock().updates)
    }

    pub fn list(&self) -> Value {
        let inner = self.lock();
        Value::Array(
            inner
                .entries
                .iter()
                .filter(|(_, e)| e.pending || e.invite.is_some())
                .map(|(k, e)| serde_json::json!({ "id": k.to_string(), "app_id": e.app_id, "kind": e.kind }))
                .collect(),
        )
    }
}

pub const SLOT_OVERLAY: u16 = 13;
pub const OP_REQUEST: u8 = 1;
pub const OP_POLL: u8 = 2;
pub const OP_STATE: u8 = 3;
pub const OP_CLOSE_ALL: u8 = 4;
pub const OP_DLC: u8 = 5;
pub const OP_RECENT: u8 = 6;
pub const OP_FRIEND_LOBBIES: u8 = 7;
pub const OP_CLOUD_CHANGES: u8 = 8;
pub const MAX_JSON: usize = 2048;

pub fn parse_request(payload: &[u8]) -> Option<(u32, String, Value)> {
    let (&op, rest) = payload.split_first()?;
    if op != OP_REQUEST || rest.len() < 6 {
        return None;
    }
    let app = u32::from_le_bytes(rest[..4].try_into().ok()?);
    let kind_len = u16::from_le_bytes(rest[4..6].try_into().ok()?) as usize;
    let rest = &rest[6..];
    let kind = std::str::from_utf8(rest.get(..kind_len)?).ok()?.to_string();
    let rest = &rest[kind_len..];
    let json_len = u16::from_le_bytes(rest.get(..2)?.try_into().ok()?) as usize;
    let body = rest.get(2..2 + json_len)?;
    let mut json: Value = if body.is_empty() { Value::Object(Default::default()) } else { serde_json::from_slice(body).ok()? };
    clamp_strings(&mut json);
    Some((app, kind, json))
}

fn clamp_strings(v: &mut Value) {
    match v {
        Value::String(s) if s.len() > MAX_JSON => {
            let mut end = MAX_JSON;
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            s.truncate(end);
        }
        Value::Array(items) => items.iter_mut().for_each(clamp_strings),
        Value::Object(map) => map.values_mut().for_each(clamp_strings),
        _ => {}
    }
}

pub fn encode_updates(updates: &[Update]) -> Vec<u8> {
    let mut out = (updates.len() as u32).to_le_bytes().to_vec();
    for u in updates {
        let (tipo, id) = match u {
            Update::Answer { id, answer: Answer::Approved } => (1u8, id),
            Update::Answer { id, answer: Answer::Denied(DenyReason::User | DenyReason::Game) } => (2, id),
            Update::Answer { id, answer: Answer::Denied(DenyReason::Steam) } => (3, id),
            Update::Shown { id } => (4, id),
            Update::Hidden { id } => (5, id),
            Update::JoinLobby { id, .. } => (6, id),
            Update::Text { id, .. } => (7, id),
            Update::Keys { id, .. } => (8, id),
        };
        out.push(tipo);
        out.extend_from_slice(&(id.len() as u16).to_le_bytes());
        out.extend_from_slice(id.as_bytes());
        if let Update::JoinLobby { lobby, friend, .. } = u {
            out.extend_from_slice(&lobby.to_le_bytes());
            out.extend_from_slice(&friend.to_le_bytes());
        }
        if let Update::Text { submitted, text, .. } = u {
            out.push(*submitted as u8);
            out.extend_from_slice(&(text.len() as u16).to_le_bytes());
            out.extend_from_slice(text.as_bytes());
        }
        if let Update::Keys { done, text, .. } = u {
            out.push(*done as u8);
            out.extend_from_slice(&(text.len() as u16).to_le_bytes());
            out.extend_from_slice(text.as_bytes());
        }
    }
    out
}

pub fn encode_state(capable: bool) -> Vec<u8> {
    vec![capable as u8]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DlcState {
    pub dlc_id: u32,
    pub owned: bool,
    pub installed: bool,
}

pub fn diff_dlc(old: &[DlcState], new: &[DlcState]) -> Vec<DlcState> {
    new.iter().filter(|n| old.iter().find(|o| o.dlc_id == n.dlc_id) != Some(n)).copied().collect()
}

pub fn encode_dlc(states: &[DlcState]) -> Vec<u8> {
    let mut out = (states.len() as u32).to_le_bytes().to_vec();
    for d in states {
        out.extend_from_slice(&d.dlc_id.to_le_bytes());
        out.push(d.owned as u8);
        out.push(d.installed as u8);
    }
    out
}

pub fn overlay_call(hub: &OverlayHub, app_id: u32, payload: &[u8]) -> (i32, Vec<u8>) {
    match payload.first().copied() {
        Some(OP_REQUEST) => {
            let Some((_, kind, json)) = parse_request(payload) else { return (-2, Vec::new()) };
            if kind == GAMEPAD_TEXT_CLOSE {
                hub.close_text();
                return (0, 0u16.to_le_bytes().to_vec());
            }
            if kind == LOBBY_INVITE {
                let id = hub.lobby_invite(app_id, &json).unwrap_or_default();
                let mut reply = (id.len() as u16).to_le_bytes().to_vec();
                reply.extend_from_slice(id.as_bytes());
                return (0, reply);
            }
            if kind == PLAYED_WITH {
                hub.played_with(app_id, &json);
                return (0, 0u16.to_le_bytes().to_vec());
            }
            if kind == TIMELINE {
                hub.timeline(app_id, &json);
                return (0, 0u16.to_le_bytes().to_vec());
            }
            if kind == ACHIEVEMENT {
                hub.achievement(app_id, &json);
                return (0, 0u16.to_le_bytes().to_vec());
            }
            let id = if kind == MICROTXN { hub.microtxn(app_id, json) } else { hub.request(app_id, &kind, json) };
            let mut reply = (id.len() as u16).to_le_bytes().to_vec();
            reply.extend_from_slice(id.as_bytes());
            (0, reply)
        }
        Some(OP_POLL) => (0, encode_updates(&hub.drain_updates())),
        Some(OP_STATE) => (0, encode_state(hub.capable())),
        Some(OP_DLC) => (0, encode_dlc(&hub.dlc())),
        Some(OP_RECENT) => (0, hub.encode_recent()),
        Some(OP_FRIEND_LOBBIES) => (0, hub.encode_friend_lobbies(app_id)),
        Some(OP_CLOUD_CHANGES) => (0, hub.encode_cloud_changes()),
        Some(OP_CLOSE_ALL) => {
            hub.close_all("game_exit");
            (0, Vec::new())
        }
        _ => (-2, Vec::new()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Respond { id: String, approve: bool },
    Shown { id: String },
    Hidden { id: String },
    Capable { on: bool },
    SteamUrl { id: String, url: String },
    WebSession,
    List,
    AcceptInvite { id: String },
    TextInput { id: String, submitted: bool, text: String },
    DeclineInvite { id: String },
    PlayersRecent { req: Option<String> },
    Query { req: Option<String>, query: Query },
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    Friends,
    FriendRequests,
    Join { steam_id: u64 },
    ChatSend { steam_id: u64, message: String },
    ChatHistory { steam_id: u64, count: u32 },
    Achievements { app_id: u32, lang: Option<String> },
    Profile { steam_id: u64 },
    Persona { state: u32 },
    FriendAdd { steam_id: u64 },
    FriendRemove { steam_id: u64 },
    Invite { steam_id: u64, connect: Option<String> },
}

pub const MAX_CHAT_HISTORY: u32 = 100;
const DEFAULT_CHAT_HISTORY: u32 = 20;
pub const MAX_PERSONA_STATE: u32 = 6;

pub const MAX_LINE: usize = 8 * 1024;

pub fn parse_command(line: &str) -> Result<Command, String> {
    if line.len() > MAX_LINE {
        return Err("line_too_long".into());
    }
    let v: Value = serde_json::from_str(line).map_err(|e| format!("invalid_json: {e}"))?;
    let text = |key: &str| v.get(key).and_then(Value::as_str).map(str::to_string).ok_or_else(|| format!("missing_field: {key}"));
    let flag = |key: &str| v.get(key).and_then(Value::as_bool).ok_or_else(|| format!("missing_field: {key}"));
    let steam_id = || match v.get("steam_id") {
        Some(Value::String(t)) => t.trim().parse::<u64>().map_err(|_| "invalid_field: steam_id".to_string()),
        Some(Value::Number(n)) => n.as_u64().ok_or_else(|| "invalid_field: steam_id".to_string()),
        _ => Err("missing_field: steam_id".to_string()),
    };
    let number = |key: &str| v.get(key).and_then(Value::as_u64).ok_or_else(|| format!("missing_field: {key}"));
    let req = v.get("req").and_then(Value::as_str).map(str::to_string);
    let query = |q: Query| Ok(Command::Query { req: req.clone(), query: q });
    match v.get("cmd").and_then(Value::as_str) {
        Some("friends") => query(Query::Friends),
        Some("friend_requests") => query(Query::FriendRequests),
        Some("join") => query(Query::Join { steam_id: steam_id()? }),
        Some("chat_send") => query(Query::ChatSend { steam_id: steam_id()?, message: text("message")? }),
        Some("chat_history") => {
            let count = v.get("count").and_then(Value::as_u64).unwrap_or(u64::from(DEFAULT_CHAT_HISTORY));
            query(Query::ChatHistory { steam_id: steam_id()?, count: count.clamp(1, u64::from(MAX_CHAT_HISTORY)) as u32 })
        }
        Some("achievements") => {
            let app_id = u32::try_from(number("app_id")?).map_err(|_| "invalid_field: app_id".to_string())?;
            let lang = v.get("lang").and_then(Value::as_str).map(str::to_string);
            query(Query::Achievements { app_id, lang })
        }
        Some("profile") => query(Query::Profile { steam_id: steam_id()? }),
        Some("persona") => {
            let state = number("state")?;
            if state > u64::from(MAX_PERSONA_STATE) {
                return Err("invalid_field: state".into());
            }
            query(Query::Persona { state: state as u32 })
        }
        Some("friend_add") => query(Query::FriendAdd { steam_id: steam_id()? }),
        Some("friend_remove") => query(Query::FriendRemove { steam_id: steam_id()? }),
        Some("invite") => query(Query::Invite { steam_id: steam_id()?, connect: v.get("connect").and_then(Value::as_str).map(str::to_string) }),
        Some("respond") => Ok(Command::Respond { id: text("id")?, approve: flag("approve")? }),
        Some("shown") => Ok(Command::Shown { id: text("id")? }),
        Some("hidden") => Ok(Command::Hidden { id: text("id")? }),
        Some("capable") => Ok(Command::Capable { on: flag("on")? }),
        Some("steam_url") => Ok(Command::SteamUrl { id: text("id")?, url: text("url")? }),
        Some("web_session") => Ok(Command::WebSession),
        Some("list") => Ok(Command::List),
        Some("text_input") => {
            let id = text("id")?;
            let cancel = v.get("cancel").and_then(Value::as_bool).unwrap_or(false);
            let typed = v.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
            Ok(Command::TextInput { id, submitted: !cancel, text: typed })
        }
        Some("accept_invite") => Ok(Command::AcceptInvite { id: text("id")? }),
        Some("decline_invite") => Ok(Command::DeclineInvite { id: text("id")? }),
        Some("players_recent") => Ok(Command::PlayersRecent { req: req.clone() }),
        Some("stop") => Ok(Command::Stop),
        Some(other) => Err(format!("unknown_cmd: {other}")),
        None => Err("missing_field: cmd".into()),
    }
}

fn result_of(r: Result<(), OverlayError>) -> Value {
    match r {
        Ok(()) => serde_json::json!({ "event": "result", "ok": true }),
        Err(e) => serde_json::json!({ "event": "error", "error": e.to_string() }),
    }
}

pub fn apply(hub: &OverlayHub, cmd: Command) -> Value {
    match cmd {
        Command::Respond { id, approve } => result_of(hub.respond(&id, approve)),
        Command::Shown { id } => result_of(hub.shown(&id)),
        Command::Hidden { id } => result_of(hub.hidden(&id)),
        Command::Capable { on } => {
            hub.set_capable(on);
            result_of(Ok(()))
        }
        Command::SteamUrl { id, url } => {
            if url.starts_with("steam://close/authorize") {
                result_of(hub.respond(&id, true))
            } else if url.starts_with("steam://close/cancel") {
                result_of(hub.respond(&id, false))
            } else if hub.is_known(&id) {
                serde_json::json!({ "event": "error", "error": "unsupported_url" })
            } else {
                result_of(Err(OverlayError::UnknownId))
            }
        }
        Command::WebSession => serde_json::json!({ "event": "error", "error": "web_session_failed", "detail": "no session" }),
        Command::List => serde_json::json!({ "event": "overlay_list", "pending": hub.list() }),
        Command::AcceptInvite { id } => result_of(hub.accept_invite(&id)),
        Command::TextInput { id, submitted, text } => result_of(hub.respond_text(&id, submitted, &text)),
        Command::DeclineInvite { id } => result_of(hub.decline_invite(&id)),
        Command::PlayersRecent { req } => with_req(hub.recent_event(), &req),
        Command::Query { req, .. } => with_req(query_error("query_failed", "no session"), &req),
        Command::Stop => result_of(Ok(())),
    }
}

pub fn with_req(mut reply: Value, req: &Option<String>) -> Value {
    if let (Some(req), Some(obj)) = (req, reply.as_object_mut()) {
        obj.insert("req".into(), Value::String(req.clone()));
    }
    reply
}

pub fn connection_event(online: bool, queued: usize) -> Value {
    serde_json::json!({ "event": "connection", "online": online, "queued": queued })
}

pub fn query_error(error: &str, detail: &str) -> Value {
    serde_json::json!({ "event": "error", "error": error, "detail": detail })
}

pub fn friends_event(friends: &[crate::bridge::social::FriendInfo]) -> Value {
    let list: Vec<Value> = friends.iter().map(crate::bridge::social::FriendInfo::to_json).collect();
    serde_json::json!({ "event": "friends", "friends": list })
}

pub fn chat_sent_event(server_timestamp: u64, modified_message: Option<&str>) -> Value {
    serde_json::json!({ "event": "result", "ok": true, "server_timestamp": server_timestamp, "modified_message": modified_message })
}

pub fn chat_history_event(steam_id: u64, messages: &[crate::steam_client::ChatMessage]) -> Value {
    let list: Vec<Value> = messages
        .iter()
        .map(|m| serde_json::json!({ "sender": m.sender.to_string(), "from_self": m.from_self, "message": m.message, "timestamp": m.timestamp }))
        .collect();
    serde_json::json!({ "event": "chat_history", "steam_id": steam_id.to_string(), "messages": list })
}

pub fn unlocked_event(mut base: Value, found: Option<&crate::steam_client::GameAchievement>) -> Value {
    if let (Some(a), Some(obj)) = (found, base.as_object_mut()) {
        obj.insert("name".into(), a.name.clone().into());
        obj.insert("description".into(), a.description.clone().into());
        obj.insert("hidden".into(), a.hidden.into());
        obj.insert("icon_unlocked".into(), a.icon_unlocked.clone().into());
        obj.insert("icon_locked".into(), a.icon_locked.clone().into());
        obj.insert("global_percent".into(), a.global_percent.into());
        obj.insert("unlock_time".into(), a.unlock_time.into());
    }
    base
}

pub fn achievements_event(app_id: u32, achievements: &[crate::steam_client::GameAchievement]) -> Value {
    let list: Vec<Value> = achievements
        .iter()
        .map(|a| {
            serde_json::json!({
                "api_name": a.api_name, "name": a.name, "description": a.description, "hidden": a.hidden,
                "icon_unlocked": a.icon_unlocked, "icon_locked": a.icon_locked, "global_percent": a.global_percent,
                "unlocked": a.unlocked, "unlock_time": a.unlock_time,
            })
        })
        .collect();
    serde_json::json!({ "event": "achievements", "app_id": app_id, "achievements": list })
}

pub fn profile_event(p: &crate::steam_client::UserProfile) -> Value {
    let mut v = serde_json::json!({ "event": "profile", "steam_id": p.steam_id.to_string() });
    if let (Some(out), Ok(Value::Object(fields))) = (v.as_object_mut(), serde_json::to_value(p)) {
        for (key, value) in fields {
            out.entry(key).or_insert(value);
        }
    }
    v
}

pub fn friend_added_event(steam_id: u64, persona: Option<&str>) -> Value {
    serde_json::json!({ "event": "result", "ok": true, "steam_id": steam_id.to_string(), "persona": persona })
}

pub fn web_session_event(steam_id: u64, cookies: &[(String, String)]) -> Value {
    let list: Vec<Value> = [".steampowered.com", ".steamcommunity.com"]
        .iter()
        .flat_map(|domain| {
            cookies.iter().map(move |(name, value)| {
                serde_json::json!({ "name": name, "value": value, "domain": domain, "path": "/", "secure": true, "http_only": name == "steamLoginSecure" })
            })
        })
        .collect();
    serde_json::json!({ "event": "web_session", "steam_id": steam_id.to_string(), "cookies": list })
}

pub fn web_session_error(detail: &str) -> Value {
    serde_json::json!({ "event": "error", "error": "web_session_failed", "detail": detail })
}

pub fn line_error(line: &str) -> Value {
    let reason = parse_command(line).err().unwrap_or_else(|| "invalid_command".into());
    let req = serde_json::from_str::<Value>(line).ok().and_then(|v| v.get("req").and_then(Value::as_str).map(str::to_string));
    with_req(serde_json::json!({ "event": "error", "error": "invalid_command", "detail": reason }), &req)
}

#[cfg(test)]
#[path = "overlay_tests.rs"]
mod tests;
