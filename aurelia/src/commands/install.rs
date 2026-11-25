
use crate::cli::*;
use crate::commands::common::*;

use anyhow::{Context, Result, bail};
use aurelia::core::config::load_launcher_config;
use aurelia::core::error::{ErrorKind, TypedError, classify};
use aurelia::core::models::{DepotPlatform, DownloadState, LibraryGame};
use aurelia::library::backlog;
use aurelia::steam_client::{InstallOptions, SteamClient};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};

fn auto_select_platform(platforms: &[DepotPlatform]) -> DepotPlatform {
    platforms
        .iter()
        .copied()
        .find(|p| *p == DepotPlatform::Windows)
        .or_else(|| platforms.first().copied())
        .unwrap_or(DepotPlatform::Windows)
}

pub(crate) async fn cmd_install(
    app_id: u32,
    platform: Option<PlatformArg>,
    restart_steam: bool,
    dry_run: bool,
    library: Option<String>,
    json: bool,
) -> Result<()> {
    if dry_run {
        let mut client = authed_client().await?;
        return cmd_install_dry_run(&mut client, app_id, platform, json).await;
    }
    let request = InstallRequest {
        app_id,
        platform,
        restart_steam,
        library,
        on_disk: 0,
        auto_attempts: 0,
    };
    run_install(request, json).await
}

pub(crate) struct InstallRequest {
    pub(crate) app_id: u32,
    pub(crate) platform: Option<PlatformArg>,
    pub(crate) restart_steam: bool,
    pub(crate) library: Option<String>,
    pub(crate) on_disk: u64,
    pub(crate) auto_attempts: u32,
}

async fn existing_install_bytes(library_root: &str, app_id: u32) -> u64 {
    let steamapps = std::path::Path::new(library_root).join("steamapps");
    let Ok(raw) = tokio::fs::read_to_string(steamapps.join(format!("appmanifest_{app_id}.acf"))).await else {
        return 0;
    };
    let Some(dir) = aurelia::core::acf::parse_app_manifest(&raw).install_dir else {
        return 0;
    };
    let root = steamapps.join("common").join(dir);
    tokio::task::spawn_blocking(move || allocated_bytes(&root)).await.unwrap_or(0)
}

fn allocated_bytes(root: &std::path::Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        for entry in read.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(entry.path());
            } else if meta.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    total += meta.blocks() * 512;
                }
                #[cfg(not(unix))]
                {
                    total += meta.len();
                }
            }
        }
    }
    total
}

pub(crate) async fn run_install(request: InstallRequest, json: bool) -> Result<()> {
    let InstallRequest {
        app_id,
        platform,
        restart_steam,
        library,
        on_disk,
        auto_attempts,
    } = request;
    let mut client = authed_client().await?;

    let is_dlc = client.resolve_dlc_parent(app_id).await.is_some();

    let (platform, cached_vdf) = match platform {
        Some(p) => (p.into(), None),
        None => {
            let (platforms, buffer) = client
                .get_available_platforms(app_id)
                .await
                .context("failed to detect available platforms")?;
            let chosen = auto_select_platform(&platforms);
            if !json {
                cli_println!("Auto-selected platform: {chosen:?}");
            }
            (chosen, Some(buffer))
        }
    };

    let library_root = match &library {
        Some(lib) => lib.clone(),
        None => load_launcher_config().await?.steam_library_path,
    };
    if !is_dlc {
        let est = client
            .estimate_install_size(app_id, platform)
            .await
            .with_context(|| format!("failed to estimate install size for app {app_id}"))?;
        if let Some(free) = available_space_for(std::path::Path::new(&library_root)) {
            let mut have = on_disk;
            if est.disk_size.saturating_sub(have) > free {
                have = have.max(existing_install_bytes(&library_root, app_id).await);
            }
            if est.disk_size.saturating_sub(have) > free {
                return Err(aurelia::core::error::TypedError::new(
                    aurelia::core::error::ErrorKind::DiskFull,
                    format!(
                        "not enough space on {library_root}: needs {} but only {} free",
                        human_bytes(est.disk_size),
                        human_bytes(free)
                    ),
                )
                .into());
            }
        }
    }

    let state = Arc::new(RwLock::new(DownloadState::default()));
    let _install_guard = InstallGuard::register(app_id, Arc::clone(&state))?;

    let manage_steam = steam_guard_stop_optional(restart_steam && is_dlc, json)?;

    let platform_name = format!("{platform:?}").to_lowercase();
    let started = backlog::now();
    let mut record = backlog::BacklogEntry {
        app_id,
        name: String::new(),
        state: backlog::BacklogState::Interrupted,
        platform: platform_name,
        library: library_root.clone(),
        install_dir: None,
        is_dlc,
        restart_steam,
        downloaded_bytes: on_disk,
        total_bytes: 0,
        reason: Some("install did not finish".into()),
        error_type: None,
        auto_attempts,
        added_at: started,
        updated_at: started,
    };
    note_backlog(backlog::upsert(record.clone()));

    let outcome = match client
        .install_game(
            app_id,
            platform,
            InstallOptions {
                cached_vdf,
                library_override: Some(library_root.clone()),
                ..Default::default()
            },
            Arc::clone(&state),
        )
        .await
    {
        Ok(rx) => {
            if let Ok(s) = state.read() {
                record.name = s.app_name.clone();
                record.install_dir = s.install_dir.clone();
                record.total_bytes = s.total_bytes;
            }
            note_backlog(backlog::upsert(record.clone()));
            match drive_progress_until(rx, json).await {
                Ok(true) => Ok(()),
                Ok(false) if stop_requested(&state) => Err(anyhow::anyhow!(TypedError::new(
                    ErrorKind::Cancelled,
                    "download stopped"
                ))),
                Ok(false) => bail!("install of app {app_id} ended without a result"),
                Err(err) => Err(err),
            }
        }
        Err(err) => Err(err).with_context(|| format!("failed to start install for app {app_id}")),
    };
    let settled = finish_backlog(app_id, record, &state, outcome);
    settle_with_restart(settled, || steam_guard_restart(manage_steam, json))?;
    let steam_restarted = manage_steam;

    let steam_restart_required = is_dlc && !manage_steam && SteamClient::steam_is_running();
    if steam_restart_required && !json {
        cli_eprintln!();
        cli_eprintln!(
            "Note: the DLC content is installed, but a running Steam client reads DLC state"
        );
        cli_eprintln!(
            "      only at startup. Restart Steam (or re-run with --restart-steam) for it to"
        );
        cli_eprintln!("      be recognized in-game.");
    }

    if json {
        print_result_line(serde_json::json!({
            "app_id": app_id,
            "status": "installed",
            "dlc": is_dlc,
            "steam_restart_required": steam_restart_required,
            "steam_restarted": steam_restarted,
        }));
    }
    Ok(())
}

