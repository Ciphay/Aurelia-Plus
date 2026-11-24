use std::path::PathBuf;

fn dir() -> Option<PathBuf> {
    Some(crate::core::config::config_dir().ok()?.join("pics-cache"))
}

fn path(appid: u32) -> Option<PathBuf> {
    Some(dir()?.join(format!("{appid}.bin")))
}

pub fn store(appid: u32, buffer: &[u8]) {
    if buffer.is_empty() {
        return;
    }
    let Some(p) = path(appid) else { return };
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = crate::core::atomic::write_atomic(&p, buffer) {
        tracing::debug!("pics cache for {appid} not written: {e:#}");
    }
}

pub fn load(appid: u32) -> Option<Vec<u8>> {
    let bytes = std::fs::read(path(appid)?).ok()?;
    (!bytes.is_empty()).then_some(bytes)
}

pub fn exists(appid: u32) -> bool {
    path(appid).is_some_and(|p| p.is_file())
}
