use crate::proc_admin;

use crate::commands::common::*;

use crate::cli::{PlatformArg, SteamModeArg};
use anyhow::{Context, Result, bail};
use aurelia::core::error::{ErrorKind, TypedError};
use aurelia::core::config::load_launcher_config;
use aurelia::core::config::load_user_configs;
use aurelia::core::models::DownloadState;
use aurelia::steam_client::SteamClient;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

pub(crate) struct PlayTarget {
    pub(crate) platform: Option<PlatformArg>,
    pub(crate) steam_mode: Option<SteamModeArg>,
    pub(crate) launch_option: Option<String>,
}

pub(crate) struct PlayWait {
    pub(crate) skip_shaders: bool,
    pub(crate) launch_timeout: Option<u64>,
}

const DEFAULT_LAUNCH_TIMEOUT_SECS: u64 = 300;

fn shader_reporter(json: bool, in_play: bool) -> aurelia::launch::shaders::ShaderProgressFn {
    use aurelia::launch::shaders::ShaderEvent;
    let last = std::sync::Mutex::new(None::<std::time::Instant>);
    Arc::new(move |event: ShaderEvent| {
        if let ShaderEvent::Progress { .. } = event {
            let mut last = last.lock().unwrap_or_else(|p| p.into_inner());
            let now = std::time::Instant::now();
            if last.is_some_and(|t| now.duration_since(t).as_millis() < 250) {
                return;
            }
            *last = Some(now);
        }
        if json {
            let value = match &event {
                ShaderEvent::Started { databases } => {
                    serde_json::json!({ "event": "shaders", "state": "started", "databases": databases })
                }
                ShaderEvent::Progress { done, total } => serde_json::json!({
                    "event": "shaders", "state": "processing", "done": done, "total": total,
                    "percent": percent_of(*done, *total),
                }),
                ShaderEvent::Finished { done, total } => serde_json::json!({
                    "event": "shaders", "state": "finished", "done": done, "total": total,
                }),
                ShaderEvent::Skipped(reason) => {
                    serde_json::json!({ "event": "shaders", "state": "skipped", "reason": reason })
                }
            };
            eprint_json_line(&value);
            return;
        }
        match event {
            ShaderEvent::Started { databases } => cli_eprintln!(
                "Processing Vulkan shaders ({databases} pipeline cache{}){}",
                if databases == 1 { "" } else { "s" },
                if in_play {
                    " — use --skip-shaders to skip"
                } else {
                    ""
                }
            ),
            ShaderEvent::Progress { done, total } => cli_eprint!(
                "\r  Processing Vulkan shaders: {done} / {total} ({:.1}%)   ",
                percent_of(done, total)
            ),
            ShaderEvent::Finished { .. } => cli_eprintln!("\r  Vulkan shaders processed.{:30}", ""),
            ShaderEvent::Skipped(reason) => tracing::info!("shader pre-caching skipped: {reason}"),
        }
    })
}

/// What Aurelia does before the game starts, as a line for the launcher (`{"event":"setup","text":...}`) or for the terminal.
fn setup_reporter(json: bool) -> Arc<dyn Fn(String) + Send + Sync> {
    Arc::new(move |text: String| {
        if json {
            cli_println!("{}", serde_json::json!({ "event": "setup", "text": text }));
        } else {
            cli_eprintln!("{text}");
        }
    })
}

/// `aurelia shaders <app_id>`: replay on demand.
pub(crate) async fn cmd_shaders(app_id: u32, force: bool, json: bool) -> Result<()> {
    use aurelia::launch::shaders::{self, ShaderEvent};
    if !cfg!(target_os = "linux") {
        bail!("Vulkan shader pre-caching is only needed on Linux");
    }
    let mut client = authed_client().await?;
    let game = find_game(&mut client, app_id).await?;
    let Some(install) = game.install_path.as_deref().filter(|_| game.is_installed) else {
        bail!(
            "{} is not installed — install it with `aurelia install {app_id}`",
            game.name
        );
    };
    let Some(dir) = shaders::shader_cache_dir(std::path::Path::new(install), app_id) else {
        bail!(
            "{} isn't in a Steam library, so it has no shader cache",
            game.name
        );
    };
    let env = shaders::cache_env(&dir)
        .into_iter()
        .map(|(k, v)| (k.to_string(), std::env::var(k).unwrap_or(v)))
        .collect();
    let reporter = shader_reporter(json, false);
    let outcome = shaders::precache(&dir, &env, force, Some(&reporter)).await?;
    let (status, done, total, reason) = match &outcome {
        ShaderEvent::Finished { done, total } => ("processed", *done, *total, None),
        ShaderEvent::Skipped(reason) => ("skipped", 0, 0, Some(reason.as_str())),
        _ => ("processed", 0, 0, None),
    };
    if json {
        print_result_line(serde_json::json!({
            "app_id": app_id,
            "status": status,
            "done": done,
            "total": total,
            "reason": reason,
            "cache": dir.to_string_lossy(),
        }));
    } else if let Some(reason) = reason {
        cli_println!("Nothing to do for {}: {reason}.", game.name);
    }
    Ok(())
}

