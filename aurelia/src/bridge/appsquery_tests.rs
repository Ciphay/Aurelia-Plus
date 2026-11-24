use super::*;

#[test]
fn malformed_questions_have_no_answer() {
    assert!(answer(1, &[]).is_none());
    assert!(answer(1, &[OP_PURCHASE_TIME]).is_none());
    assert!(answer(1, &[OP_PURCHASE_TIME, 1, 2]).is_none());
    assert!(answer(1, &[99, 1, 0, 0, 0]).is_none());
}

#[test]
fn an_unknown_app_has_purchase_time_zero() {
    let reply = answer(424_242_424, &[OP_PURCHASE_TIME, 7, 0, 0, 0]).unwrap();
    assert_eq!(reply, 0u32.to_le_bytes().to_vec());
}