fn note_backlog<T>(result: Result<T>) {
    if let Err(e) = result {
        tracing::warn!("install backlog not updated: {e:#}");
    }
}

fn stop_requested(state: &Arc<RwLock<DownloadState>>) -> bool {
    state
        .read()
        .map(|s| s.abort_signal.load(Ordering::SeqCst))
        .unwrap_or(false)
}

fn finish_backlog(
    app_id: u32,
    mut record: backlog::BacklogEntry,
    state: &Arc<RwLock<DownloadState>>,
    outcome: Result<()>,
) -> Result<()> {
    let Err(err) = outcome else {
        note_backlog(backlog::remove(app_id));
        return Ok(());
    };
    let paused = state.read().map(|s| s.is_paused).unwrap_or(false);
    if let Ok(s) = state.read() {
        record.name = s.app_name.clone();
        record.install_dir = s.install_dir.clone();
        record.downloaded_bytes = record.downloaded_bytes.max(s.downloaded_bytes);
        record.total_bytes = s.total_bytes;
    }
    let kind = classify(&err).kind;
    record.updated_at = backlog::now();
    let err = if kind == ErrorKind::Cancelled {
        record.reason = None;
        record.error_type = None;
        if paused {
            record.state = backlog::BacklogState::Paused;
            anyhow::anyhow!(TypedError::new(
                ErrorKind::Cancelled,
                format!(
                    "install of app {app_id} paused; continue with `aurelia install resume {app_id}`"
                ),
            ))
        } else {
            record.state = backlog::BacklogState::Aborted;
            err.context(format!(
                "install of app {app_id} stopped; it stays in the backlog"
            ))
        }
    } else {
        record.state = backlog::BacklogState::Interrupted;
        record.reason = Some(format!("{err:#}"));
        record.error_type = serde_json::to_value(kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string));
        err
    };
    note_backlog(backlog::upsert(record));
    Err(err)
}

pub(crate) async fn cmd_install_dry_run(
    client: &mut SteamClient,
    app_id: u32,
    platform: Option<PlatformArg>,
    json: bool,
) -> Result<()> {
    let platform: DepotPlatform = match platform {
        Some(p) => p.into(),
        None => {
            let (platforms, _) = client
                .get_available_platforms(app_id)
                .await
                .context("failed to detect available platforms")?;
            auto_select_platform(&platforms)
        }
    };

    let est = client
        .estimate_install_size(app_id, platform)
        .await
        .with_context(|| format!("failed to estimate install size for app {app_id}"))?;

    let platform_str = format!("{platform:?}").to_lowercase();
    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "platform": platform_str,
            "download_size": est.download_size,
            "disk_size": est.disk_size,
            "depot_count": est.depot_count,
        }));
    } else {
        cli_println!("Install estimate for app {app_id} ({platform_str}):");
        cli_println!("  Download size: {}", human_bytes(est.download_size));
        cli_println!("  Disk size    : {}", human_bytes(est.disk_size));
        cli_println!("  Depots       : {}", est.depot_count);
    }
    Ok(())
}

pub(crate) async fn cmd_install_stop(app_id: u32, json: bool) -> Result<()> {
    signal_install(app_id, false, json)
}

