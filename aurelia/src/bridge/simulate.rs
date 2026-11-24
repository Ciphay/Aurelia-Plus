use serde_json::{Value, json};

use crate::bridge::overlay::{self, Command, OverlayHub, Query};
use crate::bridge::social;
use crate::bridge::social::FriendInfo;

pub const STEPS: usize = 7;

const ME: u64 = 76561198000000001;
const FRIEND: u64 = 76561198000000002;
const OTHER: u64 = 76561198000000003;

fn friends() -> Vec<FriendInfo> {
    let mut rp = std::collections::BTreeMap::new();
    rp.insert("status".into(), "In match (example)".into());
    rp.insert("connect".into(), "+connect 127.0.0.1:27015".into());
    vec![
        FriendInfo { steam_id: FRIEND, persona: "Ana (example)".into(), state: 1, game_appid: 480, game_name: Some("Spacewar".into()), rich_presence: rp, ..Default::default() },
        FriendInfo { steam_id: OTHER, persona: "Beto (example)".into(), ..Default::default() },
    ]
}

fn sample_achievements() -> Vec<crate::steam_client::GameAchievement> {
    let a = |api: &str, name: &str, unlocked: bool| crate::steam_client::GameAchievement {
        api_name: api.into(),
        name: name.into(),
        description: format!("Description of {name}"),
        icon_unlocked: "https://example.test/icon.jpg".into(),
        icon_locked: "https://example.test/icon_gray.jpg".into(),
        global_percent: 12.5,
        unlocked,
        unlock_time: unlocked.then_some(1_700_000_000),
        ..Default::default()
    };
    vec![a("ACH_FIRST", "First", true), a("ACH_SECOND", "Segunda", false)]
}

pub fn emit_step(hub: &OverlayHub, app_id: u32, step: usize) {
    match step {
        0 => {
            hub.request(app_id, "store", json!({ "target": { "app_id": 1234, "flag": 0 } }));
        }
        1 => {
            hub.request(app_id, "web_page", json!({ "url": "https://example.test/pagina", "mode": "default" }));
        }
        2 => {
            hub.microtxn(
                app_id,
                json!({
                    "order_id": "9001", "trans_id": "1234567890", "sandbox": true, "refundable": false, "requires_cached_payment_method": false,
                    "realm": 1, "currency": "BRL", "currency_code": 7,
                    "url": "https://example.test/approvetxn/1234567890/", "window": { "width": 1020, "height": 440 },
                    "total": "23.95", "total_minor": 2395,
                    "items": [{ "name": "500 Coins", "qty": 1, "price": "23.95", "amount_minor": 2395, "item_id": "847133" }],
                }),
            );
        }
        3 => {
            let mut watch = social::FriendWatch::new(&[FriendInfo { steam_id: FRIEND, persona: "Ana (example)".into(), state: 1, ..Default::default() }]);
            for ev in watch.update(&friends()[..1]) {
                hub.emit(ev);
            }
            hub.emit(json!({ "event": "friend_request", "change": "received", "steam_id": OTHER.to_string(), "persona": "Beto (example)" }));
        }
        4 => {
            hub.emit(json!({ "event": "chat_typing", "steam_id": FRIEND.to_string(), "timestamp": 1_700_000_000u64 }));
            hub.emit(json!({ "event": "chat_message", "steam_id": FRIEND.to_string(), "from_self": false, "text": "Bora jogar?", "timestamp": 1_700_000_001u64 }));
            hub.emit(json!({ "event": "chat_invite", "steam_id": FRIEND.to_string(), "text": "+connect 127.0.0.1:27015", "timestamp": 1_700_000_002u64 }));
        }
        5 => {
            let found = sample_achievements().into_iter().nth(1);
            hub.emit(overlay::unlocked_event(json!({ "event": "achievement_unlocked", "app_id": app_id, "api_name": "ACH_SECOND" }), found.as_ref()));
        }
        6 => hub.dlc_changed(app_id, 1001, true, false),
        _ => {}
    }
}

fn query_reply(query: &Query) -> Value {
    match query {
        Query::Friends => overlay::friends_event(&friends()),
        Query::Join { .. } => social::join_event(&friends()[0], true),
        Query::FriendRequests => social::friend_requests_event(&[FriendInfo { steam_id: OTHER, persona: "Beto (example)".into(), ..Default::default() }]),
        Query::ChatSend { .. } => overlay::chat_sent_event(1_700_000_100, None),
        Query::ChatHistory { steam_id, .. } => overlay::chat_history_event(
            *steam_id,
            &[
                crate::steam_client::ChatMessage { sender: *steam_id, from_self: false, message: "Oi!".into(), timestamp: 1_700_000_001 },
                crate::steam_client::ChatMessage { sender: ME, from_self: true, message: "Hi, how are you?".into(), timestamp: 1_700_000_002 },
            ],
        ),
        Query::Achievements { app_id, .. } => overlay::achievements_event(*app_id, &sample_achievements()),
        Query::Profile { steam_id } => overlay::profile_event(&crate::steam_client::UserProfile {
            steam_id: *steam_id,
            persona_name: Some("Player (example)".into()),
            profile_url: format!("https://steamcommunity.com/profiles/{steam_id}"),
            visibility: 3,
            is_public: true,
            persona_state: Some(1),
            ..Default::default()
        }),
        Query::FriendAdd { steam_id } => overlay::friend_added_event(*steam_id, Some("Player (example)")),
        Query::Persona { .. } | Query::FriendRemove { .. } | Query::Invite { .. } => json!({ "event": "result", "ok": true }),
    }
}

pub fn answer(hub: &OverlayHub, line: &str) -> Value {
    match overlay::parse_command(line) {
        Ok(Command::Query { req, query }) => overlay::with_req(query_reply(&query), &req),
        Ok(Command::WebSession) => overlay::web_session_event(ME, &[("sessionid".into(), "example".into()), ("steamLoginSecure".into(), "example".into())]),
        Ok(cmd) => overlay::apply(hub, cmd),
        Err(_) => overlay::line_error(line),
    }
}

pub fn seed_recent(hub: &OverlayHub, app_id: u32) {
    hub.played_with(app_id, &json!({ "steam_id": FRIEND }));
}

#[cfg(test)]
#[path = "simulate_tests.rs"]
mod tests;
