pub mod backlog;
pub mod cloud_sync;
pub mod collections;
pub mod depot_browser;
pub mod local_library;
pub mod relocate;

use crate::core::config::load_launcher_config;
use crate::core::models::{GameLibrary, LibraryGame, OwnedGame};
use crate::core::utils::extract_quoted_values;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs;

pub const IGNORED_STEAM_APP_IDS: &[u32] = &[
    228980,
    1070560,
    1391110,
    1628350,
    1493710,
    2348590,
    1826330,
    1161040,
];

pub const IGNORED_STEAM_APP_NAME_PREFIXES: &[&str] = &[
    "Steam Linux Runtime",
    "Proton",
    "Steamworks Common Redistributables",
];

pub fn is_ignored_steam_app(app_id: u32, name: &str) -> bool {
    if IGNORED_STEAM_APP_IDS.contains(&app_id) {
        return true;
    }
    let name = name.trim_start();
    IGNORED_STEAM_APP_NAME_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

#[derive(Debug, Clone)]
pub struct InstalledAppInfo {
    pub install_path: PathBuf,
    pub active_branch: String,
    pub name: Option<String>,
    pub last_owner: Option<u64>,
    pub from_windows_steam: bool,
    pub manifest_path: PathBuf,
    pub last_updated: u64,
    pub build_id: u64,
}

pub async fn scan_installed_app_info() -> Result<HashMap<u32, InstalledAppInfo>> {
    let config = load_launcher_config().await.ok();

    let libraries = expand_library_roots(&steam_data_roots().await).await;
    tracing::debug!(
        "scanning {} librar(y/ies): {:?}",
        libraries.len(),
        libraries
    );
    let mut installed = scan_libraries(&libraries).await;

    if config.is_some_and(|cfg| cfg.windows_steam_discovery_enabled) {
        let master_steam = crate::core::utils::get_master_steam_config();
        if master_steam.wine_prefix.exists() {
            tracing::debug!(
                "scanning Windows Steam root: {:?}",
                master_steam.wine_prefix
            );
            let windows_steam_root = master_steam
                .wine_prefix
                .join("drive_c/Program Files (x86)/Steam");
            if windows_steam_root.exists() {
                let windows_installed = scan_library_info(&windows_steam_root)
                    .await
                    .unwrap_or_default();
                for (app_id, mut info) in windows_installed {
                    info.from_windows_steam = true;
                    installed.entry(app_id).or_insert(info);
                }
            }
        }
    }

    Ok(installed)
}

#[derive(Debug, Clone)]
pub struct DuplicateInstall {
    pub app_id: u32,
    pub name: Option<String>,
    pub live: InstalledAppInfo,
    pub stale: Vec<InstalledAppInfo>,
}

pub async fn find_duplicate_installs() -> Result<Vec<DuplicateInstall>> {
    let libraries = expand_library_roots(&steam_data_roots().await).await;
    let mut duplicates: Vec<DuplicateInstall> = scan_libraries_all(&libraries)
        .await
        .into_iter()
        .filter(|(_, copies)| copies.len() > 1)
        .filter_map(|(app_id, mut copies)| {
            copies.sort_by_key(|c| std::cmp::Reverse((c.last_updated, c.build_id)));
            let live = copies.first()?.clone();
            let name = live.name.clone();
            Some(DuplicateInstall {
                app_id,
                name,
                live,
                stale: copies.into_iter().skip(1).collect(),
            })
        })
        .collect();
    duplicates.sort_by(|a, b| {
        a.name
            .as_deref()
            .unwrap_or("")
            .cmp(b.name.as_deref().unwrap_or(""))
            .then(a.app_id.cmp(&b.app_id))
    });
    Ok(duplicates)
}

async fn steam_data_roots() -> Vec<PathBuf> {
    let config = load_launcher_config().await.ok();
    let config_path = config.as_ref().and_then(|cfg| {
        let p = PathBuf::from(&cfg.steam_library_path);
        (p.join("steamapps").exists() || p.join("Steam").join("steamapps").exists()).then_some(p)
    });

    let mut roots: Vec<PathBuf> = config_path.into_iter().collect();
    roots.extend(steam_root_candidates());
    roots.sort();
    roots.dedup();
    roots
}

pub async fn scan_library_info(root_path: &Path) -> Result<HashMap<u32, InstalledAppInfo>> {
    let libraries = expand_library_roots(std::slice::from_ref(&root_path.to_path_buf())).await;
    Ok(scan_libraries(&libraries).await)
}

async fn expand_library_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut libraries: Vec<PathBuf> = roots.to_vec();

    for root in roots {
        for vdf in libraryfolders_vdf_paths(root) {
            match parse_library_folders(vdf.clone()).await {
                Ok(found) => libraries.extend(found),
                Err(e) => tracing::warn!("could not parse {}: {e}", vdf.display()),
            }
        }
    }

    libraries.extend(discover_drive_libraries());

    libraries = libraries
        .into_iter()
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .collect();

    libraries.sort();
    libraries.dedup();
    libraries
}