pub(crate) async fn cmd_install_pause(app_id: u32, json: bool) -> Result<()> {
    signal_install(app_id, true, json)
}

fn signal_install(app_id: u32, pause: bool, json: bool) -> Result<()> {
    let state = active_installs()
        .lock()
        .ok()
        .and_then(|map| map.get(&app_id).cloned());
    let target = if pause {
        backlog::BacklogState::Paused
    } else {
        backlog::BacklogState::Aborted
    };

    let Some(state) = state else {
        if backlog::set_state(app_id, target)? {
            if json {
                print_result_line(serde_json::json!({
                    "app_id": app_id,
                    "state": target.as_str(),
                }));
            } else {
                cli_println!(
                    "App {app_id} is not downloading; marked {} in the backlog.",
                    target.as_str()
                );
            }
            return Ok(());
        }
        if json {
            print_json(&serde_json::json!({
                "event": "not_found",
                "app_id": app_id,
            }));
        } else {
            cli_eprintln!(
                "no active install for app {app_id} (is the daemon running, and is an install in progress?)"
            );
        }
        return Ok(());
    };

    if let Ok(mut guard) = state.write() {
        guard.is_paused = pause;
        guard.abort_signal.store(true, Ordering::SeqCst);
    }

    if json {
        if pause {
            print_json(&serde_json::json!({
                "event": "pausing",
                "app_id": app_id,
            }));
        } else {
            print_json(&serde_json::json!({
                "event": "stopping",
                "app_id": app_id,
            }));
        }
    } else if pause {
        cli_println!("Pausing install of app {app_id} ...");
    } else {
        cli_println!("Stopping install of app {app_id} ...");
    }
    Ok(())
}

struct ListRow {
    app_id: u32,
    name: String,
    total: u64,
    done: u64,
    status: String,
    downloading: bool,
    state: &'static str,
    entry: Option<backlog::BacklogEntry>,
}

pub(crate) async fn cmd_install_list(json: bool) -> Result<()> {
    let mut rows: Vec<ListRow> = Vec::new();
    if let Ok(map) = active_installs().lock() {
        for state in map.values() {
            if let Ok(s) = state.read() {
                rows.push(ListRow {
                    app_id: s.app_id,
                    name: s.app_name.clone(),
                    total: s.total_bytes,
                    done: s.downloaded_bytes,
                    status: s.status_text.clone(),
                    downloading: s.is_downloading,
                    state: "running",
                    entry: None,
                });
            }
        }
    }
    rows.sort_by_key(|r| r.app_id);
    let running: Vec<u32> = rows.iter().map(|r| r.app_id).collect();
    for entry in backlog::load()? {
        if running.contains(&entry.app_id) {
            continue;
        }
        rows.push(ListRow {
            app_id: entry.app_id,
            name: entry.name.clone(),
            total: entry.total_bytes,
            done: entry.downloaded_bytes,
            status: entry.reason.clone().unwrap_or_default(),
            downloading: false,
            state: entry.state.as_str(),
            entry: Some(entry),
        });
    }

    if json {
        let arr: Vec<_> = rows
            .iter()
            .map(|r| {
                let mut v = serde_json::json!({
                    "app_id": r.app_id,
                    "name": r.name,
                    "downloaded_bytes": r.done,
                    "total_bytes": r.total,
                    "percent": percent_of(r.done, r.total),
                    "status": r.status,
                    "is_downloading": r.downloading,
                    "state": r.state,
                });
                if let Some(e) = &r.entry {
                    v["platform"] = e.platform.clone().into();
                    v["library"] = e.library.clone().into();
                    v["error_type"] = e.error_type.clone().into();
                    v["added_at"] = e.added_at.into();
                    v["updated_at"] = e.updated_at.into();
                }
                v
            })
            .collect();
        print_json(&serde_json::Value::Array(arr));
        return Ok(());
    }

    if rows.is_empty() {
        cli_println!("No installs in progress or in the backlog.");
        return Ok(());
    }

    cli_println!(
        "{:>9}  {:<11}  {:>20}  {:>6}  NAME / STATUS",
        "APPID",
        "STATE",
        "PROGRESS",
        ""
    );
    for r in &rows {
        let progress = format!("{} / {}", human_bytes(r.done), human_bytes(r.total));
        let label = match (r.name.is_empty(), r.status.is_empty()) {
            (false, true) => r.name.clone(),
            (true, _) => r.status.clone(),
            (false, false) if r.state == "running" => r.name.clone(),
            (false, false) => format!("{} ({})", r.name, r.status),
        };
        cli_println!(
            "{:>9}  {:<11}  {:>20}  {:>5.1}%  {}",
            r.app_id,
            r.state,
            progress,
            percent_of(r.done, r.total),
            label
        );
    }
    Ok(())
}

