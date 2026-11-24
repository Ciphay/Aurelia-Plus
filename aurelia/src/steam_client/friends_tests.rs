use super::*;
use steam_vent_proto::steammessages_clientserver_friends::{
    cmsg_client_friends_list, cmsg_client_persona_state,
};

fn friends_list(incremental: bool, entries: &[(u64, u32)]) -> CMsgClientFriendsList {
    let mut msg = CMsgClientFriendsList::new();
    msg.set_bincremental(incremental);
    for &(id, rel) in entries {
        let mut f = cmsg_client_friends_list::Friend::new();
        f.set_ulfriendid(id);
        f.set_efriendrelationship(rel);
        msg.friends.push(f);
    }
    msg
}

fn persona_state(entries: &[(u64, &str, u32, u32)]) -> CMsgClientPersonaState {
    let mut msg = CMsgClientPersonaState::new();
    for &(id, name, state, app) in entries {
        let mut f = cmsg_client_persona_state::Friend::new();
        f.set_friendid(id);
        f.set_player_name(name.to_string());
        f.set_persona_state(state);
        f.set_game_played_app_id(app);
        msg.friends.push(f);
    }
    msg
}

#[test]
fn full_snapshot_populates_relationships() {
    let mut roster = Roster::new();
    let msg = friends_list(false, &[(10, RELATIONSHIP_FRIEND), (20, 2)]);
    apply_friends_list(&mut roster, &msg);

    assert_eq!(roster.len(), 2);
    assert_eq!(roster[&10].relationship, RELATIONSHIP_FRIEND);
    assert_eq!(roster[&20].relationship, 2);
    assert!(roster[&10].persona_name.is_none());
}

#[test]
fn full_snapshot_clears_previous_entries() {
    let mut roster = Roster::new();
    apply_friends_list(
        &mut roster,
        &friends_list(false, &[(1, RELATIONSHIP_FRIEND)]),
    );
    apply_friends_list(
        &mut roster,
        &friends_list(false, &[(2, RELATIONSHIP_FRIEND)]),
    );
    assert!(!roster.contains_key(&1));
    assert!(roster.contains_key(&2));
}

#[test]
fn persona_state_fills_fields() {
    let mut roster = Roster::new();
    apply_friends_list(
        &mut roster,
        &friends_list(false, &[(10, RELATIONSHIP_FRIEND)]),
    );
    apply_persona_state(&mut roster, &persona_state(&[(10, "Alice", 1, 440)]));

    let f = &roster[&10];
    assert_eq!(f.persona_name.as_deref(), Some("Alice"));
    assert_eq!(f.persona_state, Some(1));
    assert_eq!(f.game_app_id, Some(440));
}

#[test]
fn persona_state_does_not_clobber_known_name_with_empty() {
    let mut roster = Roster::new();
    apply_friends_list(
        &mut roster,
        &friends_list(false, &[(10, RELATIONSHIP_FRIEND)]),
    );
    apply_persona_state(&mut roster, &persona_state(&[(10, "Alice", 1, 0)]));
    apply_persona_state(&mut roster, &persona_state(&[(10, "", 0, 0)]));

    assert_eq!(roster[&10].persona_name.as_deref(), Some("Alice"));
    assert_eq!(roster[&10].persona_state, Some(0));
    assert_eq!(roster[&10].game_app_id, None);
}

#[test]
fn incremental_relationship_zero_removes_entry() {
    let mut roster = Roster::new();
    apply_friends_list(
        &mut roster,
        &friends_list(false, &[(10, RELATIONSHIP_FRIEND)]),
    );
    assert!(roster.contains_key(&10));

    apply_friends_list(&mut roster, &friends_list(true, &[(10, 0)]));
    assert!(!roster.contains_key(&10));
}

#[test]
fn incremental_upsert_preserves_persona_fields() {
    let mut roster = Roster::new();
    apply_friends_list(
        &mut roster,
        &friends_list(false, &[(10, RELATIONSHIP_FRIEND)]),
    );
    apply_persona_state(&mut roster, &persona_state(&[(10, "Alice", 1, 440)]));

    apply_friends_list(&mut roster, &friends_list(true, &[(10, 6)]));
    assert_eq!(roster[&10].relationship, 6);
    assert_eq!(roster[&10].persona_name.as_deref(), Some("Alice"));
    assert_eq!(roster[&10].persona_state, Some(1));
}

fn name_only(id: u64, name: &str) -> CMsgClientPersonaState {
    let mut msg = CMsgClientPersonaState::new();
    let mut f = cmsg_client_persona_state::Friend::new();
    f.set_friendid(id);
    f.set_player_name(name.to_string());
    msg.friends.push(f);
    msg
}

#[test]
fn reply_without_status_keeps_known_state_and_game() {
    let mut roster = Roster::new();
    apply_friends_list(
        &mut roster,
        &friends_list(false, &[(10, RELATIONSHIP_FRIEND)]),
    );
    let mut full = persona_state(&[(10, "Alice", 3, 440)]);
    full.friends[0].set_game_name("Team Fortress 2".to_string());
    apply_persona_state(&mut roster, &full);

    apply_persona_state(&mut roster, &name_only(10, "Alice2"));

    let f = &roster[&10];
    assert_eq!(f.persona_name.as_deref(), Some("Alice2"));
    assert_eq!(f.persona_state, Some(3));
    assert_eq!(f.game_app_id, Some(440));
    assert_eq!(f.game_name.as_deref(), Some("Team Fortress 2"));
}

