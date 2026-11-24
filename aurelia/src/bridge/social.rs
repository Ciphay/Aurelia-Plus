use std::collections::{BTreeMap, HashMap};

use serde_json::{Value, json};
use steam_vent_chat::ChatEvent;

use crate::steam_client::{Friend, RELATIONSHIP_FRIEND, RELATIONSHIP_REQUEST_RECEIVED};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FriendInfo {
    pub steam_id: u64,
    pub persona: String,
    pub state: u32,
    pub game_appid: u32,
    pub game_name: Option<String>,
    pub lobby: u64,
    pub server: Option<String>,
    pub rich_presence: BTreeMap<String, String>,
}

impl From<&Friend> for FriendInfo {
    fn from(f: &Friend) -> Self {
        FriendInfo {
            steam_id: f.steam_id,
            persona: f.persona_name.clone().unwrap_or_default(),
            state: f.persona_state.unwrap_or(0),
            game_appid: f.game_app_id.unwrap_or(0),
            game_name: f.game_name.clone(),
            lobby: f.game_lobby_id.unwrap_or(0),
            server: f.game_server.clone(),
            rich_presence: f.rich_presence.clone(),
        }
    }
}

impl FriendInfo {
    pub fn connect_line(&self) -> Option<String> {
        if self.game_appid == 0 {
            return None;
        }
        let connect = self.rich_presence.get("connect").map(|c| c.trim()).filter(|c| !c.is_empty());
        connect
            .map(str::to_string)
            .or_else(|| (self.lobby != 0).then(|| format!("+connect_lobby {}", self.lobby)))
            .or_else(|| self.server.as_ref().map(|s| format!("+connect {s}")))
    }

    pub fn joinable(&self) -> bool {
        self.connect_line().is_some()
    }

    pub fn to_json(&self) -> Value {
        json!({
            "steam_id": self.steam_id.to_string(), "persona": self.persona, "state": self.state, "game_appid": self.game_appid,
            "game_name": self.game_name, "lobby": (self.lobby != 0).then(|| self.lobby.to_string()), "server": self.server,
            "status": self.rich_presence.get("status"), "rich_presence": self.rich_presence,
            "joinable": self.joinable(), "connect": self.connect_line(),
        })
    }
}

pub fn join_event(f: &FriendInfo, installed: bool) -> Value {
    json!({
        "event": "join", "steam_id": f.steam_id.to_string(), "app_id": f.game_appid, "game_name": f.game_name,
        "connect": f.connect_line(), "lobby": (f.lobby != 0).then(|| f.lobby.to_string()), "installed": installed, "needs_install": !installed,
    })
}

pub fn split_friends(list: &[Friend]) -> (Vec<FriendInfo>, Vec<FriendInfo>) {
    let by = |relationship: u32| list.iter().filter(|f| f.relationship == relationship).map(FriendInfo::from).collect();
    (by(RELATIONSHIP_FRIEND), by(RELATIONSHIP_REQUEST_RECEIVED))
}

#[derive(Debug, Default)]
pub struct FriendWatch {
    known: HashMap<u64, FriendInfo>,
}

impl FriendWatch {
    pub fn new(initial: &[FriendInfo]) -> Self {
        FriendWatch { known: initial.iter().map(|f| (f.steam_id, f.clone())).collect() }
    }

    pub fn update(&mut self, current: &[FriendInfo]) -> Vec<Value> {
        let mut events = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for f in current {
            seen.insert(f.steam_id);
            match self.known.get(&f.steam_id) {
                None => events.push(status_event("added", f, None)),
                Some(old) if changed(old, f) => events.push(status_event("updated", f, Some(old))),
                Some(_) => {}
            }
            self.known.insert(f.steam_id, f.clone());
        }
        let gone: Vec<u64> = self.known.keys().filter(|id| !seen.contains(id)).copied().collect();
        for id in gone {
            if let Some(old) = self.known.remove(&id) {
                events.push(status_event("removed", &old, Some(&old)));
            }
        }
        events.sort_by_key(|e| e["steam_id"].as_str().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0));
        events
    }
}

fn changed(old: &FriendInfo, new: &FriendInfo) -> bool {
    old.state != new.state || old.game_appid != new.game_appid || old.lobby != new.lobby || old.server != new.server || old.rich_presence != new.rich_presence
}

