use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use super::offline::Connection;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Session {
    pub app: u32,
    pub steam_id: u64,
    pub since: u64,
    pub startup_ms: u64,
    pub opened_offline: bool,
    pub online: bool,
    pub queued: usize,
    pub port: u16,
}

struct Entry {
    id: u64,
    session: Session,
    connection: Arc<Connection>,
    queued: Arc<std::sync::atomic::AtomicUsize>,
}

#[derive(Default)]
struct Inner {
    next: u64,
    entries: Vec<Entry>,
}

fn global() -> &'static Mutex<Inner> {
    static REG: OnceLock<Mutex<Inner>> = OnceLock::new();
    REG.get_or_init(Mutex::default)
}

pub struct Registration(u64);

impl Drop for Registration {
    fn drop(&mut self) {
        global().lock().expect("registry lock poisoned").entries.retain(|e| e.id != self.0);
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

pub fn register(session: Session, connection: Arc<Connection>, queued: Arc<std::sync::atomic::AtomicUsize>) -> Registration {
    let mut g = global().lock().expect("registry lock poisoned");
    g.next += 1;
    let id = g.next;
    g.entries.push(Entry { id, session, connection, queued });
    Registration(id)
}

pub fn sessions() -> Vec<Session> {
    global()
        .lock()
        .expect("registry lock poisoned")
        .entries
        .iter()
        .map(|e| Session { online: e.connection.online(), queued: e.queued.load(std::sync::atomic::Ordering::Relaxed), ..e.session.clone() })
        .collect()
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
