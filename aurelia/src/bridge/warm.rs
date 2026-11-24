use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::snapshot::{BetaBranch, DlcEntry, SnapshotV1};

pub const NAMES_TTL_SECS: u64 = 24 * 3600;
pub const PREWARM_FRESH_SECS: u64 = 6 * 3600;
pub const APP_NET_TTL_SECS: u64 = 10 * 60;
pub const APP_NET_STALE_MAX_SECS: u64 = 30 * 24 * 3600;
pub const FORMAT: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppNet {
    pub dlc: Vec<DlcEntry>,
    pub cloud_quota: u64,
    pub betas: Vec<BetaBranch>,
    pub at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Disk {
    format: u32,
    names: HashMap<u64, (u64, String)>,
    apps: HashMap<u32, AppNet>,
    #[serde(default)]
    licenses: Vec<(u32, u32)>,
    #[serde(default)]
    package_apps: HashMap<u32, Vec<u32>>,
}

#[derive(Default)]
struct Inner {
    steamid: u64,
    loaded: bool,
    disk: Disk,
}

pub struct Warm {
    inner: Mutex<Inner>,
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

pub fn global() -> &'static Warm {
    static WARM: OnceLock<Warm> = OnceLock::new();
    WARM.get_or_init(|| Warm { inner: Mutex::new(Inner::default()) })
}

fn dir_for(steamid: u64) -> Option<PathBuf> {
    Some(crate::core::config::data_dir().ok()?.join("bridge-cache").join(steamid.to_string()))
}

fn write_private(path: &std::path::Path, bytes: &[u8]) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if crate::core::atomic::write_atomic(path, bytes).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
}

impl Warm {
    fn with<R>(&self, steamid: u64, f: impl FnOnce(&mut Inner) -> R) -> R {
        let mut inner = self.inner.lock().expect("warm cache lock poisoned");
        if inner.steamid != steamid || !inner.loaded {
            *inner = Inner { steamid, loaded: true, disk: load_disk(steamid) };
        }
        f(&mut inner)
    }

    fn save(inner: &Inner) {
        if let (Some(dir), Ok(bytes)) = (dir_for(inner.steamid), serde_json::to_vec(&inner.disk)) {
            write_private(&dir.join("cache.json"), &bytes);
        }
    }

    pub fn names(&self, steamid: u64, ids: &[u64]) -> HashMap<u64, String> {
        let now = now_secs();
        self.with(steamid, |i| {
            ids.iter()
                .filter_map(|id| i.disk.names.get(id).filter(|(at, _)| now.saturating_sub(*at) < NAMES_TTL_SECS).map(|(_, n)| (*id, n.clone())))
                .collect()
        })
    }

    pub fn put_names(&self, steamid: u64, found: &HashMap<u64, String>) {
        if found.is_empty() {
            return;
        }
        let now = now_secs();
        self.with(steamid, |i| {
            for (id, name) in found {
                i.disk.names.insert(*id, (now, name.clone()));
            }
            Self::save(i);
        });
    }

    pub fn app_net(&self, steamid: u64, app: u32) -> Option<AppNet> {
        let now = now_secs();
        self.with(steamid, |i| i.disk.apps.get(&app).filter(|n| now.saturating_sub(n.at) < APP_NET_TTL_SECS).cloned())
    }

    pub fn app_net_stale_ok(&self, steamid: u64, app: u32) -> Option<(AppNet, bool)> {
        let now = now_secs();
        self.with(steamid, |i| {
            i.disk.apps.get(&app).and_then(|n| {
                let age = now.saturating_sub(n.at);
                (age < APP_NET_STALE_MAX_SECS).then(|| (n.clone(), age < APP_NET_TTL_SECS))
            })
        })
    }

    pub fn put_app_net(&self, steamid: u64, app: u32, net: AppNet) {
        self.with(steamid, |i| {
            i.disk.apps.insert(app, net);
            Self::save(i);
        });
    }

    pub fn put_licenses(&self, steamid: u64, licenses: Vec<(u32, u32)>, package_apps: HashMap<u32, Vec<u32>>) {
        self.with(steamid, |i| {
            i.disk.licenses = licenses;
            i.disk.package_apps.extend(package_apps);
            Self::save(i);
        });
    }

    pub fn packages_missing(&self, steamid: u64, wanted: &[u32]) -> Vec<u32> {
        self.with(steamid, |i| wanted.iter().copied().filter(|p| !i.disk.package_apps.contains_key(p)).collect())
    }

    pub fn package_gives(&self, steamid: u64, app: u32) -> bool {
        self.with(steamid, |i| i.disk.licenses.iter().any(|(pkg, _)| i.disk.package_apps.get(pkg).is_some_and(|apps| apps.contains(&app))))
    }

