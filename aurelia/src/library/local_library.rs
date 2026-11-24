use crate::core::config::detect_steam_path;
use crate::core::models::OwnedGame;
use crate::core::utils::extract_quoted_values;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const STEAMID64_BASE: u64 = 76_561_197_960_265_728;

pub fn steam_install_root() -> Option<PathBuf> {
    detect_steam_path().filter(|p| p.join("appcache").is_dir())
}

pub async fn discover_local_owned_games() -> Vec<OwnedGame> {
    match steam_install_root() {
        Some(root) => discover_from_root(&root).await,
        None => Vec::new(),
    }
}

pub async fn discover_from_root(root: &Path) -> Vec<OwnedGame> {
    let playtime = read_local_playtime(root).await;

    let mut candidates: HashSet<u32> = playtime.keys().copied().collect();
    candidates.extend(read_librarycache_appids(root).await);

    if candidates.is_empty() {
        return Vec::new();
    }

    let appinfo_path = root.join("appcache").join("appinfo.vdf");
    let meta = match tokio::fs::read(&appinfo_path).await {
        Ok(bytes) => parse_appinfo(&bytes),
        Err(_) => HashMap::new(),
    };

    let mut games = Vec::new();
    for app_id in candidates {
        let Some(info) = meta
            .get(&app_id)
            .filter(|i| i.app_type.eq_ignore_ascii_case("game"))
        else {
            continue;
        };
        games.push(OwnedGame {
            app_id,
            name: info.name.clone(),
            playtime_forever_minutes: playtime.get(&app_id).copied().unwrap_or(0),
            local_manifest_ids: HashMap::new(),
            update_available: false,
        });
    }

    games.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
    });
    games
}

async fn read_local_playtime(root: &Path) -> HashMap<u32, u32> {
    let Some(config_path) = locate_localconfig(root).await else {
        return HashMap::new();
    };
    match tokio::fs::read_to_string(&config_path).await {
        Ok(text) => parse_localconfig_apps(&text),
        Err(_) => HashMap::new(),
    }
}

