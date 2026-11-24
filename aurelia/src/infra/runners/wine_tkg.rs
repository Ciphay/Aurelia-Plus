use crate::infra::runners::{CommandSpec, LaunchContext, Runner};
use crate::launch::pipeline::{LaunchError, LaunchErrorKind};
use crate::steam_client::SteamClient;
use anyhow::anyhow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct WineTkgRunner;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReadinessGrace {
    Ready,
    ExitedEarly { code: Option<i32> },
}

pub(crate) fn reclassify_after_grace(child: &mut std::process::Child) -> ReadinessGrace {
    match child.try_wait() {
        Ok(Some(status)) => ReadinessGrace::ExitedEarly {
            code: status.code(),
        },
        _ => ReadinessGrace::Ready,
    }
}

struct SteamLayout {
    library_root: PathBuf,
    steam_mode: SteamMode,
    steam_mode_source: &'static str,
    steam_prefix_mode: crate::core::models::SteamPrefixMode,
    compat_data_path: PathBuf,
    game_prefix: PathBuf,
}

impl SteamLayout {
    fn derive(ctx: &LaunchContext) -> Self {
        let library_root = PathBuf::from(&ctx.launcher_config.steam_library_path);
        let (steam_mode, steam_mode_source) = resolve_steam_mode(ctx);
        let steam_prefix_mode = ctx
            .user_config
            .as_ref()
            .map(|c| c.steam_prefix_mode.clone())
            .unwrap_or_else(|| ctx.launcher_config.steam_prefix_mode.clone());
        let master_wine_prefix = if steam_mode == SteamMode::InWineRuntime {
            crate::core::utils::get_master_steam_config().wine_prefix
        } else {
            PathBuf::new()
        };
        let game_library = crate::core::utils::game_library_root(
            &ctx.launcher_config,
            ctx.app.app_id,
            ctx.app.install_path.as_deref().map(std::path::Path::new),
        );
        let compat_data_path = game_compat_data_path(
            &game_library,
            ctx.app.app_id,
            steam_mode,
            steam_prefix_mode.clone(),
            &master_wine_prefix,
        );
        let game_prefix = effective_game_prefix(ctx);
        Self {
            library_root,
            steam_mode,
            steam_mode_source,
            steam_prefix_mode,
            compat_data_path,
            game_prefix,
        }
    }
}

