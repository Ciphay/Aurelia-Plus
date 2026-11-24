use super::*;

#[test]
fn kv_blob_is_binary_keyvalues_under_rp() {
    let kv = rich_presence_kv(&[("status".into(), "In Menus".into()), ("steam_display".into(), "#Menu".into())]);
    let mut want = vec![0x00];
    want.extend_from_slice(b"RP\0");
    want.push(0x01);
    want.extend_from_slice(b"status\0In Menus\0");
    want.push(0x01);
    want.extend_from_slice(b"steam_display\0#Menu\0");
    want.extend_from_slice(&[0x08, 0x08]);
    assert_eq!(kv, want);
    assert_eq!(rich_presence_kv(&[]), vec![0x00, b'R', b'P', 0, 0x08, 0x08]);
}

#[test]
fn payload_roundtrips_and_rejects_garbage() {
    let mut p = 2u16.to_le_bytes().to_vec();
    for s in ["a", "1", "bb", "22"] {
        p.extend_from_slice(&(s.len() as u16).to_le_bytes());
        p.extend_from_slice(s.as_bytes());
    }
    assert_eq!(parse_presence_payload(&p), Some(vec![("a".into(), "1".into()), ("bb".into(), "22".into())]));
    assert_eq!(parse_presence_payload(&p[..p.len() - 1]), None);
    assert_eq!(parse_presence_payload(&[]), None);
}

#[test]
fn nul_bytes_inside_text_are_dropped() {
    let kv = rich_presence_kv(&[("k\0x".into(), "v\0y".into())]);
    assert_eq!(&kv[4..], b"\x01kx\0vy\0\x08\x08");
}