async fn locate_localconfig(root: &Path) -> Option<PathBuf> {
    let userdata = root.join("userdata");

    if let Some(id3) = most_recent_account_id(root).await {
        let candidate = userdata
            .join(id3.to_string())
            .join("config/localconfig.vdf");
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    let mut dir = tokio::fs::read_dir(&userdata).await.ok()?;
    while let Ok(Some(entry)) = dir.next_entry().await {
        let candidate = entry.path().join("config/localconfig.vdf");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

async fn most_recent_account_id(root: &Path) -> Option<u32> {
    let path = root.join("config/loginusers.vdf");
    let text = tokio::fs::read_to_string(&path).await.ok()?;

    let mut current_id64: Option<u64> = None;
    let mut fallback: Option<u64> = None;
    for line in text.lines() {
        let parts = extract_quoted_values(line.trim());
        match parts.as_slice() {
            [id] if id.chars().all(|c| c.is_ascii_digit()) && id.len() >= 17 => {
                current_id64 = id.parse::<u64>().ok();
                if fallback.is_none() {
                    fallback = current_id64;
                }
            }
            [key, value] if key.eq_ignore_ascii_case("MostRecent") && value == "1" => {
                if let Some(id64) = current_id64 {
                    return account_id_from_id64(id64);
                }
            }
            _ => {}
        }
    }
    fallback.and_then(account_id_from_id64)
}

fn account_id_from_id64(id64: u64) -> Option<u32> {
    id64.checked_sub(STEAMID64_BASE).map(|v| v as u32)
}

async fn read_librarycache_appids(root: &Path) -> HashSet<u32> {
    let mut ids = HashSet::new();
    let cache_dir = root.join("appcache").join("librarycache");
    if let Ok(mut dir) = tokio::fs::read_dir(&cache_dir).await {
        while let Ok(Some(entry)) = dir.next_entry().await {
            if let Some(id) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            {
                ids.insert(id);
            }
        }
    }
    ids
}

fn parse_localconfig_apps(text: &str) -> HashMap<u32, u32> {
    let mut result = HashMap::new();
    let mut path: Vec<String> = Vec::new();
    let mut pending_key: Option<String> = None;

    for raw in text.lines() {
        let line = raw.trim();
        if line == "{" {
            let key = pending_key.take().unwrap_or_default();
            if path.last().is_some_and(|p| p.eq_ignore_ascii_case("apps")) {
                if let Ok(id) = key.parse::<u32>() {
                    result.entry(id).or_insert(0);
                }
            }
            path.push(key);
            continue;
        }
        if line == "}" {
            path.pop();
            pending_key = None;
            continue;
        }

        let tokens = extract_quoted_values(line);
        match tokens.as_slice() {
            [key] => pending_key = Some(key.clone()),
            [key, value] => {
                pending_key = None;
                if key.eq_ignore_ascii_case("Playtime")
                    && path.len() >= 2
                    && path[path.len() - 2].eq_ignore_ascii_case("apps")
                {
                    if let (Ok(app_id), Ok(minutes)) =
                        (path[path.len() - 1].parse::<u32>(), value.parse::<u32>())
                    {
                        result.insert(app_id, minutes);
                    }
                }
            }
            _ => {}
        }
    }
    result
}

struct AppMeta {
    name: String,
    app_type: String,
}

fn parse_appinfo(bytes: &[u8]) -> HashMap<u32, AppMeta> {
    let mut out = HashMap::new();
    let mut pos = 0usize;

    let Some(magic) = read_u32(bytes, &mut pos) else {
        return out;
    };
    if read_u32(bytes, &mut pos).is_none() {
        return out;
    }

    let (has_vdf_sha, string_table) = match magic {
        0x07564427 => (false, None),
        0x07564428 => (true, None),
        0x07564429 => {
            let Some(offset) = read_i64(bytes, &mut pos) else {
                return out;
            };
            (true, parse_string_table(bytes, offset as usize))
        }
        _ => return out,
    };

    let header_tail = 4 + 4 + 8 + 20 + 4 + if has_vdf_sha { 20 } else { 0 };

    loop {
        let Some(app_id) = read_u32(bytes, &mut pos) else {
            break;
        };
        if app_id == 0 {
            break;
        }
        let Some(size) = read_u32(bytes, &mut pos) else {
            break;
        };
        let size = size as usize;
        let record_start = pos;
        let record_end = record_start.saturating_add(size).min(bytes.len());
        let kv_start = record_start.saturating_add(header_tail);

        if kv_start <= record_end {
            if let Some(meta) =
                parse_app_common(&bytes[kv_start..record_end], string_table.as_deref())
            {
                out.insert(app_id, meta);
            }
        }

        pos = record_end;
    }

    out
}

fn parse_string_table(bytes: &[u8], offset: usize) -> Option<Vec<String>> {
    let mut pos = offset;
    let count = read_u32(bytes, &mut pos)? as usize;
    let mut table = Vec::with_capacity(count.min(1 << 20));
    for _ in 0..count {
        table.push(read_cstr(bytes, &mut pos)?);
    }
    Some(table)
}

fn parse_app_common(body: &[u8], strings: Option<&[String]>) -> Option<AppMeta> {
    let mut meta = AppMeta {
        name: String::new(),
        app_type: String::new(),
    };
    let mut pos = 0usize;
    walk_object(body, &mut pos, strings, false, &mut meta);
    if meta.name.is_empty() {
        return None;
    }
    Some(meta)
}

fn walk_object(
    buf: &[u8],
    pos: &mut usize,
    strings: Option<&[String]>,
    in_common: bool,
    meta: &mut AppMeta,
) {
    while *pos < buf.len() {
        let type_byte = buf[*pos];
        *pos += 1;
        match type_byte {
            0x08 => return,
            0x00 => {
                let Some(key) = read_key(buf, pos, strings) else {
                    return;
                };
                let nested_common = key.eq_ignore_ascii_case("common");
                walk_object(buf, pos, strings, nested_common, meta);
            }
            0x01 => {
                let Some(key) = read_key(buf, pos, strings) else {
                    return;
                };
                let Some(value) = read_cstr(buf, pos) else {
                    return;
                };
                if in_common {
                    if key.eq_ignore_ascii_case("name") {
                        meta.name = value;
                    } else if key.eq_ignore_ascii_case("type") {
                        meta.app_type = value;
                    }
                }
            }
            0x02 | 0x03 => {
                if !skip_keyed_value(buf, pos, strings, 4) {
                    return;
                }
            }
            0x07 | 0x0b => {
                if !skip_keyed_value(buf, pos, strings, 8) {
                    return;
                }
            }
            _ => return,
        }
    }
}

fn skip_keyed_value(
    buf: &[u8],
    pos: &mut usize,
    strings: Option<&[String]>,
    value_len: usize,
) -> bool {
    if read_key(buf, pos, strings).is_none() {
        return false;
    }
    *pos += value_len;
    true
}

fn read_key(buf: &[u8], pos: &mut usize, strings: Option<&[String]>) -> Option<String> {
    match strings {
        Some(table) => {
            let idx = read_u32(buf, pos)? as usize;
            table.get(idx).cloned()
        }
        None => read_cstr(buf, pos),
    }
}

fn read_u32(buf: &[u8], pos: &mut usize) -> Option<u32> {
    let end = pos.checked_add(4)?;
    let slice = buf.get(*pos..end)?;
    *pos = end;
    Some(u32::from_le_bytes(slice.try_into().ok()?))
}

fn read_i64(buf: &[u8], pos: &mut usize) -> Option<i64> {
    let end = pos.checked_add(8)?;
    let slice = buf.get(*pos..end)?;
    *pos = end;
    Some(i64::from_le_bytes(slice.try_into().ok()?))
}

fn read_cstr(buf: &[u8], pos: &mut usize) -> Option<String> {
    let start = *pos;
    let rel = buf.get(start..)?.iter().position(|&b| b == 0)?;
    let s = String::from_utf8_lossy(&buf[start..start + rel]).into_owned();
    *pos = start + rel + 1;
    Some(s)
}

#[cfg(test)]
#[path = "local_library_tests.rs"]
mod tests;
