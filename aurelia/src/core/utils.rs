use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn extract_quoted_values(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_quote = false;
    let mut current = String::new();
    for ch in line.chars() {
        if ch == '"' {
            if in_quote {
                out.push(std::mem::take(&mut current));
            }
            in_quote = !in_quote;
            continue;
        }
        if in_quote {
            current.push(ch);
        }
    }
    out
}

pub fn ensure_steam_appid(dir: &Path, app_id: u32) -> std::io::Result<bool> {
    let path = dir.join("steam_appid.txt");
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(file) => {
            write_appid_or_remove(&path, file, app_id)?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}

fn write_appid_or_remove(
    path: &Path,
    mut file: impl std::io::Write,
    app_id: u32,
) -> std::io::Result<()> {
    let result = file
        .write_all(app_id.to_string().as_bytes())
        .and_then(|()| file.flush());
    drop(file);
    if let Err(e) = result {
        let _ = std::fs::remove_file(path);
        return Err(e);
    }
    Ok(())
}

pub fn ensure_steam_appid_logged(dir: &Path, app_id: u32) {
    match ensure_steam_appid(dir, app_id) {
        Ok(true) => tracing::debug!("wrote steam_appid.txt in {}", dir.display()),
        Ok(false) => tracing::debug!("keeping existing steam_appid.txt in {}", dir.display()),
        Err(e) => tracing::warn!("could not write steam_appid.txt in {}: {e}", dir.display()),
    }
}

pub fn is_safe_dir_name(name: &str) -> bool {
    let mut parts = Path::new(name).components();
    !name.trim().is_empty()
        && !name.contains(['/', '\\'])
        && matches!(
            (parts.next(), parts.next()),
            (Some(std::path::Component::Normal(_)), None)
        )
}

/// Relative path with no escaping parts.
pub fn safe_relative_path(path: &str) -> Option<PathBuf> {
    use std::path::Component;
    if path.contains('\0') {
        return None;
    }
    let mut out = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

pub fn resolves_within(root: &Path, path: &Path) -> bool {
    let mut probe = path;
    loop {
        if std::fs::symlink_metadata(probe).is_ok() {
            return probe.canonicalize().is_ok_and(|p| p.starts_with(root));
        }
        match probe.parent() {
            Some(parent) => probe = parent,
            None => return false,
        }
    }
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn build_runner_command(runner_path: &Path) -> Result<Command> {
    let mut final_path = runner_path.to_path_buf();

    if final_path.is_dir() {
        if final_path.join("proton").exists() {
            final_path.push("proton");
        } else if final_path.join("bin/wine").exists() {
            final_path.push("bin/wine");
        } else if final_path.join("bin/wine64").exists() {
            final_path.push("bin/wine64");
        }
    }

    if let Some(file_name) = final_path.file_name().and_then(|f| f.to_str()) {
        if file_name == "proton" {
            let mut cmd = Command::new(&final_path);
            cmd.arg("run");
            return Ok(cmd);
        }
        if file_name == "wine" || file_name == "wine64" {
            return Ok(Command::new(&final_path));
        }
    }

    if final_path.exists() && final_path.is_file() {
        return Ok(Command::new(&final_path));
    }

    bail!(
        "Failed to resolve a valid runner binary from {}",
        runner_path.display()
    )
}

fn normalize_runner_name(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

pub fn resolve_runner(name: &str, library_root: &Path) -> PathBuf {
    resolve_runner_opt(name, library_root).unwrap_or_else(|| {
        tracing::warn!(
            runner = %name,
            "could not resolve runner '{}' to an on-disk runtime under any known search path; \
             returning the name verbatim as a path (the launch will likely fail resolving the binary)",
            name
        );
        Path::new(name).to_path_buf()
    })
}

pub fn resolve_runner_opt(name: &str, library_root: &Path) -> Option<PathBuf> {
    resolve_runner_opt_in(name, library_root, &crate::library::steam_root_candidates())
}

pub fn runner_library_roots(configured: &Path, steam_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut libraries = vec![configured.to_path_buf()];
    libraries.extend(steam_roots.iter().cloned());
    let mut roots = vec![configured.to_path_buf()];
    roots.extend(steam_roots.iter().cloned());
    for root in &roots {
        for vdf in [
            root.join("steamapps").join("libraryfolders.vdf"),
            root.join("config").join("libraryfolders.vdf"),
        ] {
            if let Ok(raw) = std::fs::read_to_string(&vdf) {
                libraries.extend(crate::library::library_folder_paths(&raw));
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    libraries.retain(|p| seen.insert(std::fs::canonicalize(p).unwrap_or_else(|_| p.clone())));
    libraries
}

pub fn resolve_runner_opt_in(
    name: &str,
    library_root: &Path,
    steam_roots: &[PathBuf],
) -> Option<PathBuf> {
    let name_path = Path::new(name);
    if name_path.is_absolute() || name_path.exists() {
        return Some(name_path.to_path_buf());
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let libraries = runner_library_roots(library_root, steam_roots);
    let mut search_dirs: Vec<PathBuf> = vec![
        library_root.join("steamapps/common"),
        library_root.join("compatibilitytools.d"),
    ];
    for lib in libraries.iter().skip(1) {
        search_dirs.push(lib.join("steamapps/common"));
    }
    for root in steam_roots {
        search_dirs.push(root.join("compatibilitytools.d"));
    }
    if let Ok(compat) = crate::compat::proton::compat_tools_dir() {
        search_dirs.push(compat);
    }
    search_dirs.push(PathBuf::from(&home).join(".local/share/lutris/runners/wine"));

    for dir in &search_dirs {
        let exact = dir.join(name);
        if exact.exists() {
            return Some(exact);
        }
    }

    let needle = normalize_runner_name(name);
    if !needle.is_empty() {
        let mut matches: Vec<PathBuf> = Vec::new();
        for dir in &search_dirs {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    if !entry.path().is_dir() {
                        continue;
                    }
                    let cand = normalize_runner_name(&entry.file_name().to_string_lossy());
                    if !cand.is_empty()
                        && (cand == needle || cand.contains(&needle) || needle.contains(&cand))
                    {
                        matches.push(entry.path());
                    }
                }
            }
        }
        matches.sort_by_key(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().len())
                .unwrap_or(usize::MAX)
        });
        if let Some(exact) = matches.iter().find(|p| {
            normalize_runner_name(&p.file_name().unwrap_or_default().to_string_lossy()) == needle
        }) {
            return Some(exact.clone());
        }
        if let Some(first) = matches.into_iter().next() {
            return Some(first);
        }
    }

    None
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RunnerKind {
    Proton,
    WineTkg,
    PlainWine,
    Unknown,
}

pub(crate) fn classify_runner(runner_dir: &Path) -> RunnerKind {
    if runner_dir.join("proton").exists()
        || runner_dir.join("files/bin/wine").exists()
        || runner_dir.join("files/bin/wine64").exists()
    {
        return RunnerKind::Proton;
    }

    if runner_dir.join("bin/wine").exists() || runner_dir.join("bin/wine64").exists() {
        let name = runner_dir
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if name.contains("tkg") {
            return RunnerKind::WineTkg;
        }
        return RunnerKind::PlainWine;
    }

    RunnerKind::Unknown
}

pub(crate) fn proton_bundled_bare_wine(runner_dir: &Path) -> Option<PathBuf> {
    let root = derive_runner_root(runner_dir);
    for rel in [
        "files/bin/wine64",
        "files/bin/wine",
        "dist/bin/wine64",
        "dist/bin/wine",
    ] {
        let p = root.join(rel);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

pub(crate) fn validate_steam_runtime_runner_path(path: &Path) -> Result<()> {
    if !path.exists() {
        bail!(
            "Steam-runtime runner path does not exist: {}",
            path.display()
        );
    }

    if path.is_file() {
        match path.file_name().and_then(|f| f.to_str()) {
            Some("wine") | Some("wine64") => return Ok(()),
            Some("proton") => bail!(
                "Steam-runtime runner points at a `proton` launch script ({}); background \
                 Steam requires a bare wine binary, not a `proton run` wrapper",
                path.display()
            ),
            _ => bail!(
                "Steam-runtime runner binary is not a wine/wine64 executable: {}",
                path.display()
            ),
        }
    }

    match classify_runner(path) {
        RunnerKind::WineTkg | RunnerKind::PlainWine => Ok(()),
        RunnerKind::Proton => bail!(
            "Steam-runtime runner resolves to a Proton tree ({}); background Steam must run \
             under a bare wine, not the `proton run` protonfixes wrapper. Point \
             `steam_runtime_runner` at a wine-tkg or plain-Wine build",
            path.display()
        ),
        RunnerKind::Unknown => bail!(
            "Steam-runtime runner path is not a usable wine runner (no bin/wine[64]): {}",
            path.display()
        ),
    }
}

pub const STEAM_RUNTIME_SUPPORTED: bool = cfg!(target_os = "linux");

pub fn steam_runtime_unsupported_msg() -> String {
    let alternative = if cfg!(target_os = "windows") {
        "On Windows Aurelia uses your installed Steam client instead: install Steam and \
         run games with `aurelia play <APP_ID> --steam`."
    } else {
        "Run Aurelia on Linux to use it."
    };
    format!(
        "the self-contained Windows Steam runtime is only supported on Linux: it runs \
         Steam under a Proton/Wine runner, whose binaries are Linux executables and \
         cannot be started on {}. {alternative}",
        std::env::consts::OS
    )
}

pub const PROTON_SUPPORTED: bool = cfg!(target_os = "linux");

pub fn linux_only_msg(what: &str) -> String {
    let os = std::env::consts::OS;
    let instead = if cfg!(target_os = "windows") {
        " On Windows Aurelia runs Windows games natively, without Proton/Wine."
    } else {
        ""
    };
    format!("{what} only runs on Linux, not on {os}.{instead}")
}

pub fn ensure_steam_runtime_supported() -> Result<()> {
    if STEAM_RUNTIME_SUPPORTED {
        return Ok(());
    }
    bail!("{}", steam_runtime_unsupported_msg())
}

pub fn steam_runtime_runner_unset_msg(action: &str) -> String {
    format!(
        "no Steam Runtime Runner is configured — required for {action} the Windows Steam \
         runtime.\n  1. See installed runtimes: `aurelia proton list`\n  2. Select one: \
         `aurelia config steam-runtime-runner <NAME>` (e.g. GE-Proton9-20 or experimental)\n\
         A Proton runtime works — its bundled bare Wine is used automatically."
    )
}

pub fn resolve_steam_runtime_wine(runner_name: &str, library_root: &Path) -> Result<PathBuf> {
    if runner_name.trim().is_empty() {
        bail!("No Steam Runtime Runner selected in Global Settings");
    }

    let resolved = resolve_runner_opt(runner_name, library_root).ok_or_else(|| {
        anyhow!(
            "Steam Runtime Runner `{runner_name}` could not be found on disk. \
             Install it (`aurelia proton install {runner_name}`), or point \
             `steam_runtime_runner` at a wine build. See `aurelia proton list`."
        )
    })?;

    let is_proton_script =
        resolved.is_file() && resolved.file_name().and_then(|f| f.to_str()) == Some("proton");
    let is_proton_tree = resolved.is_dir() && classify_runner(&resolved) == RunnerKind::Proton;
    if is_proton_script || is_proton_tree {
        return proton_bundled_bare_wine(&resolved).ok_or_else(|| {
            anyhow!(
                "Steam Runtime Runner `{runner_name}` is a Proton tree with no bundled wine \
                 binary (looked for files/bin/wine64 and dist/bin/wine64 under {})",
                derive_runner_root(&resolved).display()
            )
        });
    }

    validate_steam_runtime_runner_path(&resolved)?;
    if resolved.is_file() {
        return Ok(resolved);
    }
    for rel in ["bin/wine64", "bin/wine"] {
        let candidate = resolved.join(rel);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    bail!(
        "Steam Runtime Runner `{runner_name}` has no wine binary under {}",
        resolved.display()
    )
}

pub fn ensure_steam_runtime_prefix_libs(bare_wine: &Path, wine_prefix: &Path) {
    let files_root = match bare_wine.parent().and_then(|p| p.parent()) {
        Some(root) => root.to_path_buf(),
        None => return,
    };

    let sys32 = wine_prefix.join("drive_c/windows/system32");
    let syswow = wine_prefix.join("drive_c/windows/syswow64");
    let groups: [(&[&str], &Path); 4] = [
        (&["lib64/wine/dxvk", "lib/wine/dxvk/x86_64-windows"], &sys32),
        (&["lib64/vkd3d", "lib/vkd3d/x86_64-windows"], &sys32),
        (&["lib/wine/dxvk", "lib/wine/dxvk/i386-windows"], &syswow),
        (&["lib/vkd3d", "lib/vkd3d/i386-windows"], &syswow),
    ];

    for (candidates, dest) in groups {
        if !dest.is_dir() {
            continue;
        }
        for rel in candidates {
            let src = files_root.join(rel);
            let Ok(entries) = std::fs::read_dir(&src) else {
                continue;
            };
            let mut copied = 0usize;
            for entry in entries.flatten() {
                let path = entry.path();
                let is_dll = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("dll"));
                if !is_dll {
                    continue;
                }
                if let Some(name) = path.file_name() {
                    match std::fs::copy(&path, dest.join(name)) {
                        Ok(_) => copied += 1,
                        Err(e) => tracing::warn!(
                            "could not copy {} into {}: {e}",
                            path.display(),
                            dest.display()
                        ),
                    }
                }
            }
            if copied > 0 {
                tracing::info!(
                    "Steam-runtime prefix libs: copied {copied} DLL(s) from {} to {}",
                    src.display(),
                    dest.display()
                );
                break;
            }
        }
    }
}

pub fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<()> {
    std::fs::create_dir_all(&dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
        } else {
            std::fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
        }
    }
    Ok(())
}

pub fn setup_fake_steam_trap(config_dir: &Path) -> Result<PathBuf> {
    let trap_dir = config_dir.join("fake_env");
    std::fs::create_dir_all(&trap_dir)?;

    let dummy_script = "#!/bin/sh\nexit 0\n";

    let write_executable = |path: &Path| -> Result<()> {
        if path.exists() {
            return Ok(());
        }
        std::fs::write(path, dummy_script)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(path)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(path, perms)?;
        }
        Ok(())
    };

    write_executable(&trap_dir.join("steam"))?;
    write_executable(&trap_dir.join("steam.sh"))?;

    Ok(trap_dir)
}

pub fn host_steam_client_path() -> Option<PathBuf> {
    let root = crate::core::config::detect_steam_path()?;
    #[cfg(not(target_os = "windows"))]
    {
        let path_env = std::env::var_os("PATH");
        let flatpak_dirs = steam_flatpak_app_dirs();
        if !steam_client_installed(&root, path_env.as_deref(), &flatpak_dirs) {
            tracing::debug!(
                "no Steam client found for {}; ignoring host Steam",
                root.display()
            );
            return None;
        }
    }
    Some(root)
}

pub fn steam_flatpak_app_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from(
        "/var/lib/flatpak/app/com.valvesoftware.Steam",
    )];
    if let Ok(home) = crate::core::config::home_dir() {
        dirs.push(home.join(".local/share/flatpak/app/com.valvesoftware.Steam"));
    }
    dirs
}

pub fn find_on_path(name: &str, path_env: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let path_env = path_env?;
    std::env::split_paths(path_env)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable_file(candidate))
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

pub fn steam_client_installed(
    root: &Path,
    path_env: Option<&std::ffi::OsStr>,
    flatpak_dirs: &[PathBuf],
) -> bool {
    ["ubuntu12_32/steam", "steam.sh", "steam.exe"]
        .iter()
        .any(|rel| root.join(rel).is_file())
        || find_on_path("steam", path_env).is_some()
        || flatpak_dirs.iter().any(|d| d.is_dir())
}

#[cfg(target_os = "linux")]
pub fn is_steam_running() -> bool {
    let pid_file = std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".steam/steam.pid"));
    steam_running_in(Path::new("/proc"), pid_file.as_deref())
}

#[cfg(target_os = "linux")]
fn steam_running_in(proc_root: &Path, pid_file: Option<&Path>) -> bool {
    if let Some(pid) = pid_file
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|pid| pid.trim().parse::<u32>().ok())
        && proc_entry_is_live(&proc_root.join(pid.to_string()))
        && proc_is_steam(&proc_root.join(pid.to_string()))
    {
        return true;
    }
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str() else { continue };
        if !pid.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if let Ok(comm) = std::fs::read_to_string(entry.path().join("comm")) {
            let comm = comm.trim();
            if (comm == "steam" || comm == "steamwebhelper") && proc_entry_is_live(&entry.path()) {
                return true;
            }
        }
    }
    false
}

