use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const SLOT_CONNECTION: u16 = 15;

pub const QUEUE_FORMAT: u32 = 1;
pub const RETRY_FIRST: Duration = Duration::from_secs(2);
pub const RETRY_MAX: Duration = Duration::from_secs(60);
pub const SESSION_OPEN: Duration = Duration::from_secs(8);
pub const CLOUD_OPEN: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Op {
    StoreStats { app: u32, changes: Vec<(u32, u32)> },
    CloudUp { app: u32 },
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    format: u32,
    ops: Vec<Op>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Queue {
    ops: Vec<Op>,
}

impl Queue {
    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    pub fn len(&self) -> usize {
        self.ops.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn push(&mut self, op: Op) {
        match op {
            Op::StoreStats { app, changes } => {
                if let Some(Op::StoreStats { changes: pending, .. }) = self.ops.iter_mut().find(|o| matches!(o, Op::StoreStats { app: a, .. } if *a == app)) {
                    for (id, value) in changes {
                        match pending.iter_mut().find(|(i, _)| *i == id) {
                            Some(slot) => slot.1 = value,
                            None => pending.push((id, value)),
                        }
                    }
                } else if !changes.is_empty() {
                    self.ops.push(Op::StoreStats { app, changes });
                }
            }
            Op::CloudUp { app } => {
                if !self.ops.iter().any(|o| matches!(o, Op::CloudUp { app: a } if *a == app)) {
                    self.ops.push(Op::CloudUp { app });
                }
            }
        }
    }

    pub fn subtract(&mut self, sent: &Op) {
        match sent {
            Op::StoreStats { app, changes } => {
                for op in &mut self.ops {
                    if let Op::StoreStats { app: a, changes: pending } = op {
                        if a == app {
                            pending.retain(|c| !changes.contains(c));
                        }
                    }
                }
                self.ops.retain(|o| !matches!(o, Op::StoreStats { changes, .. } if changes.is_empty()));
            }
            Op::CloudUp { app } => self.ops.retain(|o| !matches!(o, Op::CloudUp { app: a } if a == app)),
        }
    }
}

fn path_for(steamid: u64) -> Option<PathBuf> {
    Some(crate::core::config::data_dir().ok()?.join("bridge-cache").join(steamid.to_string()).join("queue.json"))
}

pub fn load(steamid: u64) -> Queue {
    path_for(steamid).map(|p| load_from(&p)).unwrap_or_default()
}

pub fn load_from(path: &std::path::Path) -> Queue {
    match std::fs::read(path).ok().and_then(|b| serde_json::from_slice::<File>(&b).ok()) {
        Some(f) if f.format == QUEUE_FORMAT => Queue { ops: f.ops },
        _ => Queue::default(),
    }
}

pub fn save(steamid: u64, queue: &Queue) {
    if let Some(path) = path_for(steamid) {
        save_to(&path, queue);
    }
}

pub fn save_to(path: &std::path::Path, queue: &Queue) {
    if queue.is_empty() {
        let _ = std::fs::remove_file(path);
        return;
    }
    let Ok(bytes) = serde_json::to_vec(&File { format: QUEUE_FORMAT, ops: queue.ops.clone() }) else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if crate::core::atomic::write_atomic(path, &bytes).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
}

static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn enqueue(steamid: u64, op: Op) -> usize {
    let _g = LOCK.lock().expect("queue lock poisoned");
    let mut queue = load(steamid);
    queue.push(op);
    save(steamid, &queue);
    queue.len()
}

pub fn snapshot(steamid: u64) -> Vec<Op> {
    let _g = LOCK.lock().expect("queue lock poisoned");
    load(steamid).ops
}

pub fn ack(steamid: u64, sent: &Op) -> usize {
    let _g = LOCK.lock().expect("queue lock poisoned");
    let mut queue = load(steamid);
    queue.subtract(sent);
    save(steamid, &queue);
    queue.len()
}

pub fn pending(steamid: u64) -> usize {
    let _g = LOCK.lock().expect("queue lock poisoned");
    load(steamid).len()
}

pub fn has_cloud_up(steamid: u64, app: u32) -> bool {
    let _g = LOCK.lock().expect("queue lock poisoned");
    load(steamid).ops().iter().any(|o| matches!(o, Op::CloudUp { app: a } if *a == app))
}

pub fn clear_cloud_up(steamid: u64, app: u32) {
    let _g = LOCK.lock().expect("queue lock poisoned");
    let mut queue = load(steamid);
    queue.ops.retain(|o| !matches!(o, Op::CloudUp { app: a } if *a == app));
    save(steamid, &queue);
}

pub fn retry_delay(attempt: u32) -> Duration {
    RETRY_FIRST.saturating_mul(1u32 << attempt.min(16)).min(RETRY_MAX)
}

#[derive(Debug)]
pub struct Connection(AtomicBool);

impl Connection {
    pub fn new(online: bool) -> Self {
        Self(AtomicBool::new(online))
    }

    pub fn online(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    pub fn set(&self, online: bool) {
        self.0.store(online, Ordering::Relaxed);
    }
}

#[cfg(test)]
#[path = "offline_tests.rs"]
mod tests;
