use steam_vent_proto::steammessages_base::CMsgAuthTicket;
use steam_vent_proto::steammessages_clientserver::CMsgClientAuthList;

pub const RESERVED_IFACE: u16 = 0xFFFF;
pub const SLOT_REGISTER_TICKET: u16 = 1;
pub const SLOT_CANCEL_TICKET: u16 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveTicket {
    pub handle: u32,
    pub appid: u32,
    pub ticket: Vec<u8>,
    pub ticket_type: u32,
    pub identity: String,
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

pub fn parse_register_payload(payload: &[u8]) -> Option<ActiveTicket> {
    if payload.len() <= 8 {
        return None;
    }
    Some(ActiveTicket {
        appid: u32::from_le_bytes(payload[0..4].try_into().ok()?),
        handle: u32::from_le_bytes(payload[4..8].try_into().ok()?),
        ticket: payload[8..].to_vec(),
        ticket_type: 0,
        identity: String::new(),
    })
}

pub fn auth_part(ticket: &[u8]) -> Option<&[u8]> {
    let token_len = u32::from_le_bytes(ticket.get(0..4)?.try_into().ok()?) as usize;
    let header_at = 4usize.checked_add(token_len)?;
    let header_len = u32::from_le_bytes(ticket.get(header_at..header_at + 4)?.try_into().ok()?) as usize;
    ticket.get(..header_at.checked_add(4)?.checked_add(header_len)?)
}

pub fn registered_crc(ticket: &[u8]) -> u32 {
    crc32(auth_part(ticket).unwrap_or(ticket))
}

pub fn build_auth_list(tokens_left: u32, tickets: &[ActiveTicket], seq: u32) -> CMsgClientAuthList {
    let mut list = CMsgClientAuthList::new();
    list.tokens_left = Some(tokens_left);
    list.message_sequence = Some(seq);
    for t in tickets {
        let part = auth_part(&t.ticket).unwrap_or(&t.ticket);
        let mut m = CMsgAuthTicket::new();
        m.gameid = Some(u64::from(t.appid));
        m.ticket_crc = Some(crc32(part));
        m.ticket = Some(part.to_vec());
        if t.ticket_type != 0 {
            m.ticket_type = Some(t.ticket_type);
        }
        if !t.identity.is_empty() {
            m.server_secret = Some(t.identity.clone().into_bytes());
        }
        list.tickets.push(m);
        if !list.app_ids.contains(&t.appid) {
            list.app_ids.push(t.appid);
        }
    }
    list
}

#[cfg(test)]
#[path = "authlist_tests.rs"]
mod tests;
