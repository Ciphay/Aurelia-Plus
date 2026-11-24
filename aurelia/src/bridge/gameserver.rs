pub const SLOT_GAMESERVER: u16 = 12;
pub const OP_LOGON: u8 = 1;
pub const OP_LOGOFF: u8 = 2;

pub fn encode_logon(steam_id: u64, public_ip: Option<std::net::Ipv4Addr>) -> Vec<u8> {
    let mut out = steam_id.to_le_bytes().to_vec();
    out.extend_from_slice(&public_ip.map(u32::from).unwrap_or(0).to_le_bytes());
    out
}

#[cfg(test)]
#[path = "gameserver_tests.rs"]
mod tests;
