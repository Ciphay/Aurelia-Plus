use super::*;

const MTIME: Option<SystemTime> = Some(SystemTime::UNIX_EPOCH);

fn slot(client: bool, session_mtime: Option<SystemTime>, last_failure: Option<Instant>) -> Slot {
    Slot {
        client: client.then(|| SteamClient::new().unwrap()),
        session_mtime,
        last_failure,
        last_error: None,
        failures: 0,
    }
}

#[test]
fn never_attempted_triggers_a_restore() {
    assert!(!slot(false, None, None).is_current(MTIME));
}

#[test]
fn live_session_is_a_no_op() {
    assert!(slot(true, MTIME, None).is_current(MTIME));
}

#[test]
fn token_change_forces_restore_even_with_live_client() {
    let other = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1));
    assert!(!slot(true, MTIME, None).is_current(other));
}

#[test]
fn recent_failure_is_held_off_until_the_backoff_elapses() {
    assert!(slot(false, MTIME, Some(Instant::now())).is_current(MTIME));
}

#[test]
fn stale_failure_self_heals_after_the_backoff() {
    let past = Instant::now()
        .checked_sub(RESTORE_RETRY_BACKOFF + Duration::from_secs(1))
        .expect("instant underflow");
    assert!(!slot(false, MTIME, Some(past)).is_current(MTIME));
}

#[test]
fn header_round_trips_with_and_without_config_dir() {
    let old: Header = serde_json::from_str(r#"{"argv":["aurelia","list"]}"#).unwrap();
    assert_eq!(old.argv, ["aurelia", "list"]);
    assert!(old.config_dir.is_none());
    assert!(old.cwd.is_none());
    assert!(old.env.is_empty(), "a header from an older client has no env");

    let h = Header {
        argv: vec!["aurelia".into()],
        config_dir: Some("/tmp/iso".into()),
        cwd: Some("/home/u/shots".into()),
        env: vec![("DISPLAY".into(), ":1".into())],
    };
    let back: Header = serde_json::from_slice(&serde_json::to_vec(&h).unwrap()).unwrap();
    assert_eq!(
        back.config_dir.as_deref(),
        Some(std::path::Path::new("/tmp/iso"))
    );
    assert_eq!(
        back.cwd.as_deref(),
        Some(std::path::Path::new("/home/u/shots"))
    );
    assert_eq!(back.env, vec![("DISPLAY".to_string(), ":1".to_string())]);
}

#[test]
fn config_dir_mismatch_refuses_only_differing_dirs() {
    use std::path::Path;
    let a = Path::new("/home/u/.config/Aurelia");
    let b = Path::new("/tmp/iso");
    assert!(config_dir_mismatch(Some(a), Some(a)).is_none());
    assert!(config_dir_mismatch(None, Some(a)).is_none());
    let msg = config_dir_mismatch(Some(b), Some(a)).unwrap();
    assert!(msg.contains("/tmp/iso") && msg.contains("AURELIA_DAEMON_SOCKET"));
}

#[test]
fn warm_retry_backs_off_from_ten_seconds_to_five_minutes() {
    assert_eq!(super::warm_retry_delay(1), Duration::from_secs(10));
    assert_eq!(super::warm_retry_delay(2), Duration::from_secs(20));
    assert_eq!(super::warm_retry_delay(3), Duration::from_secs(40));
    assert_eq!(super::warm_retry_delay(50), Duration::from_secs(300));
}

#[test]
fn a_session_is_dead_only_after_consecutive_failed_checks() {
    assert!(!super::session_is_dead(0));
    assert!(!super::session_is_dead(1));
    assert!(super::session_is_dead(2));
}

fn failed(kind: ErrorKind, failures: u32) -> Slot {
    let mut s = slot(false, MTIME, Some(Instant::now()));
    s.last_error = Some(RestoreFailure { kind, retry_after: None, reason: String::new() });
    s.failures = failures;
    s
}

#[test]
fn no_network_is_retried_soon_and_ever_less_often() {
    let waits: Vec<u64> = (1..=6).map(|n| failed(ErrorKind::Offline, n).backoff().as_secs()).collect();
    assert_eq!(waits, vec![2, 4, 8, 16, 30, 30]);
}

#[test]
fn other_failures_keep_the_long_wait() {
    assert_eq!(failed(ErrorKind::AuthRequired, 1).backoff(), RESTORE_RETRY_BACKOFF);
    assert_eq!(failed(ErrorKind::Unknown, 3).backoff(), RESTORE_RETRY_BACKOFF);
}

#[test]
fn a_network_failure_is_retried_after_its_short_wait() {
    let mut s = failed(ErrorKind::Offline, 1);
    assert!(s.is_current(MTIME), "inside the 2 s window nothing is retried");
    s.last_failure = Instant::now().checked_sub(Duration::from_secs(3));
    assert!(!s.is_current(MTIME), "after 2 s the next request retries (a refused token would still wait 30 s)");
}