#[test]
fn reply_without_status_leaves_unknown_state_unset() {
    let mut roster = Roster::new();
    apply_friends_list(
        &mut roster,
        &friends_list(false, &[(10, RELATIONSHIP_FRIEND)]),
    );
    apply_persona_state(&mut roster, &name_only(10, "Alice"));
    assert_eq!(roster[&10].persona_state, None);
}

#[test]
fn stopping_a_game_clears_its_name() {
    let mut roster = Roster::new();
    apply_friends_list(
        &mut roster,
        &friends_list(false, &[(10, RELATIONSHIP_FRIEND)]),
    );
    let mut playing = persona_state(&[(10, "Alice", 1, 440)]);
    playing.friends[0].set_game_name("Team Fortress 2".to_string());
    apply_persona_state(&mut roster, &playing);
    apply_persona_state(&mut roster, &persona_state(&[(10, "Alice", 1, 0)]));

    assert_eq!(roster[&10].game_app_id, None);
    assert_eq!(roster[&10].game_name, None);
}

#[test]
fn full_snapshot_keeps_persona_of_remaining_friends() {
    let mut roster = Roster::new();
    apply_friends_list(
        &mut roster,
        &friends_list(
            false,
            &[(10, RELATIONSHIP_FRIEND), (20, RELATIONSHIP_FRIEND)],
        ),
    );
    apply_persona_state(&mut roster, &persona_state(&[(10, "Alice", 3, 440)]));

    apply_friends_list(
        &mut roster,
        &friends_list(false, &[(10, RELATIONSHIP_FRIEND)]),
    );
    assert!(!roster.contains_key(&20));
    assert_eq!(roster[&10].persona_name.as_deref(), Some("Alice"));
    assert_eq!(roster[&10].persona_state, Some(3));
}

#[test]
fn unnamed_friend_ids_skips_named_and_other_relationships() {
    let mut roster = Roster::new();
    apply_friends_list(
        &mut roster,
        &friends_list(
            false,
            &[
                (10, RELATIONSHIP_FRIEND),
                (20, RELATIONSHIP_FRIEND),
                (30, RELATIONSHIP_REQUEST_RECEIVED),
                (40, 4),
            ],
        ),
    );
    apply_persona_state(&mut roster, &persona_state(&[(10, "Alice", 1, 0)]));
    let mut ids = unnamed_friend_ids(&roster);
    ids.sort();
    assert_eq!(ids, vec![20, 30]);
}

fn with_presence(id: u64, app: u32, lobby: u64, server: (u32, u32), kv: &[(&str, &str)]) -> CMsgClientPersonaState {
    let mut f = cmsg_client_persona_state::Friend::new();
    f.set_friendid(id);
    f.set_game_played_app_id(app);
    if lobby != 0 {
        f.set_game_lobby_id(lobby);
    }
    f.set_game_server_ip(server.0);
    f.set_game_server_port(server.1);
    for (k, v) in kv {
        let mut e = cmsg_client_persona_state::friend::KV::new();
        e.set_key(k.to_string());
        e.set_value(v.to_string());
        f.rich_presence.push(e);
    }
    let mut msg = CMsgClientPersonaState::new();
    msg.friends.push(f);
    msg
}

#[test]
fn persona_state_keeps_lobby_server_and_rich_presence() {
    let mut roster = Roster::new();
    apply_persona_state(&mut roster, &with_presence(7, 480, 109775240000000001, (0x7f000001, 27015), &[("status", "In match"), ("connect", "+connect 127.0.0.1:27015")]));
    let f = &roster[&7];
    assert_eq!(f.game_lobby_id, Some(109775240000000001));
    assert_eq!(f.game_server.as_deref(), Some("127.0.0.1:27015"));
    assert_eq!(f.rich_presence.get("status").map(String::as_str), Some("In match"));
    assert_eq!(f.rich_presence.get("connect").map(String::as_str), Some("+connect 127.0.0.1:27015"));
}

#[test]
fn leaving_the_game_clears_lobby_server_and_rich_presence() {
    let mut roster = Roster::new();
    apply_persona_state(&mut roster, &with_presence(7, 480, 5, (0x7f000001, 1), &[("status", "x")]));
    apply_persona_state(&mut roster, &with_presence(7, 0, 0, (0, 0), &[]));
    let f = &roster[&7];
    assert_eq!((f.game_app_id, f.game_lobby_id, f.game_server.clone()), (None, None, None));
    assert!(f.rich_presence.is_empty());
}

#[test]
fn a_reply_without_rich_presence_keeps_the_known_one() {
    let mut roster = Roster::new();
    apply_persona_state(&mut roster, &with_presence(7, 480, 0, (0, 0), &[("status", "x")]));
    let mut f = cmsg_client_persona_state::Friend::new();
    f.set_friendid(7);
    f.set_persona_state(1);
    let mut msg = CMsgClientPersonaState::new();
    msg.friends.push(f);
    apply_persona_state(&mut roster, &msg);
    assert_eq!(roster[&7].rich_presence.len(), 1, "a response without rich presence does not erase what is known");
}
