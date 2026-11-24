use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

use super::protocol::{decode, encode, Frame, ProtoError, PROTO_VERSION};

pub type CallHandler = Arc<dyn Fn(u16, u16, Vec<u8>) -> Pin<Box<dyn Future<Output = (i32, Vec<u8>)> + Send>> + Send + Sync>;

const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn bind() -> io::Result<(TcpListener, [u8; 32])> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let mut token = [0u8; 32];
    tokio::fs::File::open("/dev/urandom").await?.read_exact(&mut token).await?;
    Ok((listener, token))
}

fn token_matches(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub async fn run(listener: TcpListener, token: [u8; 32], snapshot: Vec<u8>) -> io::Result<()> {
    let unimplemented: CallHandler = Arc::new(|_, _, _| Box::pin(async { (-1, Vec::new()) }));
    run_with_handler(listener, token, snapshot, unimplemented).await
}

pub type SharedSnapshot = Arc<std::sync::RwLock<Arc<Vec<u8>>>>;

pub async fn run_with_handler(listener: TcpListener, token: [u8; 32], snapshot: Vec<u8>, handler: CallHandler) -> io::Result<()> {
    run_with_shared(listener, token, Arc::new(std::sync::RwLock::new(Arc::new(snapshot))), handler).await
}

pub async fn run_with_shared(listener: TcpListener, token: [u8; 32], snapshot: SharedSnapshot, handler: CallHandler) -> io::Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let snapshot = Arc::clone(&snapshot);
        let handler = Arc::clone(&handler);
        tokio::spawn(async move {
            let _ = handle(stream, token, snapshot, handler).await;
        });
    }
}

async fn next_frame(stream: &mut TcpStream, buf: &mut Vec<u8>) -> io::Result<Option<Frame>> {
    loop {
        match decode(buf) {
            Ok((frame, used)) => {
                buf.drain(..used);
                return Ok(Some(frame));
            }
            Err(ProtoError::Incomplete) => {}
            Err(e) => return Err(io::Error::new(io::ErrorKind::InvalidData, format!("{e:?}"))),
        }
        let mut chunk = [0u8; 8192];
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

async fn handle(mut stream: TcpStream, token: [u8; 32], snapshot: SharedSnapshot, handler: CallHandler) -> io::Result<()> {
    let mut buf = Vec::new();
    match timeout(HELLO_TIMEOUT, next_frame(&mut stream, &mut buf)).await {
        Ok(Ok(Some(Frame::Hello { proto, token: got }))) if proto == PROTO_VERSION && token_matches(&token, &got) => {}
        _ => return Ok(()),
    }
    stream.write_all(&encode(&Frame::HelloOk { proto: PROTO_VERSION })).await?;
    while let Some(frame) = next_frame(&mut stream, &mut buf).await? {
        let reply = match frame {
            Frame::SnapshotReq => Frame::Snapshot { json: snapshot.read().map(|s| s.to_vec()).unwrap_or_default() },
            Frame::Call { seq, iface, slot, payload } => {
                let (status, payload) = handler(iface, slot, payload).await;
                Frame::Reply { seq, status, payload }
            }
            _ => continue,
        };
        stream.write_all(&encode(&reply)).await?;
    }
    Ok(())
}