fn libraryfolders_vdf_paths(root: &Path) -> Vec<PathBuf> {
    vec![
        root.join("steamapps").join("libraryfolders.vdf"),
        root.join("config").join("libraryfolders.vdf"),
    ]
}

async fn scan_libraries(libraries: &[PathBuf]) -> HashMap<u32, InstalledAppInfo> {
    scan_libraries_all(libraries)
        .await
        .into_iter()
        .filter_map(|(app_id, copies)| pick_live_copy(copies).map(|info| (app_id, info)))
        .collect()
}

fn pick_live_copy(copies: Vec<InstalledAppInfo>) -> Option<InstalledAppInfo> {
    copies.into_iter().reduce(|best, next| {
        let better = (next.last_updated, next.build_id) > (best.last_updated, best.build_id);
        if better { next } else { best }
    })
}

pub async fn scan_libraries_all(libraries: &[PathBuf]) -> HashMap<u32, Vec<InstalledAppInfo>> {
    let mut installed: HashMap<u32, Vec<InstalledAppInfo>> = HashMap::new();

    for library_root in libraries {
        let steamapps = library_root.join("steamapps");
        if !steamapps.exists() {
            continue;
        }

        let mut dir = match fs::read_dir(&steamapps).await {
            Ok(dir) => dir,
            Err(e) => {
                tracing::warn!("skipping library {}: {e}", steamapps.display());
                continue;
            }
        };

        while let Ok(Some(entry)) = dir.next_entry().await {
            let path = entry.path();
            if !is_app_manifest(&path) {
                continue;
            }

            match parse_app_manifest_info(&path).await {
                Ok(Some((app_id, info))) => {
                    installed.entry(app_id).or_default().push(info);
                }
                Ok(None) => {}
                Err(e) => tracing::warn!("skipping bad manifest {:?}: {}", path, e),
            }
        }
    }

    installed
}

const MAX_PROBE_DEPTH: usize = 2;

const MAX_PROBE_ENTRIES: usize = 512;

const PROBE_SKIP_DIRS: &[&str] = &[
    "bin",
    "boot",
    "dev",
    "etc",
    "lib",
    "lib32",
    "lib64",
    "libx32",
    "proc",
    "root",
    "run",
    "sbin",
    "srv",
    "sys",
    "tmp",
    "usr",
    "var",
    "lost+found",
    "$RECYCLE.BIN",
    "System Volume Information",
    "Recovery",
    "Windows",
    "WinSxS",
    "AppData",
    "node_modules",
];

pub fn discover_drive_libraries() -> Vec<PathBuf> {
    let mut found = Vec::new();

    for volume in volume_roots() {
        probe_for_libraries(&volume, MAX_PROBE_DEPTH, &mut found);
    }

    found.sort();
    found.dedup();
    found
}

fn probe_for_libraries(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    if dir.join("steamapps").is_dir() {
        found.push(dir.to_path_buf());
        return;
    }
    if depth == 0 {
        return;
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten().take(MAX_PROBE_ENTRIES) {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.')
            || PROBE_SKIP_DIRS
                .iter()
                .any(|skip| name.eq_ignore_ascii_case(skip))
        {
            continue;
        }
        probe_for_libraries(&entry.path(), depth - 1, found);
    }
}

fn volume_roots() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let mut roots = Vec::new();
        for letter in b'A'..=b'Z' {
            let drive = PathBuf::from(format!("{}:\\", letter as char));
            if drive.exists() {
                roots.push(drive);
            }
        }
        roots
    }

    #[cfg(not(target_os = "windows"))]
    {
        mounted_volume_roots()
    }
}