#[cfg(target_os = "linux")]
fn proc_is_steam(proc_dir: &Path) -> bool {
    let comm = std::fs::read_to_string(proc_dir.join("comm")).unwrap_or_default();
    let comm = comm.trim();
    if comm == "steam" || comm == "steamwebhelper" || comm.starts_with("steam") {
        return true;
    }
    std::fs::read(proc_dir.join("cmdline"))
        .map(|raw| String::from_utf8_lossy(&raw).to_ascii_lowercase().contains("/steam"))
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn proc_entry_is_live(proc_dir: &Path) -> bool {
    if !proc_dir.exists() {
        return false;
    }
    let Ok(status) = std::fs::read_to_string(proc_dir.join("status")) else {
        return true;
    };
    !status
        .lines()
        .find_map(|l| l.strip_prefix("State:"))
        .is_some_and(|state| state.trim_start().starts_with('Z'))
}

#[cfg(not(target_os = "linux"))]
pub fn is_steam_running() -> bool {
    false
}

#[cfg(target_os = "linux")]
pub fn ensure_steam_running() {
    if is_steam_running() {
        tracing::info!("Steam client already running; not starting it");
        return;
    }
    tracing::info!("Steam client not running; starting `steam -silent` (tray)");
    match start_steam_detached(&["-silent"]) {
        Ok(()) => tracing::info!("Launched Steam client (-silent)"),
        Err(e) => tracing::warn!("could not start Steam client: {e:#}"),
    }
}

pub fn wait_until(
    mut done: impl FnMut() -> bool,
    timeout: std::time::Duration,
    interval: std::time::Duration,
) -> bool {
    let start = std::time::Instant::now();
    loop {
        if done() {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(interval);
    }
}

pub const STEAM_FLATPAK_APP_ID: &str = "com.valvesoftware.Steam";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamLauncher {
    pub program: PathBuf,
    pub prefix_args: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteamRootKind {
    Native,
    Flatpak,
    Snap,
}

pub fn steam_root_kind(root: &Path) -> SteamRootKind {
    let check = |p: &Path| {
        let text = p.to_string_lossy();
        if text.contains(&format!("/.var/app/{STEAM_FLATPAK_APP_ID}/"))
            || text.ends_with(&format!("/.var/app/{STEAM_FLATPAK_APP_ID}"))
        {
            Some(SteamRootKind::Flatpak)
        } else if text.contains("/snap/steam/") {
            Some(SteamRootKind::Snap)
        } else {
            None
        }
    };
    check(root)
        .or_else(|| std::fs::canonicalize(root).ok().and_then(|c| check(&c)))
        .unwrap_or(SteamRootKind::Native)
}

pub fn steam_launcher(
    path_env: Option<&std::ffi::OsStr>,
    root: Option<&Path>,
    flatpak_dirs: &[PathBuf],
) -> Option<SteamLauncher> {
    let kind = root.map_or(SteamRootKind::Native, steam_root_kind);
    let flatpak_run = || {
        let installed = kind == SteamRootKind::Flatpak || flatpak_dirs.iter().any(|d| d.is_dir());
        if !installed {
            return None;
        }
        find_on_path("flatpak", path_env).map(|program| SteamLauncher {
            program,
            prefix_args: vec!["run".into(), STEAM_FLATPAK_APP_ID.into()],
        })
    };
    if kind == SteamRootKind::Flatpak
        && let Some(launcher) = flatpak_run()
    {
        return Some(launcher);
    }
    if let Some(program) = find_on_path("steam", path_env) {
        return Some(SteamLauncher {
            program,
            prefix_args: Vec::new(),
        });
    }
    if kind == SteamRootKind::Native
        && let Some(script) = root
            .map(|r| r.join("steam.sh"))
            .filter(|script| script.is_file())
    {
        return Some(SteamLauncher {
            program: script,
            prefix_args: Vec::new(),
        });
    }
    flatpak_run()
}

#[cfg(target_os = "linux")]
pub fn start_steam_detached(args: &[&str]) -> Result<()> {
    let path_env = std::env::var_os("PATH");
    let root = crate::core::config::detect_steam_path();
    let flatpak_dirs = steam_flatpak_app_dirs();
    let Some(launcher) = steam_launcher(path_env.as_deref(), root.as_deref(), &flatpak_dirs) else {
        tracing::warn!("no usable Steam launcher found");
        bail!("no `steam` on PATH, no host steam.sh and no Flatpak Steam");
    };
    let program = launcher.program;
    let mut cmd = std::process::Command::new(&program);
    cmd.args(&launcher.prefix_args)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow!("could not start {}: {e}", program.display()))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(target_os = "linux")]
fn connection_log_logged_on(text: &str) -> Option<bool> {
    let mut state = None;
    for line in text.lines() {
        if line.contains("[Logged On") {
            state = Some(true);
        } else if line.contains("[Logged Off") {
            state = Some(false);
        }
    }
    state
}

#[cfg(target_os = "linux")]
fn read_tail(path: &Path, max_bytes: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(max_bytes)))
        .ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

#[cfg(target_os = "linux")]
pub fn steam_is_logged_on() -> bool {
    let Some(root) = crate::core::config::detect_steam_path() else {
        return false;
    };
    let log = root.join("logs/connection_log.txt");
    let Some(text) = read_tail(&log, 64 * 1024) else {
        return false;
    };
    connection_log_logged_on(&text).unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
pub fn steam_is_logged_on() -> bool {
    true
}

#[cfg(target_os = "linux")]
pub async fn wait_for_steam_logged_on(timeout: std::time::Duration) -> bool {
    let start = std::time::Instant::now();
    let mut announced = false;
    loop {
        if steam_is_logged_on() {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        if !announced {
            tracing::info!("waiting for host Steam to reach logged-on before launch");
            announced = true;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

#[cfg(not(target_os = "linux"))]
pub async fn wait_for_steam_logged_on(_timeout: std::time::Duration) -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunnerComponents {
    pub dxvk: Option<ComponentInfo>,
    pub vkd3d_proton: Option<ComponentInfo>,
    pub vkd3d: Option<ComponentInfo>,
    pub nvapi: Option<ComponentInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentInfo {
    pub version: String,
    pub source: ComponentSource,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ComponentSource {
    BundledWithRunner,
    InstalledInPrefix,
    SystemWide,
}

impl std::fmt::Display for ComponentSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BundledWithRunner => write!(f, "bundled"),
            Self::InstalledInPrefix => write!(f, "in prefix"),
            Self::SystemWide => write!(f, "system"),
        }
    }
}

pub fn derive_runner_root(binary_path: &Path) -> PathBuf {
    let parent = if binary_path.is_file() {
        binary_path.parent().unwrap_or(binary_path)
    } else {
        binary_path
    };
    if parent.file_name().is_some_and(|n| n == "bin") {
        return parent.parent().unwrap_or(parent).to_path_buf();
    }

    parent.to_path_buf()
}

pub fn detect_runner_components(runner_path: &Path, wineprefix: Option<&Path>) -> RunnerComponents {
    let root = derive_runner_root(runner_path);

    RunnerComponents {
        dxvk: detect_dxvk(&root, wineprefix),
        vkd3d_proton: detect_vkd3d_proton(&root, wineprefix),
        vkd3d: detect_vkd3d(&root, wineprefix),
        nvapi: detect_nvapi(&root, wineprefix),
    }
}

fn detect_dxvk(root: &Path, prefix: Option<&Path>) -> Option<ComponentInfo> {
    if let Some(info) = detect_bundled_modern(
        root,
        &crate::compat::proton::wine_component_dirs("dxvk"),
        |_| {
            &[
                "d3d11.dll",
                "dxgi.dll",
                "d3d9.dll",
                "d3d8.dll",
                "d3d10core.dll",
            ]
        },
    ) {
        return Some(info);
    }

    if let Some(info) = check_bundled_legacy(root, "dxvk", "d3d11.dll") {
        return Some(info);
    }

    if let Some(pfx) = prefix {
        let prefix_dlls = [
            "drive_c/windows/system32/d3d11.dll",
            "drive_c/windows/syswow64/d3d11.dll",
        ];
        if let Some(info) = check_prefix(pfx, &prefix_dlls) {
            return Some(info);
        }
    }

    let system_paths = [
        "/usr/share/dxvk/x64/d3d11.dll",
        "/usr/lib/dxvk/d3d11.dll",
        "/usr/lib/x86_64-linux-gnu/dxvk/d3d11.dll",
        "/usr/local/share/dxvk/x64/d3d11.dll",
    ];
    check_system(&system_paths)
}

fn detect_vkd3d_proton(root: &Path, prefix: Option<&Path>) -> Option<ComponentInfo> {
    if let Some(info) = detect_bundled_modern(
        root,
        &crate::compat::proton::wine_component_dirs("vkd3d-proton"),
        |_| &["d3d12.dll", "d3d12core.dll"],
    ) {
        return Some(info);
    }

    if let Some(info) = check_bundled_legacy(root, "vkd3d-proton", "d3d12.dll") {
        return Some(info);
    }

    if let Some(pfx) = prefix {
        if let Some(info) = check_prefix_d3d12(pfx, true) {
            return Some(info);
        }
    }

    let system_paths = [
        "/usr/share/vkd3d-proton/x64/d3d12.dll",
        "/usr/lib/vkd3d-proton/d3d12.dll",
        "/usr/local/share/vkd3d-proton/x64/d3d12.dll",
    ];
    check_system(&system_paths)
}

fn detect_nvapi(root: &Path, prefix: Option<&Path>) -> Option<ComponentInfo> {
    if let Some(info) = detect_bundled_modern(
        root,
        &crate::compat::proton::wine_component_dirs("nvapi"),
        |arch| {
            if arch == "x86_64-windows" {
                &["nvapi64.dll"]
            } else {
                &["nvapi.dll"]
            }
        },
    ) {
        return Some(info);
    }

    if let Some(pfx) = prefix {
        let prefix_dlls = [
            "drive_c/windows/system32/nvapi64.dll",
            "drive_c/windows/syswow64/nvapi.dll",
        ];
        if let Some(info) = check_prefix(pfx, &prefix_dlls) {
            return Some(info);
        }
    }

    None
}

fn detect_vkd3d(root: &Path, prefix: Option<&Path>) -> Option<ComponentInfo> {
    if let Some(info) = detect_bundled_modern(
        root,
        &crate::compat::proton::wine_component_dirs("vkd3d"),
        |_| &["libvkd3d-1.dll", "libvkd3d-shader-1.dll"],
    ) {
        return Some(info);
    }

    if let Some(info) = check_bundled_legacy(root, "vkd3d", "libvkd3d-1.dll") {
        return Some(info);
    }

    if let Some(pfx) = prefix {
        if let Some(info) = check_prefix_d3d12(pfx, false) {
            return Some(info);
        }
    }

    let system_paths = [
        "/usr/lib/x86_64-linux-gnu/libvkd3d.so.1",
        "/usr/lib64/libvkd3d.so.1",
        "/usr/local/lib/libvkd3d.so.1",
    ];
    check_system(&system_paths)
}

fn detect_bundled_modern<S: AsRef<str>>(
    root: &Path,
    subdirs: &[S],
    required_for_arch: impl Fn(&str) -> &'static [&'static str],
) -> Option<ComponentInfo> {
    for subdir in subdirs {
        let comp_path = root.join(subdir.as_ref());
        if !comp_path.is_dir() {
            continue;
        }
        for arch in crate::compat::proton::ARCH_SUBDIRS
            .iter()
            .filter(|a| a.ends_with("-windows"))
        {
            let arch = *arch;
            let arch_path = comp_path.join(arch);
            let required = required_for_arch(arch);
            if required.iter().all(|dll| arch_path.join(dll).exists()) {
                let version = ["version", "../version"]
                    .iter()
                    .filter_map(|v| std::fs::read_to_string(arch_path.join(v)).ok())
                    .map(|s| parse_short_version(&s))
                    .find(|s| s != "unknown")
                    .unwrap_or_else(|| "found".to_string());
                return Some(ComponentInfo {
                    version,
                    source: ComponentSource::BundledWithRunner,
                    path: Some(arch_path),
                });
            }
        }
    }
    None
}

fn check_bundled_legacy(root: &Path, component: &str, dll: &str) -> Option<ComponentInfo> {
    let dll_candidates: Vec<String> = [
        "files/lib64",
        "files/lib",
        "dist/lib64",
        "dist/lib",
        "lib64",
        "lib",
    ]
    .iter()
    .map(|base| format!("{base}/wine/{component}/{dll}"))
    .collect();
    let version_files: Vec<String> = ["files/share", "dist/share", "share"]
        .iter()
        .map(|base| format!("{base}/{component}/version"))
        .collect();
    let dll_refs: Vec<&str> = dll_candidates.iter().map(String::as_str).collect();
    let version_refs: Vec<&str> = version_files.iter().map(String::as_str).collect();
    check_bundled(root, &dll_refs, &version_refs)
}

fn check_prefix_d3d12(pfx: &Path, want_proton: bool) -> Option<ComponentInfo> {
    for rel in [
        "drive_c/windows/system32/d3d12.dll",
        "drive_c/windows/syswow64/d3d12.dll",
    ] {
        let p = pfx.join(rel);
        if p.exists() && dll_contains_string(&p, "vkd3d-proton") == want_proton {
            let version = extract_version_from_dll(&p).unwrap_or_else(|| "unknown".to_string());
            return Some(ComponentInfo {
                version,
                source: ComponentSource::InstalledInPrefix,
                path: Some(p),
            });
        }
    }
    None
}

fn check_bundled(
    root: &Path,
    dll_candidates: &[&str],
    version_files: &[&str],
) -> Option<ComponentInfo> {
    let Some(found_dll) = dll_candidates.iter().find(|rel| root.join(rel).exists()) else {
        return None;
    };
    tracing::debug!(
        "Found bundled component DLL at: {}",
        root.join(found_dll).display()
    );

    let version = version_files
        .iter()
        .filter_map(|rel| {
            let p = root.join(rel);
            if p.exists() {
                tracing::debug!("Found version file: {}", p.display());
                std::fs::read_to_string(p).ok()
            } else {
                None
            }
        })
        .map(|s| parse_short_version(&s))
        .find(|s| s != "unknown")
        .or_else(|| {
            dll_candidates
                .iter()
                .map(|rel| root.join(rel))
                .find(|p| p.exists())
                .and_then(|p| extract_version_from_dll(&p))
        })
        .unwrap_or_else(|| "unknown".to_string());

    Some(ComponentInfo {
        version,
        source: ComponentSource::BundledWithRunner,
        path: root.join(found_dll).parent().map(|p| p.to_path_buf()),
    })
}

fn check_prefix(prefix: &Path, dll_candidates: &[&str]) -> Option<ComponentInfo> {
    for rel in dll_candidates {
        let p = prefix.join(rel);
        if p.exists() {
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            if size < 51_200 {
                continue;
            }

            let version = extract_version_from_dll(&p).unwrap_or_else(|| "unknown".to_string());
            return Some(ComponentInfo {
                version,
                source: ComponentSource::InstalledInPrefix,
                path: Some(p),
            });
        }
    }
    None
}

fn check_system(paths: &[&str]) -> Option<ComponentInfo> {
    for path in paths {
        let p = Path::new(path);
        if p.exists() {
            let version = extract_version_from_dll(p)
                .or_else(|| read_adjacent_version_file(p))
                .unwrap_or_else(|| "unknown".to_string());
            return Some(ComponentInfo {
                version,
                source: ComponentSource::SystemWide,
                path: Some(p.to_path_buf()),
            });
        }
    }
    None
}

fn read_adjacent_version_file(dll: &Path) -> Option<String> {
    let parent = dll.parent()?;
    let version_file = parent.join("version");
    std::fs::read_to_string(version_file)
        .ok()
        .map(|s| parse_short_version(&s))
        .filter(|s| !s.is_empty())
}

pub fn parse_short_version(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        return "unknown".to_string();
    }

    let v = if let (Some(start), Some(end)) = (s.find('('), s.rfind(')')) {
        if start < end { &s[start + 1..end] } else { s }
    } else {
        if s.contains(' ') {
            s.split_whitespace().last().unwrap_or(s)
        } else {
            s
        }
    };

    let mut v = v.trim();

    for prefix in &["vkd3d-proton-", "vkd3d-", "dxvk-"] {
        if let Some(stripped) = v.strip_prefix(prefix) {
            v = stripped;
            break;
        }
    }

    if v.starts_with('v') && v.len() > 1 && v.as_bytes()[1].is_ascii_digit() {
        v = &v[1..];
    }

    if let Some(hyphen_idx) = v.rfind("-g") {
        let suffix = &v[hyphen_idx + 2..];
        if suffix.len() >= 7 && suffix.len() <= 10 && suffix.chars().all(|c| c.is_ascii_hexdigit())
        {
            return v[..hyphen_idx].to_string();
        }
    }

    v.to_string()
}

fn dll_contains_string(path: &Path, needle: &str) -> bool {
    let needle_lower = needle.to_ascii_lowercase();
    std::fs::read(path)
        .map(|bytes| {
            bytes.windows(needle.len()).any(|w| {
                w.iter()
                    .zip(needle_lower.bytes())
                    .all(|(b, n)| b.to_ascii_lowercase() == n)
            })
        })
        .unwrap_or(false)
}

fn extract_version_from_dll(dll_path: &Path) -> Option<String> {
    let data = std::fs::read(dll_path).ok()?;

    let mut runs: Vec<String> = Vec::new();
    let mut current = Vec::new();
    for &byte in &data {
        if byte >= 0x20 && byte < 0x7f {
            current.push(byte as char);
        } else {
            if current.len() >= 4 {
                runs.push(current.iter().collect());
            }
            current.clear();
        }
    }
    if current.len() >= 4 {
        runs.push(current.iter().collect());
    }

    static SEMVER_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^v?(\d{1,3})\.(\d{1,3})(\.\d{1,3})?([-.][a-zA-Z0-9._-]+)?$").unwrap()
    });

    let mut candidates: Vec<String> = runs
        .into_iter()
        .filter(|s| SEMVER_RE.is_match(s))
        .filter(|s| {
            let parts: Vec<&str> = s.trim_start_matches('v').splitn(2, '.').collect();
            parts.len() >= 2 && parts[0].parse::<u32>().unwrap_or(100) <= 99
        })
        .collect();

    candidates.sort_by(|a, b| b.len().cmp(&a.len()));
    candidates.into_iter().next()
}

pub fn normalize_dll_name(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    lower.strip_suffix(".dll").unwrap_or(&lower).to_string()
}

fn parse_dll_override_groups(overrides: &str) -> Vec<(Vec<String>, Option<String>, String)> {
    overrides
        .split(';')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(|entry| match entry.split_once('=') {
            Some((names, mode)) => (
                names
                    .split(',')
                    .map(|n| n.trim().to_string())
                    .filter(|n| !n.is_empty())
                    .collect(),
                Some(mode.trim().to_string()),
                entry.to_string(),
            ),
            None => (Vec::new(), None, entry.to_string()),
        })
        .collect()
}

pub fn dll_override_mode(overrides: &str, dll: &str) -> Option<String> {
    let key = normalize_dll_name(dll);
    parse_dll_override_groups(overrides)
        .into_iter()
        .rev()
        .find_map(|(names, mode, _)| {
            names
                .iter()
                .any(|n| normalize_dll_name(n) == key)
                .then_some(mode)
                .flatten()
        })
}

pub fn dll_override_names(overrides: &str) -> std::collections::HashSet<String> {
    parse_dll_override_groups(overrides)
        .into_iter()
        .flat_map(|(names, _, _)| names)
        .map(|n| normalize_dll_name(&n))
        .collect()
}

pub fn merge_dll_overrides(existing: &str, entries: &[(String, String)]) -> String {
    let mut new_entries: Vec<(String, String, String)> = Vec::new();
    for (name, mode) in entries {
        let key = normalize_dll_name(name);
        if key.is_empty() {
            continue;
        }
        new_entries.retain(|(k, _, _)| *k != key);
        new_entries.push((key, name.trim().to_string(), mode.trim().to_string()));
    }
    let replaced: std::collections::HashSet<&str> =
        new_entries.iter().map(|(k, _, _)| k.as_str()).collect();

    let mut out: Vec<String> = Vec::new();
    for (names, mode, raw) in parse_dll_override_groups(existing) {
        let Some(mode) = mode else {
            out.push(raw);
            continue;
        };
        let kept: Vec<String> = names
            .into_iter()
            .filter(|n| !replaced.contains(normalize_dll_name(n).as_str()))
            .collect();
        if !kept.is_empty() {
            out.push(format!("{}={}", kept.join(","), mode));
        }
    }
    out.extend(
        new_entries
            .into_iter()
            .map(|(_, name, mode)| format!("{name}={mode}")),
    );
    out.join(";")
}

pub fn fill_dll_overrides(existing: &str, entries: &[(String, String)]) -> String {
    let present = dll_override_names(existing);
    let missing: Vec<(String, String)> = entries
        .iter()
        .filter(|(name, _)| !present.contains(&normalize_dll_name(name)))
        .cloned()
        .collect();
    merge_dll_overrides(existing, &missing)
}

pub fn dll_override_entries(overrides: &str) -> Vec<(String, String)> {
    parse_dll_override_groups(overrides)
        .into_iter()
        .filter_map(|(names, mode, _)| mode.map(|m| (names, m)))
        .flat_map(|(names, mode)| names.into_iter().map(move |n| (n, mode.clone())))
        .collect()
}

pub fn build_dll_overrides(
    dxvk_active: bool,
    vkd3d_proton_active: bool,
    vkd3d_active: bool,
    no_overlay: bool,
    force_builtin_d3d: bool,
    game_dir: Option<&std::path::Path>,
    strict_dxvk: bool,
    steam_enabled: bool,
) -> String {
    let mut overrides: Vec<String> = if steam_enabled {
        Vec::new()
    } else {
        vec![
            "vstdlib_s=n".into(),
            "tier0_s=n".into(),
            "steamclient=n".into(),
            "steamclient64=n".into(),
            "steam_api=n".into(),
            "steam_api64=n".into(),
            "lsteamclient=".into(),
        ]
    };

    if no_overlay {
        overrides.push("GameOverlayRenderer=n".into());
        overrides.push("GameOverlayRenderer64=n".into());
    }

    if force_builtin_d3d {
        for dll in &[
            "d3d8",
            "d3d9",
            "d3d10core",
            "d3d11",
            "dxgi",
            "d3d12",
            "d3d12core",
        ] {
            overrides.push(format!("{dll}=b"));
        }
        return overrides.join(";");
    }

    if dxvk_active {
        let game_has = |dll: &str| -> bool { game_dir.is_some_and(|d| d.join(dll).exists()) };

        for dll in &[
            "d3d8.dll",
            "d3d9.dll",
            "d3d10core.dll",
            "d3d11.dll",
            "dxgi.dll",
        ] {
            let stem = dll.trim_end_matches(".dll");
            let mode = if strict_dxvk { "n" } else { "n,b" };

            if strict_dxvk || !game_has(dll) {
                overrides.push(format!("{stem}={mode}"));
            }
        }
    }

    if vkd3d_proton_active || vkd3d_active {
        overrides.push("d3d12=n,b".into());
        overrides.push("d3d12core=n,b".into());
        if vkd3d_active {
            overrides.push("libvkd3d-1=n,b".into());
            overrides.push("libvkd3d-shader-1=n,b".into());
        }
    }

    overrides.join(";")
}

#[derive(Debug, Clone)]
pub struct MasterSteamConfig {
    pub root_dir: PathBuf,
    pub wine_prefix: PathBuf,
    pub layout_kind: String,
    pub steam_exe: Option<PathBuf>,
}

pub fn get_master_steam_config() -> MasterSteamConfig {
    let root_dir = crate::core::config::config_dir()
        .unwrap_or_default()
        .join("master_steam_prefix");

    let (wine_prefix, layout_kind) = if root_dir.join("pfx/drive_c").exists() {
        (root_dir.join("pfx"), "pfx".to_string())
    } else if root_dir.join("drive_c").exists() {
        (root_dir.clone(), "root".to_string())
    } else {
        (root_dir.join("pfx"), "pfx".to_string())
    };

    let steam_exe = find_steam_exe_in_prefix(&wine_prefix);

    MasterSteamConfig {
        root_dir,
        wine_prefix,
        layout_kind,
        steam_exe,
    }
}

pub fn find_steam_exe_in_prefix(prefix: &Path) -> Option<PathBuf> {
    let steam_dirs = [
        "drive_c/Program Files (x86)/Steam",
        "drive_c/Program Files/Steam",
    ];

    for rel_dir in steam_dirs {
        let dir = prefix.join(rel_dir);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("steam.exe")
            {
                return Some(entry.path());
            }
        }
    }

    None
}

