use super::*;
use steam_vent_chat::{MessageEvent, TypingEvent};
use steamid_ng::SteamID;

fn friend(id: u64, persona: &str, state: u32, game: u32) -> FriendInfo {
    FriendInfo { steam_id: id, persona: persona.into(), state, game_appid: game, ..Default::default() }
}

const A: u64 = 76561198000000001;
const B: u64 = 76561198000000002;

#[test]
fn first_reading_equal_to_the_initial_list_emits_nothing() {
    let list = [friend(A, "Ana", 1, 0)];
    assert!(FriendWatch::new(&list).update(&list).is_empty());
}

#[test]
fn state_and_game_changes_emit_friend_status_with_previous_values() {
    let mut w = FriendWatch::new(&[friend(A, "Ana", 0, 0)]);
    let ev = w.update(&[friend(A, "Ana", 1, 480)]);
    assert_eq!(ev.len(), 1);
    let e = &ev[0];
    assert_eq!((e["event"].as_str(), e["change"].as_str(), e["steam_id"].as_str(), e["persona"].as_str()), (Some("friend_status"), Some("updated"), Some(A.to_string().as_str()), Some("Ana")));
    assert_eq!((e["state"].as_u64(), e["game_appid"].as_u64(), e["prev_state"].as_u64(), e["prev_game_appid"].as_u64()), (Some(1), Some(480), Some(0), Some(0)));
    assert!(w.update(&[friend(A, "Ana", 1, 480)]).is_empty(), "no change, no event");
}

#[test]
fn a_friend_who_opens_a_lobby_becomes_joinable() {
    let mut w = FriendWatch::new(&[friend(A, "Ana", 1, 480)]);
    let mut open = friend(A, "Ana", 1, 480);
    open.lobby = 109775240000000001;
    let ev = w.update(&[open]);
    assert_eq!(ev.len(), 1);
    assert_eq!((ev[0]["joinable"].as_bool(), ev[0]["prev_joinable"].as_bool(), ev[0]["lobby"].as_str()), (Some(true), Some(false), Some("109775240000000001")));
    assert_eq!(ev[0]["connect"], "+connect_lobby 109775240000000001");
}

#[test]
fn rich_presence_changes_emit_and_the_game_connect_string_wins() {
    let mut w = FriendWatch::new(&[friend(A, "Ana", 1, 480)]);
    let mut f = friend(A, "Ana", 1, 480);
    f.lobby = 5;
    f.rich_presence.insert("connect".into(), "+connect 1.2.3.4:27015".into());
    f.rich_presence.insert("status".into(), "In match".into());
    let ev = w.update(&[f.clone()]);
    assert_eq!((ev[0]["connect"].as_str(), ev[0]["status"].as_str()), (Some("+connect 1.2.3.4:27015"), Some("In match")), "the game's connect wins over the lobby");
    f.rich_presence.insert("status".into(), "No menu".into());
    assert_eq!(w.update(&[f]).len(), 1, "only the rich presence changed");
}

#[test]
fn a_friend_not_in_a_game_is_never_joinable() {
    let mut f = friend(A, "Ana", 1, 0);
    f.lobby = 5;
    f.server = Some("1.2.3.4:27015".into());
    assert!(!f.joinable() && f.connect_line().is_none());
    f.game_appid = 480;
    f.lobby = 0;
    assert_eq!(f.connect_line().as_deref(), Some("+connect 1.2.3.4:27015"), "no connect and no lobby, the server wins");
}

#[test]
fn only_a_name_change_is_silent_but_is_remembered() {
    let mut w = FriendWatch::new(&[friend(A, "", 1, 0)]);
    assert!(w.update(&[friend(A, "Ana", 1, 0)]).is_empty());
    let ev = w.update(&[friend(A, "Ana", 3, 0)]);
    assert_eq!((ev[0]["persona"].as_str(), ev[0]["prev_state"].as_u64()), (Some("Ana"), Some(1)));
}

#[test]
fn new_and_removed_friends_are_reported() {
    let mut w = FriendWatch::new(&[friend(A, "Ana", 1, 0)]);
    let ev = w.update(&[friend(B, "Beto", 0, 0)]);
    let kinds: Vec<(&str, &str)> = ev.iter().map(|e| (e["change"].as_str().unwrap(), e["steam_id"].as_str().unwrap())).collect();
    assert_eq!(kinds, [("removed", A.to_string().as_str()), ("added", B.to_string().as_str())]);
    assert!(ev.iter().find(|e| e["change"] == "added").unwrap()["prev_state"].is_null());
}