fn apply_steam_mode(cfg: &mut aurelia::core::models::UserAppConfig, mode: SteamModeArg) {
    use aurelia::core::models::SteamRuntimePolicy;
    cfg.use_steam_runtime = false;
    cfg.steam_runtime_policy = match mode {
        SteamModeArg::Runtime => SteamRuntimePolicy::Enabled,
        SteamModeArg::Host | SteamModeArg::None => SteamRuntimePolicy::Disabled,
    };
}

pub(crate) async fn cmd_launch_targets(app_id: u32, steam: bool, json: bool) -> Result<()> {
    let mut client = match launch_access().await? {
        LaunchAccess::Online(c) | LaunchAccess::Offline(c) => c,
        LaunchAccess::NoAccount(_) => return authed_client().await.map(|_| ()),
    };
    let game = find_game(&mut client, app_id).await?;
    if !game.is_installed {
        bail!("{} is not installed", game.name);
    }
    let options = client
        .get_product_info(app_id)
        .await
        .with_context(|| format!("failed to load launch options for app {app_id}"))?;

    let launcher_config = load_launcher_config().await?;
    let game_cfg = launcher_config.game_configs.get(&app_id);
    let platform_preference = game_cfg.and_then(|c| c.platform_preference.as_deref());
    let prefer_windows = cfg!(target_os = "windows")
        || platform_preference == Some("windows")
        || game_cfg.is_some_and(|c| c.forced_proton_version.is_some());
    let prefer_native = platform_preference == Some("linux") && !cfg!(target_os = "windows");
    let default_entry =
        aurelia::steam_client::select_launch_entry(&options, &game, prefer_windows, prefer_native)
            .map(|e| e.target);

    let user_configs = load_user_configs().await?;
    let steam = steam || !game.is_owned;
    let (default_mode, _) = aurelia::infra::runners::resolve_steam_mode_for(
        user_configs.get(&app_id),
        &launcher_config,
        steam,
    );
    let mut targets = aurelia::steam_client::launch_targets(
        &options,
        &game,
        aurelia::core::utils::get_master_steam_config()
            .steam_exe
            .is_some(),
        aurelia::core::utils::host_steam_client_path().is_some(),
        steam,
        default_entry,
        default_mode,
    );
    if !aurelia::core::utils::PROTON_SUPPORTED {
        targets.retain(|t| t.id != "linux" && t.id != "windows-runtime");
        if !targets.iter().any(|t| t.default) {
            if let Some(first) = targets.iter_mut().find(|t| t.available) {
                first.default = true;
            }
        }
    }

    if json {
        let arr: Vec<_> = targets
            .iter()
            .map(|t| {
                serde_json::json!({
                    "id": t.id,
                    "platform": t.platform,
                    "steam_mode": t.steam_mode,
                    "flags": t.flags,
                    "available": t.available,
                    "missing": t.missing,
                    "default": t.default,
                })
            })
            .collect();
        print_json(&serde_json::json!({ "app_id": app_id, "targets": arr }));
        return Ok(());
    }

    if targets.is_empty() {
        cli_println!("{} has no Linux or Windows launch entries.", game.name);
        return Ok(());
    }
    cli_println!("Launch targets for {} (* = default):", game.name);
    for t in &targets {
        let mark = if t.default { "*" } else { " " };
        match t.missing {
            None => cli_println!(
                " {mark} {:<20} aurelia play {app_id} {}",
                t.id,
                t.flags.join(" ")
            ),
            Some(missing) => cli_println!(" {mark} {:<20} (needs {missing})", t.id),
        }
    }
    Ok(())
}

const UPDATE_CHECK_BUDGET: std::time::Duration = std::time::Duration::from_secs(3);
const DRM_PROBE_BUDGET: std::time::Duration = std::time::Duration::from_secs(3);