pub(crate) async fn cmd_install_resume(app_id: Option<u32>, all: bool, json: bool) -> Result<()> {
    let entries = backlog::load()?;
    let ids: Vec<u32> = if all {
        entries.iter().map(|e| e.app_id).collect()
    } else {
        let id = app_id.unwrap_or_default();
        if !entries.iter().any(|e| e.app_id == id) {
            return Err(TypedError::new(
                ErrorKind::NotFound,
                format!("app {id} is not in the install backlog (see `aurelia install list`)"),
            )
            .into());
        }
        vec![id]
    };
    if ids.is_empty() {
        if !json {
            cli_println!("The install backlog is empty.");
        }
        return Ok(());
    }
    resume_entries(&ids, false, json).await
}

pub(crate) async fn resume_entries(ids: &[u32], auto: bool, json: bool) -> Result<()> {
    let mut failures: Vec<(u32, anyhow::Error)> = Vec::new();
    for &app_id in ids {
        let Some(entry) = backlog::get(app_id)? else {
            continue;
        };
        if active_installs()
            .lock()
            .is_ok_and(|m| m.contains_key(&app_id))
        {
            continue;
        }
        if !json {
            let name = if entry.name.is_empty() {
                format!("app {app_id}")
            } else {
                entry.name.clone()
            };
            cli_println!("Resuming {name} ({}) ...", entry.state.as_str());
        }
        let auto_attempts = if auto {
            match backlog::bump_auto_attempts(app_id) {
                Ok(Some(n)) => n,
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!("install backlog not updated: {e:#}");
                    continue;
                }
            }
        } else {
            0
        };
        let request = InstallRequest {
            app_id,
            platform: Some(if entry.platform == "linux" {
                PlatformArg::Linux
            } else {
                PlatformArg::Windows
            }),
            restart_steam: entry.restart_steam,
            library: Some(entry.library.clone()),
            on_disk: entry.downloaded_bytes,
            auto_attempts,
        };
        if let Err(e) = run_install(request, json).await {
            if ids.len() == 1 {
                return Err(e);
            }
            let kind = classify(&e).kind;
            if json {
                print_result_line(serde_json::json!({
                    "app_id": app_id,
                    "ok": false,
                    "type": kind,
                    "error": format!("{e:#}"),
                }));
            } else {
                cli_eprintln!("App {app_id}: {e:#}");
            }
            failures.push((app_id, e));
        }
    }
    let Some((_, last)) = failures.last() else {
        return Ok(());
    };
    let kind = classify(last).kind;
    let summary = anyhow::anyhow!(TypedError::new(
        kind,
        format!("{} of {} resumes did not finish", failures.len(), ids.len()),
    ));
    if json {
        return Err(anyhow::anyhow!(aurelia::core::error::Reported(summary)));
    }
    Err(summary)
}

pub(crate) async fn cmd_install_forget(app_id: u32, delete_files: bool, json: bool) -> Result<()> {
    if active_installs()
        .lock()
        .is_ok_and(|m| m.contains_key(&app_id))
    {
        bail!("app {app_id} is downloading; stop it first with `aurelia install stop {app_id}`");
    }
    let Some(entry) = backlog::get(app_id)? else {
        return Err(TypedError::new(
            ErrorKind::NotFound,
            format!("app {app_id} is not in the install backlog"),
        )
        .into());
    };
    let mut deleted: Option<PathBuf> = None;
    if delete_files {
        let Some(dir) = backlog::deletable_dir(&entry) else {
            bail!(
                "won't delete files for app {app_id}: its folder is unknown, shared with a base \
                 game, or belongs to a finished install. Remove it by hand if needed."
            );
        };
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .with_context(|| format!("failed deleting {}", dir.display()))?;
        }
        deleted = Some(dir);
    }
    backlog::remove(app_id)?;
    if json {
        print_result_line(serde_json::json!({
            "app_id": app_id,
            "status": "forgotten",
            "deleted": deleted.as_ref().map(|d| d.to_string_lossy().into_owned()),
        }));
    } else {
        cli_println!("Removed app {app_id} from the install backlog.");
        if let Some(dir) = deleted {
            cli_println!("Deleted {}", dir.display());
        }
    }
    Ok(())
}

pub(crate) async fn cmd_uninstall(app_id: u32, delete_prefix: bool, json: bool) -> Result<()> {
    let client = authed_client().await?;
    client
        .uninstall_game(app_id, delete_prefix)
        .await
        .with_context(|| format!("failed to uninstall app {app_id}"))?;
    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "uninstalled": true,
            "deleted_prefix": delete_prefix,
        }));
    } else {
        cli_println!("Uninstalled app {app_id}.");
    }
    Ok(())
}

pub(crate) async fn cmd_move(
    app_id: u32,
    library: PathBuf,
    restart_steam: bool,
    json: bool,
) -> Result<()> {
    let client = authed_client().await?;

    let managed = steam_guard_stop(restart_steam, json)?;
    with_steam_stopped(managed, json, async {
        let rx = client
            .move_install(app_id, library.clone())
            .await
            .with_context(|| format!("failed to start moving app {app_id}"))?;
        drive_progress(rx, json)
            .await
            .with_context(|| format!("failed to move app {app_id}"))
    })
    .await?;

    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "status": "moved",
            "library": library.to_string_lossy(),
            "steam_restarted": managed,
        }));
    } else {
        cli_println!("Moved app {app_id} to {}.", library.display());
    }
    Ok(())
}

