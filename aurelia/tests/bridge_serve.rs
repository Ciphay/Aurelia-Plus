use std::process::Command;
use std::time::{Duration, Instant};

use tempfile::tempdir;

#[test]
fn serve_without_session_exits_77_with_auth_required() {
    let tmp = tempdir().unwrap();
    let started = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_aurelia"))
        .args(["--json", "bridge", "serve", "--app", "289650"])
        .env("AURELIA_CONFIG_DIR", tmp.path())
        .env("AURELIA_NO_DAEMON", "1")
        .env("AURELIA_DISABLE_KEYRING", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("failed running the aurelia binary");
    assert!(started.elapsed() < Duration::from_secs(20), "demorou demais");
    assert_eq!(out.status.code(), Some(77), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("auth_required"), "{text}");
}
