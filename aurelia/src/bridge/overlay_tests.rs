use super::*;
use serde_json::json;

fn drain(rx: &mut tokio::sync::mpsc::UnboundedReceiver<serde_json::Value>) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    while let Ok(v) = rx.try_recv() {
        out.push(v);
    }
    out
}

#[test]
fn request_emits_overlay_request_event_with_unique_ids() {
    let (hub, mut rx) = OverlayHub::new();
    let a = hub.request(550, "store", json!({"target": {"app_id": 1234, "dlc": true}}));
    let b = hub.request(550, "web_page", json!({"url": "https://example.test"}));
    assert_ne!(a, b);
    let events = drain(&mut rx);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["event"], "overlay_request");
    assert_eq!(events[0]["id"], a);
    assert_eq!(events[0]["app_id"], 550);
    assert_eq!(events[0]["kind"], "store");
    assert_eq!(events[0]["target"]["app_id"], 1234);
    assert_eq!(events[1]["url"], "https://example.test");
}

#[test]
fn respond_approve_queues_answer_and_closes_with_answered() {
    let (hub, mut rx) = OverlayHub::new();
    let id = hub.microtxn(550, json!({"order_id": "77", "total": "9.99"}));
    let first = drain(&mut rx);
    assert_eq!(first[0]["event"], "microtxn_request");
    assert_eq!(first[0]["order_id"], "77");
    hub.respond(&id, true).unwrap();
    let closed = drain(&mut rx);
    assert_eq!(closed[0]["event"], "overlay_closed");
    assert_eq!(closed[0]["reason"], "answered");
    assert_eq!(hub.drain_updates(), vec![Update::Answer { id, answer: Answer::Approved }]);
}

#[test]
fn respond_deny_by_user_is_queued_as_denied_user() {
    let (hub, _rx) = OverlayHub::new();
    let id = hub.microtxn(550, json!({}));
    hub.respond(&id, false).unwrap();
    assert_eq!(hub.drain_updates(), vec![Update::Answer { id, answer: Answer::Denied(DenyReason::User) }]);
}

#[test]
fn respond_unknown_id_is_unknown_id_error() {
    let (hub, _rx) = OverlayHub::new();
    assert_eq!(hub.respond("999", true), Err(OverlayError::UnknownId));
}

#[test]
fn respond_twice_is_not_pending_error() {
    let (hub, _rx) = OverlayHub::new();
    let id = hub.microtxn(550, json!({}));
    hub.respond(&id, true).unwrap();
    assert_eq!(hub.respond(&id, true), Err(OverlayError::NotPending));
}

#[test]
fn respond_to_plain_overlay_request_is_not_microtxn_error() {
    let (hub, _rx) = OverlayHub::new();
    let id = hub.request(550, "friends", json!({}));
    assert_eq!(hub.respond(&id, true), Err(OverlayError::NotMicrotxn));
}

#[test]
fn two_microtxns_answer_independently() {
    let (hub, _rx) = OverlayHub::new();
    let a = hub.microtxn(550, json!({}));
    let b = hub.microtxn(550, json!({}));
    assert_ne!(a, b);
    hub.respond(&b, true).unwrap();
    hub.respond(&a, false).unwrap();
    assert_eq!(
        hub.drain_updates(),
        vec![
            Update::Answer { id: b, answer: Answer::Approved },
            Update::Answer { id: a, answer: Answer::Denied(DenyReason::User) },
        ]
    );
}

#[test]
fn close_all_denies_pending_with_game_exit_once() {
    let (hub, mut rx) = OverlayHub::new();
    let a = hub.microtxn(550, json!({}));
    let b = hub.microtxn(550, json!({}));
    drain(&mut rx);
    hub.close_all("game_exit");
    let events = drain(&mut rx);
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|e| e["event"] == "overlay_closed" && e["reason"] == "game_exit"));
    let ids: Vec<_> = events.iter().map(|e| e["id"].as_str().unwrap().to_string()).collect();
    assert!(ids.contains(&a) && ids.contains(&b));
    hub.close_all("game_exit");
    assert!(drain(&mut rx).is_empty());
    assert!(hub.drain_updates().iter().all(|u| matches!(u, Update::Answer { answer: Answer::Denied(DenyReason::Game), .. })));
}

#[test]
fn dismiss_emits_dismissed_by_steam() {
    let (hub, mut rx) = OverlayHub::new();
    let id = hub.microtxn(550, json!({}));
    drain(&mut rx);
    hub.dismiss(&id);
    let events = drain(&mut rx);
    assert_eq!(events[0]["event"], "overlay_closed");
    assert_eq!(events[0]["reason"], "dismissed_by_steam");
    assert_eq!(hub.respond(&id, true), Err(OverlayError::NotPending));
    assert_eq!(hub.drain_updates(), vec![Update::Answer { id, answer: Answer::Denied(DenyReason::Steam) }]);
}

#[test]
fn capable_defaults_false_and_toggles() {
    let (hub, _rx) = OverlayHub::new();
    assert!(!hub.capable());
    hub.set_capable(true);
    assert!(hub.capable());
    hub.set_capable(false);
    assert!(!hub.capable());
}

#[test]
fn shown_and_hidden_queue_updates_in_order() {
    let (hub, _rx) = OverlayHub::new();
    let id = hub.request(550, "store", json!({}));
    hub.shown(&id).unwrap();
    hub.hidden(&id).unwrap();
    assert_eq!(hub.drain_updates(), vec![Update::Shown { id: id.clone() }, Update::Hidden { id }]);
    assert!(hub.drain_updates().is_empty());
}

#[test]
fn shown_unknown_id_is_unknown_id_error() {
    let (hub, _rx) = OverlayHub::new();
    assert_eq!(hub.shown("5"), Err(OverlayError::UnknownId));
}