fn status_event(change: &str, f: &FriendInfo, prev: Option<&FriendInfo>) -> Value {
    let mut ev = json!({
        "event": "friend_status", "change": change,
        "prev_state": prev.map(|p| p.state), "prev_game_appid": prev.map(|p| p.game_appid), "prev_joinable": prev.map(FriendInfo::joinable),
    });
    if let (Some(out), Value::Object(fields)) = (ev.as_object_mut(), f.to_json()) {
        for (key, value) in fields {
            out.entry(key).or_insert(value);
        }
    }
    ev
}

#[derive(Debug, Default)]
pub struct RequestWatch {
    known: HashMap<u64, FriendInfo>,
}

impl RequestWatch {
    pub fn new(initial: &[FriendInfo]) -> Self {
        RequestWatch { known: initial.iter().map(|f| (f.steam_id, f.clone())).collect() }
    }

    pub fn update(&mut self, current: &[FriendInfo]) -> Vec<Value> {
        let mut events = Vec::new();
        for f in current {
            if self.known.insert(f.steam_id, f.clone()).is_none() {
                events.push(request_event("received", f));
            }
        }
        let now: std::collections::HashSet<u64> = current.iter().map(|f| f.steam_id).collect();
        let gone: Vec<u64> = self.known.keys().filter(|id| !now.contains(id)).copied().collect();
        for id in gone {
            if let Some(old) = self.known.remove(&id) {
                events.push(request_event("closed", &old));
            }
        }
        events.sort_by_key(|e| e["steam_id"].as_str().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0));
        events
    }
}

fn request_event(change: &str, f: &FriendInfo) -> Value {
    json!({ "event": "friend_request", "change": change, "steam_id": f.steam_id.to_string(), "persona": f.persona })
}

pub fn friend_requests_event(requests: &[FriendInfo]) -> Value {
    let list: Vec<Value> = requests.iter().map(|f| json!({ "steam_id": f.steam_id.to_string(), "persona": f.persona })).collect();
    json!({ "event": "friend_requests", "requests": list })
}

pub fn chat_event(event: ChatEvent) -> Option<Value> {
    match event {
        ChatEvent::Message(e) => Some(chat_message(u64::from(e.source), false, e.message_no_bbcode.unwrap_or(e.message), e.server_timestamp)),
        ChatEvent::EchoMessage(e) => Some(chat_message(u64::from(e.source), true, e.message_no_bbcode.unwrap_or(e.message), e.server_timestamp)),
        ChatEvent::Typing(e) => Some(json!({ "event": "chat_typing", "steam_id": u64::from(e.source).to_string(), "timestamp": e.server_timestamp })),
        ChatEvent::GameInvite(e) => {
            let text = e.message_no_bbcode.unwrap_or(e.message);
            let mut ev = json!({ "event": "chat_invite", "steam_id": u64::from(e.source).to_string(), "text": text, "timestamp": e.server_timestamp });
            if let Some(lobby) = lobby_in(ev["text"].as_str().unwrap_or_default()) {
                ev["lobby"] = json!(lobby);
            }
            Some(ev)
        }
        _ => None,
    }
}

fn lobby_in(text: &str) -> Option<String> {
    let rest = text.split("connect_lobby").nth(1)?.trim_start();
    let id: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (!id.is_empty()).then_some(id)
}

fn chat_message(steam_id: u64, from_self: bool, text: String, timestamp: u64) -> Value {
    json!({ "event": "chat_message", "steam_id": steam_id.to_string(), "from_self": from_self, "text": text, "timestamp": timestamp })
}

