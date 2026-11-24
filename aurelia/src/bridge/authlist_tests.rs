use super::*;

#[test]
fn crc32_matches_the_standard_check_value() {
    assert_eq!(crc32(b"123456789"), 0xCBF43926);
    assert_eq!(crc32(b""), 0);
}

#[test]
fn register_payload_roundtrips() {
    let mut payload = 2357570u32.to_le_bytes().to_vec();
    payload.extend_from_slice(&7u32.to_le_bytes());
    payload.extend_from_slice(&[9, 8, 7]);
    let t = parse_register_payload(&payload).unwrap();
    assert_eq!((t.appid, t.handle, t.ticket.as_slice()), (2357570, 7, &[9u8, 8, 7][..]));
    assert!(parse_register_payload(&payload[..8]).is_none());
}

fn full_ticket(token_byte: u8, ownership: &[u8]) -> Vec<u8> {
    let mut t = 20u32.to_le_bytes().to_vec();
    t.extend_from_slice(&[token_byte; 20]);
    t.extend_from_slice(&24u32.to_le_bytes());
    t.extend_from_slice(&[0x55; 24]);
    t.extend_from_slice(&(ownership.len() as u32).to_le_bytes());
    t.extend_from_slice(ownership);
    t
}

#[test]
fn auth_part_is_token_plus_session_header() {
    let t = full_ticket(7, &[1, 2, 3]);
    assert_eq!(auth_part(&t), Some(&t[..52]));
    assert_eq!(auth_part(&t[..40]), None);
    assert_eq!(auth_part(&[1, 2, 3]), None);
}

#[test]
fn auth_list_registers_only_the_auth_part_of_every_active_ticket() {
    let ta = full_ticket(1, &[1, 2, 3]);
    let tb = full_ticket(2, &[4, 5]);
    let a = ActiveTicket { handle: 1, appid: 2357570, ticket: ta.clone(), ticket_type: 0, identity: String::new() };
    let b = ActiveTicket { handle: 2, appid: 2357570, ticket: tb, ticket_type: 0, identity: String::new() };
    let list = build_auth_list(8, &[a, b], 3);
    assert_eq!(list.tokens_left, Some(8));
    assert_eq!(list.message_sequence, Some(3));
    assert_eq!(list.app_ids, vec![2357570]);
    assert_eq!(list.tickets.len(), 2);
    let t = &list.tickets[0];
    assert_eq!(t.gameid, Some(2357570));
    assert_eq!(t.ticket.as_deref(), Some(&ta[..52]));
    assert_eq!(t.ticket_crc, Some(crc32(&ta[..52])));
    assert_eq!(t.steamid, None);
    assert_eq!(t.h_steam_pipe, None);
}

#[test]
fn registered_crc_is_the_crc_of_the_auth_part() {
    let t = full_ticket(9, &[1]);
    assert_eq!(registered_crc(&t), crc32(&t[..52]));
}

#[test]
fn empty_list_clears_the_registration() {
    let list = build_auth_list(0, &[], 9);
    assert!(list.tickets.is_empty() && list.app_ids.is_empty());
}

#[test]
fn web_api_ticket_registers_its_type_and_identity() {
    let ticket = {
        let mut t = Vec::new();
        t.extend_from_slice(&20u32.to_le_bytes());
        t.extend_from_slice(&[7u8; 20]);
        t.extend_from_slice(&24u32.to_le_bytes());
        t.extend_from_slice(&[1u8; 24]);
        t.extend_from_slice(&2u32.to_le_bytes());
        t.extend_from_slice(&[9u8; 2]);
        t
    };
    let web = ActiveTicket { handle: 3, appid: 480, ticket: ticket.clone(), ticket_type: 5, identity: "my-service".into() };
    let plain = ActiveTicket { handle: 4, appid: 480, ticket, ticket_type: 0, identity: String::new() };
    let list = build_auth_list(10, &[web, plain], 1);
    assert_eq!(list.tickets[0].ticket_type, Some(5));
    assert_eq!(list.tickets[0].server_secret.as_deref(), Some(&b"my-service"[..]));
    assert_eq!(list.tickets[1].ticket_type, None);
    assert_eq!(list.tickets[1].server_secret, None);
}
