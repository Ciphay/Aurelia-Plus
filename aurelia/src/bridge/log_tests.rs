use super::*;

#[test]
fn lines_are_appended_with_a_timestamp_prefix() {
    let dir = std::env::temp_dir().join(format!("aurelia-bridge-log-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let file = dir.join("bridge.log");
    append_to(&file, "first");
    append_to(&file, "second line");
    let text = std::fs::read_to_string(&file).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].ends_with(" first") && lines[1].ends_with(" second line"));
    let stamp = lines[0].split(' ').next().unwrap();
    assert!(stamp.parse::<u64>().is_ok(), "prefix is unix seconds: {stamp}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unwritable_path_is_ignored() {
    append_to(std::path::Path::new("/proc/definitely/not/here/bridge.log"), "x");
}

#[test]
fn rotation_keeps_the_newest_files_and_drops_the_oldest() {
    let dir = std::env::temp_dir().join(format!("aurelia-logrot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("bridge.log");
    for round in 0..5 {
        std::fs::write(&file, format!("round {round} {}", "x".repeat(100))).unwrap();
        rotate_if_big(&file, 50, 3);
        assert!(!file.exists(), "the big file moves aside");
    }
    let read = |n: u32| std::fs::read_to_string(dir.join(format!("bridge.log.{n}"))).unwrap();
    assert!(read(1).starts_with("round 4"));
    assert!(read(2).starts_with("round 3"));
    assert!(read(3).starts_with("round 2"));
    assert!(!dir.join("bridge.log.4").exists());
}

#[test]
fn small_files_are_left_alone() {
    let dir = std::env::temp_dir().join(format!("aurelia-logsmall-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("bridge.log");
    std::fs::write(&file, "short").unwrap();
    rotate_if_big(&file, 50, 3);
    assert!(file.exists());
}
