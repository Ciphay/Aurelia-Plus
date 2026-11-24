use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

pub const SLOT_CM_CALL: u16 = 7;
pub const SLOT_CM_SUBSCRIBE: u16 = 8;
pub const SLOT_CM_POLL: u16 = 9;

pub const MAX_WAIT_MS: u32 = 60_000;
const QUEUE_LIMIT: usize = 4096;

#[derive(Debug, PartialEq, Eq)]
pub struct CallRequest {
    pub expect_reply: bool,
    pub protobuf: bool,
    pub routing: bool,
    pub gameserver: bool,
    pub emsg: i32,
    pub wait_ms: u32,
    pub job_name: Option<String>,
    pub body: Vec<u8>,
}

pub fn parse_call_payload(p: &[u8]) -> Option<CallRequest> {
    if p.len() < 14 {
        return None;
    }
    let flags = p[0];
    let emsg = i32::from_le_bytes(p[4..8].try_into().ok()?);
    let wait_ms = u32::from_le_bytes(p[8..12].try_into().ok()?).min(MAX_WAIT_MS);
    let name_len = u16::from_le_bytes(p[12..14].try_into().ok()?) as usize;
    let name_end = 14usize.checked_add(name_len)?;
    let name = p.get(14..name_end)?;
    let job_name = if name.is_empty() { None } else { Some(String::from_utf8(name.to_vec()).ok()?) };
    Some(CallRequest { expect_reply: flags & 1 != 0, protobuf: flags & 2 != 0, routing: flags & 4 != 0, gameserver: flags & 8 != 0, emsg, wait_ms, job_name, body: p[name_end..].to_vec() })
}

pub fn encode_call_reply(emsg: i32, eresult: i32, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&emsg.to_le_bytes());
    out.extend_from_slice(&eresult.to_le_bytes());
    out.extend_from_slice(body);
    out
}

pub fn parse_subscribe_payload(p: &[u8]) -> Option<(i32, bool, u32)> {
    let client = p.get(5..9).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes).unwrap_or(0);
    Some((i32::from_le_bytes(p.get(..4)?.try_into().ok()?), p.get(4).copied().unwrap_or(0) != 0, client))
}

pub fn parse_poll_payload(p: &[u8]) -> u32 {
    p.get(..4).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes).unwrap_or(0)
}

#[derive(Default)]
pub struct PushHub {
    queues: Mutex<HashMap<u32, std::sync::Arc<PushQueue>>>,
}

const MAX_CLIENTS: usize = 64;

impl PushHub {
    pub fn queue(&self, client: u32) -> std::sync::Arc<PushQueue> {
        let mut g = self.queues.lock().expect("push hub lock poisoned");
        let client = if g.contains_key(&client) || g.len() < MAX_CLIENTS { client } else { 0 };
        std::sync::Arc::clone(g.entry(client).or_default())
    }
}

#[derive(Default)]
pub struct PushQueue {
    inner: Mutex<PushInner>,
}

#[derive(Default)]
struct PushInner {
    queue: VecDeque<(i32, Vec<u8>)>,
    subscribed: HashMap<i32, ()>,
}

impl PushQueue {
    pub fn subscribe(&self, emsg: i32) -> bool {
        self.inner.lock().expect("push queue lock poisoned").subscribed.insert(emsg, ()).is_none()
    }

    pub fn push(&self, emsg: i32, body: Vec<u8>) {
        let mut g = self.inner.lock().expect("push queue lock poisoned");
        if g.queue.len() >= QUEUE_LIMIT {
            g.queue.pop_front();
        }
        g.queue.push_back((emsg, body));
    }

    pub fn drain(&self, max_bytes: usize) -> Vec<u8> {
        let mut g = self.inner.lock().expect("push queue lock poisoned");
        let mut out = Vec::new();
        while let Some((emsg, body)) = g.queue.front() {
            if !out.is_empty() && out.len() + 8 + body.len() > max_bytes {
                break;
            }
            out.extend_from_slice(&emsg.to_le_bytes());
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(body);
            g.queue.pop_front();
        }
        out
    }
}

#[cfg(test)]
#[path = "relay_tests.rs"]
mod tests;