pub fn resolve_master_wineprefix() -> PathBuf {
    get_master_steam_config().wine_prefix
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedGpu {
    pub name: String,
    pub pci_id: Option<String>,
    pub is_discrete: bool,
}

pub fn list_available_gpus() -> Vec<DetectedGpu> {
    let mut gpus = Vec::new();

    let drm_path = Path::new("/sys/class/drm");
    if let Ok(entries) = std::fs::read_dir(drm_path) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("card") && !name.contains('-') {
                let card_path = entry.path();

                let device_path = card_path.join("device");
                let vendor = std::fs::read_to_string(device_path.join("vendor"))
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                let device = std::fs::read_to_string(device_path.join("device"))
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();

                let pci_id = if !vendor.is_empty() && !device.is_empty() {
                    Some(format!(
                        "{}:{}",
                        vendor.replace("0x", ""),
                        device.replace("0x", "")
                    ))
                } else {
                    None
                };

                let is_discrete = pci_id.as_ref().is_some_and(|id| {
                    id.starts_with("10de") || id.starts_with("1002")
                });

                let gpu_name = match pci_id.as_deref() {
                    Some(id) if id.starts_with("10de") => format!("NVIDIA GPU ({})", name),
                    Some(id) if id.starts_with("1002") => format!("AMD GPU ({})", name),
                    Some(id) if id.starts_with("8086") => format!("Intel GPU ({})", name),
                    _ => format!("Unknown GPU ({})", name),
                };

                gpus.push(DetectedGpu {
                    name: gpu_name,
                    pci_id,
                    is_discrete,
                });
            }
        }
    }

    if gpus.is_empty() && Path::new("/dev/nvidia0").exists() {
        gpus.push(DetectedGpu {
            name: "NVIDIA Discrete GPU".to_string(),
            pci_id: Some("10de:unknown".to_string()),
            is_discrete: true,
        });
    }

    gpus.sort_by(|a, b| b.is_discrete.cmp(&a.is_discrete));
    gpus
}

