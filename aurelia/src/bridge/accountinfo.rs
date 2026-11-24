pub const SLOT_ACCOUNT_INFO: u16 = 14;

pub fn encode(info: Option<&steam_vent::AccountInfo>) -> Vec<u8> {
    let mut out = vec![0u8; 12];
    if let Some(i) = info {
        out[0] = 1;
        out[1] = i.phone_verified as u8;
        out[2] = i.phone_identifying as u8;
        out[3] = i.phone_needs_reverify as u8;
        out[4..8].copy_from_slice(&i.two_factor_state.to_le_bytes());
        out[8..12].copy_from_slice(&i.account_flags.to_le_bytes());
    }
    out
}

#[cfg(test)]
#[path = "accountinfo_tests.rs"]
mod tests;