#[async_trait::async_trait]
impl Runner for WineTkgRunner {
    fn name(&self) -> &str {
        "Wine-TKG"
    }
    async fn prepare_prefix(&self, ctx: &LaunchContext) -> std::result::Result<(), LaunchError> {
        let SteamLayout {
            library_root,
            steam_mode,
            steam_mode_source: runtime_source,
            steam_prefix_mode,
            compat_data_path,
            game_prefix: effective_game_prefix,
        } = SteamLayout::derive(ctx);
        let use_steam_runtime = steam_mode == SteamMode::InWineRuntime;
        std::fs::create_dir_all(&effective_game_prefix).map_err(|e| {
            LaunchError::new(
                LaunchErrorKind::Permission,
                format!("failed creating {}", effective_game_prefix.display()),
            )
            .with_source(anyhow!(e))
        })?;

        std::fs::create_dir_all(&compat_data_path).map_err(|e| {
            LaunchError::new(
                LaunchErrorKind::Permission,
                format!("failed creating {}", compat_data_path.display()),
            )
            .with_source(anyhow!(e))
        })?;

        tracing::info!("Effective game prefix: {}", effective_game_prefix.display());
        tracing::info!(
            "Shared steam compatibility data enabled: {}",
            ctx.launcher_config.use_shared_compat_data
        );
        tracing::info!("Steam Runtime Prefix Mode: {:?}", steam_prefix_mode);

        let mut allowed_runner_roots: Vec<PathBuf> = Vec::new();
        if let Ok(proton) = resolve_proton_required(ctx) {
            let runner = crate::core::utils::resolve_runner(proton, &library_root);
            allowed_runner_roots.push(crate::core::utils::derive_runner_root(&runner));
        }
        if use_steam_runtime {
            if let Ok(cmd) = resolve_background_steam_command(ctx, &library_root) {
                allowed_runner_roots.push(crate::core::utils::derive_runner_root(Path::new(
                    cmd.get_program(),
                )));
            }
        }

        if use_steam_runtime {
            let steam_cfg = crate::core::utils::get_master_steam_config();
            tracing::info!("Unified Master Steam resolution (Game Launch):");
            tracing::info!("  - Root Dir: {}", steam_cfg.root_dir.display());
            tracing::info!("  - Wine Prefix: {}", steam_cfg.wine_prefix.display());
            tracing::info!("  - Layout Kind: {}", steam_cfg.layout_kind);

            let master_steam_dir = match &steam_cfg.steam_exe {
                Some(exe) => exe.parent().unwrap().to_path_buf(),
                None => {
                    return Err(LaunchError::new(
                        LaunchErrorKind::Environment,
                        format!(
                            "use_steam_runtime is enabled but steam.exe was not found in {}.\n\
                             Go to Settings → 'Install / Manage Windows Steam Runtime' first.",
                            steam_cfg.wine_prefix.display()
                        ),
                    )
                    .with_context("master_prefix", steam_cfg.wine_prefix.to_string_lossy()));
                }
            };

            tracing::info!(
                "  - Steam Exe: {}",
                steam_cfg.steam_exe.as_ref().unwrap().display()
            );

            let (prefix_steam_dir, steam_wineprefix) = match steam_prefix_mode {
                crate::core::models::SteamPrefixMode::Shared => {
                    (master_steam_dir.clone(), steam_cfg.wine_prefix.clone())
                }
                crate::core::models::SteamPrefixMode::PerGame => {
                    let target_steam_dir =
                        effective_game_prefix.join("drive_c/Program Files (x86)/Steam");

                    tracing::info!(
                        "Deploying required Steam runtime files to {}",
                        target_steam_dir.display()
                    );
                    let _ = std::fs::create_dir_all(&target_steam_dir);

                    let required_files = [
                        "steam.exe",
                        "steamclient.dll",
                        "steamclient64.dll",
                        "tier0_s.dll",
                        "tier0_s64.dll",
                        "vstdlib_s.dll",
                        "vstdlib_s64.dll",
                    ];

                    for file in required_files {
                        let src = master_steam_dir.join(file);
                        let dst = target_steam_dir.join(file);
                        if src.exists() && !dst.exists() {
                            #[cfg(unix)]
                            {
                                if let Err(e) = std::os::unix::fs::symlink(&src, &dst) {
                                    tracing::warn!(
                                        "Symlink failed for {}, falling back to copy: {}",
                                        file,
                                        e
                                    );
                                    let _ = std::fs::copy(&src, &dst);
                                }
                            }
                            #[cfg(not(unix))]
                            {
                                let _ = std::fs::copy(&src, &dst);
                            }
                        }
                    }

                    let required_dirs = ["bin", "public"];
                    for dir in required_dirs {
                        let src = master_steam_dir.join(dir);
                        let dst = target_steam_dir.join(dir);
                        if src.exists() && !dst.exists() {
                            #[cfg(unix)]
                            {
                                if let Err(e) = std::os::unix::fs::symlink(&src, &dst) {
                                    tracing::warn!(
                                        "Symlink failed for {}, falling back to copy: {}",
                                        dir,
                                        e
                                    );
                                    let _ = crate::core::utils::copy_dir_all(&src, &dst);
                                }
                            }
                            #[cfg(not(unix))]
                            {
                                let _ = crate::core::utils::copy_dir_all(&src, &dst);
                            }
                        }
                    }

                    (target_steam_dir, effective_game_prefix.clone())
                }
            };

            tracing::debug!("Runtime Steam dir : {}", prefix_steam_dir.display());
            tracing::debug!("Runtime WINEPREFIX : {}", steam_wineprefix.display());

            SteamClient::write_headless_steam_cfg(&prefix_steam_dir);

            let slc = ctx
                .user_config
                .as_ref()
                .map(|c| c.steam_launch_config.clone())
                .unwrap_or_default();

            let mut steam_args = vec![
                "-silent".to_string(),
                "-tcp".to_string(),
                "-noverifyfiles".to_string(),
                "-noreactlogin".to_string(),
                "-cef-disable-gpu".to_string(),
                "-cef-disable-sandbox".to_string(),
            ];

            if slc.no_friends_ui {
                steam_args.push("-nofriendsui".to_string());
            }
            if slc.no_chat_ui {
                steam_args.push("-nochatui".to_string());
            }
            if slc.no_browser {
                steam_args.push("-no-browser".to_string());
            }
            if slc.no_overlay {
                steam_args.push("-disable-overlay".to_string());
            }
            if slc.no_vr {
                steam_args.push("-noopenvr".to_string());
            }
            if slc.big_picture {
                steam_args.push("-bigpicture".to_string());
            }

            let stale_servers = SteamClient::kill_stale_wineservers_in_prefix(
                &steam_wineprefix,
                &allowed_runner_roots,
            );
            if stale_servers > 0 {
                SteamClient::kill_steam_in_prefix(&steam_wineprefix);
            }

            let steam_running = SteamClient::is_steam_running_in_prefix(&steam_wineprefix);

            ctx.with_verification(|v| {
                v.steam_running_before_launch = steam_running;
                v.effective_game_wineprefix =
                    Some(effective_game_prefix.to_string_lossy().to_string());
                v.effective_steam_wineprefix = Some(steam_wineprefix.to_string_lossy().to_string());
                v.per_game_prefix_requested =
                    steam_prefix_mode == crate::core::models::SteamPrefixMode::PerGame;
                v.per_game_prefix_honored = effective_game_prefix == steam_wineprefix;
                v.steam_runtime_policy = format!(
                    "{:?}",
                    ctx.user_config
                        .as_ref()
                        .map(|c| &c.steam_runtime_policy)
                        .unwrap_or(&crate::core::models::SteamRuntimePolicy::Auto)
                );
                v.steam_runtime_source = runtime_source.to_string();
                v.windows_steam_discovery_enabled =
                    ctx.launcher_config.windows_steam_discovery_enabled;
            });

            if steam_running {
                tracing::info!("Steam already running in prefix — skipping spawn");
            } else {
                let mut steam_cmd = resolve_background_steam_command(ctx, &library_root)?;
                crate::core::utils::ensure_steam_runtime_prefix_libs(
                    &PathBuf::from(steam_cmd.get_program()),
                    &steam_wineprefix,
                );
                steam_cmd.current_dir(&prefix_steam_dir);
                steam_cmd
                    .arg("C:\\Program Files (x86)\\Steam\\steam.exe")
                    .args(&steam_args);
                steam_cmd
                    .env("WINEPREFIX", &steam_wineprefix)
                    .env(
                        "WINEDLLOVERRIDES",
                        "vstdlib_s=n,b;tier0_s=n,b;steamclient=n,b;steamclient64=n,b;\
                                 steam_api=n,b;steam_api64=n,b;lsteamclient=;\
                                 GameOverlayRenderer=n;GameOverlayRenderer64=n",
                    )
                    .env("WINEPATH", "C:\\Program Files (x86)\\Steam")
                    .env("STEAM_DISABLE_BROWSER", "1")
                    .env("STEAM_NO_BROWSER", "1")
                    .env("STEAMCMD", "1")
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());

                tracing::debug!(
                    program = ?steam_cmd.get_program(),
                    args = ?steam_cmd.get_args().collect::<Vec<_>>(),
                    "spawning background Steam",
                );

                ctx.with_verification(|v| {
                    v.steam_runtime_exe =
                        Some(steam_cmd.get_program().to_string_lossy().to_string());
                    v.steam_runtime_args = steam_cmd
                        .get_args()
                        .map(|a| a.to_string_lossy().to_string())
                        .collect();
                    v.steam_runtime_milestone = "steam_process_spawn_requested".to_string();
                    v.steam_auto_start_attempted = true;
                });

                let start_time = std::time::Instant::now();
                let mut steam_process = steam_cmd.spawn().map_err(|e| {
                    LaunchError::new(LaunchErrorKind::Process, "Failed to spawn background Steam")
                        .with_source(anyhow!(e))
                })?;

                ctx.with_verification(|v| {
                    v.steam_runtime_milestone = "steam_process_spawned".to_string();
                });

                let readiness_timeout = 8;
                tracing::info!(
                    "Waiting for Steam to initialise (max {}s)...",
                    readiness_timeout
                );

                let steam_pid_path = prefix_steam_dir.join("steam.pid");
                let steam_pipe = steam_wineprefix.join("drive_c/windows/temp/.steampath");
                let steam_config_vdf = prefix_steam_dir.join("config/config.vdf");
                let steam_logs_dir = prefix_steam_dir.join("logs");

                let ready = 'wait: {
                    let mut ready_signal: Option<String> = None;
                    for i in 0..readiness_timeout {
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

                        if let Ok(Some(status)) = steam_process.try_wait() {
                            tracing::error!(
                                "Background Steam exited after {}s with: {}",
                                i + 1,
                                status
                            );
                            ctx.with_verification(|v| {
                                v.steam_runtime_exit_code = status.code();
                                v.steam_runtime_lifetime_ms =
                                    Some(start_time.elapsed().as_millis() as u64);
                                v.steam_runtime_milestone =
                                    "steam_process_exited_early".to_string();
                            });
                            break 'wait false;
                        }

                        if steam_pid_path.exists() {
                            tracing::info!("Steam ready after {}s (steam.pid found)", i + 1);
                            ready_signal = Some(format!("steam.pid after {}s", i + 1));
                            break;
                        }

                        if steam_pipe.exists() {
                            tracing::info!("Steam ready after {}s (.steampath found)", i + 1);
                            ready_signal = Some(format!(".steampath after {}s", i + 1));
                            break;
                        }

                        if steam_config_vdf.exists() {
                            tracing::info!("Steam ready after {}s (config.vdf found)", i + 1);
                            ready_signal = Some(format!("config.vdf after {}s", i + 1));
                            break;
                        }

                        let log_count = std::fs::read_dir(&steam_logs_dir)
                            .map(|d| d.count())
                            .unwrap_or(0);
                        if log_count >= 2 {
                            tracing::info!(
                                "Steam ready after {}s ({} log files found)",
                                i + 1,
                                log_count
                            );
                            ctx.with_verification(|v| {
                                v.steam_runtime_milestone =
                                    "steam_ready_signal_observed".to_string();
                            });
                            ready_signal =
                                Some(format!("{} log files after {}s", log_count, i + 1));
                            break;
                        }

                        tracing::debug!("Waiting... {}s", i + 1);
                    }

                    if let Some(reason) = ready_signal {
                        const READINESS_GRACE_SECS: u64 = 2;
                        tokio::time::sleep(std::time::Duration::from_secs(READINESS_GRACE_SECS))
                            .await;
                        match reclassify_after_grace(&mut steam_process) {
                            ReadinessGrace::ExitedEarly { code } => {
                                tracing::error!(
                                    "Background Steam signalled ready ({}) but exited within {}s grace (code {:?})",
                                    reason,
                                    READINESS_GRACE_SECS,
                                    code
                                );
                                ctx.with_verification(|v| {
                                    v.steam_runtime_exit_code = code;
                                    v.steam_runtime_lifetime_ms =
                                        Some(start_time.elapsed().as_millis() as u64);
                                    v.steam_runtime_milestone =
                                        "steam_process_exited_early".to_string();
                                });
                                break 'wait false;
                            }
                            ReadinessGrace::Ready => break 'wait true,
                        }
                    }

                    tracing::warn!(
                        "Steam did not signal ready after {}s, launching game anyway",
                        readiness_timeout
                    );
                    ctx.with_verification(|v| {
                        v.steam_runtime_milestone = "steam_ready_timeout".to_string();
                    });
                    true
                };

                if !ready {
                    ctx.with_verification(|v| {
                        v.steam_auto_start_failed = true;
                    });
                    return Err(LaunchError::new(
                        LaunchErrorKind::Process,
                        "Background Steam crashed before the game could start",
                    ));
                }
            }