fn msg(text: &str, bbcode_free: Option<&str>) -> MessageEvent {
    MessageEvent { source: SteamID::try_from(A).unwrap(), message: text.into(), message_no_bbcode: bbcode_free.map(str::to_string), server_timestamp: 77 }
}

#[test]
fn chat_message_prefers_text_without_bbcode_and_echo_is_from_self() {
    let ev = chat_event(ChatEvent::Message(msg("[b]oi[/b]", Some("oi")))).unwrap();
    assert_eq!(ev, json!({"event": "chat_message", "steam_id": A.to_string(), "from_self": false, "text": "oi", "timestamp": 77}));
    let echo = chat_event(ChatEvent::EchoMessage(msg("eco", None))).unwrap();
    assert_eq!((echo["from_self"].as_bool(), echo["text"].as_str()), (Some(true), Some("eco")));
}

#[test]
fn typing_and_game_invite_have_their_own_events() {
    let typing = chat_event(ChatEvent::Typing(TypingEvent { source: SteamID::try_from(A).unwrap(), server_timestamp: 5 })).unwrap();
    assert_eq!(typing, json!({"event": "chat_typing", "steam_id": A.to_string(), "timestamp": 5}));
    let invite = chat_event(ChatEvent::GameInvite(msg("vem jogar", None))).unwrap();
    assert_eq!((invite["event"].as_str(), invite["text"].as_str()), (Some("chat_invite"), Some("vem jogar")));
}

#[test]
fn recent_players_put_the_latest_first_and_dedupe() {
    let mut r = RecentPlayers::default();
    r.record(A, 480, 100);
    r.record(B, 480, 200);
    r.record(A, 440, 300);
    let ev = r.event();
    let ids: Vec<&str> = ev["players"].as_array().unwrap().iter().map(|p| p["steam_id"].as_str().unwrap()).collect();
    assert_eq!(ids, [A.to_string(), B.to_string()]);
    assert_eq!((ev["players"][0]["app_id"].as_u64(), ev["players"][0]["last_played"].as_u64()), (Some(440), Some(300)));
}

#[test]
fn recent_players_are_capped() {
    let mut r = RecentPlayers::default();
    for i in 0..(MAX_RECENT as u64 + 5) {
        r.record(1000 + i, 480, i);
    }
    let ev = r.event();
    assert_eq!(ev["players"].as_array().unwrap().len(), MAX_RECENT);
    assert_eq!(ev["players"][0]["steam_id"], (1000 + MAX_RECENT as u64 + 4).to_string());
}