pub(crate) async fn cmd_play(
    app_id: u32,
    proton: Option<String>,
    windows: bool,
    native_engine: bool,
    umu: bool,
    script: Option<PathBuf>,
    no_script: bool,
    steam: bool,
    target: PlayTarget,
    wait: PlayWait,
    no_update: bool,
    json: bool,
) -> Result<()> {
    let linux_target = matches!(target.platform, Some(PlatformArg::Linux));
    if linux_target && proton.is_some() {
        bail!("--platform linux runs the native build; it can't be combined with --proton");
    }
    let steam = match target.steam_mode {
        Some(SteamModeArg::Host) => true,
        Some(SteamModeArg::None) => false,
        _ => steam,
    };
    if native_engine && !cfg!(target_os = "linux") {
        anyhow::bail!("--native-engine (luxtorpeda) is only available on Linux");
    }
    if umu && !cfg!(target_os = "linux") {
        anyhow::bail!("--umu (umu-launcher) is only available on Linux");
    }
    if proton.is_some() && !cfg!(target_os = "linux") {
        bail!("--proton is only available on Linux; on Windows games run natively, so omit it");
    }
    if linux_target && !cfg!(target_os = "linux") {
        bail!("--platform linux runs the native Linux build, which only runs on Linux");
    }
    if matches!(target.steam_mode, Some(SteamModeArg::Runtime))
        && !aurelia::core::utils::STEAM_RUNTIME_SUPPORTED
    {
        bail!(
            "--steam-mode runtime: {}",
            aurelia::core::utils::steam_runtime_unsupported_msg()
        );
    }
    if windows && !cfg!(target_os = "windows") {
        anyhow::bail!(
            "--windows runs the game with no Proton/Wine layer and only works on a Windows host; \
             on Linux omit it (optionally pass --proton <runner>) to run the game through Proton"
        );
    }
    let mut client = match launch_access().await? {
        LaunchAccess::Online(c) | LaunchAccess::Offline(c) => c,
        LaunchAccess::NoAccount(_) => {
            return Err(TypedError::new(
                ErrorKind::AuthRequired,
                "no Steam account is connected — sign in to play Steam games (games added by hand still start)",
            )
            .into());
        }
    };
    let mut game = find_game(&mut client, app_id).await?;

    if game.from_windows_steam {
        if target.launch_option.is_some() {
            tracing::warn!(
                "in-Wine Steam picks the launch option itself; ignoring --launch-option"
            );
        }
        let launcher_config = load_launcher_config().await?;
        let install_path = game.install_path.clone().unwrap_or_default();
        if !json {
            cli_println!(
                "⚠ {} lives in the in-Wine Steam runtime's own library — launching it \
                 through the in-Wine Steam (Wine). Aurelia's Proton/DXVK settings and \
                 session tracking don't apply to this launch.",
                game.name
            );
            cli_println!("Launching {} via the in-Wine Steam ...", game.name);
        }
        let status = |message: &str| {
            if json {
                eprint_json_line(&serde_json::json!({ "event": "message", "message": message }));
            } else {
                cli_println!("{message}");
            }
        };
        let timeout = wait.launch_timeout.unwrap_or(DEFAULT_LAUNCH_TIMEOUT_SECS);
        aurelia::launch::launch_game_via_master_steam(
            &launcher_config,
            app_id,
            std::path::Path::new(&install_path),
            std::time::Duration::from_secs(timeout),
            Some(&status),
        )
        .await
        .with_context(|| format!("failed to launch {} via the in-Wine Steam", game.name))?;
        if json {
            print_json(&serde_json::json!({
                "app_id": app_id,
                "name": game.name,
                "status": "finished",
                "runtime": "in-wine-steam",
            }));
        } else {
            cli_println!("Finished playing {}.", game.name);
        }
        return Ok(());
    }

    if !game.is_installed {
        bail!(
            "{} is not installed — install it with `aurelia install {app_id}`",
            game.name
        );
    }

    if let Some(id) = target.launch_option.as_deref() {
        use aurelia::steam_client::LaunchTarget;
        let required = match target.platform {
            Some(PlatformArg::Linux) => Some((LaunchTarget::NativeLinux, "--platform linux")),
            Some(PlatformArg::Windows) => Some((LaunchTarget::WindowsProton, "--platform windows")),
            None if proton.is_some() => Some((LaunchTarget::WindowsProton, "--proton")),
            None if cfg!(target_os = "windows") => {
                Some((LaunchTarget::WindowsProton, "a Windows host"))
            }
            None => None,
        };
        let options = client
            .get_product_info(app_id)
            .await
            .with_context(|| format!("failed to load launch options for app {app_id}"))?;
        let entry = aurelia::steam_client::find_launch_option(&options, id)?;
        aurelia::steam_client::check_launch_option_target(entry, required)?;
    }

    let update_required = game.is_installed && game.is_family_shared;

    if game.is_installed && (!no_update || update_required) {
        let checked = match tokio::time::timeout(UPDATE_CHECK_BUDGET, client.check_for_updates(std::slice::from_mut(&mut game))).await {
            Ok(result) => result,
            Err(_) if update_required => Err(anyhow::anyhow!("Steam did not answer in time")),
            Err(_) => {
                tracing::warn!("pre-launch update check did not finish in {UPDATE_CHECK_BUDGET:?}; launching current version");
                Ok(())
            }
        };
        if let Err(e) = checked {
            if update_required {
                bail!(
                    "could not verify the latest build for Family-Shared game {} ({e:#}); \
                     it must be up to date to launch",
                    game.name
                );
            }
            tracing::warn!("pre-launch update check failed ({e:#}); launching current version");
        }
        if game.update_available {
            if !json {
                let kind = if update_required {
                    "Required update"
                } else {
                    "Update available"
                };
                cli_println!("{kind} for {} — installing ...", game.name);
            }
            let state = Arc::new(RwLock::new(DownloadState::default()));
            let update_result = match InstallGuard::register(app_id, Arc::clone(&state)) {
                Ok(_guard) => match client.update_game(app_id, state).await {
                    Ok(rx) => drive_progress(rx, json).await,
                    Err(e) => Err(e),
                },
                Err(e) => Err(e),
            };
            if let Err(e) = update_result {
                if update_required {
                    bail!(
                        "required update for Family-Shared game {} failed ({e:#}); \
                         it can't launch until updated",
                        game.name
                    );
                }
                tracing::warn!("pre-launch update failed ({e:#}); launching current version");
            }
        }
    }

    if !steam && game.is_owned && !game.is_family_shared && !client.is_offline() {
        let probe = tokio::time::timeout(DRM_PROBE_BUDGET, client.get_app_ticket(app_id)).await;
        match probe {
            Ok(Ok(_)) => {
                tracing::info!(appid = app_id, "DRM preflight: ownership ticket issued");
            }
            Ok(Err(e)) => {
                let warning = format!(
                    "Steam issued no ownership ticket for {} ({e:#}). If the game uses \
                     Steam DRM it may not start standalone — try `aurelia play {app_id} --steam`.",
                    game.name
                );
                if json {
                    tracing::warn!(appid = app_id, "{warning}");
                } else {
                    cli_println!("⚠ {warning}");
                }
            }
            Err(_) => {
                tracing::warn!(appid = app_id, "DRM preflight timed out; launching anyway");
            }
        }
    }

    let force_windows = windows || cfg!(target_os = "windows");

    let launcher_config = load_launcher_config().await?;
    let game_cfg = launcher_config.game_configs.get(&app_id);
    let forced_proton = game_cfg
        .and_then(|c| c.forced_proton_version.clone())
        .filter(|_| !linux_target);
    let platform_preference = match target.platform {
        Some(PlatformArg::Linux) => Some("linux"),
        Some(PlatformArg::Windows) => Some("windows"),
        None => game_cfg.and_then(|c| c.platform_preference.as_deref()),
    };
    let prefers_windows = platform_preference == Some("windows");
    let prefers_linux = platform_preference == Some("linux") && !force_windows;

    let proton_path = proton
        .or(forced_proton)
        .or_else(|| prefers_windows.then(|| launcher_config.proton_version.clone()));

    let user_configs = load_user_configs().await?;
    let mut user_config = user_configs.get(&app_id).cloned();
    if let Some(mode) = target.steam_mode {
        apply_steam_mode(user_config.get_or_insert_with(Default::default), mode);
    }
    let user_config = user_config.as_ref();

    if !json {
        cli_println!("Launching {} ...", game.name);
    }
    let announce_target: &(dyn Fn(Option<&str>) + Send + Sync) =
        &|proton: Option<&str>| match proton {
            Some(p) => cli_println!("Platform: Windows via Proton ({p})"),
            None if cfg!(target_os = "windows") => cli_println!("Platform: Windows (native)"),
            None => cli_println!("Platform: native Linux"),
        };
    client
        .play_game(
            &game,
            proton_path.as_deref(),
            user_config,
            force_windows,
            native_engine,
            umu,
            script,
            no_script,
            steam,
            prefers_linux,
            target.launch_option.as_deref(),
            (!json).then_some(&announce_target),
            aurelia::launch::shaders::ShaderLaunch {
                skip: wait.skip_shaders,
                on_event: Some(shader_reporter(json, true)),
                on_setup: Some(setup_reporter(json)),
            },
        )
        .await
        .with_context(|| format!("failed to launch {}", game.name))?;
    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "name": game.name,
            "status": "finished",
        }));
    } else {
        cli_println!("Finished playing {}.", game.name);
    }
    Ok(())
}

