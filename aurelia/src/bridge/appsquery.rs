pub const SLOT_APPS: u16 = 16;
pub const OP_PURCHASE_TIME: u8 = 1;

pub fn answer(steam_id: u64, payload: &[u8]) -> Option<Vec<u8>> {
    let (&op, rest) = payload.split_first()?;
    let app = u32::from_le_bytes(rest.get(..4)?.try_into().ok()?);
    match op {
        OP_PURCHASE_TIME => Some(super::warm::global().purchase_time(steam_id, app).unwrap_or(0).to_le_bytes().to_vec()),
        _ => None,
    }
}

#[cfg(test)]
#[path = "appsquery_tests.rs"]
mod tests;