pub fn detect_exe_architecture(exe_path: &Path) -> crate::core::models::ExecutableArchitecture {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = match std::fs::File::open(exe_path) {
        Ok(f) => f,
        Err(_) => return crate::core::models::ExecutableArchitecture::Unknown,
    };

    let mut mz_header = [0u8; 2];
    if file.read_exact(&mut mz_header).is_err() || &mz_header != b"MZ" {
        return crate::core::models::ExecutableArchitecture::Unknown;
    }

    if file.seek(SeekFrom::Start(0x3C)).is_err() {
        return crate::core::models::ExecutableArchitecture::Unknown;
    }

    let mut pe_offset_buf = [0u8; 4];
    if file.read_exact(&mut pe_offset_buf).is_err() {
        return crate::core::models::ExecutableArchitecture::Unknown;
    }
    let pe_offset = u32::from_le_bytes(pe_offset_buf);

    if file.seek(SeekFrom::Start(pe_offset as u64)).is_err() {
        return crate::core::models::ExecutableArchitecture::Unknown;
    }

    let mut pe_signature = [0u8; 4];
    if file.read_exact(&mut pe_signature).is_err() || &pe_signature != b"PE\0\0" {
        return crate::core::models::ExecutableArchitecture::Unknown;
    }

    let mut machine_buf = [0u8; 2];
    if file.read_exact(&mut machine_buf).is_err() {
        return crate::core::models::ExecutableArchitecture::Unknown;
    }
    let machine = u16::from_le_bytes(machine_buf);

    match machine {
        0x014c => crate::core::models::ExecutableArchitecture::X86,
        0x8664 => crate::core::models::ExecutableArchitecture::X86_64,
        _ => crate::core::models::ExecutableArchitecture::Unknown,
    }
}

