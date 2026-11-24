use crate::core::config::config_dir;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunningGame {
    pub app_id: u32,
    pub name: String,
    pub pid: u32,
    #[serde(default)]
    pub pid_start_time: Option<u64>,
    #[serde(default)]
    pub wineprefix: Option<PathBuf>,
}

impl RunningGame {
    pub fn new(app_id: u32, name: String, pid: u32, wineprefix: Option<PathBuf>) -> Self {
        Self {
            app_id,
            name,
            pid,
            pid_start_time: process_start_time(pid),
            wineprefix,
        }
    }

    pub fn set_pid(&mut self, pid: u32) {
        self.pid = pid;
        self.pid_start_time = process_start_time(pid);
    }

    pub fn owns_pid(&self) -> bool {
        self.pid_start_time
            .is_some_and(|t| process_start_time(self.pid) == Some(t))
    }
}

pub fn process_start_time(pid: u32) -> Option<u64> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let pid = Pid::from_u32(pid);
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing(),
    );
    sys.process(pid).map(|p| p.start_time())
}

fn running_dir() -> Result<PathBuf> {
    Ok(config_dir()?.join("running"))
}

fn record_path(app_id: u32) -> Result<PathBuf> {
    Ok(running_dir()?.join(format!("{app_id}.json")))
}

pub fn record_launch(game: &RunningGame) -> Result<()> {
    let dir = running_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("failed creating {}", dir.display()))?;
    let path = record_path(game.app_id)?;
    let body = serde_json::to_string_pretty(game)?;
    crate::core::atomic::write_atomic(&path, body.as_bytes())
}

pub fn load(app_id: u32) -> Option<RunningGame> {
    let path = record_path(app_id).ok()?;
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn clear(app_id: u32) {
    if let Ok(path) = record_path(app_id) {
        let _ = std::fs::remove_file(path);
    }
}

pub fn list() -> Vec<RunningGame> {
    let Ok(dir) = running_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut games: Vec<RunningGame> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .filter_map(|raw| serde_json::from_str(&raw).ok())
        .collect();
    games.sort_by(|a, b| a.name.cmp(&b.name));
    games
}

pub fn processes_for_app(app_id: u32) -> Vec<u32> {
    #[cfg(unix)]
    {
        let compat = format!("STEAM_COMPAT_APP_ID={app_id}");
        let native = format!("SteamAppId={app_id}");
        let mut pids = Vec::new();
        let Ok(proc_dir) = std::fs::read_dir("/proc") else {
            return pids;
        };
        for entry in proc_dir.flatten() {
            let path = entry.path();
            let Some(pid) = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.parse::<u32>().ok())
            else {
                continue;
            };
            let Ok(environ) = std::fs::read(path.join("environ")) else {
                continue;
            };
            if environ
                .split(|&b| b == 0)
                .any(|e| e == compat.as_bytes() || e == native.as_bytes())
            {
                pids.push(pid);
            }
        }
        pids
    }
    #[cfg(not(unix))]
    {
        let _ = app_id;
        Vec::new()
    }
}

pub fn list_active() -> Vec<RunningGame> {
    list().into_iter().filter_map(reconcile).collect()
}

pub fn reconcile(mut game: RunningGame) -> Option<RunningGame> {
    if game.owns_pid() {
        return Some(game);
    }
    match processes_for_app(game.app_id).first() {
        Some(&pid) => {
            game.set_pid(pid);
            let _ = record_launch(&game);
            Some(game)
        }
        None => {
            clear(game.app_id);
            None
        }
    }
}

#[cfg(test)]
#[path = "running_tests.rs"]
mod tests;
