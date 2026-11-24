use std::collections::VecDeque;

pub const SLOT_CREATE_TICKET: u16 = 3;

#[derive(Debug, Default)]
pub struct TokenPool {
    tokens: VecDeque<Vec<u8>>,
    capacity: usize,
}

impl TokenPool {
    pub fn new(capacity: usize) -> Self {
        Self { tokens: VecDeque::new(), capacity }
    }

    pub fn add(&mut self, tokens: Vec<Vec<u8>>) {
        for token in tokens {
            if !token.is_empty() {
                self.tokens.push_back(token);
            }
        }
        while self.tokens.len() > self.capacity {
            self.tokens.pop_front();
        }
    }

    pub fn take(&mut self) -> Option<Vec<u8>> {
        self.tokens.pop_front()
    }

    pub fn peek(&self) -> Option<Vec<u8>> {
        self.tokens.front().cloned()
    }

    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }
}

pub const TICKET_TYPE_DEFAULT: u32 = 2;
pub const TICKET_TYPE_WEB_API: u32 = 5;

pub fn build_auth_ticket(token: &[u8], ticket_type: u32, nonces: [u32; 2], elapsed_ms: u32, sequence: u32, ownership: &[u8]) -> Vec<u8> {
    let mut t = Vec::with_capacity(4 + token.len() + 28 + 4 + ownership.len());
    t.extend_from_slice(&(token.len() as u32).to_le_bytes());
    t.extend_from_slice(token);
    t.extend_from_slice(&24u32.to_le_bytes());
    for word in [1, ticket_type, nonces[0], nonces[1], elapsed_ms, sequence] {
        t.extend_from_slice(&word.to_le_bytes());
    }
    t.extend_from_slice(&(ownership.len() as u32).to_le_bytes());
    t.extend_from_slice(ownership);
    t
}

#[derive(Debug, PartialEq, Eq)]
pub struct CreateRequest {
    pub appid: u32,
    pub handle: u32,
    pub ticket_type: u32,
    pub identity: String,
}

pub fn parse_create_payload(payload: &[u8]) -> Option<CreateRequest> {
    if payload.len() < 8 {
        return None;
    }
    let appid = u32::from_le_bytes(payload[0..4].try_into().ok()?);
    let handle = u32::from_le_bytes(payload[4..8].try_into().ok()?);
    if payload.len() == 8 {
        return Some(CreateRequest { appid, handle, ticket_type: TICKET_TYPE_DEFAULT, identity: String::new() });
    }
    let ticket_type = u32::from_le_bytes(payload.get(8..12)?.try_into().ok()?);
    let n = u16::from_le_bytes(payload.get(12..14)?.try_into().ok()?) as usize;
    let identity = String::from_utf8(payload.get(14..14 + n)?.to_vec()).ok()?;
    Some(CreateRequest { appid, handle, ticket_type, identity })
}

#[cfg(test)]
#[path = "ticket_tests.rs"]
mod tests;