fn deploy_one_dll_symlink(src: &Path, dest: &Path, log: bool) -> Result<()> {
    if dest.exists() {
        let meta = std::fs::symlink_metadata(dest)?;
        if !meta.file_type().is_symlink() {
            let backup = dest.with_extension("dll.bak");
            if !backup.exists() {
                if log {
                    tracing::info!(
                        "Backing up original DLL: {} -> {}",
                        dest.display(),
                        backup.display()
                    );
                }
                std::fs::rename(dest, &backup)?;
            } else {
                std::fs::remove_file(dest)?;
            }
        } else {
            std::fs::remove_file(dest)?;
        }
    }

    if log {
        tracing::info!("Symlinking {} -> {}", src.display(), dest.display());
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(src, dest)?;
    #[cfg(not(unix))]
    std::fs::copy(src, dest)?;

    Ok(())
}

pub fn deploy_dll_symlinks(
    prefix: &Path,
    resolutions: &[crate::launch::dll_provider_resolver::DllResolution],
    target_arch: &crate::core::models::ExecutableArchitecture,
) -> Result<Vec<PathBuf>> {
    let mut deployed = Vec::new();
    let is_64bit_prefix = prefix.join("drive_c/windows/syswow64").exists();

    for res in resolutions {
        if res.chosen_provider != crate::launch::dll_provider_resolver::DllProvider::Runner
            && res.chosen_provider != crate::launch::dll_provider_resolver::DllProvider::Custom
        {
            continue;
        }

        if let Some(src_path) = &res.chosen_path {
            let dll_name = format!("{}.dll", res.name);

            let dest_dir = match target_arch {
                crate::core::models::ExecutableArchitecture::X86_64 => {
                    prefix.join("drive_c/windows/system32")
                }
                crate::core::models::ExecutableArchitecture::X86 => {
                    if is_64bit_prefix {
                        prefix.join("drive_c/windows/syswow64")
                    } else {
                        prefix.join("drive_c/windows/system32")
                    }
                }
                _ => continue,
            };

            if !dest_dir.exists() {
                continue;
            }

            let dest_path = dest_dir.join(&dll_name);

            deploy_one_dll_symlink(src_path, &dest_path, true)?;

            deployed.push(dest_path);

            if is_64bit_prefix {
                let (other_arch, other_dir) = match target_arch {
                    crate::core::models::ExecutableArchitecture::X86_64 => (
                        crate::core::models::ExecutableArchitecture::X86,
                        prefix.join("drive_c/windows/syswow64"),
                    ),
                    crate::core::models::ExecutableArchitecture::X86 => (
                        crate::core::models::ExecutableArchitecture::X86_64,
                        prefix.join("drive_c/windows/system32"),
                    ),
                    _ => continue,
                };

                if let Some(other_src) = find_sibling_dll(src_path, target_arch, &other_arch) {
                    let other_dest = other_dir.join(&dll_name);
                    deploy_one_dll_symlink(&other_src, &other_dest, false)?;
                    deployed.push(other_dest);
                }
            }
        }
    }

    Ok(deployed)
}

fn find_sibling_dll(
    path: &Path,
    current_arch: &crate::core::models::ExecutableArchitecture,
    target_arch: &crate::core::models::ExecutableArchitecture,
) -> Option<PathBuf> {
    use crate::core::models::ExecutableArchitecture as Arch;
    let flip = match (current_arch, target_arch) {
        (Arch::X86_64, Arch::X86) => false,
        (Arch::X86, Arch::X86_64) => true,
        _ => return None,
    };

    let path_str = path.to_string_lossy();
    for (tag64, tag32) in [("x86_64", "i386"), ("x64", "x32")] {
        let (current_tag, target_tag) = if flip { (tag32, tag64) } else { (tag64, tag32) };
        if path_str.contains(current_tag) {
            let other_path = PathBuf::from(path_str.replace(current_tag, target_tag));
            if other_path.exists() {
                return Some(other_path);
            }
        }
    }

    None
}

pub fn cleanup_dll_symlinks(prefix: &Path) -> Result<()> {
    let target_dlls = [
        "d3d8.dll",
        "d3d9.dll",
        "dxgi.dll",
        "d3d10core.dll",
        "d3d11.dll",
        "d3d12.dll",
        "d3d12core.dll",
        "libvkd3d-1.dll",
        "libvkd3d-shader-1.dll",
    ];

    let dirs = [
        prefix.join("drive_c/windows/system32"),
        prefix.join("drive_c/windows/syswow64"),
    ];

    for dir in dirs {
        if !dir.exists() {
            continue;
        }
        for dll in &target_dlls {
            let p = dir.join(dll);
            if p.exists() {
                let meta = std::fs::symlink_metadata(&p)?;
                if meta.file_type().is_symlink() {
                    tracing::info!("Cleaning up symlink: {}", p.display());
                    std::fs::remove_file(&p)?;

                    let backup = p.with_extension("dll.bak");
                    if backup.exists() {
                        tracing::info!("Restoring backup: {} -> {}", backup.display(), p.display());
                        std::fs::rename(&backup, &p)?;
                    }
                }
            }
        }
    }

    Ok(())
}

pub fn steam_wineprefix_for_game(
    config: &crate::core::config::LauncherConfig,
    app_id: u32,
    user_configs: &crate::core::models::UserConfigStore,
    install_path: Option<&Path>,
) -> std::path::PathBuf {
    wineprefix_for_game(config, app_id, user_configs.get(&app_id), install_path)
}

pub fn wineprefix_for_game(
    config: &crate::core::config::LauncherConfig,
    app_id: u32,
    user_config: Option<&crate::core::models::UserAppConfig>,
    install_path: Option<&Path>,
) -> std::path::PathBuf {
    let use_steam_runtime = match user_config.map(|c| &c.steam_runtime_policy) {
        Some(crate::core::models::SteamRuntimePolicy::Enabled) => true,
        Some(crate::core::models::SteamRuntimePolicy::Disabled) => false,
        Some(crate::core::models::SteamRuntimePolicy::Auto) | None => {
            user_config.map(|c| c.use_steam_runtime).unwrap_or(false)
        }
    };

    let use_per_game_compat_data = user_config
        .map(|c| {
            use_steam_runtime
                && c.steam_prefix_mode == crate::core::models::SteamPrefixMode::PerGame
        })
        .unwrap_or(config.use_shared_compat_data);

    if use_per_game_compat_data {
        proton_compat_prefix(config, app_id, install_path)
    } else {
        resolve_master_wineprefix()
    }
}

pub fn is_launch_url(executable: &str) -> bool {
    let Some((scheme, rest)) = executable.split_once("://") else { return false };
    scheme.len() > 1 && !rest.is_empty() && scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

pub fn proton_compat_prefix(
    config: &crate::core::config::LauncherConfig,
    app_id: u32,
    install_path: Option<&Path>,
) -> std::path::PathBuf {
    game_compat_data_dir(config, app_id, install_path).join("pfx")
}

pub fn game_compat_data_dir(
    config: &crate::core::config::LauncherConfig,
    app_id: u32,
    install_path: Option<&Path>,
) -> PathBuf {
    let configured = Path::new(&config.steam_library_path);
    let root = game_library_root(config, app_id, install_path);
    let dir = compat_data_dir(&root, app_id);
    if root != configured {
        let old = compat_data_dir(configured, app_id);
        if old.is_dir() && !dir.exists() {
            tracing::warn!(
                "app {app_id}: prefix now lives in its own library ({}); \
                 an older prefix exists at {} (move it there to keep saves)",
                dir.display(),
                old.display()
            );
        }
    }
    dir
}

pub fn game_library_root(
    config: &crate::core::config::LauncherConfig,
    app_id: u32,
    install_path: Option<&Path>,
) -> PathBuf {
    if let Some(lib) = install_path.and_then(|p| library_from_install_path(p, app_id)) {
        return lib;
    }
    game_library_root_in(
        Path::new(&config.steam_library_path),
        &crate::library::steam_root_candidates(),
        app_id,
    )
}

pub fn library_from_install_path(install_path: &Path, app_id: u32) -> Option<PathBuf> {
    let common = install_path.parent()?;
    let steamapps = common.parent()?;
    let is = |p: &Path, name: &str| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case(name))
    };
    if !is(common, "common") || !is(steamapps, "steamapps") {
        return None;
    }
    steamapps
        .join(format!("appmanifest_{app_id}.acf"))
        .is_file()
        .then(|| steamapps.parent().map(Path::to_path_buf))
        .flatten()
}

