pub const SLOT_OTHER_STATS: u16 = 19;
pub const OP_GET: u8 = 1;
pub const OP_STORE: u8 = 2;

#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    Get { gameserver: bool, steam_id: u64 },
    Store { gameserver: bool, steam_id: u64, crc: u32, stats: Vec<(u32, u32)> },
}

pub fn parse(p: &[u8]) -> Option<Request> {
    let op = *p.first()?;
    let gameserver = *p.get(1)? != 0;
    let steam_id = u64::from_le_bytes(p.get(2..10)?.try_into().ok()?);
    if steam_id == 0 {
        return None;
    }
    match op {
        OP_GET => Some(Request::Get { gameserver, steam_id }),
        OP_STORE => {
            let crc = u32::from_le_bytes(p.get(10..14)?.try_into().ok()?);
            let n = u32::from_le_bytes(p.get(14..18)?.try_into().ok()?) as usize;
            if n > 4096 || p.len() != 18 + n * 8 {
                return None;
            }
            let stats = (0..n).map(|i| (u32::from_le_bytes(p[18 + i * 8..22 + i * 8].try_into().unwrap()), u32::from_le_bytes(p[22 + i * 8..26 + i * 8].try_into().unwrap()))).collect();
            Some(Request::Store { gameserver, steam_id, crc, stats })
        }
        _ => None,
    }
}

#[cfg(test)]
#[path = "otherstats_tests.rs"]
mod tests;