            let slc = ctx
                .user_config
                .as_ref()
                .map(|c| c.steam_launch_config.clone())
                .unwrap_or_default();
            if slc.no_browser || slc.no_friends_ui || slc.no_overlay || slc.no_chat_ui {
                let prefix = steam_wineprefix.clone();
                let (no_browser, no_friends_ui, no_overlay, no_chat_ui) = (
                    slc.no_browser,
                    slc.no_friends_ui,
                    slc.no_overlay,
                    slc.no_chat_ui,
                );
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(8));
                    SteamClient::enforce_disabled_steam_features_in_prefix(
                        &prefix,
                        no_browser,
                        no_friends_ui,
                        no_overlay,
                        no_chat_ui,
                    );
                });
            }
        }

        SteamClient::kill_stale_wineservers_in_prefix(
            &effective_game_prefix,
            &allowed_runner_roots,
        );

        let (_install_dir, _executable, game_working_dir) = ctx.game_paths()?;

        crate::core::utils::ensure_steam_appid_logged(&game_working_dir, ctx.app.app_id);

        Ok(())
    }

    async fn build_env(
        &self,
        ctx: &LaunchContext,
    ) -> std::result::Result<HashMap<String, String>, LaunchError> {
        let mut env = HashMap::new();
        let app_id_str = ctx.app.app_id.to_string();

        let layout = SteamLayout::derive(ctx);
        let SteamLayout {
            ref library_root,
            steam_mode,
            ref compat_data_path,
            game_prefix: ref effective_game_prefix,
            ..
        } = layout;

        env.insert("SteamAppId".to_string(), app_id_str.clone());
        env.insert("SteamGameId".to_string(), app_id_str.clone());
        env.insert("STEAM_COMPAT_APP_ID".to_string(), app_id_str);
        env.insert(
            "WINEPREFIX".to_string(),
            effective_game_prefix.to_string_lossy().to_string(),
        );
        env.insert(
            "STEAM_COMPAT_DATA_PATH".to_string(),
            compat_data_path.to_string_lossy().to_string(),
        );
        let (compat_install_dir, _, _) = ctx.game_paths()?;
        let compat_library = crate::core::utils::game_library_root(
            &ctx.launcher_config,
            ctx.app.app_id,
            ctx.app.install_path.as_deref().map(std::path::Path::new),
        );
        env.insert(
            "STEAM_COMPAT_INSTALL_PATH".to_string(),
            compat_install_dir.to_string_lossy().to_string(),
        );
        env.insert(
            "STEAM_COMPAT_LIBRARY_PATHS".to_string(),
            compat_library.to_string_lossy().to_string(),
        );

        if let Ok(session) = crate::core::config::load_session().await {
            if let Some(steam_id) = session.steam_id {
                env.insert("SteamUser".to_string(), steam_id.to_string());
            }
            if let Some(account_name) = session.account_name {
                env.insert("SteamAppUser".to_string(), account_name);
            }
        }

        let glc = ctx
            .user_config
            .as_ref()
            .map(|c| c.graphics_layers.clone())
            .unwrap_or_default();
        let no_overlay = ctx
            .user_config
            .as_ref()
            .map(|c| c.steam_launch_config.no_overlay)
            .unwrap_or(true);

        let (_install_dir, _executable, game_working_dir) = ctx.game_paths()?;

        let proton = forced_proton(ctx)
            .or_else(|| ctx.proton_path.as_deref().filter(|p| !p.is_empty()))
            .unwrap_or("wine");

        let active_runner_path = crate::core::utils::resolve_runner(proton, &library_root);
        let _components = crate::core::utils::detect_runner_components(
            &active_runner_path,
            Some(&effective_game_prefix),
        );

        let (policy_dxvk, force_builtin, strict_dxvk) = match glc.graphics_backend_policy {
            crate::core::models::GraphicsBackendPolicy::Auto => (false, false, false),
            crate::core::models::GraphicsBackendPolicy::WineD3D => (false, true, false),
            crate::core::models::GraphicsBackendPolicy::DXVK => (true, false, true),
        };

        let effective_dxvk = glc.dxvk_enabled || policy_dxvk;

        let force_builtin_d3d = force_builtin && !effective_dxvk;

        let (policy_vkd3dp, policy_vkd3dw) = match glc.d3d12_policy {
            crate::core::models::D3D12ProviderPolicy::Auto => (false, false),
            crate::core::models::D3D12ProviderPolicy::Vkd3dProton => (true, false),
            crate::core::models::D3D12ProviderPolicy::Vkd3dWine => (false, true),
        };
        let effective_vkd3d_proton = glc.vkd3d_proton_enabled || policy_vkd3dp;
        let effective_vkd3d = glc.vkd3d_enabled || policy_vkd3dw;

        let nvapi_enabled_cfg = ctx
            .user_config
            .as_ref()
            .map(|c| c.graphics_layers.nvapi_enabled)
            .unwrap_or(true);
        let nvapi_active = _components.nvapi.is_some() && nvapi_enabled_cfg;
        if nvapi_active {
            tracing::info!("NVAPI component detected and enabled, will be exposed to game");
        } else if _components.nvapi.is_some() {
            tracing::info!("NVAPI component detected but disabled by per-game settings");
        }

        let host_bridge = steam_mode == SteamMode::HostBridge;

        let use_symlinks = glc.use_symlinks_in_prefix;
        let mut dll_overrides = crate::core::utils::build_dll_overrides(
            effective_dxvk,
            effective_vkd3d_proton,
            effective_vkd3d,
            no_overlay,
            force_builtin_d3d,
            Some(&game_working_dir),
            strict_dxvk,
            host_bridge,
        );

        for res in &ctx.dll_resolutions {
            if res.chosen_provider == crate::launch::dll_provider_resolver::DllProvider::GameLocal
                || (res.chosen_provider
                    == crate::launch::dll_provider_resolver::DllProvider::Custom
                    && !use_symlinks)
                || (res.chosen_provider
                    == crate::launch::dll_provider_resolver::DllProvider::Runner
                    && res.name.contains("nvapi"))
            {
                let already_native =
                    crate::core::utils::dll_override_mode(&dll_overrides, &res.name)
                        .is_some_and(|m| m.starts_with('n'));
                if !already_native {
                    tracing::info!(
                        "Adding native override for resolved DLL: {} (provider: {:?})",
                        res.name,
                        res.chosen_provider
                    );
                    dll_overrides = crate::core::utils::merge_dll_overrides(
                        &dll_overrides,
                        &[(res.name.clone(), "n".to_string())],
                    );
                }
            } else if res.chosen_provider
                == crate::launch::dll_provider_resolver::DllProvider::Internal
            {
                tracing::info!(
                    "Resolved DLL {} is handled internally (alias), skipping explicit override",
                    res.name
                );
            }
        }

        if !ctx.game_fixups.dll_overrides.is_empty() {
            let existing_names = crate::core::utils::dll_override_names(&dll_overrides);
            for (dll, mode) in &ctx.game_fixups.dll_overrides {
                let key = crate::core::utils::normalize_dll_name(dll);
                if existing_names.contains(&key) {
                    tracing::info!(
                        "Skipping fixup DLL override {}={} (explicit override already present)",
                        dll,
                        mode
                    );
                    continue;
                }
                tracing::info!("Applying fixup DLL override: {}={}", dll, mode);
                dll_overrides = crate::core::utils::merge_dll_overrides(
                    &dll_overrides,
                    &[(dll.clone(), mode.clone())],
                );
            }
        }

        tracing::info!("Final WINEDLLOVERRIDES: {}", dll_overrides);
        env.insert("WINEDLLOVERRIDES".to_string(), dll_overrides);

        let mut wine_dll_dirs: Vec<String> = Vec::new();

        for res in &ctx.dll_resolutions {
            if (res.chosen_provider == crate::launch::dll_provider_resolver::DllProvider::Runner
                || res.chosen_provider == crate::launch::dll_provider_resolver::DllProvider::Custom)
                && !use_symlinks
            {
                let name = res.name.to_lowercase();
                let is_dxvk_dll = matches!(
                    name.as_str(),
                    "d3d8" | "d3d9" | "d3d10" | "d3d10_1" | "d3d10core" | "d3d11" | "dxgi"
                );
                let is_d3d12_dll = matches!(
                    name.as_str(),
                    "d3d12" | "d3d12core" | "libvkd3d-1" | "libvkd3d-shader-1"
                );

                let is_nvapi_dll = matches!(name.as_str(), "nvapi" | "nvapi64" | "nvofapi64");
                let selected = (is_dxvk_dll && effective_dxvk)
                    || (is_d3d12_dll && (effective_vkd3d_proton || effective_vkd3d))
                    || is_nvapi_dll;

                if !selected {
                    continue;
                }

                if let Some(path) = &res.chosen_path {
                    if let Some(parent) = path.parent() {
                        push_dll_dir_once(&mut wine_dll_dirs, parent);

                        let folder_name = parent.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        let sibling_arch = match folder_name {
                            "x86_64-windows" => Some("i386-windows"),
                            "i386-windows" => Some("x86_64-windows"),
                            _ => None,
                        };
                        if let Some(arch) = sibling_arch {
                            let sibling = parent.parent().unwrap().join(arch);
                            if sibling.exists() {
                                push_dll_dir_once(&mut wine_dll_dirs, &sibling);
                            }
                        }
                    }
                }
            }
        }

        let active_runner = crate::core::utils::resolve_runner(proton, &library_root);
        let runner_root = crate::core::utils::derive_runner_root(&active_runner);
        for lib_sub in crate::compat::proton::UNIFIED_LIB_SUBDIRS {
            let p = runner_root.join(lib_sub);
            if p.exists() {
                push_dll_dir_once(&mut wine_dll_dirs, &p);

                for arch in crate::compat::proton::ARCH_SUBDIRS {
                    let arch_p = p.join(arch);
                    if arch_p.exists() {
                        push_dll_dir_once(&mut wine_dll_dirs, &arch_p);
                    }
                }
            }
        }

        if !wine_dll_dirs.is_empty() {
            let existing = env.get("WINEDLLPATH").cloned().unwrap_or_default();
            let combined = if existing.is_empty() {
                wine_dll_dirs.join(":")
            } else {
                format!("{}:{}", wine_dll_dirs.join(":"), existing)
            };
            env.insert("WINEDLLPATH".to_string(), combined);
        }

        let mut wine_path = vec!["C:\\Program Files (x86)\\Steam".to_string()];
        wine_path.extend(wine_dll_dirs.iter().cloned());
        env.insert("WINEPATH".to_string(), wine_path.join(";"));

        let fake_trap =
            |env: &mut HashMap<String, String>| -> std::result::Result<(), LaunchError> {
                let fake_env = crate::infra::runners::insert_fake_steam_trap(env)?;
                ctx.with_verification(|v| {
                    v.steam_client_install_path_exposed_to_game =
                        Some(fake_env.to_string_lossy().to_string());
                    v.steam_client_install_path_source = Some("fake_trap".to_string());
                });
                Ok(())
            };
        let expose = |env: &mut HashMap<String, String>, path: PathBuf, source: &str| {
            env.insert(
                "STEAM_COMPAT_CLIENT_INSTALL_PATH".to_string(),
                path.to_string_lossy().to_string(),
            );
            ctx.with_verification(|v| {
                v.steam_client_install_path_exposed_to_game =
                    Some(path.to_string_lossy().to_string());
                v.steam_client_install_path_source = Some(source.to_string());
            });
        };

        match steam_mode {
            SteamMode::HostBridge => match crate::core::utils::host_steam_client_path() {
                Some(path) => expose(&mut env, path, "real_host"),
                None => fake_trap(&mut env)?,
            },
            SteamMode::InWineRuntime => {
                let steam_cfg = crate::core::utils::get_master_steam_config();
                let steam_client_path = match layout.steam_prefix_mode.clone() {
                    crate::core::models::SteamPrefixMode::Shared => steam_cfg
                        .steam_exe
                        .as_ref()
                        .and_then(|e| e.parent().map(|p| p.to_path_buf())),
                    crate::core::models::SteamPrefixMode::PerGame => {
                        Some(effective_game_prefix.join("drive_c/Program Files (x86)/Steam"))
                    }
                };

                match steam_client_path {
                    Some(path) => expose(&mut env, path, "real"),
                    None => fake_trap(&mut env)?,
                }
            }
            SteamMode::Standalone => {
                if ctx.steam_enabled {
                    tracing::warn!(
                        "--steam was requested but neither a host Steam client nor the in-Wine \
                         Steam runtime is installed; launching standalone. DRM-protected games \
                         will not run. Install the runtime with `aurelia steam-runtime install`."
                    );
                }
                fake_trap(&mut env)?;
            }
        }

        if let Ok(display) = std::env::var("DISPLAY") {
            env.insert("DISPLAY".to_string(), display);
        }
        if let Ok(wayland) = std::env::var("WAYLAND_DISPLAY") {
            env.insert("WAYLAND_DISPLAY".to_string(), wayland);
        }
        if let Ok(xdg_runtime) = std::env::var("XDG_RUNTIME_DIR") {
            env.insert("XDG_RUNTIME_DIR".to_string(), xdg_runtime);
        }
        if let Ok(xauthority) = std::env::var("XAUTHORITY") {
            env.insert("XAUTHORITY".to_string(), xauthority);
        }

        if let Some(gpu_pref) = ctx
            .user_config
            .as_ref()
            .and_then(|c| c.gpu_preference.as_ref())
        {
            let available_gpus = crate::core::utils::list_available_gpus();
            if let Some(gpu) = available_gpus.iter().find(|g| &g.name == gpu_pref) {
                if gpu.name.contains("NVIDIA") {
                    env.insert("__NV_PRIME_RENDER_OFFLOAD".to_string(), "1".to_string());
                    env.insert(
                        "__NV_PRIME_RENDER_OFFLOAD_PROVIDER".to_string(),
                        "NVIDIA-G0".to_string(),
                    );
                    env.insert(
                        "__VK_LAYER_NV_optimus".to_string(),
                        "NVIDIA_only".to_string(),
                    );
                    env.insert(
                        "__GLX_VENDOR_LIBRARY_NAME".to_string(),
                        "nvidia".to_string(),
                    );
                } else if gpu.name.contains("AMD")
                    || gpu.name.contains("Intel")
                    || gpu.name.contains("Unknown")
                {
                    static CARD_RE: std::sync::LazyLock<regex::Regex> =
                        std::sync::LazyLock::new(|| regex::Regex::new(r"card(\d+)").unwrap());
                    if let Some(card_idx) = CARD_RE
                        .captures(&gpu.name)
                        .and_then(|caps| caps.get(1))
                        .and_then(|m| m.as_str().parse::<u32>().ok())
                    {
                        let dri_prime = if card_idx > 0 { "1" } else { "0" };
                        env.insert("DRI_PRIME".to_string(), dri_prime.to_string());
                    }
                }
            }
        }

        if !ctx.game_fixups.env.is_empty() {
            let user_has = |key: &str| {
                ctx.user_config
                    .as_ref()
                    .is_some_and(|c| c.env_variables.contains_key(key))
            };
            for (key, val) in &ctx.game_fixups.env {
                if key == "WINEDLLOVERRIDES" {
                    let computed = env.get(key).cloned().unwrap_or_default();
                    let merged = crate::core::utils::fill_dll_overrides(
                        &computed,
                        &crate::core::utils::dll_override_entries(val),
                    );
                    tracing::info!("Applying fixup DLL overrides: {}", merged);
                    env.insert(key.clone(), merged);
                    continue;
                }
                if user_has(key) {
                    tracing::info!(
                        "Skipping fixup env {} (explicit per-game value present)",
                        key
                    );
                    continue;
                }
                tracing::info!("Applying fixup env: {}={}", key, val);
                env.insert(key.clone(), val.clone());
            }
        }

        if let Some(config) = &ctx.user_config {
            for (key, val) in &config.env_variables {
                if key == "WINEDLLOVERRIDES" {
                    let computed = env.get(key).cloned().unwrap_or_default();
                    let merged = crate::core::utils::merge_dll_overrides(
                        &computed,
                        &crate::core::utils::dll_override_entries(val),
                    );
                    tracing::info!("Final WINEDLLOVERRIDES (with per-game env): {}", merged);
                    env.insert(key.clone(), merged);
                    continue;
                }
                env.insert(key.clone(), val.clone());
            }

            if effective_dxvk && !env.contains_key("DXVK_HUD") {
                env.insert("DXVK_HUD".to_string(), "compiler".to_string());
            }
            if (effective_vkd3d_proton || effective_vkd3d) && !env.contains_key("VKD3D_DEBUG") {
                env.insert("VKD3D_DEBUG".to_string(), "warn".to_string());
            }
        }

        let wants_mangohud = ctx
            .user_config
            .as_ref()
            .map(|c| {
                c.env_variables.contains_key("MANGOHUD")
                    || c.launch_options
                        .split_whitespace()
                        .any(|a| a == "-mangohud" || a == "--mangohud")
            })
            .unwrap_or(false);

        if wants_mangohud {
            match SteamClient::find_mangohud_lib() {
                Some(lib) => {
                    let existing = std::env::var("LD_PRELOAD").unwrap_or_default();
                    let new_preload = if existing.is_empty() {
                        lib.to_string_lossy().to_string()
                    } else {
                        format!("{}:{}", lib.to_string_lossy(), existing)
                    };
                    env.insert("LD_PRELOAD".to_string(), new_preload);
                    env.insert("MANGOHUD".to_string(), "1".to_string());
                    env.insert("MANGOHUD_DLSYM".to_string(), "1".to_string());
                }
                None => {
                    tracing::warn!("MangoHud requested but libMangoHud.so not found — skipping");
                }
            }
        }

        env.insert(
            "WINEDEBUG".to_string(),
            "err+all,warn+module,warn+loaddll".to_string(),
        );

        let log_dir = crate::core::config::config_dir()
            .unwrap_or_else(|_| PathBuf::from("/tmp"))
            .join("logs");
        let log_path = log_dir.join(format!("wine_{}.log", ctx.app.app_id));
        env.insert(
            "WINE_LOG_OUTPUT".to_string(),
            log_path.to_string_lossy().to_string(),
        );

        Ok(env)
    }

    async fn build_command(
        &self,
        ctx: &LaunchContext,
    ) -> std::result::Result<CommandSpec, LaunchError> {
        let library_root = PathBuf::from(&ctx.launcher_config.steam_library_path);

        let proton = resolve_proton_required(ctx)?;
        let active_runner = crate::core::utils::resolve_runner(proton, &library_root);

        match crate::core::utils::classify_runner(&active_runner) {
            crate::core::utils::RunnerKind::Proton => {
                tracing::info!(
                    "Game runner classified as Proton: launching via `proton run` (protonfixes active)"
                );
            }
            crate::core::utils::RunnerKind::WineTkg | crate::core::utils::RunnerKind::PlainWine => {
                tracing::info!(
                    fixup_env = ctx.game_fixups.env.len(),
                    fixup_dll = ctx.game_fixups.dll_overrides.len(),
                    "Game runner classified as bare Wine: launching via wine + fixup layer"
                );
            }
            crate::core::utils::RunnerKind::Unknown => {
                tracing::warn!(
                    "Game runner '{}' did not classify as Proton or Wine; relying on build_runner_command resolution",
                    active_runner.display()
                );
            }
        }

        let use_umu = ctx.use_umu;

        let mut spec = CommandSpec::default();

        if use_umu {
            let umu_run = ctx.umu_run.clone().ok_or_else(|| {
                LaunchError::new(
                    LaunchErrorKind::Runner,
                    "umu is active but the plugin `umu-run` path was not resolved",
                )
            })?;
            spec.program = umu_run;
        } else {
            let base_cmd =
                crate::core::utils::build_runner_command(&active_runner).map_err(|e| {
                    LaunchError::new(
                        LaunchErrorKind::Runner,
                        format!(
                            "Invalid Compatibility Layer path: {}",
                            active_runner.display()
                        ),
                    )
                    .with_source(e)
                })?;
            spec.program = base_cmd.get_program().into();
            spec.args = base_cmd
                .get_args()
                .map(|s| s.to_string_lossy().to_string())
                .collect();
        }

        let (_install_dir, executable, game_working_dir) = ctx.game_paths()?;

        spec.cwd = Some(game_working_dir);
        let target = executable.to_string_lossy().to_string();
        if crate::core::utils::is_launch_url(&target) {
            spec.args.extend(["cmd.exe".to_string(), "/c".to_string(), "start".to_string(), String::new(), target]);
        } else {
            spec.args.push(target);
        }

        let args = ctx
            .launch_info
            .arguments
            .split_whitespace()
            .map(ToString::to_string);
        spec.args.extend(args);

        if !ctx.game_fixups.launch_args.is_empty() {
            let user_opts: std::collections::HashSet<&str> = ctx.user_config.as_ref()
                .map(|c| c.launch_options.split_whitespace().collect())
                .unwrap_or_default();
            for arg in &ctx.game_fixups.launch_args {
                if user_opts.contains(arg.as_str()) || spec.args.contains(arg) {
                    tracing::info!("Skipping fixup launch arg {} (already present)", arg);
                    continue;
                }
                tracing::info!("Applying fixup launch arg: {}", arg);
                spec.args.push(arg.clone());
            }
        }

        let user_launch_args = ctx
            .user_config
            .as_ref()
            .map(|c| {
                c.launch_options
                    .split_whitespace()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
            .into_iter()
            .filter(|a| a != "-mangohud" && a != "--mangohud");
        spec.args.extend(user_launch_args);

        spec.env = self.build_env(ctx).await?;

        if use_umu {
            let proton_root = crate::core::utils::derive_runner_root(&active_runner);
            spec.env
                .insert("GAMEID".to_string(), ctx.app.app_id.to_string());
            spec.env.insert(
                "PROTONPATH".to_string(),
                proton_root.to_string_lossy().to_string(),
            );
        }

        Ok(spec)
    }

    fn launch(&self, spec: &CommandSpec) -> std::result::Result<std::process::Child, LaunchError> {
        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args);
        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }
        for (key, val) in &spec.env {
            cmd.env(key, val);
        }

        let log_path = spec.env.get("WINE_LOG_OUTPUT").map(PathBuf::from);
        if let Some(path) = log_path {
            std::fs::create_dir_all(path.parent().unwrap()).ok();
            if let Ok(log_file) = std::fs::File::create(&path) {
                cmd.stderr(log_file);
            } else {
                cmd.stderr(crate::infra::logging::relay::child_stdio());
            }
        } else {
            cmd.stderr(crate::infra::logging::relay::child_stdio());
        }

        cmd.stdout(crate::infra::logging::relay::child_stdio());

        tracing::debug!(
            program = ?cmd.get_program(),
            args = ?cmd.get_args().collect::<Vec<_>>(),
            working_dir = ?cmd.get_current_dir(),
            "runner launch",
        );

        cmd.spawn()
            .map(crate::infra::logging::relay::attach)
            .map_err(|e| {
                LaunchError::new(LaunchErrorKind::Process, "failed to spawn runner process")
                    .with_source(anyhow!(e))
            })
    }
}