pub fn game_library_root_in(configured: &Path, steam_roots: &[PathBuf], app_id: u32) -> PathBuf {
    let manifest = format!("appmanifest_{app_id}.acf");
    let has_manifest = |lib: &Path| lib.join("steamapps").join(&manifest).is_file();
    if has_manifest(configured) {
        return configured.to_path_buf();
    }
    let mut roots = vec![configured.to_path_buf()];
    roots.extend(steam_roots.iter().cloned());
    let mut candidates: Vec<PathBuf> = steam_roots.to_vec();
    for root in &roots {
        for vdf in [
            root.join("steamapps").join("libraryfolders.vdf"),
            root.join("config").join("libraryfolders.vdf"),
        ] {
            if let Ok(raw) = std::fs::read_to_string(&vdf) {
                candidates.extend(crate::library::library_folder_paths(&raw));
            }
        }
    }
    candidates
        .into_iter()
        .find(|lib| has_manifest(lib))
        .unwrap_or_else(|| configured.to_path_buf())
}

pub fn compat_data_dir(library_root: &Path, app_id: u32) -> PathBuf {
    library_root
        .join("steamapps")
        .join("compatdata")
        .join(app_id.to_string())
}

pub fn game_save_prefix(
    config: &crate::core::config::LauncherConfig,
    app_id: u32,
    user_configs: &crate::core::models::UserConfigStore,
    install_path: Option<&Path>,
) -> std::path::PathBuf {
    let proton_prefix = proton_compat_prefix(config, app_id, install_path);
    if proton_prefix.join("drive_c").is_dir() {
        return proton_prefix;
    }
    steam_wineprefix_for_game(config, app_id, user_configs, install_path)
}