#[cfg(not(target_os = "windows"))]
fn mounted_volume_roots() -> Vec<PathBuf> {
    const DISK_FSTYPES: &[&str] = &[
        "ext2", "ext3", "ext4", "btrfs", "xfs", "f2fs", "jfs", "reiserfs", "zfs", "bcachefs",
        "ntfs", "ntfs3", "fuseblk", "exfat", "vfat", "msdos", "udf", "apfs", "hfs", "hfsplus",
        "nfs", "nfs4", "cifs", "smb3",
    ];

    let mut roots: Vec<PathBuf> = Vec::new();

    if let Ok(mounts) = std::fs::read_to_string("/proc/self/mounts") {
        for line in mounts.lines() {
            let mut fields = line.split_whitespace();
            let (Some(_source), Some(target), Some(fstype)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            if !DISK_FSTYPES.contains(&fstype) {
                continue;
            }
            roots.push(PathBuf::from(unescape_mount_target(target)));
        }
    }

    for parent in ["/Volumes", "/run/media", "/media", "/mnt"] {
        let Ok(entries) = std::fs::read_dir(parent) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if let Ok(nested) = std::fs::read_dir(&path) {
                roots.extend(nested.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
            }
            roots.push(path);
        }
    }

    roots.sort();
    roots.dedup();
    roots
}

#[cfg(not(target_os = "windows"))]
fn unescape_mount_target(target: &str) -> String {
    if !target.contains('\\') {
        return target.to_string();
    }
    let bytes = target.as_bytes();
    // Decoded byte-wise, not char-wise: an escape yields one raw byte, which may
    // be part of a multi-byte UTF-8 sequence in the mount point's name.
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let octal = (bytes[i] == b'\\' && i + 3 < bytes.len())
            .then(|| std::str::from_utf8(&bytes[i + 1..i + 4]).ok())
            .flatten()
            .and_then(|digits| u8::from_str_radix(digits, 8).ok());
        match octal {
            Some(byte) => {
                out.push(byte);
                i += 4;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Collect every Steam library root we can discover: the configured library,
/// every standard Steam data root, anything their `libraryfolders.vdf` files
/// reference, and a probe of all connected drives.
pub async fn all_library_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();

    if let Ok(cfg) = load_launcher_config().await {
        let p = PathBuf::from(&cfg.steam_library_path);
        if !p.as_os_str().is_empty() {
            roots.push(p);
        }
    }
    roots.extend(steam_root_candidates());

    expand_library_roots(&roots).await
}

/// Every standard location a Steam data root can live in on this platform.
///
/// All of them are returned, not just the first that exists: the packaging
/// formats install side by side, and a machine can easily carry a stale native
/// `~/.steam` next to the Flatpak or Snap install that holds the real library.
pub fn steam_root_candidates() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    #[cfg(target_os = "windows")]
    {
        for var in ["PROGRAMFILES(X86)", "PROGRAMFILES"] {
            if let Ok(program_files) = std::env::var(var) {
                roots.push(PathBuf::from(program_files).join("Steam"));
            }
        }
        roots.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(home) = crate::core::config::home_dir() {
            // Native installs. `.steam/steam` and `.steam/root` are symlinks
            // Steam maintains into the real data directory.
            for relative in [
                ".steam/steam",
                ".steam/root",
                ".local/share/Steam",
                // Debian/Ubuntu's steam package uses its own data directory.
                ".steam/debian-installation",
                ".var/app/com.valvesoftware.Steam/.local/share/Steam",
                ".var/app/com.valvesoftware.Steam/.steam/steam",
                "snap/steam/common/.local/share/Steam",
                "snap/steam/common/.steam/steam",
                "Library/Application Support/Steam",
            ] {
                roots.push(home.join(relative));
            }
        }
        if let Some(xdg) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
            roots.push(PathBuf::from(xdg).join("Steam"));
        }
    }

    roots.retain(|root| root.exists());
    roots.sort();
    roots.dedup();
    roots
}

fn is_app_manifest(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };

    name.starts_with("appmanifest_") && name.ends_with(".acf")
}

pub async fn parse_library_folders(path: PathBuf) -> Result<Vec<PathBuf>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let raw = fs::read_to_string(&path)
        .await
        .with_context(|| format!("failed reading {}", path.display()))?;

    let libraries = library_folder_paths(&raw);
    tracing::debug!(
        "libraryfolders.vdf {} lists {} librar{}",
        path.display(),
        libraries.len(),
        if libraries.len() == 1 { "y" } else { "ies" }
    );
    Ok(libraries)
}

pub fn library_folder_paths(raw: &str) -> Vec<PathBuf> {
    fn is_index(key: &str) -> bool {
        !key.is_empty() && key.chars().all(|ch| ch.is_ascii_digit())
    }

    let mut libraries: Vec<PathBuf> = Vec::new();
    let mut block_keys: Vec<String> = Vec::new();
    let mut pending_key: Option<String> = None;

    let mut push = |value: &str| {
        let unescaped = unescape_vdf(value);
        if !unescaped.is_empty() {
            libraries.push(PathBuf::from(unescaped));
        }
    };

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        if trimmed == "{" {
            block_keys.push(pending_key.take().unwrap_or_default());
            continue;
        }
        if trimmed == "}" {
            block_keys.pop();
            pending_key = None;
            continue;
        }

        let parts = extract_quoted_values(trimmed);
        match parts.as_slice() {
            [key] => pending_key = Some(key.clone()),
            [key, value, ..] => {
                let depth = block_keys.len();
                if depth == 1 && is_index(key) {
                    push(value);
                } else if depth == 2
                    && key.eq_ignore_ascii_case("path")
                    && block_keys.last().is_some_and(|k| is_index(k))
                {
                    push(value);
                }
            }
            [] => {}
        }
    }

    libraries.sort();
    libraries.dedup();
    libraries
}

