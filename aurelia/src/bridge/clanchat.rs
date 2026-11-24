use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

pub const SLOT_CLAN: u16 = 18;
pub const OP_JOIN: u8 = 1;
pub const OP_LEAVE: u8 = 2;
pub const OP_SEND: u8 = 3;
pub const OP_MEMBERS: u8 = 4;
pub const OP_POLL: u8 = 5;

const QUEUE_LIMIT: usize = 512;
pub const INDIVIDUAL_BASE: u64 = 76_561_197_960_265_728;

pub const ENTER_SUCCESS: i32 = 1;
pub const ENTER_DOESNT_EXIST: i32 = 2;
pub const ENTER_NOT_ALLOWED: i32 = 3;
pub const ENTER_ERROR: i32 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Room {
    pub group_id: u64,
    pub chat_id: u64,
    pub members: Vec<u64>,
    pub owner: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incoming {
    pub clan: u64,
    pub sender: u64,
    pub time: u32,
    pub text: String,
}

#[derive(Default)]
pub struct ClanChat {
    rooms: Mutex<HashMap<u64, Room>>,
    queue: Mutex<VecDeque<Incoming>>,
}

impl ClanChat {
    pub fn join(&self, clan: u64, room: Room) {
        self.rooms.lock().expect("rooms lock poisoned").insert(clan, room);
    }

    pub fn leave(&self, clan: u64) -> Option<Room> {
        self.rooms.lock().expect("rooms lock poisoned").remove(&clan)
    }

    pub fn room(&self, clan: u64) -> Option<Room> {
        self.rooms.lock().expect("rooms lock poisoned").get(&clan).cloned()
    }

    pub fn clan_of_group(&self, group_id: u64) -> Option<u64> {
        self.rooms.lock().expect("rooms lock poisoned").iter().find(|(_, r)| r.group_id == group_id).map(|(c, _)| *c)
    }

    pub fn push(&self, msg: Incoming) {
        let mut q = self.queue.lock().expect("queue lock poisoned");
        if q.len() >= QUEUE_LIMIT {
            q.pop_front();
        }
        q.push_back(msg);
    }

    pub fn drain(&self) -> Vec<Incoming> {
        self.queue.lock().expect("queue lock poisoned").drain(..).collect()
    }
}

pub fn parse(p: &[u8]) -> Option<(u8, u64, &[u8])> {
    let op = *p.first()?;
    let clan = u64::from_le_bytes(p.get(1..9)?.try_into().ok()?);
    Some((op, clan, &p[9..]))
}

pub fn encode_join(response: i32, member_count: usize, owner: u64) -> Vec<u8> {
    let mut out = response.to_le_bytes().to_vec();
    out.extend_from_slice(&(member_count as u32).to_le_bytes());
    out.extend_from_slice(&owner.to_le_bytes());
    out
}

pub fn encode_members(members: &[u64]) -> Vec<u8> {
    let mut out = (members.len() as u32).to_le_bytes().to_vec();
    for m in members {
        out.extend_from_slice(&m.to_le_bytes());
    }
    out
}

pub fn encode_messages(list: &[Incoming]) -> Vec<u8> {
    let mut out = (list.len() as u32).to_le_bytes().to_vec();
    for m in list {
        let text = m.text.as_bytes();
        let text = &text[..text.len().min(4096)];
        out.extend_from_slice(&m.clan.to_le_bytes());
        out.extend_from_slice(&m.sender.to_le_bytes());
        out.extend_from_slice(&m.time.to_le_bytes());
        out.extend_from_slice(&(text.len() as u16).to_le_bytes());
        out.extend_from_slice(text);
    }
    out
}

#[cfg(test)]
#[path = "clanchat_tests.rs"]
mod tests;
