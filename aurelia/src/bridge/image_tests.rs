use super::*;

#[test]
fn parses_avatar_and_icon_requests() {
    let mut a = vec![1u8];
    a.extend_from_slice(&76561198354875885u64.to_le_bytes());
    a.push(2);
    assert_eq!(parse_request(&a), Some(ImageRequest::Avatar { steam_id: 76561198354875885, size: 2 }));
    let mut i = vec![2u8];
    i.extend_from_slice(&289650u32.to_le_bytes());
    i.push(1);
    i.extend_from_slice(b"ACH_1");
    assert_eq!(parse_request(&i), Some(ImageRequest::AchievementIcon { app_id: 289650, achieved: true, name: "ACH_1".into() }));
}

#[test]
fn avatar_sizes_map_to_steam_files() {
    let full = "https://avatars.cloudflare.steamstatic.com/abc_full.jpg";
    assert_eq!(avatar_variant(full, 0), "https://avatars.cloudflare.steamstatic.com/abc.jpg");
    assert_eq!(avatar_variant(full, 1), "https://avatars.cloudflare.steamstatic.com/abc_medium.jpg");
    assert_eq!(avatar_variant(full, 2), full);
}