fn unescape_vdf(value: &str) -> String {
    if !value.contains('\\') {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

pub fn probe_install_dir_by_appid(steamapps: &Path, appid: u32) -> Option<PathBuf> {
    let common = steamapps.join("common");
    let appid_str = appid.to_string();
    let entries = std::fs::read_dir(common).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(path.join("steam_appid.txt")) {
            if content.trim() == appid_str {
                return Some(path);
            }
        }
    }
    None
}

async fn parse_app_manifest_info(path: &Path) -> Result<Option<(u32, InstalledAppInfo)>> {
    let raw = fs::read_to_string(path)
        .await
        .with_context(|| format!("failed reading {}", path.display()))?;

    let manifest = crate::core::acf::parse_app_manifest(&raw);

    let (Some(id), Some(dir)) = (manifest.app_id, manifest.install_dir.clone()) else {
        return Ok(None);
    };

    if !manifest.fully_installed() {
        return Ok(None);
    }
    let steamapps = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut install_path = steamapps.join("common").join(&dir);
    if !install_path.is_dir() {
        if let Some(found) = probe_install_dir_by_appid(&steamapps, id) {
            tracing::debug!(
                "app {id}: installdir {dir:?} missing, using {} instead",
                found.display()
            );
            install_path = found;
        }
    }
    Ok(Some((
        id,
        InstalledAppInfo {
            install_path,
            active_branch: manifest.active_branch,
            name: manifest.name,
            last_owner: manifest.last_owner,
            from_windows_steam: false,
            manifest_path: path.to_path_buf(),
            last_updated: manifest.last_updated,
            build_id: manifest.build_id,
        },
    )))
}

pub fn build_game_library(
    owned: Vec<OwnedGame>,
    installed_info: HashMap<u32, InstalledAppInfo>,
    steam_id: Option<u64>,
) -> GameLibrary {
    let mut games = Vec::new();
    let mut owned_app_ids = std::collections::HashSet::new();

    for owned_game in owned {
        if is_ignored_steam_app(owned_game.app_id, &owned_game.name) {
            continue;
        }
        owned_app_ids.insert(owned_game.app_id);
        let info = installed_info.get(&owned_game.app_id);
        let install_path = info.map(|i| i.install_path.to_string_lossy().to_string());
        let from_windows_steam = info.is_some_and(|i| i.from_windows_steam);
        let active_branch = info
            .map(|i| i.active_branch.clone())
            .unwrap_or_else(|| "public".to_string());

        games.push(LibraryGame {
            app_id: owned_game.app_id,
            name: owned_game.name,
            playtime_forever_minutes: Some(owned_game.playtime_forever_minutes),
            is_installed: install_path.is_some(),
            install_path,
            local_manifest_ids: owned_game.local_manifest_ids,
            update_available: owned_game.update_available,
            update_queued: false,
            active_branch,
            is_owned: true,
            is_family_shared: false,
            online_required: None,
            platform: None,
            from_windows_steam,
        });
    }

    for (app_id, info) in installed_info {
        if owned_app_ids.contains(&app_id) {
            continue;
        }
        let candidate_name = info.name.as_deref().unwrap_or("");
        if is_ignored_steam_app(app_id, candidate_name) {
            continue;
        }

        let family_shared =
            matches!((info.last_owner, steam_id), (Some(owner), Some(me)) if owner != me);
        let from_windows_steam = info.from_windows_steam;

        games.push(LibraryGame {
            app_id,
            name: info.name.unwrap_or_else(|| format!("App {app_id}")),
            playtime_forever_minutes: None,
            is_installed: true,
            install_path: Some(info.install_path.to_string_lossy().to_string()),
            local_manifest_ids: HashMap::new(),
            update_available: false,
            update_queued: false,
            active_branch: info.active_branch,
            is_owned: false,
            is_family_shared: family_shared,
            online_required: None,
            platform: None,
            from_windows_steam,
        });
    }

    games.sort_by(|a, b| a.name.cmp(&b.name));
    GameLibrary { games }
}

#[cfg(test)]
#[path = "library_tests.rs"]
mod tests;
