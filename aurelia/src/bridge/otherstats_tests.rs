use super::*;

#[test]
fn a_get_request_names_the_player_and_the_session() {
    let mut p = vec![OP_GET, 1];
    p.extend_from_slice(&76561198000000042u64.to_le_bytes());
    assert_eq!(parse(&p), Some(Request::Get { gameserver: true, steam_id: 76561198000000042 }));
    p[1] = 0;
    assert_eq!(parse(&p), Some(Request::Get { gameserver: false, steam_id: 76561198000000042 }));
}

#[test]
fn a_store_request_carries_the_crc_and_the_changes_and_rejects_bad_lengths() {
    let mut p = vec![OP_STORE, 0];
    p.extend_from_slice(&7u64.to_le_bytes());
    p.extend_from_slice(&0xABCDu32.to_le_bytes());
    p.extend_from_slice(&2u32.to_le_bytes());
    for (id, v) in [(1u32, 10u32), (2, 20)] {
        p.extend_from_slice(&id.to_le_bytes());
        p.extend_from_slice(&v.to_le_bytes());
    }
    assert_eq!(parse(&p), Some(Request::Store { gameserver: false, steam_id: 7, crc: 0xABCD, stats: vec![(1, 10), (2, 20)] }));
    p.pop();
    assert_eq!(parse(&p), None);
    assert_eq!(parse(&[OP_GET]), None);
    assert_eq!(parse(&[9, 0, 1, 0, 0, 0, 0, 0, 0, 0]), None);
    let zero = [OP_GET, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    assert_eq!(parse(&zero), None, "player zero does not exist");
}
