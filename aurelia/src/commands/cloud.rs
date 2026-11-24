use crate::cli::*;
use crate::commands::common::*;

use anyhow::{Context, Result};
use std::path::PathBuf;

pub(crate) async fn cmd_cloud_sync(
    app_id: u32,
    up: bool,
    down: bool,
    path: Option<PathBuf>,
    resolve: Option<CloudResolve>,
    json: bool,
) -> Result<()> {
    use aurelia::library::cloud_sync::{ConflictPolicy, SyncDirection};

    let mut client = authed_client().await?;

    let direction = if up {
        SyncDirection::Up
    } else if down {
        SyncDirection::Down
    } else {
        SyncDirection::Both
    };
    let direction_str = match direction {
        SyncDirection::Up => "up",
        SyncDirection::Down => "down",
        SyncDirection::Both => "both",
    };

    let policy = resolve.map_or(ConflictPolicy::Detect, ConflictPolicy::from);

    let CloudSyncRun { outcome, remote_root, wine_prefix } = run_cloud_sync(&mut client, app_id, direction, policy, path).await?;

    let status = if outcome.has_failures() {
        "failed"
    } else if outcome.has_conflicts() {
        "conflicts"
    } else if outcome.has_skips() {
        "incomplete"
    } else {
        "ok"
    };

    if json {
        let conflicts: Vec<_> = outcome
            .conflicts
            .iter()
            .map(|c| {
                serde_json::json!({
                    "filename": c.filename,
                    "local_path": c.local_path,
                    "local_hash": c.local_hash,
                    "local_size": c.local_size,
                    "local_timestamp": c.local_timestamp,
                    "cloud_hash": c.cloud_hash,
                    "cloud_size": c.cloud_size,
                    "cloud_timestamp": c.cloud_timestamp,
                })
            })
            .collect();
        print_json(&serde_json::json!({
            "app_id": app_id,
            "direction": direction_str,
            "remote_root": remote_root.to_string_lossy(),
            "status": status,
            "downloaded": outcome.downloaded,
            "uploaded": outcome.uploaded,
            "conflicts": conflicts,
            "skipped": outcome.skipped,
            "skipped_root_tokens": outcome.skipped_tokens(),
            "failed": outcome.failed,
            "wine_prefix": wine_prefix.to_string_lossy(),
        }));
        return Ok(());
    }

    if outcome.has_conflicts() {
        cli_println!(
            "{} Cloud save(s) diverged from local — neither copy was changed:",
            outcome.conflicts.len()
        );
        for c in &outcome.conflicts {
            cli_println!(
                "  {}  (cloud {} bytes @ {}, local {} bytes @ {})",
                c.filename,
                c.cloud_size,
                c.cloud_timestamp,
                c.local_size,
                c.local_timestamp
            );
        }
        cli_println!(
            "\nResolve with: `aurelia cloud sync {app_id} --resolve cloud`  (use the Steam copy)\n            or `aurelia cloud sync {app_id} --resolve local`  (use the on-disk copy)"
        );
    } else {
        cli_println!(
            "Synced Cloud saves for app {app_id} ({direction_str}): {} down, {} up.",
            outcome.downloaded.len(),
            outcome.uploaded.len()
        );
    }

    cli_println!("Save prefix: {}", wine_prefix.display());

    if outcome.has_failures() {
        cli_println!(
            "\nERROR: {} Cloud file(s) failed to transfer — this save set is INCOMPLETE:",
            outcome.failed.len()
        );
        for f in outcome.failed.iter().take(SKIP_SAMPLE) {
            cli_println!("  [{}] {}: {}", f.direction, f.filename, f.error);
        }
        if outcome.failed.len() > SKIP_SAMPLE {
            cli_println!("  ... and {} more", outcome.failed.len() - SKIP_SAMPLE);
        }
        cli_println!(
            "\nRe-run `aurelia cloud sync {app_id} --down` to retry just the missing files.\nDo not play until it completes — the game may overwrite a half-restored save."
        );
    }

    if outcome.has_skips() {
        let tokens = outcome.skipped_tokens();
        cli_println!(
            "\nWARNING: {} Cloud file(s) were skipped — Aurelia could not work out where they belong on disk.",
            outcome.skipped.len()
        );
        if tokens.is_empty() {
            for s in outcome.skipped.iter().take(SKIP_SAMPLE) {
                cli_println!("  {}: {}", s.filename, s.reason);
            }
        } else {
            cli_println!(
                "Unmapped save root token(s): {}",
                tokens
                    .iter()
                    .map(|t| format!("%{t}%"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            for s in outcome.skipped.iter().take(SKIP_SAMPLE) {
                cli_println!("  {}", s.filename);
            }
        }
        if outcome.skipped.len() > SKIP_SAMPLE {
            cli_println!("  ... and {} more", outcome.skipped.len() - SKIP_SAMPLE);
        }
        if tokens.iter().any(|t| t.starts_with("Win")) {
            cli_println!(
                "\n%Win*% roots live inside the game's Proton prefix. Check that the game is\ninstalled and that its compatdata prefix exists (`aurelia info {app_id}`)."
            );
        } else {
            cli_println!(
                "\nThis token has no mapping in this build — please report it so it can be added."
            );
        }
        cli_println!("These saves are still safe in the Cloud; nothing was overwritten.");
    }
    Ok(())
}

pub(crate) struct CloudSyncRun {
    pub(crate) outcome: aurelia::library::cloud_sync::SyncOutcome,
    pub(crate) remote_root: PathBuf,
    pub(crate) wine_prefix: PathBuf,
}

pub(crate) async fn run_cloud_sync(
    client: &mut aurelia::steam_client::SteamClient,
    app_id: u32,
    direction: aurelia::library::cloud_sync::SyncDirection,
    policy: aurelia::library::cloud_sync::ConflictPolicy,
    path: Option<PathBuf>,
) -> Result<CloudSyncRun> {
    let t = std::time::Instant::now();
    let step = |name: &str| aurelia::bridge::log::bridge_log(&format!("cloud step {name}: {} ms (app {app_id})", t.elapsed().as_millis()));
    let cloud = client.cloud_client()?;

    let remote_root = match path {
        Some(p) => p,
        None => aurelia::library::cloud_sync::default_cloud_root(cloud.steam_id(), app_id)
            .context("could not resolve the local cloud save directory")?
            .join("remote"),
    };
    let (_, install_path) = client.is_game_available(app_id).await;
    step("is_game_available");
    let launcher_config = aurelia::core::config::LauncherConfig::load().await?;
    let user_configs = aurelia::core::config::load_user_configs().await?;
    let wine_prefix = aurelia::core::utils::game_save_prefix(
        &launcher_config,
        app_id,
        &user_configs,
        install_path.as_deref().map(std::path::Path::new),
    );
    let ufs = client.fetch_ufs_config_cached(app_id).await.unwrap_or_default();
    step("fetch_ufs_config");
    let target_os = cloud_target_os(client, app_id, &launcher_config).await;
    step("cloud_target_os");
    let resolver = aurelia::library::cloud_sync::CloudPathResolver::new(
        remote_root.clone(),
        install_path.map(PathBuf::from),
    )
    .with_wine_prefix((target_os == "windows").then(|| wine_prefix.clone()))
    .with_root_overrides(&ufs.root_overrides, target_os)
    .with_steam_id(cloud.steam_id());

    let outcome = cloud
        .sync(app_id, &resolver, &ufs.savefiles, direction, policy)
        .await
        .with_context(|| format!("cloud sync failed for app {app_id}"))?;
    step("sync");
    Ok(CloudSyncRun { outcome, remote_root, wine_prefix })
}

static TARGET_OS: std::sync::Mutex<Vec<((u32, bool, bool), std::time::Instant, &'static str)>> = std::sync::Mutex::new(Vec::new());
const TARGET_OS_TTL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

fn launch_preferences(app_id: u32, launcher_config: &aurelia::core::config::LauncherConfig) -> (bool, bool) {
    let game_cfg = launcher_config.game_configs.get(&app_id);
    let platform = game_cfg.and_then(|c| c.platform_preference.as_deref());
    let prefer_windows = platform == Some("windows") || game_cfg.is_some_and(|c| c.forced_proton_version.is_some());
    (prefer_windows, platform == Some("linux"))
}

pub(crate) async fn prewarm_cloud_os(client: &mut aurelia::steam_client::SteamClient, app_id: u32) {
    if let Ok(config) = aurelia::core::config::LauncherConfig::load().await {
        let _ = cloud_target_os(client, app_id, &config).await;
    }
}

async fn cloud_target_os(
    client: &mut aurelia::steam_client::SteamClient,
    app_id: u32,
    launcher_config: &aurelia::core::config::LauncherConfig,
) -> &'static str {
    if cfg!(target_os = "windows") {
        return "windows";
    }
    let (prefer_windows, prefer_native) = launch_preferences(app_id, launcher_config);
    let key = (app_id, prefer_windows, prefer_native);
    if let Some(os) = TARGET_OS.lock().expect("target os lock poisoned").iter().find(|(k, at, _)| *k == key && at.elapsed() < TARGET_OS_TTL).map(|(_, _, os)| *os) {
        return os;
    }
    let Ok(game) = find_game(client, app_id).await else {
        return "windows";
    };
    let Ok(options) = client.get_product_info(app_id).await else {
        return "windows";
    };
    let os = match aurelia::steam_client::select_launch_entry(&options, &game, prefer_windows, prefer_native).map(|e| e.target) {
        Some(aurelia::steam_client::LaunchTarget::NativeLinux) => "linux",
        _ => "windows",
    };
    let mut cache = TARGET_OS.lock().expect("target os lock poisoned");
    cache.retain(|(k, _, _)| *k != key);
    cache.push((key, std::time::Instant::now(), os));
    os
}

const SKIP_SAMPLE: usize = 5;

pub(crate) async fn cmd_cloud_list(app_id: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let cloud = client.cloud_client()?;
    let mut files = cloud
        .get_file_list(app_id)
        .await
        .with_context(|| format!("failed listing Cloud files for app {app_id}"))?;
    files.sort_by(|a, b| a.filename.cmp(&b.filename));

    if json {
        let arr: Vec<_> = files
            .iter()
            .map(|f| {
                serde_json::json!({
                    "filename": f.filename,
                    "size": f.size,
                    "timestamp": f.timestamp,
                    "sha_hash": f.sha_hash,
                })
            })
            .collect();
        print_json(&serde_json::json!({ "app_id": app_id, "files": arr }));
        return Ok(());
    }

    if files.is_empty() {
        cli_println!("No Steam Cloud files for app {app_id}.");
        return Ok(());
    }
    cli_println!("{:>12}  {:<19}  NAME", "SIZE", "MODIFIED");
    let mut total = 0u64;
    for f in &files {
        total += f.size;
        cli_println!(
            "{:>12}  {:<19}  {}",
            human_bytes(f.size),
            format_unix_timestamp(f.timestamp),
            f.filename
        );
    }
    cli_println!("\n{} file(s), {}.", files.len(), human_bytes(total));
    Ok(())
}