    pub fn purchase_time(&self, steamid: u64, app: u32) -> Option<u32> {
        self.with(steamid, |i| {
            i.disk.licenses.iter().filter(|(pkg, t)| *t > 0 && i.disk.package_apps.get(pkg).is_some_and(|apps| apps.contains(&app))).map(|(_, t)| *t).min()
        })
    }

    pub fn invalidate_app(&self, steamid: u64, app: u32) {
        self.with(steamid, |i| {
            i.disk.apps.remove(&app);
        });
    }
}

static REFRESHING: Mutex<Vec<(u64, u32)>> = Mutex::new(Vec::new());

pub struct RefreshGuard(u64, u32);

impl Drop for RefreshGuard {
    fn drop(&mut self) {
        REFRESHING.lock().expect("refresh lock poisoned").retain(|k| *k != (self.0, self.1));
    }
}

pub fn begin_refresh(steamid: u64, app: u32) -> Option<RefreshGuard> {
    let mut g = REFRESHING.lock().expect("refresh lock poisoned");
    if g.contains(&(steamid, app)) {
        return None;
    }
    g.push((steamid, app));
    Some(RefreshGuard(steamid, app))
}

fn load_disk(steamid: u64) -> Disk {
    let Some(path) = dir_for(steamid).map(|d| d.join("cache.json")) else { return Disk::default() };
    match std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Disk>(&b).ok()) {
        Some(d) if d.format == FORMAT => d,
        _ => Disk { format: FORMAT, ..Disk::default() },
    }
}

pub fn save_snapshot(steamid: u64, app: u32, snapshot: &SnapshotV1) {
    if let (Some(dir), Ok(bytes)) = (dir_for(steamid), serde_json::to_vec(snapshot)) {
        write_private(&dir.join(format!("snapshot-{app}.json")), &bytes);
    }
}

pub fn load_snapshot(steamid: u64, app: u32) -> Option<(SnapshotV1, u64)> {
    let path = dir_for(steamid)?.join(format!("snapshot-{app}.json"));
    let at = std::fs::metadata(&path).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let snapshot: SnapshotV1 = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
    (snapshot.schema == super::snapshot::SCHEMA).then_some((snapshot, at))
}

pub fn save_stats(steamid: u64, app: u32, eresult: i32, reply: &[u8]) {
    if let Some(dir) = dir_for(steamid) {
        let mut bytes = eresult.to_le_bytes().to_vec();
        bytes.extend_from_slice(reply);
        write_private(&dir.join(format!("stats-{app}.bin")), &bytes);
    }
}

pub fn load_stats(steamid: u64, app: u32) -> Option<(i32, Vec<u8>)> {
    let bytes = std::fs::read(dir_for(steamid)?.join(format!("stats-{app}.bin"))).ok()?;
    let eresult = i32::from_le_bytes(bytes.get(..4)?.try_into().ok()?);
    Some((eresult, bytes[4..].to_vec()))
}

pub fn last_account_with_snapshot(app: u32) -> Option<u64> {
    let root = crate::core::config::data_dir().ok()?.join("bridge-cache");
    let mut best: Option<(u64, u64)> = None;
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let Some(id) = entry.file_name().to_str().and_then(|n| n.parse::<u64>().ok()) else { continue };
        let at = std::fs::metadata(entry.path().join(format!("snapshot-{app}.json"))).ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs());
        if let Some(at) = at {
            if best.is_none_or(|(_, b)| at > b) {
                best = Some((id, at));
            }
        }
    }
    best.map(|(id, _)| id)
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AccountCache {
    pub steam_id: u64,
    pub names: usize,
    pub app_info: Vec<(u32, u64)>,
    pub snapshots: Vec<(u32, u64)>,
    pub queued: usize,
}

pub fn accounts() -> Vec<AccountCache> {
    let Ok(root) = crate::core::config::data_dir().map(|d| d.join("bridge-cache")) else { return Vec::new() };
    let now = now_secs();
    let mut out = Vec::new();
    for entry in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let Some(steam_id) = entry.file_name().to_str().and_then(|n| n.parse::<u64>().ok()) else { continue };
        let disk = load_disk(steam_id);
        let mut app_info: Vec<(u32, u64)> = disk.apps.iter().map(|(a, n)| (*a, now.saturating_sub(n.at))).collect();
        app_info.sort_unstable();
        let mut snapshots: Vec<(u32, u64)> = std::fs::read_dir(entry.path())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|f| {
                let name = f.file_name().into_string().ok()?;
                let app = name.strip_prefix("snapshot-")?.strip_suffix(".json")?.parse::<u32>().ok()?;
                let at = f.metadata().ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
                Some((app, now.saturating_sub(at)))
            })
            .collect();
        snapshots.sort_unstable();
        out.push(AccountCache { steam_id, names: disk.names.len(), app_info, snapshots, queued: super::offline::pending(steam_id) });
    }
    out.sort_by_key(|a| a.steam_id);
    out
}

#[cfg(test)]
#[path = "warm_tests.rs"]
mod tests;
