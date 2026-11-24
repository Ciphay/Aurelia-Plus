use super::*;

fn all_frames() -> Vec<Frame> {
    vec![
        Frame::Hello { proto: PROTO_VERSION, token: [7; 32] },
        Frame::HelloOk { proto: PROTO_VERSION },
        Frame::SnapshotReq,
        Frame::Snapshot { json: br#"{"account":{}}"#.to_vec() },
        Frame::Call { seq: 9, iface: 3, slot: 12, payload: vec![1, 2, 3] },
        Frame::Reply { seq: 9, status: -1, payload: vec![] },
        Frame::Error { code: 77, message: "auth_required".into() },
    ]
}

#[test]
fn roundtrip_every_frame() {
    for f in all_frames() {
        let bytes = encode(&f);
        let (back, used) = decode(&bytes).unwrap();
        assert_eq!(back, f);
        assert_eq!(used, bytes.len());
    }
}

#[test]
fn decode_rejects_oversize_frame() {
    let mut buf = (MAX_FRAME + 1).to_le_bytes().to_vec();
    buf.push(3);
    assert_eq!(decode(&buf), Err(ProtoError::TooLarge(MAX_FRAME + 1)));
}

#[test]
fn decode_reports_incomplete_input() {
    let bytes = encode(&Frame::Call { seq: 1, iface: 1, slot: 1, payload: vec![0; 10] });
    for cut in [0, 2, 4, bytes.len() - 1] {
        assert_eq!(decode(&bytes[..cut]), Err(ProtoError::Incomplete));
    }
}

#[test]
fn decode_rejects_unknown_tag() {
    let buf = [1u8, 0, 0, 0, 0xEE];
    assert_eq!(decode(&buf), Err(ProtoError::UnknownTag(0xEE)));
}