pub fn resolve_path_ignore_case(base: &Path, rel: &str) -> Option<PathBuf> {
    let mut current = if rel.starts_with('/') {
        PathBuf::from("/")
    } else {
        base.to_path_buf()
    };
    for part in rel
        .split(['/', '\\'])
        .filter(|p| !p.is_empty() && *p != ".")
    {
        let mut matches: Vec<PathBuf> = std::fs::read_dir(&current)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.eq_ignore_ascii_case(part))
            })
            .map(|e| e.path())
            .collect();
        let exact = current.join(part);
        current = if let Some(i) = matches
            .iter()
            .position(|m| m.file_name() == exact.file_name())
        {
            matches.swap_remove(i)
        } else if !matches.is_empty() {
            // Stable pick among case variants.
            matches.sort();
            matches.swap_remove(0)
        } else if exact.exists() {
            // Never listed, e.g. `..`.
            exact
        } else {
            return None;
        };
    }
    current.exists().then_some(current)
}

#[cfg(test)]
#[path = "utils_path_ignore_case_tests.rs"]
mod path_ignore_case_tests;

#[cfg(test)]
#[path = "utils_save_prefix_tests.rs"]
mod save_prefix_tests;

#[cfg(test)]
#[path = "utils_resolve_runner_tests.rs"]
mod resolve_runner_tests;

