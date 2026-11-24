use super::*;

fn call(flags: u8, emsg: i32, wait: u32, name: &str, body: &[u8]) -> Vec<u8> {
    let mut p = vec![flags, 0, 0, 0];
    p.extend_from_slice(&emsg.to_le_bytes());
    p.extend_from_slice(&wait.to_le_bytes());
    p.extend_from_slice(&(name.len() as u16).to_le_bytes());
    p.extend_from_slice(name.as_bytes());
    p.extend_from_slice(body);
    p
}

#[test]
fn call_payload_round_trip() {
    let r = parse_call_payload(&call(3, 151, 5000, "Inventory.GetInventory#1", &[1, 2, 3])).unwrap();
    assert!(r.expect_reply && r.protobuf && !r.routing);
    assert!(parse_call_payload(&call(7, 1, 0, "", &[])).unwrap().routing);
    assert!(parse_call_payload(&call(8 | 1, 1, 0, "", &[])).unwrap().gameserver);
    assert_eq!(r.emsg, 151);
    assert_eq!(r.wait_ms, 5000);
    assert_eq!(r.job_name.as_deref(), Some("Inventory.GetInventory#1"));
    assert_eq!(r.body, vec![1, 2, 3]);
}

#[test]
fn call_without_name_and_reply() {
    let r = parse_call_payload(&call(2, 6602, 0, "", &[])).unwrap();
    assert!(!r.expect_reply && r.protobuf && r.job_name.is_none() && r.body.is_empty());
}

#[test]
fn call_wait_is_capped_and_truncated_input_rejected() {
    assert_eq!(parse_call_payload(&call(1, 1, u32::MAX, "", &[])).unwrap().wait_ms, MAX_WAIT_MS);
    assert!(parse_call_payload(&[0; 13]).is_none());
    let mut bad = call(1, 1, 0, "abc", &[]);
    bad.truncate(15);
    assert!(parse_call_payload(&bad).is_none());
}

#[test]
fn reply_layout() {
    assert_eq!(encode_call_reply(147, 1, &[9]), vec![147, 0, 0, 0, 1, 0, 0, 0, 9]);
}

#[test]
fn subscribe_is_idempotent_and_queue_drains_in_order() {
    let q = PushQueue::default();
    assert!(q.subscribe(6613));
    assert!(!q.subscribe(6613));
    q.push(6613, vec![1]);
    q.push(6614, vec![2, 3]);
    let d = q.drain(1024);
    assert_eq!(d, vec![0xD5, 0x19, 0, 0, 1, 0, 0, 0, 1, 0xD6, 0x19, 0, 0, 2, 0, 0, 0, 2, 3]);
    assert!(q.drain(1024).is_empty());
}

#[test]
fn drain_respects_budget_but_always_returns_one() {
    let q = PushQueue::default();
    q.push(1, vec![0; 100]);
    q.push(2, vec![0; 100]);
    assert_eq!(q.drain(10).len(), 108);
    assert_eq!(q.drain(10_000).len(), 108);
}

#[test]
fn queue_drops_oldest_when_full() {
    let q = PushQueue::default();
    for i in 0..(QUEUE_LIMIT as i32 + 5) {
        q.push(i, vec![]);
    }
    let d = q.drain(usize::MAX);
    assert_eq!(i32::from_le_bytes(d[..4].try_into().unwrap()), 5);
}

#[test]
fn subscribe_payload_with_and_without_server_flag() {
    assert_eq!(parse_subscribe_payload(&[1, 0, 0, 0]), Some((1, false, 0)));
    assert_eq!(parse_subscribe_payload(&[1, 0, 0, 0, 1]), Some((1, true, 0)));
    assert_eq!(parse_subscribe_payload(&[1, 0, 0, 0, 0, 7, 0, 0, 0]), Some((1, false, 7)));
    assert_eq!(parse_subscribe_payload(&[1, 0]), None);
}

#[test]
fn each_process_gets_its_own_queue() {
    let hub = PushHub::default();
    let (a, b) = (hub.queue(10), hub.queue(20));
    assert!(std::sync::Arc::ptr_eq(&a, &hub.queue(10)));
    a.push(5434, vec![1]);
    b.push(5434, vec![1]);
    assert_eq!(a.drain(1024).len(), 9);
    assert_eq!(b.drain(1024).len(), 9);
    assert_eq!(parse_poll_payload(&[20, 0, 0, 0]), 20);
    assert_eq!(parse_poll_payload(&[]), 0);
}
