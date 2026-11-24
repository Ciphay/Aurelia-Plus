use std::io;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

pub fn dir_size(path: &Path) -> u64 {
    WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

pub fn move_dir_with_progress(
    src: &Path,
    dst: &Path,
    total_bytes: u64,
    mut on_progress: impl FnMut(u64, &str),
) -> io::Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if std::fs::rename(src, dst).is_ok() {
        on_progress(total_bytes, "");
        return Ok(());
    }

    copy_dir(src, dst, &mut on_progress)?;
    std::fs::remove_dir_all(src)?;
    Ok(())
}

fn copy_dir(src: &Path, dst: &Path, on_progress: &mut impl FnMut(u64, &str)) -> io::Result<()> {
    let mut copied: u64 = 0;
    std::fs::create_dir_all(dst)?;

    let mut buf = vec![0u8; 4 * 1024 * 1024];

    for entry in WalkDir::new(src).into_iter().filter_map(|e| e.ok()) {
        let rel = entry.path().strip_prefix(src).map_err(io::Error::other)?;
        let target = dst.join(rel);

        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            copy_file_chunked(entry.path(), &target, &mut copied, &mut buf, on_progress)?;
        }
    }
    Ok(())
}

fn copy_file_chunked(
    src: &Path,
    dst: &Path,
    copied: &mut u64,
    buf: &mut [u8],
    on_progress: &mut impl FnMut(u64, &str),
) -> io::Result<()> {
    use io::{Read, Write};

    let mut reader = std::fs::File::open(src)?;
    let mut writer = std::fs::File::create(dst)?;
    let name = src
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();

    loop {
        let n = reader.read(buf)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
        *copied += n as u64;
        on_progress(*copied, &name);
    }
    writer.flush()?;
    Ok(())
}

pub fn find_libraryfolders_vdf(roots: &[PathBuf]) -> Option<PathBuf> {
    roots
        .iter()
        .map(|root| root.join("steamapps").join("libraryfolders.vdf"))
        .find(|candidate| candidate.exists())
}

fn normalize_path(p: &str) -> String {
    let unified = p.replace("\\\\", "\\").replace('\\', "/");
    let trimmed = unified.trim_end_matches('/');
    if cfg!(windows) {
        trimmed.to_lowercase()
    } else {
        trimmed.to_string()
    }
}

pub fn update_libraryfolders_apps(
    vdf: &str,
    appid: u32,
    from: &Path,
    to: &Path,
    size: u64,
) -> Option<String> {
    let from_norm = normalize_path(&from.to_string_lossy());
    let to_norm = normalize_path(&to.to_string_lossy());
    let appid_key = format!("\"{appid}\"");

    let newline = if vdf.contains("\r\n") { "\r\n" } else { "\n" };

    let mut out: Vec<String> = Vec::new();
    let mut current_path: Option<String> = None;
    let mut apps_pending = false;
    let mut in_apps_of: Option<Folder> = None;
    let mut found_from = false;
    let mut found_to = false;

    for raw_line in vdf.split_inclusive('\n') {
        let line = raw_line.trim_end_matches(['\r', '\n']);
        let trimmed = line.trim();

        if in_apps_of.is_none() {
            if let Some(path_val) = parse_kv(trimmed, "path") {
                current_path = Some(normalize_path(&path_val));
            }
        }

        if trimmed.eq_ignore_ascii_case("\"apps\"") {
            apps_pending = true;
            out.push(line.to_string());
            continue;
        }

        if apps_pending && trimmed == "{" {
            apps_pending = false;
            in_apps_of = match current_path.as_deref() {
                Some(p) if p == to_norm => Some(Folder::To),
                Some(p) if p == from_norm => Some(Folder::From),
                _ => None,
            };
            out.push(line.to_string());

            if matches!(in_apps_of, Some(Folder::To)) {
                let indent = leading_ws(line);
                out.push(format!("{indent}\t{appid_key}\t\t\"{size}\""));
                found_to = true;
            }
            continue;
        }

        if matches!(in_apps_of, Some(Folder::From)) && trimmed.starts_with(&appid_key) {
            found_from = true;
            continue;
        }

        if in_apps_of.is_some() && trimmed == "}" {
            in_apps_of = None;
            current_path = None;
            out.push(line.to_string());
            continue;
        }

        out.push(line.to_string());
    }

    if !found_to {
        return None;
    }
    let _ = found_from;

    let mut result = out.join(newline);
    if vdf.ends_with('\n') {
        result.push_str(newline);
    }
    Some(result)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Folder {
    From,
    To,
}

fn leading_ws(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

fn parse_kv(line: &str, key: &str) -> Option<String> {
    let mut parts = line.split('"').filter(|s| !s.trim().is_empty());
    let k = parts.next()?;
    if !k.eq_ignore_ascii_case(key) {
        return None;
    }
    parts.next().map(|v| v.to_string())
}

#[cfg(test)]
#[path = "relocate_tests.rs"]
mod tests;