#[cfg(test)]
#[path = "utils_runner_classification_tests.rs"]
mod runner_classification_tests;

#[cfg(test)]
#[path = "utils_safe_dir_name_tests.rs"]
mod safe_dir_name_tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "utils_steam_logon_tests.rs"]
mod steam_logon_tests;

#[cfg(test)]
#[path = "utils_steam_appid_tests.rs"]
mod steam_appid_tests;

#[cfg(test)]
#[path = "utils_dll_overrides_tests.rs"]
mod dll_overrides_tests;

#[cfg(test)]
#[path = "utils_steam_client_tests.rs"]
mod steam_client_tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "utils_steam_launcher_tests.rs"]
mod steam_launcher_tests;

#[cfg(test)]
#[path = "utils_wait_until_tests.rs"]
mod wait_until_tests;

#[cfg(test)]
#[path = "utils_game_library_tests.rs"]
mod game_library_tests;

#[cfg(test)]
mod launch_url_tests {
    use super::is_launch_url;

    #[test]
    fn launch_urls_are_told_apart_from_file_paths() {
        assert!(is_launch_url("steam2ea://launchgame/1222670?platform=steam&theme=ts4"));
        assert!(is_launch_url("uplay://launch/635/0"));
        assert!(is_launch_url("com.epicgames.launcher://apps/x?action=launch"));
        assert!(!is_launch_url("Game/Bin/TS4_x64.exe"));
        assert!(!is_launch_url("C:\\Games\\a.exe"), "a drive letter is not a scheme");
        assert!(!is_launch_url("run://"), "nothing after it");
        assert!(!is_launch_url("://x"));
        assert!(!is_launch_url(""));
    }
}
