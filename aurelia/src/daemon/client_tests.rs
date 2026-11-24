use super::super::{DaemonInfo, current_build_id};
use super::daemon_needs_restart;

fn info(version: &str) -> DaemonInfo {
    DaemonInfo {
        version: version.to_string(),
        pid: 1234,
        build_id: current_build_id(),
    }
}

#[test]
fn same_version_is_reused() {
    assert!(!daemon_needs_restart(Some(&info("0.1.20")), "0.1.20"));
}

#[test]
fn different_version_triggers_restart() {
    assert!(daemon_needs_restart(Some(&info("0.1.19")), "0.1.20"));
    assert!(daemon_needs_restart(Some(&info("0.2.0")), "0.1.20"));
}

#[test]
fn missing_marker_triggers_restart() {
    assert!(daemon_needs_restart(None, "0.1.20"));
}

#[test]
fn different_build_id_triggers_restart() {
    let mut stale = info("0.1.20");
    stale.build_id = Some("12345-678".to_string());
    assert!(daemon_needs_restart(Some(&stale), "0.1.20"));
}

#[test]
fn missing_build_id_triggers_restart() {
    let mut old = info("0.1.20");
    old.build_id = None;
    assert!(daemon_needs_restart(Some(&old), "0.1.20"));
}

#[tokio::test]
async fn lost_connection_is_not_success() {
    let (client, mut server) = tokio::io::duplex(4096);
    let srv = tokio::spawn(async move {
        let _ = super::proto::read_frame(&mut server).await;
        drop(server);
    });
    let out = super::forward(client, vec!["install".into(), "1".into()]).await.unwrap();
    srv.await.unwrap();
    match out {
        super::Forwarded::Done(code) => assert_eq!(code, super::CONNECTION_LOST_EXIT),
        _ => panic!("expected Done"),
    }
}