async fn steam_guarded_relocate<F, Fut>(
    app_id: u32,
    library: PathBuf,
    restart_steam: bool,
    json: bool,
    status: &'static str,
    verb: &'static str,
    human: impl FnOnce(&std::path::Path) -> String,
    op: F,
) -> Result<()>
where
    F: FnOnce(SteamClient, PathBuf) -> Fut,
    Fut: Future<Output = Result<std::path::PathBuf>>,
{
    let client = authed_client().await?;
    let managed = steam_guard_stop(restart_steam, json)?;
    let path = with_steam_stopped(managed, json, async {
        op(client, library.clone())
            .await
            .with_context(|| format!("failed to {verb} app {app_id}"))
    })
    .await?;

    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "status": status,
            "library": library.to_string_lossy(),
            "install_path": path.to_string_lossy(),
            "steam_restarted": managed,
        }));
    } else {
        cli_println!("{}", human(&path));
    }
    Ok(())
}

pub(crate) async fn cmd_relink(
    app_id: u32,
    library: PathBuf,
    restart_steam: bool,
    json: bool,
) -> Result<()> {
    let human = |_: &std::path::Path| format!("Relinked app {app_id} to {}.", library.display());
    steam_guarded_relocate(
        app_id,
        library.clone(),
        restart_steam,
        json,
        "relinked",
        "relink",
        human,
        |client, lib| async move { client.relink_install(app_id, lib).await },
    )
    .await
}

pub(crate) async fn cmd_import(
    app_id: u32,
    library: PathBuf,
    platform: Option<PlatformArg>,
    restart_steam: bool,
    json: bool,
) -> Result<()> {
    let platform: DepotPlatform =
        platform
            .map(Into::into)
            .unwrap_or(if cfg!(target_os = "windows") {
                DepotPlatform::Windows
            } else {
                DepotPlatform::Linux
            });
    let human = |path: &std::path::Path| format!("Imported app {app_id} from {}.", path.display());
    steam_guarded_relocate(
        app_id,
        library,
        restart_steam,
        json,
        "imported",
        "import",
        human,
        move |client, lib| async move { client.import_install(app_id, lib, platform).await },
    )
    .await
}

pub(crate) async fn cmd_available(app_id: u32, json: bool) -> Result<()> {
    let client = SteamClient::new()?;
    let (available, install_path) = client.is_game_available(app_id).await;
    let (pinned, pinned_manifests) = aurelia::core::config::game_pin_state(app_id).await;
    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "available": available,
            "install_path": install_path,
            "pinned": pinned,
            "pinned_manifests": manifests_json(&pinned_manifests),
        }));
    } else {
        cli_println!(
            "App {app_id}: {}{}",
            if available {
                "available"
            } else {
                "not available"
            },
            if pinned { " (pinned)" } else { "" }
        );
        if let Some(p) = install_path {
            cli_println!("  path: {p}");
        }
    }
    Ok(())
}

fn manifests_json(
    map: &std::collections::HashMap<u32, u64>,
) -> std::collections::BTreeMap<String, u64> {
    map.iter().map(|(d, m)| (d.to_string(), *m)).collect()
}

pub(crate) async fn cmd_verify(app_id: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let (_install_guard, state) = InstallGuard::begin(app_id)?;
    let rx = client
        .verify_game(app_id, state)
        .await
        .with_context(|| format!("failed to verify app {app_id}"))?;
    drive_progress(rx, json).await?;
    report_operation(app_id, "verified", json);
    Ok(())
}

pub(crate) async fn cmd_update(app_id: u32, force: bool, json: bool) -> Result<()> {
    let (pinned, _) = aurelia::core::config::game_pin_state(app_id).await;
    if pinned && !force {
        bail!("app {app_id} is pinned — run `aurelia unpin {app_id}` first (or pass --force)");
    }

    let client = authed_client().await?;
    let (_install_guard, state) = InstallGuard::begin(app_id)?;
    let rx = client
        .update_game(app_id, state)
        .await
        .with_context(|| format!("failed to update app {app_id}"))?;
    drive_progress(rx, json).await?;
    report_operation(app_id, "updated", json);
    Ok(())
}

/// How long `update <id> --check` waits for Steam. A launcher asks this right before opening a game: a connection that does not answer
/// must not hold the game back, so past this the answer is "no update known".
const UPDATE_CHECK_BUDGET: std::time::Duration = std::time::Duration::from_secs(4);

