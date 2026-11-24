use std::io::Write;
use std::path::Path;

pub const MAX_BYTES: u64 = 4 * 1024 * 1024;
pub const KEEP: u32 = 3;

pub fn rotate_if_big(file: &Path, max: u64, keep: u32) {
    if std::fs::metadata(file).map_or(true, |m| m.len() < max) {
        return;
    }
    let numbered = |n: u32| {
        let mut name = file.as_os_str().to_owned();
        name.push(format!(".{n}"));
        std::path::PathBuf::from(name)
    };
    let _ = std::fs::remove_file(numbered(keep));
    for n in (1..keep).rev() {
        let _ = std::fs::rename(numbered(n), numbered(n + 1));
    }
    let _ = std::fs::rename(file, numbered(1));
}

pub fn append_to(file: &Path, line: &str) {
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    rotate_if_big(file, MAX_BYTES, KEEP);
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
        let _ = writeln!(f, "{secs} {line}");
    }
}

pub fn bridge_log(line: &str) {
    if let Ok(dir) = crate::core::config::config_dir() {
        append_to(&dir.join("logs").join("bridge.log"), line);
    }
}

#[cfg(test)]
#[path = "log_tests.rs"]
mod tests;
