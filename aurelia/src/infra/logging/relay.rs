use std::future::Future;
use std::io::Read;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayStream {
    Stdout,
    Stderr,
}

pub type RelaySink = Arc<dyn Fn(RelayStream, &[u8]) + Send + Sync>;

#[derive(Clone)]
pub struct Relay {
    pub verbosity: u8,
    sink: RelaySink,
}

impl Relay {
    pub fn new(verbosity: u8, sink: RelaySink) -> Self {
        Self { verbosity, sink }
    }

    fn send(&self, stream: RelayStream, bytes: &[u8]) {
        (self.sink)(stream, bytes);
    }
}

tokio::task_local! {
    static RELAY: Relay;
}

pub async fn scoped<F: Future>(relay: Relay, fut: F) -> F::Output {
    RELAY.scope(relay, fut).await
}

pub fn current() -> Option<Relay> {
    RELAY.try_with(Relay::clone).ok()
}

pub(super) fn current_verbosity() -> Option<u8> {
    RELAY.try_with(|r| r.verbosity).ok()
}

pub(super) fn send_current(stream: RelayStream, bytes: &[u8]) -> bool {
    RELAY.try_with(|r| r.send(stream, bytes)).is_ok()
}

static LOCAL_VERBOSITY: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub(super) fn set_local_verbosity(verbosity: u8) {
    LOCAL_VERBOSITY.store(verbosity, std::sync::atomic::Ordering::Relaxed);
}

pub fn child_stdio() -> std::process::Stdio {
    match current() {
        Some(relay) if relay.verbosity > 0 => std::process::Stdio::piped(),
        Some(_) => std::process::Stdio::null(),
        None if LOCAL_VERBOSITY.load(std::sync::atomic::Ordering::Relaxed) > 0 => {
            std::process::Stdio::inherit()
        }
        None => std::process::Stdio::null(),
    }
}

pub fn attach(mut child: std::process::Child) -> std::process::Child {
    let Some(relay) = current() else { return child };
    if let Some(out) = child.stdout.take() {
        pump(out, RelayStream::Stdout, relay.clone());
    }
    if let Some(err) = child.stderr.take() {
        pump(err, RelayStream::Stderr, relay);
    }
    child
}

fn pump<R: Read + Send + 'static>(mut src: R, stream: RelayStream, relay: Relay) {
    let spawned = std::thread::Builder::new()
        .name("aurelia-child-output".into())
        .spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match src.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => relay.send(stream, &buf[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("could not start child output relay: {e}");
    }
}

static WATCHERS: std::sync::Mutex<Vec<(u64, Relay)>> = std::sync::Mutex::new(Vec::new());

const DAEMON_PREFIX: &[u8] = b"[daemon] ";

pub struct DaemonWatch(u64);

impl Drop for DaemonWatch {
    fn drop(&mut self) {
        if let Ok(mut watchers) = WATCHERS.lock() {
            watchers.retain(|(id, _)| *id != self.0);
        }
    }
}

pub fn watch_daemon(relay: Relay) -> DaemonWatch {
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if let Ok(mut watchers) = WATCHERS.lock() {
        watchers.push((id, relay));
    }
    DaemonWatch(id)
}

pub(super) fn has_daemon_watcher(level: u8, max: u8) -> bool {
    WATCHERS
        .lock()
        .is_ok_and(|w| w.iter().any(|(_, r)| r.verbosity.min(max) == level))
}

fn broadcast(stream: RelayStream, bytes: &[u8], level: Option<(u8, u8)>) {
    let Ok(watchers) = WATCHERS.lock() else {
        return;
    };
    let mut line = DAEMON_PREFIX.to_vec();
    line.extend_from_slice(bytes);
    for (_, relay) in watchers.iter() {
        if level.is_none_or(|(level, max)| relay.verbosity.min(max) == level) {
            relay.send(stream, &line);
        }
    }
}

pub fn broadcast_daemon_output(stream: RelayStream, bytes: &[u8]) -> bool {
    let watching = WATCHERS.lock().is_ok_and(|w| !w.is_empty());
    if watching {
        broadcast(stream, bytes, None);
    }
    watching
}

#[derive(Clone, Copy)]
pub(super) struct DaemonWatchWriter {
    pub(super) level: u8,
    pub(super) max: u8,
}

impl std::io::Write for DaemonWatchWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        broadcast(RelayStream::Stderr, buf, Some((self.level, self.max)));
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for DaemonWatchWriter {
    type Writer = DaemonWatchWriter;

    fn make_writer(&'a self) -> Self::Writer {
        *self
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct RelayWriter;

impl std::io::Write for RelayWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        send_current(RelayStream::Stderr, buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for RelayWriter {
    type Writer = RelayWriter;

    fn make_writer(&'a self) -> Self::Writer {
        RelayWriter
    }
}

#[cfg(test)]
#[path = "relay_tests.rs"]
mod tests;