#[test]
fn list_reports_pending() {
    let (hub, _rx) = OverlayHub::new();
    let m = hub.microtxn(550, json!({"order_id": "1"}));
    let _plain = hub.request(550, "store", json!({}));
    let list = hub.list();
    let pending = list.as_array().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["id"], m);
    assert_eq!(pending[0]["kind"], "microtxn");
}

fn request_payload(app: u32, kind: &str, json: &[u8]) -> Vec<u8> {
    let mut p = vec![OP_REQUEST];
    p.extend_from_slice(&app.to_le_bytes());
    p.extend_from_slice(&(kind.len() as u16).to_le_bytes());
    p.extend_from_slice(kind.as_bytes());
    p.extend_from_slice(&(json.len() as u16).to_le_bytes());
    p.extend_from_slice(json);
    p
}

#[test]
fn parse_request_roundtrip() {
    let p = request_payload(550, "store", br#"{"target":{"app_id":1234}}"#);
    let (app, kind, json) = parse_request(&p).unwrap();
    assert_eq!((app, kind.as_str()), (550, "store"));
    assert_eq!(json["target"]["app_id"], 1234);
}

#[test]
fn parse_request_rejects_short_payload() {
    assert!(parse_request(&[]).is_none());
    assert!(parse_request(&[OP_REQUEST, 1, 0, 0]).is_none());
    let mut p = request_payload(550, "store", b"{}");
    p.truncate(p.len() - 1);
    assert!(parse_request(&p).is_none());
}

#[test]
fn parse_request_rejects_wrong_op() {
    let mut p = request_payload(550, "store", b"{}");
    p[0] = OP_POLL;
    assert!(parse_request(&p).is_none());
}

#[test]
fn parse_request_truncates_oversized_strings_to_2048() {
    let long = format!(r#"{{"url":"{}"}}"#, "a".repeat(5000));
    let p = request_payload(550, "web_page", long.as_bytes());
    let (_, _, json) = parse_request(&p).unwrap();
    assert_eq!(json["url"].as_str().unwrap().len(), MAX_JSON);
}

#[test]
fn encode_updates_layout() {
    let updates = [
        Update::Answer { id: "7".into(), answer: Answer::Approved },
        Update::Answer { id: "8".into(), answer: Answer::Denied(DenyReason::User) },
        Update::Answer { id: "9".into(), answer: Answer::Denied(DenyReason::Steam) },
        Update::Answer { id: "10".into(), answer: Answer::Denied(DenyReason::Game) },
        Update::Shown { id: "1".into() },
        Update::Hidden { id: "1".into() },
    ];
    let b = encode_updates(&updates);
    assert_eq!(&b[..4], &6u32.to_le_bytes());
    assert_eq!(&b[4..8], &[1, 1, 0, b'7']);
    assert_eq!(&b[8..12], &[2, 1, 0, b'8']);
    assert_eq!(&b[12..16], &[3, 1, 0, b'9']);
    assert_eq!(&b[16..21], &[2, 2, 0, b'1', b'0']);
    assert_eq!(&b[21..25], &[4, 1, 0, b'1']);
    assert_eq!(&b[25..29], &[5, 1, 0, b'1']);
    assert_eq!(b.len(), 29);
}

#[test]
fn encode_state_layout() {
    assert_eq!(encode_state(true), vec![1]);
    assert_eq!(encode_state(false), vec![0]);
}

#[test]
fn overlay_call_request_returns_id_and_emits_event() {
    let (hub, mut rx) = OverlayHub::new();
    let (status, reply) = overlay_call(&hub, 550, &request_payload(0, "store", br#"{"target":{"app_id":1234}}"#));
    assert_eq!(status, 0);
    let id_len = u16::from_le_bytes([reply[0], reply[1]]) as usize;
    let id = std::str::from_utf8(&reply[2..2 + id_len]).unwrap();
    let ev = drain(&mut rx);
    assert_eq!(ev[0]["id"], id);
    assert_eq!(ev[0]["app_id"], 550, "the app is the one from bridge serve, not the one the shim sent (0)");
    assert_eq!(ev[0]["kind"], "store");
}

#[test]
fn overlay_call_microtxn_kind_creates_pending_microtxn() {
    let (hub, mut rx) = OverlayHub::new();
    let (status, _) = overlay_call(&hub, 550, &request_payload(0, "microtxn", br#"{"order_id":"5"}"#));
    assert_eq!(status, 0);
    assert_eq!(drain(&mut rx)[0]["event"], "microtxn_request");
    assert_eq!(hub.list().as_array().unwrap().len(), 1);
}

#[test]
fn overlay_call_poll_returns_updates_once() {
    let (hub, _rx) = OverlayHub::new();
    let id = hub.request(550, "store", json!({}));
    hub.shown(&id).unwrap();
    let (status, reply) = overlay_call(&hub, 550, &[OP_POLL]);
    assert_eq!(status, 0);
    assert_eq!(reply, encode_updates(&[Update::Shown { id }]));
    assert_eq!(overlay_call(&hub, 550, &[OP_POLL]).1, encode_updates(&[]));
}

#[test]
fn overlay_call_state_reports_capable() {
    let (hub, _rx) = OverlayHub::new();
    assert_eq!(overlay_call(&hub, 550, &[OP_STATE]), (0, vec![0]));
    hub.set_capable(true);
    assert_eq!(overlay_call(&hub, 550, &[OP_STATE]), (0, vec![1]));
}

#[test]
fn overlay_call_close_all_denies_pending() {
    let (hub, mut rx) = OverlayHub::new();
    hub.microtxn(550, json!({}));
    drain(&mut rx);
    assert_eq!(overlay_call(&hub, 550, &[OP_CLOSE_ALL]).0, 0);
    assert_eq!(drain(&mut rx)[0]["reason"], "game_exit");
}

#[test]
fn overlay_call_rejects_garbage() {
    let (hub, _rx) = OverlayHub::new();
    assert_eq!(overlay_call(&hub, 550, &[]).0, -2);
    assert_eq!(overlay_call(&hub, 550, &[99]).0, -2);
    assert_eq!(overlay_call(&hub, 550, &[OP_REQUEST, 1]).0, -2);
}

#[test]
fn parse_command_respond_approve() {
    assert_eq!(parse_command(r#"{"cmd":"respond","id":"9","approve":true}"#), Ok(Command::Respond { id: "9".into(), approve: true }));
    assert_eq!(parse_command(r#"{"cmd":"respond","id":"9","approve":false}"#), Ok(Command::Respond { id: "9".into(), approve: false }));
}

#[test]
fn parse_command_other_commands() {
    assert_eq!(parse_command(r#"{"cmd":"shown","id":"3"}"#), Ok(Command::Shown { id: "3".into() }));
    assert_eq!(parse_command(r#"{"cmd":"hidden","id":"3"}"#), Ok(Command::Hidden { id: "3".into() }));
    assert_eq!(parse_command(r#"{"cmd":"capable","on":true}"#), Ok(Command::Capable { on: true }));
    assert_eq!(parse_command(r#"{"cmd":"stop"}"#), Ok(Command::Stop));
    assert_eq!(parse_command(r#"{"cmd":"list"}"#), Ok(Command::List));
}

#[test]
fn parse_command_rejects_unknown_cmd() {
    assert!(parse_command(r#"{"cmd":"explode"}"#).is_err());
    assert!(parse_command(r#"{"id":"1"}"#).is_err());
}

#[test]
fn parse_command_rejects_missing_fields() {
    assert!(parse_command(r#"{"cmd":"respond","id":"9"}"#).is_err());
    assert!(parse_command(r#"{"cmd":"shown"}"#).is_err());
    assert!(parse_command(r#"{"cmd":"capable"}"#).is_err());
}

#[test]
fn parse_command_rejects_broken_json() {
    assert!(parse_command("{not json").is_err());
    assert!(parse_command("").is_err());
}

#[test]
fn parse_command_rejects_oversized_line() {
    let line = format!(r#"{{"cmd":"list","pad":"{}"}}"#, "x".repeat(9000));
    assert!(parse_command(&line).is_err());
}

#[test]
fn apply_respond_unknown_id_is_error_event() {
    let (hub, _rx) = OverlayHub::new();
    let out = apply(&hub, Command::Respond { id: "42".into(), approve: true });
    assert_eq!(out, json!({"event": "error", "error": "unknown_id"}));
}

#[test]
fn apply_respond_not_microtxn_is_error_event() {
    let (hub, _rx) = OverlayHub::new();
    let id = hub.request(550, "store", json!({}));
    assert_eq!(apply(&hub, Command::Respond { id, approve: true }), json!({"event": "error", "error": "not_microtxn"}));
}

#[test]
fn apply_respond_ok_is_result_event() {
    let (hub, _rx) = OverlayHub::new();
    let id = hub.microtxn(550, json!({}));
    assert_eq!(apply(&hub, Command::Respond { id, approve: true }), json!({"event": "result", "ok": true}));
}

#[test]
fn apply_capable_sets_flag() {
    let (hub, _rx) = OverlayHub::new();
    assert_eq!(apply(&hub, Command::Capable { on: true }), json!({"event": "result", "ok": true}));
    assert!(hub.capable());
}

#[test]
fn apply_list_reports_pending_microtxns() {
    let (hub, _rx) = OverlayHub::new();
    let id = hub.microtxn(550, json!({}));
    let out = apply(&hub, Command::List);
    assert_eq!(out["event"], "overlay_list");
    assert_eq!(out["pending"][0]["id"], id);
}

#[test]
fn invalid_line_event_is_an_error_event() {
    let ev = line_error("{quebrado");
    assert_eq!(ev["event"], "error");
    assert_eq!(ev["error"], "invalid_command");
}

#[test]
fn dlc_changed_emits_event_with_the_new_state() {
    let (hub, mut rx) = OverlayHub::new();
    hub.dlc_changed(550, 1234, true, false);
    let events = drain(&mut rx);
    assert_eq!(events, vec![json!({"event": "dlc_changed", "app_id": 550, "dlc_id": 1234, "owned": true, "installed": false})]);
}

fn dlc(id: u32, owned: bool, installed: bool) -> DlcState {
    DlcState { dlc_id: id, owned, installed }
}

#[test]
fn diff_dlc_reports_only_differences() {
    let old = [dlc(1, true, true), dlc(2, false, false), dlc(3, true, false)];
    let new = [dlc(1, true, true), dlc(2, true, false), dlc(3, true, true)];
    assert_eq!(diff_dlc(&old, &new), vec![dlc(2, true, false), dlc(3, true, true)]);
}

#[test]
fn diff_dlc_with_no_changes_is_empty() {
    let s = [dlc(1, true, true)];
    assert!(diff_dlc(&s, &s).is_empty());
}

#[test]
fn diff_dlc_reports_dlc_that_appeared() {
    assert_eq!(diff_dlc(&[dlc(1, true, true)], &[dlc(1, true, true), dlc(9, true, false)]), vec![dlc(9, true, false)]);
}

#[test]
fn set_dlc_emits_events_only_for_differences_and_keeps_the_new_state() {
    let (hub, mut rx) = OverlayHub::new();
    hub.init_dlc(vec![dlc(1, true, true), dlc(2, false, false)]);
    assert!(drain(&mut rx).is_empty(), "the initial state emits no event");
    hub.set_dlc(550, vec![dlc(1, true, true), dlc(2, true, true)]);
    assert_eq!(drain(&mut rx), vec![json!({"event": "dlc_changed", "app_id": 550, "dlc_id": 2, "owned": true, "installed": true})]);
    hub.set_dlc(550, vec![dlc(1, true, true), dlc(2, true, true)]);
    assert!(drain(&mut rx).is_empty(), "no change, no event");
    assert_eq!(hub.dlc(), vec![dlc(1, true, true), dlc(2, true, true)]);
}

#[test]
fn encode_dlc_layout() {
    let b = encode_dlc(&[dlc(1234, true, false), dlc(7, false, true)]);
    assert_eq!(&b[..4], &2u32.to_le_bytes());
    assert_eq!(&b[4..10], &[0xD2, 0x04, 0, 0, 1, 0]);
    assert_eq!(&b[10..16], &[7, 0, 0, 0, 0, 1]);
    assert_eq!(b.len(), 16);
}

#[test]
fn overlay_call_dlc_returns_the_current_list() {
    let (hub, _rx) = OverlayHub::new();
    hub.init_dlc(vec![dlc(5, true, true)]);
    assert_eq!(overlay_call(&hub, 550, &[OP_DLC]), (0, encode_dlc(&[dlc(5, true, true)])));
}

#[test]
fn parse_command_steam_url() {
    assert_eq!(
        parse_command(r#"{"cmd":"steam_url","id":"9","url":"steam://close/cancel"}"#),
        Ok(Command::SteamUrl { id: "9".into(), url: "steam://close/cancel".into() })
    );
    assert!(parse_command(r#"{"cmd":"steam_url","id":"9"}"#).is_err());
}

#[test]
fn steam_url_authorize_approves_and_cancel_denies_like_the_steam_overlay() {
    let (hub, _rx) = OverlayHub::new();
    let a = hub.microtxn(550, json!({}));
    let b = hub.microtxn(550, json!({}));
    assert_eq!(apply(&hub, Command::SteamUrl { id: a.clone(), url: "steam://close/authorize".into() }), json!({"event": "result", "ok": true}));
    assert_eq!(apply(&hub, Command::SteamUrl { id: b.clone(), url: "steam://close/cancel?x=1".into() }), json!({"event": "result", "ok": true}));
    assert_eq!(
        hub.drain_updates(),
        vec![
            Update::Answer { id: a, answer: Answer::Approved },
            Update::Answer { id: b, answer: Answer::Denied(DenyReason::User) },
        ]
    );
}

#[test]
fn steam_url_with_other_target_is_an_unsupported_url_error() {
    let (hub, _rx) = OverlayHub::new();
    let id = hub.microtxn(550, json!({}));
    assert_eq!(
        apply(&hub, Command::SteamUrl { id: id.clone(), url: "steam://open/friends".into() }),
        json!({"event": "error", "error": "unsupported_url"})
    );
    assert_eq!(hub.drain_updates(), vec![], "a URL that is not a decision does not answer the request");
    assert_eq!(hub.list().as_array().unwrap().len(), 1);
}

#[test]
fn steam_url_for_unknown_id_is_unknown_id_error() {
    let (hub, _rx) = OverlayHub::new();
    assert_eq!(apply(&hub, Command::SteamUrl { id: "77".into(), url: "steam://close/cancel".into() }), json!({"event": "error", "error": "unknown_id"}));
}

#[test]
fn parse_command_web_session() {
    assert_eq!(parse_command(r#"{"cmd":"web_session"}"#), Ok(Command::WebSession));
}

#[test]
fn web_session_event_carries_cookies_for_the_steam_domains() {
    let ev = web_session_event(76561198000000001, &[("steamLoginSecure".into(), "1%7C%7Ct".into()), ("sessionid".into(), "abc".into())]);
    assert_eq!(ev["event"], "web_session");
    assert_eq!(ev["steam_id"], "76561198000000001");
    let cookies = ev["cookies"].as_array().unwrap();
    let domains: std::collections::BTreeSet<&str> = cookies.iter().map(|c| c["domain"].as_str().unwrap()).collect();
    assert_eq!(domains, [".steamcommunity.com", ".steampowered.com"].into_iter().collect());
    assert_eq!(cookies.len(), 4, "each cookie is emitted once per domain");
    let login = cookies.iter().find(|c| c["name"] == "steamLoginSecure" && c["domain"] == ".steampowered.com").unwrap();
    assert_eq!((login["value"].as_str(), login["secure"].as_bool(), login["http_only"].as_bool(), login["path"].as_str()), (Some("1%7C%7Ct"), Some(true), Some(true), Some("/")));
    let sid = cookies.iter().find(|c| c["name"] == "sessionid").unwrap();
    assert_eq!(sid["http_only"], false);
}

#[test]
fn web_session_error_event_is_typed() {
    assert_eq!(web_session_error("no session"), json!({"event": "error", "error": "web_session_failed", "detail": "no session"}));
}

fn query_of(line: &str) -> (Option<String>, Query) {
    match parse_command(line) {
        Ok(Command::Query { req, query }) => (req, query),
        other => panic!("esperava Query, veio {other:?}"),
    }
}

#[test]
fn parse_query_friends_keeps_req() {
    assert_eq!(query_of(r#"{"cmd":"friends","req":"a1"}"#), (Some("a1".into()), Query::Friends));
    assert_eq!(query_of(r#"{"cmd":"friends"}"#), (None, Query::Friends));
}

#[test]
fn steam_id_accepts_text_or_number_and_rejects_the_rest() {
    let want = Query::Profile { steam_id: 76561198000000001 };
    assert_eq!(query_of(r#"{"cmd":"profile","steam_id":"76561198000000001"}"#).1, want);
    assert_eq!(query_of(r#"{"cmd":"profile","steam_id":76561198000000001}"#).1, want);
    assert_eq!(parse_command(r#"{"cmd":"profile","steam_id":"abc"}"#), Err("invalid_field: steam_id".into()));
    assert_eq!(parse_command(r#"{"cmd":"profile"}"#), Err("missing_field: steam_id".into()));
}

#[test]
fn chat_send_needs_message_and_chat_history_clamps_count() {
    assert_eq!(
        query_of(r#"{"cmd":"chat_send","steam_id":"7","message":"oi"}"#).1,
        Query::ChatSend { steam_id: 7, message: "oi".into() }
    );
    assert_eq!(parse_command(r#"{"cmd":"chat_send","steam_id":"7"}"#), Err("missing_field: message".into()));
    assert_eq!(query_of(r#"{"cmd":"chat_history","steam_id":"7"}"#).1, Query::ChatHistory { steam_id: 7, count: 20 });
    assert_eq!(query_of(r#"{"cmd":"chat_history","steam_id":"7","count":5000}"#).1, Query::ChatHistory { steam_id: 7, count: MAX_CHAT_HISTORY });
    assert_eq!(query_of(r#"{"cmd":"chat_history","steam_id":"7","count":0}"#).1, Query::ChatHistory { steam_id: 7, count: 1 });
}

#[test]
fn achievements_language_is_optional_and_app_id_is_validated() {
    assert_eq!(query_of(r#"{"cmd":"achievements","app_id":480}"#).1, Query::Achievements { app_id: 480, lang: None });
    assert_eq!(
        query_of(r#"{"cmd":"achievements","app_id":480,"lang":"brazilian"}"#).1,
        Query::Achievements { app_id: 480, lang: Some("brazilian".into()) }
    );
    assert_eq!(parse_command(r#"{"cmd":"achievements","app_id":99999999999}"#), Err("invalid_field: app_id".into()));
    assert_eq!(parse_command(r#"{"cmd":"achievements"}"#), Err("missing_field: app_id".into()));
}

#[test]
fn persona_state_is_limited_to_the_known_range() {
    assert_eq!(query_of(r#"{"cmd":"persona","state":6}"#).1, Query::Persona { state: 6 });
    assert_eq!(parse_command(r#"{"cmd":"persona","state":7}"#), Err("invalid_field: state".into()));
}

#[test]
fn friend_add_and_remove_parse() {
    assert_eq!(query_of(r#"{"cmd":"friend_add","steam_id":"9"}"#).1, Query::FriendAdd { steam_id: 9 });
    assert_eq!(query_of(r#"{"cmd":"friend_remove","steam_id":"9"}"#).1, Query::FriendRemove { steam_id: 9 });
}

#[test]
fn with_req_adds_the_field_only_when_present() {
    assert_eq!(with_req(json!({"event": "result"}), &Some("x".into())), json!({"event": "result", "req": "x"}));
    assert_eq!(with_req(json!({"event": "result"}), &None), json!({"event": "result"}));
}

#[test]
fn friends_event_writes_steam_ids_as_text_and_tells_who_is_joinable() {
    let ana = crate::bridge::social::FriendInfo { steam_id: 76561198000000001, persona: "Ana".into(), state: 1, game_appid: 480, lobby: 109775240000000001, ..Default::default() };
    let ev = friends_event(&[ana]);
    let f = &ev["friends"][0];
    assert_eq!(ev["event"], "friends");
    assert_eq!((f["steam_id"].as_str(), f["persona"].as_str(), f["state"].as_u64(), f["game_appid"].as_u64()), (Some("76561198000000001"), Some("Ana"), Some(1), Some(480)));
    assert_eq!((f["joinable"].as_bool(), f["lobby"].as_str(), f["connect"].as_str()), (Some(true), Some("109775240000000001"), Some("+connect_lobby 109775240000000001")));
}

#[test]
fn chat_history_event_shapes_messages() {
    let m = crate::steam_client::ChatMessage { sender: 8, from_self: false, message: "hey there".into(), timestamp: 100 };
    assert_eq!(
        chat_history_event(8, &[m]),
        json!({"event": "chat_history", "steam_id": "8", "messages": [{"sender": "8", "from_self": false, "message": "hey there", "timestamp": 100}]})
    );
}

#[test]
fn achievements_event_carries_rarity_icons_and_unlock_time() {
    let a = crate::steam_client::GameAchievement {
        api_name: "ACH_1".into(), name: "First".into(), description: "d".into(), hidden: false,
        icon_unlocked: "u".into(), icon_locked: "l".into(), global_percent: 12.5, unlocked: true, unlock_time: Some(5),
    };
    let ev = achievements_event(480, &[a]);
    assert_eq!(ev["event"], "achievements");
    assert_eq!(ev["app_id"], 480);
    let first = &ev["achievements"][0];
    assert_eq!((first["api_name"].as_str(), first["global_percent"].as_f64(), first["unlock_time"].as_u64()), (Some("ACH_1"), Some(12.5), Some(5)));
}

#[test]
fn profile_event_is_tagged_and_steam_id_is_text() {
    let ev = profile_event(&crate::steam_client::UserProfile { steam_id: 76561198000000001, persona_name: Some("Ana".into()), ..Default::default() });
    assert_eq!((ev["event"].as_str(), ev["steam_id"].as_str(), ev["persona_name"].as_str()), (Some("profile"), Some("76561198000000001"), Some("Ana")));
}

#[test]
fn query_without_a_client_is_an_error_that_keeps_req() {
    let (hub, _rx) = OverlayHub::new();
    let reply = apply(&hub, Command::Query { req: Some("r".into()), query: Query::Friends });
    assert_eq!((reply["event"].as_str(), reply["error"].as_str(), reply["req"].as_str()), (Some("error"), Some("query_failed"), Some("r")));
}

#[test]
fn line_error_keeps_req_when_the_line_is_json() {
    let ev = line_error(r#"{"cmd":"profile","req":"q9"}"#);
    assert_eq!((ev["error"].as_str(), ev["req"].as_str()), (Some("invalid_command"), Some("q9")));
    assert!(line_error("not json").get("req").is_none());
}

fn ach_payload(name: &str) -> Vec<u8> {
    let kind = b"achievement";
    let json = format!(r#"{{"api_name":"{name}"}}"#);
    let mut p = vec![OP_REQUEST];
    p.extend_from_slice(&480u32.to_le_bytes());
    p.extend_from_slice(&(kind.len() as u16).to_le_bytes());
    p.extend_from_slice(kind);
    p.extend_from_slice(&(json.len() as u16).to_le_bytes());
    p.extend_from_slice(json.as_bytes());
    p
}

#[test]
fn achievement_request_becomes_an_event_without_a_pending_entry() {
    let (hub, mut rx) = OverlayHub::new();
    let (status, reply) = overlay_call(&hub, 480, &ach_payload("ACH_WIN_ONE_GAME"));
    assert_eq!((status, reply), (0, vec![0, 0]));
    assert_eq!(drain(&mut rx), vec![json!({"event": "achievement_unlocked", "app_id": 480, "api_name": "ACH_WIN_ONE_GAME"})]);
    assert_eq!(hub.list(), json!([]), "does not become a pending request");
}

#[test]
fn unlocked_event_gains_the_achievement_details_when_known() {
    let base = json!({"event": "achievement_unlocked", "app_id": 480, "api_name": "ACH_1"});
    let a = crate::steam_client::GameAchievement {
        api_name: "ACH_1".into(), name: "First".into(), description: "d".into(), icon_unlocked: "u".into(), icon_locked: "l".into(),
        global_percent: 12.5, unlocked: true, unlock_time: Some(9), ..Default::default()
    };
    let ev = unlocked_event(base.clone(), Some(&a));
    assert_eq!((ev["name"].as_str(), ev["global_percent"].as_f64(), ev["unlock_time"].as_u64(), ev["icon_unlocked"].as_str()), (Some("First"), Some(12.5), Some(9), Some("u")));
    assert_eq!(unlocked_event(base.clone(), None), base, "without the definition, only the API name follows");
}

#[test]
fn invite_takes_an_optional_connect_line() {
    assert_eq!(
        query_of(r#"{"cmd":"invite","steam_id":"9","connect":"+connect 1.2.3.4","req":"i"}"#),
        (Some("i".into()), Query::Invite { steam_id: 9, connect: Some("+connect 1.2.3.4".into()) })
    );
    assert_eq!(query_of(r#"{"cmd":"invite","steam_id":"9"}"#).1, Query::Invite { steam_id: 9, connect: None });
    assert_eq!(parse_command(r#"{"cmd":"invite"}"#), Err("missing_field: steam_id".into()));
}

fn played_with_payload(steam_id: u64) -> Vec<u8> {
    let kind = b"played_with";
    let json = format!(r#"{{"steam_id":{steam_id}}}"#);
    let mut p = vec![OP_REQUEST];
    p.extend_from_slice(&480u32.to_le_bytes());
    p.extend_from_slice(&(kind.len() as u16).to_le_bytes());
    p.extend_from_slice(kind);
    p.extend_from_slice(&(json.len() as u16).to_le_bytes());
    p.extend_from_slice(json.as_bytes());
    p
}

#[test]
fn played_with_from_the_shim_shows_up_in_players_recent_without_a_pending_entry() {
    let (hub, mut rx) = OverlayHub::new();
    assert_eq!(overlay_call(&hub, 480, &played_with_payload(76561198000000007)), (0, vec![0, 0]));
    assert_eq!(overlay_call(&hub, 480, &played_with_payload(0)), (0, vec![0, 0]), "id 0 is ignored");
    let parsed = parse_command(r#"{"cmd":"players_recent","req":"r"}"#).unwrap();
    let ev = apply(&hub, parsed);
    assert_eq!((ev["event"].as_str(), ev["req"].as_str()), (Some("players_recent"), Some("r")));
    assert_eq!(ev["players"].as_array().unwrap().len(), 1);
    assert_eq!((ev["players"][0]["steam_id"].as_str(), ev["players"][0]["app_id"].as_u64()), (Some("76561198000000007"), Some(480)));
    assert_eq!(hub.list(), json!([]));
    assert!(drain(&mut rx).is_empty(), "emits no event: the launcher queries when it wants");
}

fn invite_payload(lobby: u64, inviter: u64) -> Vec<u8> {
    let kind = b"lobby_invite";
    let json = format!(r#"{{"inviter":{inviter},"lobby":{lobby},"game":480}}"#);
    let mut p = vec![OP_REQUEST];
    p.extend_from_slice(&480u32.to_le_bytes());
    p.extend_from_slice(&(kind.len() as u16).to_le_bytes());
    p.extend_from_slice(kind);
    p.extend_from_slice(&(json.len() as u16).to_le_bytes());
    p.extend_from_slice(json.as_bytes());
    p
}

#[test]
fn lobby_invite_from_the_shim_becomes_an_event_and_stays_pending_until_answered() {
    let (hub, mut rx) = OverlayHub::new();
    let (status, _) = overlay_call(&hub, 480, &invite_payload(109775240000000001, 76561198000000002));
    assert_eq!(status, 0);
    let ev = &drain(&mut rx)[0];
    assert_eq!((ev["event"].as_str(), ev["lobby"].as_str(), ev["inviter"].as_str(), ev["game"].as_str()), (Some("lobby_invite"), Some("109775240000000001"), Some("76561198000000002"), Some("480")));
    assert_eq!(hub.list().as_array().unwrap().len(), 1, "o convite aparece em `list`");
}

#[test]
fn accept_invite_queues_the_join_for_the_game_and_closes_the_invite() {
    let (hub, mut rx) = OverlayHub::new();
    overlay_call(&hub, 480, &invite_payload(109775240000000001, 76561198000000002));
    let id = drain(&mut rx)[0]["id"].as_str().unwrap().to_string();
    assert_eq!(apply(&hub, parse_command(&format!(r#"{{"cmd":"accept_invite","id":"{id}"}}"#)).unwrap()), json!({"event": "result", "ok": true}));
    assert_eq!(hub.drain_updates(), vec![Update::JoinLobby { id: id.clone(), lobby: 109775240000000001, friend: 76561198000000002 }]);
    let closed = drain(&mut rx);
    assert_eq!((closed[0]["event"].as_str(), closed[0]["reason"].as_str()), (Some("overlay_closed"), Some("accepted")));
    assert_eq!(hub.accept_invite(&id), Err(OverlayError::UnknownId), "only accepts once");
}

#[test]
fn join_update_encoding_carries_lobby_and_friend() {
    let bytes = encode_updates(&[Update::JoinLobby { id: "7".into(), lobby: 1, friend: 2 }]);
    let mut want = 1u32.to_le_bytes().to_vec();
    want.extend([6u8, 1, 0, b'7']);
    want.extend(1u64.to_le_bytes());
    want.extend(2u64.to_le_bytes());
    assert_eq!(bytes, want);
}

#[test]
fn decline_and_wrong_kind_are_handled() {
    let (hub, mut rx) = OverlayHub::new();
    overlay_call(&hub, 480, &invite_payload(5, 6));
    let id = drain(&mut rx)[0]["id"].as_str().unwrap().to_string();
    hub.decline_invite(&id).unwrap();
    assert_eq!(drain(&mut rx)[0]["reason"], "declined");
    assert!(hub.drain_updates().is_empty(), "declining does not reach the game");
    let store = hub.request(480, "store", json!({}));
    assert_eq!(hub.accept_invite(&store), Err(OverlayError::NotInvite));
    assert_eq!(hub.accept_invite("999"), Err(OverlayError::UnknownId));
}

#[test]
fn an_invite_without_a_lobby_is_ignored() {
    let (hub, mut rx) = OverlayHub::new();
    overlay_call(&hub, 480, &invite_payload(0, 6));
    assert!(drain(&mut rx).is_empty());
}

#[test]
fn closing_the_game_closes_pending_invites() {
    let (hub, mut rx) = OverlayHub::new();
    overlay_call(&hub, 480, &invite_payload(5, 6));
    drain(&mut rx);
    hub.close_all("game_exit");
    assert_eq!(drain(&mut rx)[0]["reason"], "game_exit");
    assert_eq!(hub.list(), json!([]));
}

#[test]
fn parse_join_and_friend_requests() {
    assert_eq!(query_of(r#"{"cmd":"join","steam_id":"9","req":"j"}"#), (Some("j".into()), Query::Join { steam_id: 9 }));
    assert_eq!(query_of(r#"{"cmd":"friend_requests"}"#).1, Query::FriendRequests);
    assert_eq!(parse_command(r#"{"cmd":"join"}"#), Err("missing_field: steam_id".into()));
}

fn text_request(hub: &OverlayHub) -> String {
    hub.request(480, "gamepad_text", json!({"description": "Your name", "max_chars": 16, "existing": ""}))
}

#[test]
fn the_keyboard_answer_reaches_the_game_once_and_closes_the_request() {
    let (hub, mut rx) = OverlayHub::new();
    let id = text_request(&hub);
    assert_eq!(drain(&mut rx)[0]["kind"], "gamepad_text");
    let line = format!(r#"{{"cmd":"text_input","id":"{id}","text":"Ana"}}"#);
    assert_eq!(apply(&hub, parse_command(&line).unwrap()), json!({"event": "result", "ok": true}));
    assert_eq!(hub.drain_updates(), vec![Update::Text { id: id.clone(), submitted: true, text: "Ana".into() }]);
    assert_eq!(drain(&mut rx)[0]["reason"], "answered");
    assert_eq!(hub.respond_text(&id, true, "x"), Err(OverlayError::UnknownId), "only responds once");
}

#[test]
fn cancelling_the_keyboard_is_reported_as_not_submitted() {
    let (hub, mut rx) = OverlayHub::new();
    let id = text_request(&hub);
    drain(&mut rx);
    let line = format!(r#"{{"cmd":"text_input","id":"{id}","cancel":true}}"#);
    apply(&hub, parse_command(&line).unwrap());
    assert_eq!(hub.drain_updates(), vec![Update::Text { id, submitted: false, text: String::new() }]);
    assert_eq!(drain(&mut rx)[0]["reason"], "cancelled");
}

#[test]
fn text_input_only_answers_a_keyboard_request_and_is_capped() {
    let (hub, _rx) = OverlayHub::new();
    let store = hub.request(480, "store", json!({}));
    assert_eq!(hub.respond_text(&store, true, "x"), Err(OverlayError::NotTextInput));
    let id = text_request(&hub);
    hub.respond_text(&id, true, &"é".repeat(5000)).unwrap();
    let Update::Text { text, .. } = &hub.drain_updates()[0] else { panic!("esperava Text") };
    assert!(text.len() <= MAX_TEXT && text.chars().all(|c| c == 'é'), "truncated without splitting a character");
}

#[test]
fn text_update_encoding_carries_the_flag_and_the_text() {
    let bytes = encode_updates(&[Update::Text { id: "7".into(), submitted: true, text: "oi".into() }]);
    let mut want = 1u32.to_le_bytes().to_vec();
    want.extend([7u8, 1, 0, b'7', 1, 2, 0, b'o', b'i']);
    assert_eq!(bytes, want);
}

#[test]
fn the_shim_can_read_recent_players_friend_lobbies_and_cloud_changes() {
    let (hub, _rx) = OverlayHub::new();
    hub.played_with(480, &json!({"steam_id": 76561198000000007u64}));
    hub.set_friend_lobbies(vec![
        FriendLobby { friend: 11, lobby: 111, app_id: 480 },
        FriendLobby { friend: 22, lobby: 222, app_id: 440 },
    ]);
    hub.set_cloud_changes(vec!["save1.dat".into(), "perfil/ana.cfg".into()]);

    let (status, recent) = overlay_call(&hub, 480, &[OP_RECENT]);
    assert_eq!((status, u32::from_le_bytes(recent[..4].try_into().unwrap())), (0, 1));
    assert_eq!(u64::from_le_bytes(recent[4..12].try_into().unwrap()), 76561198000000007);
    assert_eq!(u32::from_le_bytes(recent[12..16].try_into().unwrap()), 480);

    let (_, lobbies) = overlay_call(&hub, 480, &[OP_FRIEND_LOBBIES]);
    assert_eq!(u32::from_le_bytes(lobbies[..4].try_into().unwrap()), 1, "only the lobbies of this game");
    assert_eq!((u64::from_le_bytes(lobbies[4..12].try_into().unwrap()), u64::from_le_bytes(lobbies[12..20].try_into().unwrap())), (11, 111));

    let (_, changes) = overlay_call(&hub, 480, &[OP_CLOUD_CHANGES]);
    assert_eq!(u32::from_le_bytes(changes[..4].try_into().unwrap()), 2);
    assert_eq!(u16::from_le_bytes(changes[4..6].try_into().unwrap()), 9);
    assert_eq!(&changes[6..15], b"save1.dat");
}

fn floating_request(hub: &OverlayHub) -> String {
    hub.request(480, "gamepad_text", json!({"floating": true, "mode": 0, "left": 10, "top": 20, "width": 300, "height": 100}))
}

#[test]
fn a_floating_keyboard_keeps_the_request_open_while_keys_arrive_and_closes_on_done() {
    let (hub, mut rx) = OverlayHub::new();
    let id = floating_request(&hub);
    drain(&mut rx);
    hub.respond_text(&id, true, "ol").unwrap();
    hub.respond_text(&id, true, "á").unwrap();
    assert_eq!(
        hub.drain_updates(),
        vec![
            Update::Keys { id: id.clone(), text: "ol".into(), done: false },
            Update::Keys { id: id.clone(), text: "á".into(), done: false }
        ]
    );
    assert!(drain(&mut rx).is_empty(), "the keyboard stays open");
    hub.respond_text(&id, true, "").unwrap();
    assert_eq!(hub.drain_updates(), vec![Update::Keys { id: id.clone(), text: String::new(), done: true }]);
    assert_eq!(drain(&mut rx)[0]["reason"], "answered");
    assert_eq!(hub.respond_text(&id, true, "x"), Err(OverlayError::UnknownId));
}

#[test]
fn cancelling_a_floating_keyboard_closes_it() {
    let (hub, mut rx) = OverlayHub::new();
    let id = floating_request(&hub);
    drain(&mut rx);
    hub.respond_text(&id, false, "").unwrap();
    assert_eq!(hub.drain_updates(), vec![Update::Keys { id, text: String::new(), done: true }]);
    assert_eq!(drain(&mut rx)[0]["reason"], "cancelled");
}

#[test]
fn the_game_can_close_the_keyboard_itself() {
    let (hub, mut rx) = OverlayHub::new();
    floating_request(&hub);
    drain(&mut rx);
    let kind = b"gamepad_text_close";
    let mut p = vec![OP_REQUEST];
    p.extend_from_slice(&480u32.to_le_bytes());
    p.extend_from_slice(&(kind.len() as u16).to_le_bytes());
    p.extend_from_slice(kind);
    p.extend_from_slice(&0u16.to_le_bytes());
    assert_eq!(overlay_call(&hub, 480, &p).0, 0);
    assert_eq!(drain(&mut rx)[0]["reason"], "dismissed_by_game");
}

#[test]
fn keys_update_encoding_carries_the_end_flag_and_the_text() {
    let bytes = encode_updates(&[Update::Keys { id: "7".into(), text: "ab".into(), done: false }]);
    let mut want = 1u32.to_le_bytes().to_vec();
    want.extend([8u8, 1, 0, b'7', 0, 2, 0, b'a', b'b']);
    assert_eq!(bytes, want);
}

#[test]
fn a_timeline_request_becomes_a_timeline_event_with_the_games_fields() {
    let (hub, mut rx) = OverlayHub::new();
    let kind = b"timeline";
    let body = br#"{"kind":"instant","title":"Ace","description":"5 kills","icon":"trophy","priority":3,"offset":-2.5}"#;
    let mut p = vec![OP_REQUEST];
    p.extend_from_slice(&550u32.to_le_bytes());
    p.extend_from_slice(&(kind.len() as u16).to_le_bytes());
    p.extend_from_slice(kind);
    p.extend_from_slice(&(body.len() as u16).to_le_bytes());
    p.extend_from_slice(body);
    assert_eq!(overlay_call(&hub, 550, &p).0, 0);
    let ev = rx.try_recv().expect("evento");
    assert_eq!(ev["event"], "timeline_event");
    assert_eq!(ev["app_id"], 550);
    assert_eq!(ev["title"], "Ace");
    assert_eq!(ev["kind"], "instant");
    assert_eq!(ev["offset"], -2.5);
    assert!(rx.try_recv().is_err(), "no overlay request is left pending");
}