#[test]
fn recent_players_survive_a_save_and_a_broken_file_reads_as_empty() {
    let dir = std::env::temp_dir().join(format!("aurelia-recent-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("recent_players.json");
    let mut r = RecentPlayers::default();
    r.record(A, 480, 100);
    r.save(&path).unwrap();
    assert_eq!(RecentPlayers::load(&path), r);
    std::fs::write(&path, b"{ quebrado").unwrap();
    assert_eq!(RecentPlayers::load(&path), RecentPlayers::default());
    assert_eq!(RecentPlayers::load(&dir.join("nao_existe.json")), RecentPlayers::default());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn chat_invite_to_a_lobby_carries_the_lobby_id() {
    let ev = chat_event(ChatEvent::GameInvite(msg("+connect_lobby 109775240000000001 +foo", None))).unwrap();
    assert_eq!(ev["lobby"].as_str(), Some("109775240000000001"));
    let plain = chat_event(ChatEvent::GameInvite(msg("+connect 1.2.3.4:27015", None))).unwrap();
    assert!(plain.get("lobby").is_none());
}

#[test]
fn friend_requests_are_announced_once_and_closed_when_they_go_away() {
    let mut w = RequestWatch::new(&[]);
    let ev = w.update(&[friend(B, "Beto", 0, 0)]);
    assert_eq!((ev[0]["event"].as_str(), ev[0]["change"].as_str(), ev[0]["steam_id"].as_str()), (Some("friend_request"), Some("received"), Some(B.to_string().as_str())));
    assert!(w.update(&[friend(B, "Beto", 0, 0)]).is_empty());
    assert_eq!(w.update(&[])[0]["change"], "closed");
}

#[test]
fn the_roster_splits_into_friends_and_received_requests() {
    let mk = |id: u64, rel: u32| Friend { steam_id: id, relationship: rel, persona_name: Some(format!("p{id}")), persona_state: None, game_app_id: None, game_name: None, game_lobby_id: None, game_server: None, rich_presence: Default::default() };
    let (friends, requests) = split_friends(&[mk(1, 3), mk(2, 2), mk(3, 4), mk(4, 3)]);
    assert_eq!(friends.iter().map(|f| f.steam_id).collect::<Vec<_>>(), [1, 4]);
    assert_eq!(requests.iter().map(|f| f.steam_id).collect::<Vec<_>>(), [2]);
}

#[test]
fn join_event_says_what_to_open_and_whether_it_is_installed() {
    let mut f = friend(A, "Ana", 1, 480);
    f.game_name = Some("Spacewar".into());
    f.lobby = 109775240000000001;
    let ev = join_event(&f, false);
    assert_eq!((ev["app_id"].as_u64(), ev["connect"].as_str(), ev["lobby"].as_str(), ev["installed"].as_bool(), ev["needs_install"].as_bool()),
        (Some(480), Some("+connect_lobby 109775240000000001"), Some("109775240000000001"), Some(false), Some(true)));
}

#[test]
fn steam_coplay_file_round_trips_in_the_client_format() {
    let list = vec![
        RecentPlayer { steam_id: 76561198000000042, app_id: 550, last_played: 1_795_000_000 },
        RecentPlayer { steam_id: 76561198000000043, app_id: 730, last_played: 1_794_000_000 },
    ];
    let text = format_steam_coplay(&list);
    assert!(text.starts_with("\"coplay\"\n{"), "{text}");
    for key in ["\"steamID\"", "\"gameID\"", "\"playTime\""] {
        assert!(text.contains(key), "{key} in {text}");
    }
    assert_eq!(parse_steam_coplay(&text), list);
}

#[test]
fn a_coplay_file_written_by_the_real_client_is_read_whatever_the_subkey_names_and_comments() {
    let text = "// gerado pelo cliente\n\"coplay\"\n{\n\t\"a\"\n\t{\n\t\t\"steamID\"\t\"76561198000000099\"\n\t\t\"gameID\"\t\"440\"\n\t\t\"playTime\"\t\"1700000000\"\n\t}\n\t\"b\"\n\t{\n\t\t\"steamID\"\t\"0\"\n\t\t\"gameID\"\t\"1\"\n\t\t\"playTime\"\t\"5\"\n\t}\n\t\"c\"\n\t{\n\t\t\"steamID\"\t\"76561198000000100\"\n\t}\n}\n";
    let got = parse_steam_coplay(text);
    assert_eq!(got, vec![RecentPlayer { steam_id: 76561198000000099, app_id: 440, last_played: 1_700_000_000 }], "zero id and incomplete entry are left out");
    assert!(parse_steam_coplay("junk { no end").is_empty());
    assert!(parse_steam_coplay("").is_empty());
}

#[test]
fn merging_keeps_the_newest_time_per_player_and_sorts_by_recency() {
    let mut mine = RecentPlayers::default();
    mine.record(1, 550, 100);
    mine.record(2, 550, 200);
    mine.merge(vec![
        RecentPlayer { steam_id: 1, app_id: 730, last_played: 300 },
        RecentPlayer { steam_id: 2, app_id: 730, last_played: 50 },
        RecentPlayer { steam_id: 3, app_id: 440, last_played: 150 },
    ]);
    let got: Vec<(u64, u32, u64)> = mine.entries().iter().map(|p| (p.steam_id, p.app_id, p.last_played)).collect();
    assert_eq!(got, vec![(1, 730, 300), (2, 550, 200), (3, 440, 150)]);
}

#[test]
fn saving_in_the_client_format_is_atomic_and_readable_back() {
    let dir = std::env::temp_dir().join(format!("aurelia-coplay-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("config").join("coplay_1.vdf");
    let mut r = RecentPlayers::default();
    r.record(7, 550, 1234);
    r.save_steam(&path).unwrap();
    assert_eq!(parse_steam_coplay(&std::fs::read_to_string(&path).unwrap()), r.entries());
    assert!(!path.with_extension("vdf.aurelia-tmp").exists());
}
