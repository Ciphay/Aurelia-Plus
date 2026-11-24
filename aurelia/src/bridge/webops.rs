pub const SLOT_WEB: u16 = 17;
pub const OP_FAVORITE_ADD: u8 = 1;
pub const OP_FAVORITE_REMOVE: u8 = 2;
pub const OP_MARKET: u8 = 3;

pub fn parse(p: &[u8]) -> Option<(u8, u32, u64)> {
    super::ugc::parse_request(p)
}

pub fn favorite_request(op: u8, app: u32, id: u64) -> Option<(&'static str, Vec<(&'static str, String)>)> {
    let url = match op {
        OP_FAVORITE_ADD => "https://steamcommunity.com/sharedfiles/favorite",
        OP_FAVORITE_REMOVE => "https://steamcommunity.com/sharedfiles/unfavorite",
        _ => return None,
    };
    Some((url, vec![("id", id.to_string()), ("appid", app.to_string())]))
}

pub fn encode_market(allowed: bool, reason: u32, allowed_at: u32, guard_days: i32, device_days: i32) -> Vec<u8> {
    let mut out = vec![allowed as u8, 0, 0, 0];
    out.extend_from_slice(&reason.to_le_bytes());
    out.extend_from_slice(&allowed_at.to_le_bytes());
    out.extend_from_slice(&guard_days.to_le_bytes());
    out.extend_from_slice(&device_days.to_le_bytes());
    out
}

#[cfg(test)]
#[path = "webops_tests.rs"]
mod tests;
