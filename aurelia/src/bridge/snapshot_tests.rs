use std::collections::HashSet;
use std::time::Instant;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use super::*;
use crate::core::models::OwnedGame;

fn game(id: u32) -> OwnedGame {
    OwnedGame {
        app_id: id,
        name: format!("Game {id}"),
        playtime_forever_minutes: 0,
        local_manifest_ids: Default::default(),
        update_available: false,
    }
}

fn account() -> Account {
    Account { steamid: 76561198000000001, persona: "Tester".into(), country: "BR".into() }
}

#[test]
fn snapshot_serializes_to_expected_schema() {
    let installed = HashSet::from([2u32]);
    let ticket = encode_ticket(2, &[1, 2, 3], &[4, 5]);
    let snap = assemble(account(), &[game(1), game(2)], &installed, Some(ticket));
    let v = serde_json::to_value(&snap).unwrap();
    assert_eq!(v["schema"], 1);
    assert_eq!(v["account"]["steamid"], 76561198000000001u64);
    assert_eq!(v["account"]["persona"], "Tester");
    assert_eq!(v["account"]["country"], "BR");
    assert_eq!(v["apps"][0], serde_json::json!({"appid": 1, "name": "Game 1", "installed": false}));
    assert_eq!(v["apps"][1]["installed"], true);
    assert_eq!(v["ticket"]["appid"], 2);
    assert!(assemble(account(), &[], &installed, None).ticket.is_none());
}

#[test]
fn ticket_fields_are_base64() {
    let t = encode_ticket(7, &[1, 2, 3], &[4, 5]);
    assert_eq!(STANDARD.decode(&t.ownership_ticket_b64).unwrap(), vec![1, 2, 3]);
    assert_eq!(STANDARD.decode(&t.game_connect_token_b64).unwrap(), vec![4, 5]);
    assert_eq!(t.appid, 7);
}

#[test]
fn snapshot_with_5000_apps_is_under_8mib_and_5s() {
    let games: Vec<OwnedGame> = (0..5000).map(game).collect();
    let started = Instant::now();
    let snap = assemble(account(), &games, &HashSet::new(), None);
    let bytes = serde_json::to_vec(&snap).unwrap();
    assert!(started.elapsed().as_secs() < 5);
    assert!(bytes.len() < 8 * 1024 * 1024, "{} bytes", bytes.len());
    assert_eq!(snap.apps.len(), 5000);
}

#[test]
fn snapshot_carries_app_info_only_when_set() {
    let snap = assemble(account(), &[], &HashSet::new(), None);
    assert!(serde_json::to_value(&snap).unwrap().get("app").is_none());
    let app = AppInfo {
        appid: 2,
        install_dir: "/games/two".into(),
        cloud_dir: "/home/u/.local/share/Steam/userdata/1/2/remote".into(),
        cloud_quota: 1_000_000_000,
        build_id: 26384159,
        beta_name: "public".into(),
        dlc: vec![DlcEntry { appid: 20, owned: true, installed: false, name: "Pacote".into() }],
        installed_depots: vec![21, 22],
        betas: vec![BetaBranch { name: "beta".into(), description: "Next patch".into(), build_id: 99, time_updated: 1_700_000_000, private: false }],
        purchase_time: 1_600_000_000,
    };
    let v = serde_json::to_value(snap.with_app(app)).unwrap();
    assert_eq!(v["app"]["purchase_time"], 1_600_000_000u64);
    assert_eq!(v["app"]["betas"][0], serde_json::json!({"name": "beta", "description": "Next patch", "build_id": 99, "time_updated": 1_700_000_000u64, "private": false}));
    assert_eq!(v["app"]["appid"], 2);
    assert_eq!(v["app"]["installed_depots"], serde_json::json!([21, 22]));
    assert_eq!(v["app"]["install_dir"], "/games/two");
    assert_eq!(v["app"]["cloud_dir"], "/home/u/.local/share/Steam/userdata/1/2/remote");
    assert_eq!(v["app"]["cloud_quota"], 1_000_000_000u64);
    assert_eq!(v["app"]["build_id"], 26384159u64);
    assert_eq!(v["app"]["beta_name"], "public");
    assert_eq!(v["app"]["dlc"][0], serde_json::json!({"appid": 20, "owned": true, "installed": false, "name": "Pacote"}));
}

#[test]
fn snapshot_carries_friends_only_when_any() {
    let snap = assemble(account(), &[], &HashSet::new(), None);
    assert!(serde_json::to_value(&snap).unwrap().get("friends").is_none());
    let friends = vec![FriendEntry { steamid: 76561198000000042, persona: "Ana".into(), state: 1, game_appid: 570 }];
    let v = serde_json::to_value(snap.with_friends(friends)).unwrap();
    assert_eq!(v["friends"][0], serde_json::json!({"steamid": 76561198000000042u64, "persona": "Ana", "state": 1, "game_appid": 570}));
}

#[test]
fn the_profile_xml_gives_the_display_name() {
    assert_eq!(profile_name("<profile><steamID64>1</steamID64><steamID><![CDATA[Rafaelo]]></steamID></profile>").as_deref(), Some("Rafaelo"));
    assert_eq!(profile_name("<profile><steamID>Ana</steamID></profile>").as_deref(), Some("Ana"));
    assert_eq!(profile_name("<profile><steamID><![CDATA[]]></steamID></profile>"), None);
    assert_eq!(profile_name("<response><error>not found</error></response>"), None);
}

#[test]
fn the_miniprofile_gives_the_account_name_of_an_unconfigured_profile() {
    assert_eq!(miniprofile_name("<div><span class=\"persona offline\">itachi &amp; co</span></div>").as_deref(), Some("itachi & co"));
    assert_eq!(miniprofile_name("<span class=\"persona online\"></span>"), None);
    assert_eq!(miniprofile_name("<div>nothing</div>"), None);
}

fn owned(app_id: u32, name: &str) -> OwnedGame {
    OwnedGame { app_id, name: name.into(), playtime_forever_minutes: 0, local_manifest_ids: Default::default(), update_available: false }
}

#[test]
fn offline_snapshot_has_account_games_and_app_but_no_ticket() {
    let account = Account { steamid: 76561198000000001, persona: "Player".into(), country: String::new() };
    let games = [owned(550, "Left 4 Dead 2"), owned(480, "Spacewar")];
    let installed: HashSet<u32> = [550].into_iter().collect();
    let app = AppInfo { appid: 550, install_dir: "/g/l4d2".into(), cloud_dir: "/c/550/remote".into(), cloud_quota: 0, build_id: 7, beta_name: "public".into(), dlc: Vec::new(), installed_depots: vec![551], betas: Vec::new(), purchase_time: 0 };
    let s = assemble_offline(account.clone(), &games, &installed, Some(app.clone()));
    assert_eq!(s.account, account);
    assert!(!s.online, "the shim must answer BLoggedOn false");
    assert!(s.ticket.is_none(), "no ownership ticket without Steam");
    assert!(s.friends.is_empty());
    assert_eq!(s.app, Some(app));
    assert_eq!(s.apps.len(), 2);
    assert!(s.apps.iter().find(|a| a.appid == 550).unwrap().installed);
    assert!(!s.apps.iter().find(|a| a.appid == 480).unwrap().installed);
    assert_eq!(s.schema, SCHEMA);
}
