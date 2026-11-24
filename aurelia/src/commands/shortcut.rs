use crate::commands::common::*;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Shortcut {
    pub app_id: u32,
    pub name: String,
    pub exe: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub start_dir: Option<String>,
    #[serde(default)]
    pub proton: Option<String>,
    #[serde(default)]
    pub steam_appid: Option<u32>,
}

impl Shortcut {
    pub(crate) fn dir(&self) -> PathBuf {
        match &self.start_dir {
            Some(d) if !d.is_empty() => PathBuf::from(d),
            _ => Path::new(&self.exe).parent().map(Path::to_path_buf).unwrap_or_default(),
        }
    }

    pub(crate) fn api_app_id(&self) -> Option<u32> {
        self.steam_appid.or_else(|| std::fs::read_to_string(self.dir().join("steam_appid.txt")).ok()?.trim().parse().ok())
    }

    pub(crate) fn compat_data(&self) -> Result<PathBuf> {
        Ok(aurelia::core::config::data_dir()?.join("shortcuts").join(self.app_id.to_string()))
    }
}

pub(crate) fn shortcut_id(exe: &str, name: &str) -> u32 {
    let mut h = crc32fast::Hasher::new();
    h.update(exe.as_bytes());
    h.update(name.as_bytes());
    h.finalize() | 0x8000_0000
}

fn store_path() -> Result<PathBuf> {
    Ok(aurelia::core::config::data_dir()?.join("shortcuts.json"))
}

pub(crate) fn load() -> Vec<Shortcut> {
    store_path().ok().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(list: &[Shortcut]) -> Result<()> {
    let path = store_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(list)?)?;
    std::fs::rename(&tmp, &path).with_context(|| format!("could not write {}", path.display()))
}

pub(crate) fn find(app_id: u32) -> Option<Shortcut> {
    if app_id & 0x8000_0000 == 0 {
        return None;
    }
    load().into_iter().find(|s| s.app_id == app_id)
}

pub(crate) fn cmd_shortcut_add(name: String, exe: PathBuf, args: Vec<String>, start_dir: Option<PathBuf>, proton: Option<String>, steam_appid: Option<u32>, json: bool) -> Result<()> {
    let exe = std::fs::canonicalize(&exe).with_context(|| format!("{} does not exist", exe.display()))?;
    if !exe.is_file() {
        bail!("{} is not a file", exe.display());
    }
    let exe_str = exe.to_string_lossy().into_owned();
    let app_id = shortcut_id(&exe_str, &name);
    let mut list = load();
    list.retain(|s| s.app_id != app_id);
    let shortcut = Shortcut {
        app_id,
        name,
        exe: exe_str,
        args,
        start_dir: start_dir.map(|d| d.to_string_lossy().into_owned()),
        proton,
        steam_appid,
    };
    list.push(shortcut.clone());
    save(&list)?;
    if json {
        print_json(&shortcut);
    } else {
        cli_println!("Shortcut {} added: {} ({})", shortcut.app_id, shortcut.name, shortcut.exe);
    }
    Ok(())
}

pub(crate) fn cmd_shortcut_list(json: bool) -> Result<()> {
    let list = load();
    if json {
        let items: Vec<serde_json::Value> = list
            .iter()
            .map(|s| {
                let mut v = serde_json::to_value(s).unwrap_or_default();
                if let (Some(o), Ok(dir)) = (v.as_object_mut(), s.compat_data()) {
                    o.insert("compat_data".into(), dir.to_string_lossy().into_owned().into());
                }
                v
            })
            .collect();
        print_json(&items);
        return Ok(());
    }
    if list.is_empty() {
        cli_println!("No shortcuts.");
    }
    for s in &list {
        cli_println!("{:>10}  {}  {}", s.app_id, s.name, s.exe);
    }
    Ok(())
}

pub(crate) fn cmd_shortcut_remove(app_id: u32, json: bool) -> Result<()> {
    let mut list = load();
    let before = list.len();
    list.retain(|s| s.app_id != app_id);
    if list.len() == before {
        bail!("no shortcut {app_id}");
    }
    save(&list)?;
    if json {
        print_json(&serde_json::json!({ "removed": app_id }));
    } else {
        cli_println!("Shortcut {app_id} removed (its Wine prefix stays on disk).");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_has_the_high_bit_and_is_stable() {
        let a = shortcut_id("/g/game.exe", "Game");
        assert!(a & 0x8000_0000 != 0);
        assert_eq!(a, shortcut_id("/g/game.exe", "Game"));
        assert_ne!(a, shortcut_id("/g/game.exe", "Other"));
    }
}