pub const MAX_RECENT: usize = 200;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecentPlayers {
    list: Vec<RecentPlayer>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RecentPlayer {
    pub steam_id: u64,
    pub app_id: u32,
    pub last_played: u64,
}

pub fn steam_coplay_path(steam_id: u64) -> Option<std::path::PathBuf> {
    let home = crate::core::config::home_dir().ok()?;
    Some(home.join(".local/share/Steam/userdata").join((steam_id as u32).to_string()).join("config").join(format!("coplay_{steam_id}.vdf")))
}

enum Kv {
    Text(String),
    Block(Vec<(String, Kv)>),
}

fn parse_kv(text: &str) -> Vec<(String, Kv)> {
    fn tokens(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut it = text.chars().peekable();
        while let Some(c) = it.next() {
            match c {
                '"' => {
                    let mut s = String::new();
                    while let Some(c) = it.next() {
                        match c {
                            '"' => break,
                            '\\' => match it.next() { Some('n') => s.push('\n'), Some('t') => s.push('\t'), Some(o) => s.push(o), None => break },
                            c => s.push(c),
                        }
                    }
                    out.push(s);
                }
                '{' => out.push("\u{1}{".to_string()),
                '}' => out.push("\u{1}}".to_string()),
                '/' if it.peek() == Some(&'/') => {
                    for c in it.by_ref() {
                        if c == '\n' { break; }
                    }
                }
                _ => {}
            }
        }
        out
    }
    fn block(t: &[String], i: &mut usize) -> Vec<(String, Kv)> {
        let mut out = Vec::new();
        while *i < t.len() {
            if t[*i] == "\u{1}}" { *i += 1; return out; }
            let key = t[*i].clone();
            *i += 1;
            match t.get(*i).map(String::as_str) {
                Some("\u{1}{") => { *i += 1; out.push((key, Kv::Block(block(t, i)))); }
                Some(v) if v != "\u{1}}" => { out.push((key, Kv::Text(v.to_string()))); *i += 1; }
                _ => {}
            }
        }
        out
    }
    let t = tokens(text);
    let mut i = 0;
    block(&t, &mut i)
}

pub fn parse_steam_coplay(text: &str) -> Vec<RecentPlayer> {
    let mut out = Vec::new();
    for (key, node) in parse_kv(text) {
        let (true, Kv::Block(children)) = (key.eq_ignore_ascii_case("coplay"), node) else { continue };
        for (_, child) in children {
            let Kv::Block(fields) = child else { continue };
            let get = |name: &str| fields.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).and_then(|(_, v)| if let Kv::Text(t) = v { t.trim().parse::<u64>().ok() } else { None });
            if let (Some(steam_id), Some(game), Some(time)) = (get("steamID"), get("gameID"), get("playTime")) {
                if steam_id != 0 {
                    out.push(RecentPlayer { steam_id, app_id: (game & 0xFFFF_FFFF) as u32, last_played: time });
                }
            }
        }
    }
    out
}

pub fn format_steam_coplay(list: &[RecentPlayer]) -> String {
    let mut out = String::from("\"coplay\"\n{\n");
    for (i, p) in list.iter().enumerate() {
        out.push_str(&format!("\t\"{i}\"\n\t{{\n\t\t\"steamID\"\t\t\"{}\"\n\t\t\"gameID\"\t\t\"{}\"\n\t\t\"playTime\"\t\t\"{}\"\n\t}}\n", p.steam_id, p.app_id, p.last_played));
    }
    out.push_str("}\n");
    out
}

impl RecentPlayers {
    pub fn merge(&mut self, other: Vec<RecentPlayer>) {
        for p in other {
            match self.list.iter_mut().find(|q| q.steam_id == p.steam_id) {
                Some(q) if q.last_played >= p.last_played => {}
                Some(q) => *q = p,
                None => self.list.push(p),
            }
        }
        self.list.sort_by(|a, b| b.last_played.cmp(&a.last_played));
        self.list.truncate(MAX_RECENT);
    }

    pub fn save_steam(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("vdf.aurelia-tmp");
        std::fs::write(&tmp, format_steam_coplay(&self.list))?;
        std::fs::rename(&tmp, path)
    }

    pub fn load(path: &std::path::Path) -> Self {
        let list: Vec<RecentPlayer> = std::fs::read(path).ok().and_then(|raw| serde_json::from_slice(&raw).ok()).unwrap_or_default();
        RecentPlayers { list }
    }

    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&self.list)?)?;
        std::fs::rename(&tmp, path)
    }

    pub fn record(&mut self, steam_id: u64, app_id: u32, now: u64) {
        self.list.retain(|p| p.steam_id != steam_id);
        self.list.insert(0, RecentPlayer { steam_id, app_id, last_played: now });
        self.list.truncate(MAX_RECENT);
    }

    pub fn entries(&self) -> &[RecentPlayer] {
        &self.list
    }

    pub fn event(&self) -> Value {
        let players: Vec<Value> = self
            .list
            .iter()
            .map(|p| json!({ "steam_id": p.steam_id.to_string(), "app_id": p.app_id, "last_played": p.last_played }))
            .collect();
        json!({ "event": "players_recent", "players": players })
    }
}

#[cfg(test)]
#[path = "social_tests.rs"]
mod tests;