/// `aurelia update <APP_ID> --check`: does this one installed game have an update? Quick (the library comes from the cache and the installed
/// manifests, only the depot manifests of this game are asked of Steam) and nothing is downloaded or changed. Offline, or if Steam does not
/// answer in time, the answer is `update_available: false` with `checked: false`.
pub(crate) async fn cmd_update_check(app_id: u32, json: bool) -> Result<()> {
    let (pinned, _) = aurelia::core::config::game_pin_state(app_id).await;
    let mut client = match launch_access().await? {
        LaunchAccess::Online(c) | LaunchAccess::Offline(c) => c,
        LaunchAccess::NoAccount(_) => authed_client().await?,
    };
    let owned = aurelia::core::config::load_library_cache().await.unwrap_or_default();
    let installed = aurelia::library::scan_installed_app_info().await.unwrap_or_default();
    let mut games = aurelia::library::build_game_library(owned, installed, client.steam_id()).games;
    games.retain(|g| g.app_id == app_id && g.is_installed);
    let Some(game) = games.first_mut() else {
        return Err(TypedError::new(ErrorKind::NotFound, format!("app {app_id} is not installed")).into());
    };
    let mut checked = false;
    let mut update = false;
    if client.is_authenticated() && !client.is_offline() {
        match tokio::time::timeout(UPDATE_CHECK_BUDGET, client.check_for_updates(std::slice::from_mut(game))).await {
            Ok(Ok(())) => {
                checked = true;
                update = game.update_available;
            }
            Ok(Err(e)) => tracing::warn!("update check of {app_id} failed: {e:#}"),
            Err(_) => tracing::warn!("update check of {app_id} did not finish in {UPDATE_CHECK_BUDGET:?}"),
        }
    }
    let update = update && !pinned;
    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "name": game.name,
            "update_available": update,
            "pinned": pinned,
            "checked": checked,
        }));
    } else if update {
        cli_println!("{} has an update. Run `aurelia update {app_id}` to install it.", game.name);
    } else if pinned {
        cli_println!("{} is pinned.", game.name);
    } else if checked {
        cli_println!("{} is up to date.", game.name);
    } else {
        cli_println!("Could not check {} for updates right now.", game.name);
    }
    Ok(())
}

/// `aurelia update` with no app id: list every installed game whose active branch
/// has a newer build available than what's on disk.
pub(crate) async fn cmd_check_updates(json: bool) -> Result<()> {
    let mut client = authed_client().await?;
    if client.is_offline() {
        bail!("offline — connect to Steam to check for updates");
    }

    let mut games = load_library(&mut client).await;
    games.retain(|g| g.is_installed);
    if games.is_empty() {
        if json {
            print_json(&serde_json::json!({ "updates": [] }));
        } else {
            cli_println!("No installed games found.");
        }
        return Ok(());
    }

    tracing::info!("Checking {} installed game(s) for updates ...", games.len());
    client.check_for_updates(&mut games).await?;

    let cfg = load_launcher_config().await?;
    let is_pinned = |app_id: u32| cfg.game_configs.get(&app_id).is_some_and(|g| g.pinned);

    let mut updates: Vec<&LibraryGame> = games
        .iter()
        .filter(|g| g.update_available && !is_pinned(g.app_id))
        .collect();
    updates.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    let mut pinned: Vec<&LibraryGame> = games.iter().filter(|g| is_pinned(g.app_id)).collect();
    pinned.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    if json {
        let arr: Vec<_> = updates
            .iter()
            .map(|g| {
                serde_json::json!({
                    "app_id": g.app_id,
                    "name": g.name,
                    "active_branch": g.active_branch,
                })
            })
            .collect();
        let pinned_arr: Vec<_> = pinned
            .iter()
            .map(|g| serde_json::json!({ "app_id": g.app_id, "name": g.name }))
            .collect();
        print_json(&serde_json::json!({ "updates": arr, "pinned": pinned_arr }));
        return Ok(());
    }

    if updates.is_empty() {
        cli_println!("All installed games are up to date.");
    } else {
        cli_println!("{:>9}  NAME", "APPID");
        for g in &updates {
            let branch = branch_suffix(g);
            cli_println!("{:>9}  {}{}", g.app_id, g.name, branch);
        }
        cli_println!(
            "\n{} game(s) need an update. Run `aurelia update <app_id>` to install one.",
            updates.len()
        );
    }

    if !pinned.is_empty() {
        cli_println!("\nPinned (held at a fixed version; `aurelia unpin <app_id>` to release):");
        for g in &pinned {
            cli_println!("{:>9}  {}", g.app_id, g.name);
        }
    }
    Ok(())
}

