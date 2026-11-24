use std::time::Duration;

use aurelia::bridge::protocol::{decode, encode, Frame, ProtoError, PROTO_VERSION};
use aurelia::bridge::server::{bind, run, run_with_handler};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

const SNAPSHOT: &[u8] = br#"{"account":{"steamid":1}}"#;

async fn start() -> (u16, [u8; 32], tokio::task::JoinHandle<()>) {
    let (listener, token) = bind().await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let h = tokio::spawn(async move {
        let _ = run(listener, token, SNAPSHOT.to_vec()).await;
    });
    (port, token, h)
}

async fn read_all_frames(s: &mut TcpStream) -> Vec<Frame> {
    let mut buf = Vec::new();
    let mut out = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = timeout(Duration::from_secs(2), s.read(&mut chunk)).await.expect("connection did not close").unwrap_or(0);
        if n == 0 {
            return out;
        }
        buf.extend_from_slice(&chunk[..n]);
        loop {
            match decode(&buf) {
                Ok((f, used)) => {
                    buf.drain(..used);
                    out.push(f);
                }
                Err(ProtoError::Incomplete) => break,
                Err(e) => panic!("{e:?}"),
            }
        }
    }
}

#[tokio::test]
async fn connection_without_valid_token_is_closed_and_gets_no_snapshot() {
    let (port, _token, h) = start().await;
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(&encode(&Frame::Hello { proto: PROTO_VERSION, token: [0; 32] })).await.unwrap();
    let _ = s.write_all(&encode(&Frame::SnapshotReq)).await;
    let frames = read_all_frames(&mut s).await;
    assert!(frames.iter().all(|f| !matches!(f, Frame::Snapshot { .. } | Frame::HelloOk { .. })), "{frames:?}");
    h.abort();
}

#[tokio::test]
async fn valid_token_gets_snapshot_and_unimplemented_call_replies_minus_one() {
    let (port, token, h) = start().await;
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(&encode(&Frame::Hello { proto: PROTO_VERSION, token })).await.unwrap();
    s.write_all(&encode(&Frame::SnapshotReq)).await.unwrap();
    s.write_all(&encode(&Frame::Call { seq: 5, iface: 1, slot: 2, payload: vec![] })).await.unwrap();
    s.shutdown().await.unwrap();
    let frames = read_all_frames(&mut s).await;
    assert_eq!(frames[0], Frame::HelloOk { proto: PROTO_VERSION });
    assert_eq!(frames[1], Frame::Snapshot { json: SNAPSHOT.to_vec() });
    assert_eq!(frames[2], Frame::Reply { seq: 5, status: -1, payload: vec![] });
    h.abort();
}

#[tokio::test]
async fn calls_reach_the_handler_and_its_reply_is_returned() {
    let (listener, token) = bind().await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let h = tokio::spawn(async move {
        let handler: aurelia::bridge::server::CallHandler = std::sync::Arc::new(|iface, slot, payload| {
            Box::pin(async move { (0, format!("{iface}:{slot}:{}", payload.len()).into_bytes()) })
        });
        let _ = run_with_handler(listener, token, SNAPSHOT.to_vec(), handler).await;
    });
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(&encode(&Frame::Hello { proto: PROTO_VERSION, token })).await.unwrap();
    s.write_all(&encode(&Frame::Call { seq: 7, iface: 0xFFFF, slot: 1, payload: vec![1, 2, 3] })).await.unwrap();
    s.shutdown().await.unwrap();
    let frames = read_all_frames(&mut s).await;
    assert_eq!(frames[1], Frame::Reply { seq: 7, status: 0, payload: b"65535:1:3".to_vec() });
    h.abort();
}
