use super::*;

fn req(op: u8, app: u32, name: &str, data: &[u8]) -> Vec<u8> {
    let mut p = vec![op];
    p.extend_from_slice(&app.to_le_bytes());
    p.extend_from_slice(&(name.len() as u16).to_le_bytes());
    p.extend_from_slice(name.as_bytes());
    p.extend_from_slice(data);
    p
}

#[test]
fn parses_read_and_write() {
    assert_eq!(parse_request(&req(OP_READ, 7, "serverbrowser_hist.vdf", b"")), Some((OP_READ, 7, "serverbrowser_hist.vdf".into(), vec![])));
    assert_eq!(parse_request(&req(OP_WRITE, 7, "a.vdf", b"xy")).unwrap().3, b"xy".to_vec());
}

#[test]
fn rejects_paths_and_short_input() {
    assert!(parse_request(&req(OP_READ, 7, "../x", b"")).is_none());
    assert!(parse_request(&req(OP_READ, 7, "a/b", b"")).is_none());
    assert!(parse_request(&req(OP_READ, 7, "", b"")).is_none());
    assert!(parse_request(&[1, 0, 0]).is_none());
}
