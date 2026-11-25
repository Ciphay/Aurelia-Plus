//! Which files of a game are install scripts. A depot marks them in its manifest (file flag 0x100); Steam finds them there. The manifest is
//! only at hand while the game downloads, so the list is kept per app (`<config>/install-scripts/<appid>.json`, per depot) for the launch.

use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Record {
    /// depot id -> relative paths (with `/`)
    depots: BTreeMap<String, Vec<String>>,
}

fn path(app_id: u32) -> Option<PathBuf> {
    Some(crate::core::config::config_dir().ok()?.join("install-scripts").join(format!("{app_id}.json")))
}

fn read(app_id: u32) -> Option<Record> {
    serde_json::from_slice(&std::fs::read(path(app_id)?).ok()?).ok()
}

/// Remember what `depot_id` marks as install scripts (an empty list too: the depot was seen and has none).
pub fn save_depot(app_id: u32, depot_id: u32, files: Vec<String>) {
    let Some(p) = path(app_id) else { return };
    let mut record = read(app_id).unwrap_or_default();
    record.depots.insert(depot_id.to_string(), files);
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec(&record) {
        if let Err(e) = crate::core::atomic::write_atomic(&p, &bytes) {
            tracing::debug!("install scripts of {app_id} not recorded: {e:#}");
        }
    }
}

/// The install scripts of every depot seen, relative to the install folder; `None` when this app was never recorded (installed before
/// Aurelia kept the list).
pub fn load(app_id: u32) -> Option<Vec<String>> {
    let record = read(app_id)?;
    let mut all: Vec<String> = record.depots.into_values().flatten().collect();
    all.sort();
    all.dedup();
    Some(all)
}