pub(crate) fn cmd_running(json: bool) -> Result<()> {
    let running = aurelia::compat::running::list_active();
    if json {
        let arr: Vec<_> = running
            .iter()
            .map(|g| serde_json::json!({ "app_id": g.app_id, "name": g.name, "pid": g.pid }))
            .collect();
        print_json(&serde_json::json!({ "running": arr }));
        return Ok(());
    }
    if running.is_empty() {
        cli_println!("No games are running (none launched via `aurelia play`).");
    } else {
        cli_println!("{:>9}  {:>8}  NAME", "APPID", "PID");
        for g in &running {
            cli_println!("{:>9}  {:>8}  {}", g.app_id, g.pid, g.name);
        }
    }
    Ok(())
}

pub(crate) async fn cmd_stop(app_id: Option<u32>, force: bool, json: bool) -> Result<()> {
    let Some(app_id) = app_id else {
        return cmd_running(json);
    };

    let stopped = SteamClient::stop_game(app_id, force)
        .with_context(|| format!("failed to stop app {app_id}"))?;
    if json {
        print_json(&serde_json::json!({
            "app_id": stopped.app_id,
            "name": stopped.name,
            "status": "stopped",
            "forced": force,
        }));
    } else {
        let how = if force { " (forced)" } else { "" };
        cli_println!("Stopped {} (app {}){}.", stopped.name, stopped.app_id, how);
    }
    Ok(())
}

