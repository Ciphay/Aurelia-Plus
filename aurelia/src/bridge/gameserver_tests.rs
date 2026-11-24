use super::*;

#[test]
fn logon_reply_is_the_steamid_and_the_public_ip() {
    assert_eq!(encode_logon(0x0102030405060708, Some("1.2.3.4".parse().unwrap())), vec![8, 7, 6, 5, 4, 3, 2, 1, 4, 3, 2, 1]);
    assert_eq!(&encode_logon(1, None)[8..], &[0, 0, 0, 0]);
}