fn effective_game_prefix(ctx: &LaunchContext) -> PathBuf {
    crate::core::utils::wineprefix_for_game(
        &ctx.launcher_config,
        ctx.app.app_id,
        ctx.user_config.as_ref(),
        ctx.app.install_path.as_deref().map(std::path::Path::new),
    )
}

fn push_dll_dir_once(dirs: &mut Vec<String>, path: &Path) {
    let s = path.to_string_lossy().to_string();
    if !dirs.contains(&s) {
        dirs.push(s);
    }
}

fn game_compat_data_path(
    library_root: &Path,
    app_id: u32,
    steam_mode: SteamMode,
    steam_prefix_mode: crate::core::models::SteamPrefixMode,
    master_wine_prefix: &Path,
) -> PathBuf {
    use crate::core::models::SteamPrefixMode;
    if steam_mode == SteamMode::InWineRuntime && steam_prefix_mode == SteamPrefixMode::Shared {
        if let Some(parent) = master_wine_prefix.parent() {
            if parent.join("pfx") == master_wine_prefix {
                return parent.to_path_buf();
            }
        }
    }
    crate::core::utils::compat_data_dir(library_root, app_id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteamMode {
    HostBridge,
    InWineRuntime,
    Standalone,
}

fn resolve_steam_mode(ctx: &LaunchContext) -> (SteamMode, &'static str) {
    resolve_steam_mode_for(
        ctx.user_config.as_ref(),
        &ctx.launcher_config,
        ctx.steam_enabled,
    )
}

pub fn resolve_steam_mode_for(
    user_config: Option<&crate::core::models::UserAppConfig>,
    launcher_config: &crate::core::config::LauncherConfig,
    steam_enabled: bool,
) -> (SteamMode, &'static str) {
    use crate::core::models::SteamRuntimePolicy;

    let per_game = user_config
        .map(|c| c.steam_runtime_policy)
        .unwrap_or_default();
    let legacy_forced = user_config.map(|c| c.use_steam_runtime).unwrap_or(false);
    let effective = match per_game {
        SteamRuntimePolicy::Auto if legacy_forced => SteamRuntimePolicy::Enabled,
        SteamRuntimePolicy::Auto => launcher_config.steam_runtime_policy,
        explicit => explicit,
    };

    let master_installed = || {
        crate::core::utils::get_master_steam_config()
            .steam_exe
            .is_some()
    };
    let host_installed = || crate::core::utils::host_steam_client_path().is_some();

    match effective {
        SteamRuntimePolicy::Enabled => {
            if master_installed() {
                (SteamMode::InWineRuntime, "policy_enabled")
            } else if steam_enabled && host_installed() {
                (SteamMode::HostBridge, "policy_enabled_host_fallback")
            } else {
                (SteamMode::Standalone, "policy_enabled_no_runtime")
            }
        }
        SteamRuntimePolicy::Disabled => {
            if steam_enabled && host_installed() {
                (SteamMode::HostBridge, "host")
            } else if steam_enabled {
                (SteamMode::Standalone, "steam_flag_no_host")
            } else {
                (SteamMode::Standalone, "policy_disabled")
            }
        }
        SteamRuntimePolicy::Auto => {
            if !steam_enabled {
                if host_installed() && crate::core::utils::is_steam_running() {
                    tracing::info!("host Steam detected; bridging Steamworks");
                    (SteamMode::HostBridge, "host_running_opportunistic")
                } else {
                    (SteamMode::Standalone, "default")
                }
            } else if host_installed() {
                (SteamMode::HostBridge, "host")
            } else if master_installed() {
                (SteamMode::InWineRuntime, "steam_flag_no_host")
            } else {
                (SteamMode::Standalone, "steam_flag_no_steam")
            }
        }
    }
}

fn forced_proton(ctx: &LaunchContext) -> Option<&str> {
    ctx.launcher_config
        .game_configs
        .get(&ctx.app.app_id)
        .and_then(|c| c.forced_proton_version.as_ref())
        .map(|s| s.as_str())
}

fn resolve_proton_required(ctx: &LaunchContext) -> std::result::Result<&str, LaunchError> {
    if let Some(forced) = forced_proton(ctx) {
        Ok(forced)
    } else {
        ctx.proton_path
            .as_deref()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| {
                LaunchError::new(
                    LaunchErrorKind::Environment,
                    "proton path is required for Windows launch",
                )
            })
    }
}

fn resolve_background_steam_command(
    ctx: &LaunchContext,
    library_root: &Path,
) -> std::result::Result<Command, LaunchError> {
    let configured = &ctx.launcher_config.steam_runtime_runner;
    if !configured.as_os_str().is_empty() {
        let name = configured.to_string_lossy();
        let bare_wine = crate::core::utils::resolve_steam_runtime_wine(&name, library_root)
            .map_err(|e| {
                LaunchError::new(
                    LaunchErrorKind::Runner,
                    format!(
                        "configured steam_runtime_runner '{name}' could not be resolved to a bare wine for background Steam"
                    ),
                )
                .with_source(e)
            })?;
        tracing::info!(
            "Background Steam runner: configured steam_runtime_runner '{name}' -> bare wine {}",
            bare_wine.display()
        );
        return Ok(Command::new(bare_wine));
    }

    let proton = resolve_proton_required(ctx)?;
    let game_runner = crate::core::utils::resolve_runner(proton, library_root);
    match crate::core::utils::classify_runner(&game_runner) {
        crate::core::utils::RunnerKind::WineTkg | crate::core::utils::RunnerKind::PlainWine => {
            tracing::info!(
                "Background Steam runner: bare wine tree {}",
                game_runner.display()
            );
            crate::core::utils::build_runner_command(&game_runner).map_err(|e| {
                LaunchError::new(
                    LaunchErrorKind::Runner,
                    format!(
                        "Invalid Compatibility Layer path: {}",
                        game_runner.display()
                    ),
                )
                .with_source(e)
            })
        }
        crate::core::utils::RunnerKind::Proton => {
            let bare = crate::core::utils::proton_bundled_bare_wine(&game_runner).ok_or_else(|| {
                LaunchError::new(
                    LaunchErrorKind::Environment,
                    format!(
                        "the only available runtime is a Proton tree ({}) but its bundled bare wine \
                         (files/bin/wine64) could not be found; background Steam needs a bare wine",
                        game_runner.display()
                    ),
                )
            })?;
            tracing::info!(
                "Background Steam runner: Proton-bundled bare wine {}",
                bare.display()
            );
            Ok(Command::new(bare))
        }
        crate::core::utils::RunnerKind::Unknown => Err(LaunchError::new(
            LaunchErrorKind::Runner,
            format!(
                "no suitable Steam-runtime runner found for background Steam. Configure \
                 `steam_runtime_runner` with a wine-tkg or plain-Wine build (the game runner '{}' \
                 is not a usable bare-wine runtime)",
                game_runner.display()
            ),
        )),
    }
}

#[cfg(test)]
mod compat_path_tests {
    use super::*;
    use crate::core::models::SteamPrefixMode;

    #[test]
    fn shared_inwine_resolves_to_master_pfx() {
        let master = PathBuf::from("/home/u/.config/Aurelia/master_steam_prefix/pfx");
        let got = game_compat_data_path(
            Path::new("/games"),
            123,
            SteamMode::InWineRuntime,
            SteamPrefixMode::Shared,
            &master,
        );
        assert_eq!(got.join("pfx"), master);
        assert_eq!(
            got,
            PathBuf::from("/home/u/.config/Aurelia/master_steam_prefix")
        );
    }

    #[test]
    fn per_game_inwine_uses_appid_compatdata() {
        let master = PathBuf::from("/home/u/.config/Aurelia/master_steam_prefix/pfx");
        let got = game_compat_data_path(
            Path::new("/games"),
            123,
            SteamMode::InWineRuntime,
            SteamPrefixMode::PerGame,
            &master,
        );
        assert_eq!(got, PathBuf::from("/games/steamapps/compatdata/123"));
    }

    #[test]
    fn standalone_uses_appid_compatdata() {
        let master = PathBuf::from("/m/pfx");
        let got = game_compat_data_path(
            Path::new("/games"),
            123,
            SteamMode::Standalone,
            SteamPrefixMode::Shared,
            &master,
        );
        assert_eq!(got, PathBuf::from("/games/steamapps/compatdata/123"));
    }

    #[test]
    fn root_layout_master_falls_back_to_appid() {
        let master = PathBuf::from("/home/u/.config/Aurelia/master_steam_prefix");
        let got = game_compat_data_path(
            Path::new("/games"),
            123,
            SteamMode::InWineRuntime,
            SteamPrefixMode::Shared,
            &master,
        );
        assert_eq!(got, PathBuf::from("/games/steamapps/compatdata/123"));
    }
}
