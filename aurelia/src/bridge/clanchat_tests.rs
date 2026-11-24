use super::*;

fn room() -> Room {
    Room { group_id: 77, chat_id: 5, members: vec![INDIVIDUAL_BASE + 1, INDIVIDUAL_BASE + 2], owner: INDIVIDUAL_BASE + 1 }
}

#[test]
fn a_joined_room_is_found_by_clan_and_by_group_and_goes_away_on_leave() {
    let c = ClanChat::default();
    c.join(103_582_791_429_521_408, room());
    assert_eq!(c.room(103_582_791_429_521_408).unwrap().members.len(), 2);
    assert_eq!(c.clan_of_group(77), Some(103_582_791_429_521_408));
    assert_eq!(c.clan_of_group(78), None);
    assert!(c.leave(103_582_791_429_521_408).is_some());
    assert!(c.room(103_582_791_429_521_408).is_none());
    assert!(c.leave(103_582_791_429_521_408).is_none());
}

#[test]
fn messages_queue_in_order_drain_once_and_are_capped() {
    let c = ClanChat::default();
    for i in 0..(QUEUE_LIMIT + 3) {
        c.push(Incoming { clan: 1, sender: 2, time: i as u32, text: format!("m{i}") });
    }
    let got = c.drain();
    assert_eq!(got.len(), QUEUE_LIMIT);
    assert_eq!(got[0].text, "m3", "as mais antigas se perdem");
    assert!(c.drain().is_empty());
}

#[test]
fn requests_and_answers_have_the_layout_the_shim_reads() {
    let mut req = vec![OP_SEND];
    req.extend_from_slice(&42u64.to_le_bytes());
    req.extend_from_slice(b"oi");
    assert_eq!(parse(&req), Some((OP_SEND, 42, &b"oi"[..])));
    assert_eq!(parse(&[OP_JOIN, 1, 2]), None);
    assert_eq!(parse(&[]), None);
    let join = encode_join(ENTER_SUCCESS, 3, 9);
    assert_eq!(&join[..8], [1, 0, 0, 0, 3, 0, 0, 0]);
    assert_eq!(u64::from_le_bytes(join[8..16].try_into().unwrap()), 9);
    assert_eq!(encode_members(&[9, 10]).len(), 4 + 16);
    let m = encode_messages(&[Incoming { clan: 1, sender: 2, time: 3, text: "abc".into() }]);
    assert_eq!(u32::from_le_bytes(m[..4].try_into().unwrap()), 1);
    assert_eq!(u16::from_le_bytes(m[24..26].try_into().unwrap()), 3);
    assert_eq!(&m[26..], b"abc");
}
