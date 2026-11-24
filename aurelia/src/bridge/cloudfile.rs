pub const SLOT_CLOUDFILE: u16 = 11;
pub const OP_READ: u8 = 1;
pub const OP_WRITE: u8 = 2;

pub fn parse_request(p: &[u8]) -> Option<(u8, u32, String, Vec<u8>)> {
    if p.len() < 7 {
        return None;
    }
    let app = u32::from_le_bytes(p[1..5].try_into().ok()?);
    let n = u16::from_le_bytes(p[5..7].try_into().ok()?) as usize;
    let name = String::from_utf8(p.get(7..7 + n)?.to_vec()).ok()?;
    if name.is_empty() || name.contains(['/', '\\']) || name.contains("..") {
        return None;
    }
    Some((p[0], app, name, p[7 + n..].to_vec()))
}

#[cfg(test)]
#[path = "cloudfile_tests.rs"]
mod tests;