fn parse_manifest_overrides(
    depots: &[u32],
    manifests: &[String],
) -> Result<std::collections::HashMap<u32, u64>> {
    use std::collections::HashMap;
    let mut map: HashMap<u32, u64> = HashMap::new();
    let mut bare: Vec<u64> = Vec::new();

    for m in manifests {
        if let Some((d, mm)) = m.split_once(':') {
            let depot: u32 = d
                .trim()
                .parse()
                .with_context(|| format!("invalid depot id in --manifest {m:?}"))?;
            let manifest: u64 = mm
                .trim()
                .parse()
                .with_context(|| format!("invalid manifest id in --manifest {m:?}"))?;
            if map.insert(depot, manifest).is_some() {
                bail!("depot {depot} specified more than once");
            }
        } else {
            let manifest: u64 = m.trim().parse().with_context(|| {
                format!("invalid manifest id {m:?} (use a number, or <depot>:<manifest>)")
            })?;
            bare.push(manifest);
        }
    }

    if !bare.is_empty() || !depots.is_empty() {
        if depots.len() != bare.len() {
            bail!(
                "--depot and --manifest must be given in equal numbers (got {} depot(s) and {} bare manifest(s)); \
                 pair each --depot with a --manifest, or use --manifest <depot>:<manifest>",
                depots.len(),
                bare.len()
            );
        }
        for (d, m) in depots.iter().zip(bare) {
            if map.insert(*d, m).is_some() {
                bail!("depot {d} specified more than once");
            }
        }
    }

    if map.is_empty() {
        bail!("provide at least one depot/manifest pair, e.g. --depot 1234 --manifest 5678");
    }
    Ok(map)
}

pub(crate) async fn cmd_manifests(app_id: u32, depot: Option<u32>, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let mut manifests = client
        .list_depot_manifests(app_id)
        .await
        .with_context(|| format!("failed listing manifests for app {app_id}"))?;
    if let Some(d) = depot {
        manifests.retain(|m| m.depot_id == d);
    }

    if json {
        let arr: Vec<_> = manifests
            .iter()
            .map(|m| {
                serde_json::json!({
                    "depot_id": m.depot_id,
                    "depot_name": m.depot_name,
                    "branch": m.branch,
                    "manifest_id": m.manifest_id,
                    "size": m.size,
                })
            })
            .collect();
        print_json(&serde_json::Value::Array(arr));
        return Ok(());
    }

    if manifests.is_empty() {
        cli_println!(
            "No depot manifests found for app {app_id}{}.",
            depot.map(|d| format!(" (depot {d})")).unwrap_or_default()
        );
        return Ok(());
    }

    cli_println!(
        "{:>9}  {:<14}  {:>20}  {:>12}  NAME",
        "DEPOT",
        "BRANCH",
        "MANIFEST_ID",
        "SIZE"
    );
    for m in &manifests {
        cli_println!(
            "{:>9}  {:<14}  {:>20}  {:>12}  {}",
            m.depot_id,
            m.branch,
            m.manifest_id,
            human_bytes(m.size),
            m.depot_name.as_deref().unwrap_or("")
        );
    }

    let mut depot_ids: Vec<u32> = manifests.iter().map(|m| m.depot_id).collect();
    depot_ids.sort_unstable();
    depot_ids.dedup();
    cli_println!("\nOnly current manifest ids are shown (Steam does not expose older ones).");
    cli_println!("Find historical manifest ids for a downgrade on SteamDB:");
    for d in depot_ids {
        cli_println!("  https://steamdb.info/depot/{d}/manifests/");
    }
    Ok(())
}

pub(crate) async fn cmd_downgrade(args: DowngradeArgs, json: bool) -> Result<()> {
    let overrides = parse_manifest_overrides(&args.depots, &args.manifests)?;

    if args.branch_password.is_some() && !json {
        cli_eprintln!(
            "Note: --branch-password is recorded for reference only; the manifest ids you supply are downloaded directly."
        );
    }

    let mut client = authed_client().await?;

    let (platforms, cached_vdf) = client
        .get_available_platforms(args.app_id)
        .await
        .context("failed to detect available platforms")?;
    let platform = platforms.first().copied().unwrap_or(DepotPlatform::Windows);

    if !json {
        cli_println!(
            "Downgrading app {} — pinning {} depot(s):",
            args.app_id,
            overrides.len()
        );
        let mut pairs: Vec<(&u32, &u64)> = overrides.iter().collect();
        pairs.sort();
        for (d, m) in pairs {
            cli_println!("  depot {d} -> manifest {m}");
        }
    }

    let state = Arc::new(RwLock::new(DownloadState::default()));
    let _install_guard = InstallGuard::register(args.app_id, Arc::clone(&state))?;
    let rx = client
        .install_game(
            args.app_id,
            platform,
            InstallOptions {
                cached_vdf: Some(cached_vdf),
                library_override: args.library.clone(),
                manifest_overrides: Some(overrides.clone()),
                branch: args.branch.clone(),
                ..Default::default()
            },
            Arc::clone(&state),
        )
        .await
        .with_context(|| format!("failed to start downgrade for app {}", args.app_id))?;
    drive_progress(rx, json).await?;

    if args.verify {
        if !json {
            cli_println!("Verifying ...");
        }
        let vstate = Arc::new(RwLock::new(DownloadState::default()));
        let vrx = client
            .verify_game(args.app_id, vstate)
            .await
            .with_context(|| format!("failed to verify app {}", args.app_id))?;
        drive_progress(vrx, json).await?;
    }

    let pinned = !args.no_pin;
    if pinned {
        aurelia::core::config::set_game_pin(args.app_id, overrides.clone())
            .await
            .with_context(|| {
                format!(
                    "downgrade succeeded but failed to record the pin for app {}",
                    args.app_id
                )
            })?;
    }

    if json {
        print_json(&serde_json::json!({
            "app_id": args.app_id,
            "status": "downgraded",
            "pinned": pinned,
            "manifests": manifests_json(&overrides),
        }));
    } else {
        cli_println!(
            "Downgraded app {}{}.",
            args.app_id,
            if pinned { " and pinned it" } else { "" }
        );
        if pinned {
            cli_println!(
                "Aurelia's `update` / `check-updates` will now hold it here. Note: launching"
            );
            cli_println!(
                "through the OFFICIAL Steam client can still re-queue an update (the pin is"
            );
            cli_println!("authoritative only for Aurelia's own commands).");
        }
    }
    Ok(())
}

