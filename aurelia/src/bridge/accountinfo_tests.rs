use super::*;

#[test]
fn encode_unknown_is_all_zero() {
    assert_eq!(encode(None), vec![0u8; 12]);
}

#[test]
fn encode_known_account_layout() {
    let info = steam_vent::AccountInfo { phone_verified: true, phone_identifying: false, phone_needs_reverify: true, two_factor_state: 1, account_flags: 0x0102 };
    assert_eq!(encode(Some(&info)), vec![1, 1, 0, 1, 1, 0, 0, 0, 0x02, 0x01, 0, 0]);
}