pub(crate) fn cmd_kill(json: bool) -> Result<()> {
    let procs = proc_admin::find_aurelia_processes();
    let pids: Vec<u32> = procs.iter().map(|p| p.pid).collect();
    let killed = proc_admin::kill_pids(&pids);

    if json {
        print_json(&serde_json::json!({ "found": pids.len(), "killed": killed, "pids": pids }));
    } else if pids.is_empty() {
        cli_println!("No other aurelia processes are running.");
    } else {
        cli_println!(
            "Killed {killed} of {} aurelia process(es) (including the daemon).",
            pids.len()
        );
    }
    Ok(())
}

pub(crate) fn cmd_daemon_stop(pid: Option<u32>, json: bool) -> Result<()> {
    let daemons: Vec<_> = proc_admin::find_aurelia_processes()
        .into_iter()
        .filter(|p| p.is_daemon)
        .collect();

    let targets: Vec<u32> = match pid {
        Some(pid) => {
            if !daemons.iter().any(|d| d.pid == pid) {
                bail!("PID {pid} is not a running aurelia daemon (see `aurelia daemon list`)");
            }
            vec![pid]
        }
        None => daemons.iter().map(|d| d.pid).collect(),
    };
    let killed = proc_admin::kill_pids(&targets);

    if json {
        print_json(&serde_json::json!({ "killed": killed, "pids": targets }));
    } else if targets.is_empty() {
        cli_println!("No aurelia daemon is running.");
    } else {
        cli_println!("Stopped {killed} aurelia daemon(s).");
    }
    Ok(())
}

pub(crate) async fn cmd_daemon_start(json: bool) -> Result<()> {
    let started = crate::daemon::client::start_background().await?;
    if json {
        print_json(&serde_json::json!({ "started": started }));
    } else if started {
        cli_println!("Started the aurelia daemon in the background.");
    } else {
        cli_println!("An aurelia daemon is already running.");
    }
    Ok(())
}

pub(crate) fn cmd_daemon_list(json: bool) -> Result<()> {
    let daemons: Vec<_> = proc_admin::find_aurelia_processes()
        .into_iter()
        .filter(|p| p.is_daemon)
        .collect();

    if json {
        let arr: Vec<_> = daemons
            .iter()
            .map(|d| serde_json::json!({ "pid": d.pid, "command": d.command }))
            .collect();
        print_json(&serde_json::json!({ "daemons": arr }));
        return Ok(());
    }

    if daemons.is_empty() {
        cli_println!("No aurelia daemon is running.");
        return Ok(());
    }
    cli_println!("{:>8}  COMMAND", "PID");
    for d in &daemons {
        cli_println!("{:>8}  {}", d.pid, d.command);
    }
    Ok(())
}
