use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn session(app: u32) -> Session {
    Session { app, steam_id: 1, since: 10, startup_ms: 1400, opened_offline: false, online: true, queued: 0, port: 1234 }
}

#[test]
fn sessions_come_and_go_with_their_registration() {
    let (c1, q1) = (Arc::new(Connection::new(true)), Arc::new(AtomicUsize::new(0)));
    let (c2, q2) = (Arc::new(Connection::new(true)), Arc::new(AtomicUsize::new(0)));
    let a = register(session(424_001), Arc::clone(&c1), Arc::clone(&q1));
    let b = register(session(424_002), Arc::clone(&c2), q2);
    let apps: Vec<u32> = sessions().iter().map(|s| s.app).filter(|a| *a >= 424_000 && *a < 425_000).collect();
    assert_eq!(apps, vec![424_001, 424_002]);
    drop(a);
    let apps: Vec<u32> = sessions().iter().map(|s| s.app).filter(|a| *a >= 424_000 && *a < 425_000).collect();
    assert_eq!(apps, vec![424_002]);
    drop(b);
}

#[test]
fn state_is_read_live() {
    let (c, q) = (Arc::new(Connection::new(false)), Arc::new(AtomicUsize::new(0)));
    let reg = register(session(424_010), Arc::clone(&c), Arc::clone(&q));
    let find = || sessions().into_iter().find(|s| s.app == 424_010).unwrap();
    assert!(!find().online);
    c.set(true);
    q.store(3, Ordering::Relaxed);
    let s = find();
    assert!(s.online);
    assert_eq!(s.queued, 3);
    drop(reg);
}
