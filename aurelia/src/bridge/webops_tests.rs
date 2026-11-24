use super::*;

#[test]
fn favorites_post_to_the_community_endpoints() {
    let (url, form) = favorite_request(OP_FAVORITE_ADD, 550, 123).unwrap();
    assert_eq!(url, "https://steamcommunity.com/sharedfiles/favorite");
    assert_eq!(form, vec![("id", "123".to_string()), ("appid", "550".to_string())]);
    assert_eq!(favorite_request(OP_FAVORITE_REMOVE, 550, 123).unwrap().0, "https://steamcommunity.com/sharedfiles/unfavorite");
    assert!(favorite_request(OP_MARKET, 0, 0).is_none());
}

#[test]
fn market_answer_has_the_layout_the_shim_reads() {
    let b = encode_market(true, 0, 0, 15, 7);
    assert_eq!(b.len(), 20);
    assert_eq!(b[0], 1);
    assert_eq!(i32::from_le_bytes(b[12..16].try_into().unwrap()), 15);
    assert_eq!(i32::from_le_bytes(b[16..20].try_into().unwrap()), 7);
    let denied = encode_market(false, 128, 1_795_600_000, 15, 0);
    assert_eq!(u32::from_le_bytes(denied[4..8].try_into().unwrap()), 128);
    assert_eq!(u32::from_le_bytes(denied[8..12].try_into().unwrap()), 1_795_600_000);
}