pub(crate) async fn cmd_pin(app_id: u32, json: bool) -> Result<()> {
    let client = SteamClient::new()?;
    let manifests = client
        .installed_depot_manifests(app_id)
        .await
        .with_context(|| format!("failed reading installed manifests for app {app_id}"))?;
    if manifests.is_empty() {
        bail!("app {app_id} has no installed depots to pin (is it installed?)");
    }
    aurelia::core::config::set_game_pin(app_id, manifests.clone()).await?;

    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "pinned": true,
            "manifests": manifests_json(&manifests),
        }));
    } else {
        cli_println!(
            "Pinned app {app_id} to its installed manifests ({} depot(s)). Aurelia won't update it until `aurelia unpin {app_id}`.",
            manifests.len()
        );
    }
    Ok(())
}

pub(crate) async fn cmd_unpin(app_id: u32, json: bool) -> Result<()> {
    let (was_pinned, _) = aurelia::core::config::game_pin_state(app_id).await;
    aurelia::core::config::clear_game_pin(app_id).await?;

    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "pinned": false,
            "was_pinned": was_pinned,
        }));
    } else if was_pinned {
        cli_println!("Unpinned app {app_id}.");
    } else {
        cli_println!("App {app_id} was not pinned.");
    }
    Ok(())
}

#[cfg(test)]
#[path = "install_downgrade_tests.rs"]
mod downgrade_tests;

pub(crate) async fn cmd_duplicates(json: bool) -> Result<()> {
    let duplicates = aurelia::library::find_duplicate_installs()
        .await
        .context("could not scan Steam libraries for duplicate installs")?;

    if json {
        let arr: Vec<_> = duplicates
            .iter()
            .map(|d| {
                serde_json::json!({
                    "app_id": d.app_id,
                    "name": d.name,
                    "keep": {
                        "install_path": d.live.install_path.to_string_lossy(),
                        "manifest_path": d.live.manifest_path.to_string_lossy(),
                        "last_updated": d.live.last_updated,
                        "build_id": d.live.build_id,
                    },
                    "remove": d.stale.iter().map(|s| serde_json::json!({
                        "install_path": s.install_path.to_string_lossy(),
                        "manifest_path": s.manifest_path.to_string_lossy(),
                        "last_updated": s.last_updated,
                        "build_id": s.build_id,
                        "size_bytes": dir_size(&s.install_path),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        print_json(&serde_json::json!({ "duplicates": arr }));
        return Ok(());
    }

    if duplicates.is_empty() {
        cli_println!("No game is installed in more than one Steam library.");
        return Ok(());
    }

    cli_println!(
        "{} game(s) installed in more than one Steam library.\n\
         Each duplicate leaves two appmanifests disagreeing about what is installed,\n\
         which is why such a game can report an update on every check.\n",
        duplicates.len()
    );

    let mut reclaimable = 0u64;
    for d in &duplicates {
        let name = d
            .name
            .clone()
            .unwrap_or_else(|| format!("App {}", d.app_id));
        cli_println!("{name}  ({})", d.app_id);
        cli_println!(
            "  keep    {}  (updated {}, build {})",
            d.live.install_path.display(),
            format_unix_timestamp(d.live.last_updated),
            d.live.build_id
        );
        for s in &d.stale {
            let size = dir_size(&s.install_path);
            reclaimable += size;
            cli_println!(
                "  remove  {}  (updated {}, build {}, {})",
                s.install_path.display(),
                format_unix_timestamp(s.last_updated),
                s.build_id,
                human_bytes(size)
            );
        }
        cli_println!("");
    }

    cli_println!(
        "Removing every copy above frees {}.\n",
        human_bytes(reclaimable)
    );
    cli_println!("Close Steam first — it rewrites appmanifests on exit — then run:\n");
    for d in &duplicates {
        for s in &d.stale {
            cli_println!(
                "  rm -rf {}",
                shell_quote(&s.install_path.to_string_lossy())
            );
            cli_println!(
                "  rm -f  {}",
                shell_quote(&s.manifest_path.to_string_lossy())
            );
        }
    }
    cli_println!(
        "\nDelete the appmanifest as well as the directory: on its own the manifest\n\
         still advertises the copy, and discovery would keep finding it."
    );
    Ok(())
}

fn dir_size(path: &std::path::Path) -> u64 {
    walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}
