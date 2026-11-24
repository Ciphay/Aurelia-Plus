use super::*;
use crate::bridge::authlist::auth_part;

#[test]
fn pool_hands_out_each_token_once_in_arrival_order() {
    let mut pool = TokenPool::new(3);
    pool.add(vec![vec![1; 20], vec![], vec![2; 20]]);
    assert_eq!(pool.len(), 2);
    assert_eq!(pool.take(), Some(vec![1; 20]));
    assert_eq!(pool.take(), Some(vec![2; 20]));
    assert_eq!(pool.take(), None);
}

#[test]
fn peek_copies_the_next_token_without_consuming_it() {
    let mut pool = TokenPool::new(3);
    assert_eq!(pool.peek(), None);
    pool.add(vec![vec![1; 20], vec![2; 20]]);
    assert_eq!(pool.peek(), Some(vec![1; 20]));
    assert_eq!(pool.len(), 2);
    assert_eq!(pool.take(), Some(vec![1; 20]));
}

#[test]
fn pool_keeps_only_the_newest_tokens_up_to_its_capacity() {
    let mut pool = TokenPool::new(2);
    pool.add(vec![vec![1; 20], vec![2; 20], vec![3; 20]]);
    assert_eq!(pool.len(), 2);
    assert_eq!(pool.take(), Some(vec![2; 20]));
}

#[test]
fn ticket_layout_matches_the_recorded_real_client_ticket() {
    let token = [0xAA; 20];
    let ownership = [0xBB; 178];
    let t = build_auth_ticket(&token, 2, [0x11223344, 0x55667788], 1234, 2, &ownership);
    assert_eq!(t.len(), 234);
    assert_eq!(&t[0..4], &20u32.to_le_bytes());
    assert_eq!(&t[4..24], &token);
    assert_eq!(&t[24..28], &24u32.to_le_bytes());
    let words: Vec<u32> = (0..6).map(|i| u32::from_le_bytes(t[28 + 4 * i..32 + 4 * i].try_into().unwrap())).collect();
    assert_eq!(words, vec![1, 2, 0x11223344, 0x55667788, 1234, 2]);
    assert_eq!(&t[52..56], &178u32.to_le_bytes());
    assert_eq!(&t[56..], &ownership);
    assert_eq!(auth_part(&t), Some(&t[..52]));
}

#[test]
fn create_payload_is_appid_then_handle() {
    let mut p = 2357570u32.to_le_bytes().to_vec();
    p.extend_from_slice(&9u32.to_le_bytes());
    assert_eq!(parse_create_payload(&p), Some(CreateRequest { appid: 2357570, handle: 9, ticket_type: 2, identity: String::new() }));
    assert_eq!(parse_create_payload(&p[..7]), None);
}

#[test]
fn create_payload_with_type_and_identity() {
    let mut p = Vec::new();
    p.extend_from_slice(&480u32.to_le_bytes());
    p.extend_from_slice(&5u32.to_le_bytes());
    p.extend_from_slice(&5u32.to_le_bytes());
    p.extend_from_slice(&3u16.to_le_bytes());
    p.extend_from_slice(b"abc");
    assert_eq!(parse_create_payload(&p), Some(CreateRequest { appid: 480, handle: 5, ticket_type: 5, identity: "abc".into() }));
    assert_eq!(parse_create_payload(&p[..13]), None);
    let t = build_auth_ticket(&[1u8; 20], TICKET_TYPE_WEB_API, [0, 0], 0, 1, &[]);
    assert_eq!(u32::from_le_bytes(t[4 + 20 + 4 + 4..4 + 20 + 4 + 8].try_into().unwrap()), 5);
}
