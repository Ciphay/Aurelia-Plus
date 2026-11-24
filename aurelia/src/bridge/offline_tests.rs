use super::*;

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("aurelia-offline-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("queue.json")
}

#[test]
fn stats_of_the_same_app_merge_and_last_value_wins() {
    let mut q = Queue::default();
    q.push(Op::StoreStats { app: 550, changes: vec![(1, 10), (2, 5)] });
    q.push(Op::StoreStats { app: 550, changes: vec![(2, 7), (3, 1)] });
    q.push(Op::StoreStats { app: 730, changes: vec![(1, 1)] });
    assert_eq!(q.len(), 2);
    assert_eq!(q.ops()[0], Op::StoreStats { app: 550, changes: vec![(1, 10), (2, 7), (3, 1)] });
}

#[test]
fn cloud_up_is_one_mark_per_app_and_empty_stats_are_dropped() {
    let mut q = Queue::default();
    q.push(Op::CloudUp { app: 550 });
    q.push(Op::CloudUp { app: 550 });
    q.push(Op::CloudUp { app: 730 });
    q.push(Op::StoreStats { app: 550, changes: vec![] });
    assert_eq!(q.len(), 2);
}

#[test]
fn queue_survives_a_round_trip_and_an_empty_one_removes_the_file() {
    let path = tmp("roundtrip");
    let mut q = Queue::default();
    q.push(Op::StoreStats { app: 550, changes: vec![(1, 2)] });
    q.push(Op::CloudUp { app: 550 });
    save_to(&path, &q);
    assert_eq!(load_from(&path), q);
    save_to(&path, &Queue::default());
    assert!(!path.exists());
    assert!(load_from(&path).is_empty());
}

#[test]
fn truncated_or_old_format_files_read_as_empty() {
    let path = tmp("broken");
    std::fs::write(&path, b"{\"format\":1,\"ops\":[{\"kind\":\"stor").unwrap();
    assert!(load_from(&path).is_empty());
    std::fs::write(&path, br#"{"format":99,"ops":[{"kind":"cloud_up","app":1}]}"#).unwrap();
    assert!(load_from(&path).is_empty());
}

#[test]
fn acknowledging_removes_only_what_was_sent_and_keeps_newer_values() {
    let mut q = Queue::default();
    q.push(Op::StoreStats { app: 550, changes: vec![(1, 1), (2, 2)] });
    q.push(Op::CloudUp { app: 550 });
    let sent = q.ops()[0].clone();
    q.push(Op::StoreStats { app: 550, changes: vec![(1, 9), (3, 3)] });
    q.subtract(&sent);
    assert_eq!(q.ops(), &[Op::StoreStats { app: 550, changes: vec![(1, 9), (3, 3)] }, Op::CloudUp { app: 550 }]);
    q.subtract(&Op::CloudUp { app: 550 });
    assert_eq!(q.len(), 1);
}

#[test]
fn a_fully_acknowledged_stats_op_disappears() {
    let mut q = Queue::default();
    q.push(Op::StoreStats { app: 550, changes: vec![(1, 1)] });
    let sent = q.ops()[0].clone();
    q.subtract(&sent);
    assert!(q.is_empty());
}

#[test]
fn retry_delay_grows_and_is_capped() {
    assert_eq!(retry_delay(0), RETRY_FIRST);
    assert_eq!(retry_delay(1), RETRY_FIRST * 2);
    assert_eq!(retry_delay(2), RETRY_FIRST * 4);
    assert_eq!(retry_delay(30), RETRY_MAX);
}

#[test]
fn connection_flag_flips() {
    let c = Connection::new(false);
    assert!(!c.online());
    c.set(true);
    assert!(c.online());
}
