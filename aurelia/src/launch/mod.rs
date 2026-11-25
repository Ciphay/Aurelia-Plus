pub mod dll_provider_resolver;
pub mod fixups;
pub mod game_process;
pub mod install_scripts;
pub mod launch_script;
pub mod pipeline;
pub mod shaders;
pub mod stages;
pub mod validators;

#[cfg(test)]
mod verification_tests;

#[cfg(test)]
#[path = "game_process_tests.rs"]
mod game_process_tests;

#[cfg(test)]
#[path = "wait_tests.rs"]
mod wait_tests;

use crate::core::config::{LauncherConfig, config_dir};
use crate::core::utils::MasterSteamConfig;
use crate::steam_client::SteamClient;
use anyhow::{Context, Result, anyhow};
use std::path::{Path, PathBuf};
use std::process::Command;

fn stop_prefix_processes(wine_prefix: &Path) {
    SteamClient::kill_steam_in_prefix(wine_prefix);
    #[cfg(unix)]
    SteamClient::kill_wine_processes_in_prefix(wine_prefix, true);
}

fn resolve_runtime_wine(config: &LauncherConfig) -> Result<PathBuf> {
    crate::core::utils::ensure_steam_runtime_supported()?;
    let runner_name = config.steam_runtime_runner.to_string_lossy();
    let library_root = PathBuf::from(&config.steam_library_path);
    crate::core::utils::resolve_steam_runtime_wine(&runner_name, &library_root)
}

fn require_steam_exe(steam_cfg: &MasterSteamConfig) -> Result<PathBuf> {
    steam_cfg.steam_exe.clone().ok_or_else(|| {
        anyhow!(
            "the Windows Steam runtime is not installed yet (no steam.exe under {}). \
             Run `aurelia steam-runtime install` first.",
            steam_cfg.wine_prefix.display()
        )
    })
}

pub async fn install_master_steam(config: &LauncherConfig) -> Result<()> {
    let base_dir = config_dir()?;
    let steam_cfg = crate::core::utils::get_master_steam_config();

    let wine = resolve_runtime_wine(config)?;

    tracing::info!("Unified Master Steam resolution:");
    tracing::info!("  - Root Dir: {}", steam_cfg.root_dir.display());
    tracing::info!("  - Wine Prefix: {}", steam_cfg.wine_prefix.display());
    tracing::info!("  - Layout Kind: {}", steam_cfg.layout_kind);
    tracing::info!("  - Wine Binary: {}", wine.display());

    let steam_exe = match steam_cfg.steam_exe.clone() {
        Some(exe) => {
            tracing::info!("  - Steam Exe: {} (already installed)", exe.display());
            exe
        }
        None => {
            tracing::info!("  - Steam Exe: NOT FOUND (running installer)");
            run_steam_installer(&wine, &steam_cfg, &base_dir).await?
        }
    };

    crate::core::utils::ensure_steam_runtime_prefix_libs(&wine, &steam_cfg.wine_prefix);

    launch_master_steam(&wine, &steam_exe, &steam_cfg, &base_dir)
}

async fn run_steam_installer(
    wine: &Path,
    steam_cfg: &MasterSteamConfig,
    base_dir: &Path,
) -> Result<PathBuf> {
    let runtimes_dir = base_dir.join("runtimes");
    std::fs::create_dir_all(&runtimes_dir)?;
    let setup_exe = runtimes_dir.join("SteamSetup.exe");
    ensure_steam_setup(&setup_exe).await?;

    std::fs::create_dir_all(&steam_cfg.wine_prefix).with_context(|| {
        format!(
            "Failed to create master Steam prefix at {}",
            steam_cfg.wine_prefix.display()
        )
    })?;

    let mut cmd = Command::new(wine);
    cmd.arg(&setup_exe);
    cmd.arg("/S");
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    apply_master_steam_env(&mut cmd, steam_cfg, base_dir)?;
    apply_install_diagnostics(&mut cmd, base_dir);

    tracing::info!("Running Steam installer: {:?}", cmd);

    let status = tokio::process::Command::from(cmd)
        .status()
        .await
        .with_context(|| format!("Failed to run Steam installer under {}", wine.display()))?;

    if !status.success() {
        return Err(anyhow!(
            "SteamSetup.exe failed under {} (exit status: {status}). \
             Re-run with AURELIA_DIAGNOSE_INSTALL=1 to capture a wine debug log.",
            wine.display()
        ));
    }

    crate::core::utils::get_master_steam_config()
        .steam_exe
        .ok_or_else(|| {
            anyhow!(
                "SteamSetup.exe exited successfully but no steam.exe appeared under {}. \
                 Re-run with AURELIA_DIAGNOSE_INSTALL=1 to capture a wine debug log.",
                steam_cfg.wine_prefix.display()
            )
        })
}

fn launch_master_steam(
    wine: &Path,
    steam_exe: &Path,
    steam_cfg: &MasterSteamConfig,
    base_dir: &Path,
) -> Result<()> {
    let mut cmd = Command::new(wine);
    cmd.arg(steam_exe);
    cmd.arg("-tcp");
    cmd.arg("-noreactlogin");
    cmd.arg("-cef-disable-gpu");
    cmd.arg("-cef-disable-sandbox");
    cmd.arg("-cef-disable-gpu-compositing");
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    apply_master_steam_env(&mut cmd, steam_cfg, base_dir)?;
    apply_install_diagnostics(&mut cmd, base_dir);

    tracing::info!("Launching Master Steam: {:?}", cmd);
    let _child = cmd
        .spawn()
        .context("Failed to spawn master steam process")?;
    Ok(())
}

fn apply_master_steam_env(
    cmd: &mut Command,
    steam_cfg: &MasterSteamConfig,
    base_dir: &Path,
) -> Result<()> {
    cmd.env("WINEPREFIX", &steam_cfg.wine_prefix);
    cmd.env("STEAM_COMPAT_DATA_PATH", &steam_cfg.root_dir);
    cmd.env("WINEPATH", "C:\\Program Files (x86)\\Steam");

    let fake_env = crate::core::utils::setup_fake_steam_trap(base_dir)?;
    cmd.env("STEAM_COMPAT_CLIENT_INSTALL_PATH", &fake_env);
    cmd.env("WINEDLLOVERRIDES", "vstdlib_s=n;tier0_s=n;steamclient=n;steamclient64=n;steam_api=n;steam_api64=n;lsteamclient=");

    for var in [
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XDG_RUNTIME_DIR",
        "XAUTHORITY",
    ] {
        if let Ok(value) = std::env::var(var) {
            cmd.env(var, value);
        }
    }
    Ok(())
}

fn apply_install_diagnostics(cmd: &mut Command, base_dir: &Path) {
    if std::env::var("AURELIA_DIAGNOSE_INSTALL").as_deref() != Ok("1") {
        return;
    }
    let logs_dir = base_dir.join("logs");
    if let Err(e) = std::fs::create_dir_all(&logs_dir) {
        tracing::warn!(
            "AURELIA_DIAGNOSE_INSTALL set but could not create log dir {}: {}",
            logs_dir.display(),
            e
        );
        return;
    }
    let stamp = crate::core::utils::now_unix();
    let log_path = logs_dir.join(format!("steam_runtime_install_{stamp}.log"));
    match std::fs::File::create(&log_path) {
        Ok(file) => {
            cmd.env("WINEDEBUG", "+setupapi,+file,+module");
            if let Ok(err_file) = file.try_clone() {
                cmd.stderr(std::process::Stdio::from(err_file));
            }
            cmd.stdout(std::process::Stdio::from(file));
            tracing::info!(
                "AURELIA_DIAGNOSE_INSTALL=1: capturing Steam-runtime install diagnostics to {}",
                log_path.display()
            );
        }
        Err(e) => tracing::warn!(
            "AURELIA_DIAGNOSE_INSTALL set but could not create diagnostic log {}: {}",
            log_path.display(),
            e
        ),
    }
}

pub async fn repair_master_steam(config: &LauncherConfig) -> Result<()> {
    crate::core::utils::ensure_steam_runtime_supported()?;
    if config.steam_runtime_runner.as_os_str().is_empty() {
        return Err(anyhow!(
            "{}",
            crate::core::utils::steam_runtime_runner_unset_msg("repairing")
        ));
    }

    let steam_cfg = crate::core::utils::get_master_steam_config();

    tracing::info!(
        "Repair: stopping any processes holding the master prefix {}",
        steam_cfg.wine_prefix.display()
    );
    stop_prefix_processes(&steam_cfg.wine_prefix);

    if steam_cfg.wine_prefix.exists() {
        let mut bak = steam_cfg.wine_prefix.clone().into_os_string();
        bak.push(format!(".bak-{}", crate::core::utils::now_unix()));
        let bak = PathBuf::from(bak);
        if bak.exists() {
            return Err(anyhow!(
                "backup {} already exists; retry shortly",
                bak.display()
            ));
        }
        tracing::info!(
            "Repair: backing up {} -> {}",
            steam_cfg.wine_prefix.display(),
            bak.display()
        );
        std::fs::rename(&steam_cfg.wine_prefix, &bak)
            .with_context(|| format!("failed backing up master prefix to {}", bak.display()))?;
    } else {
        tracing::info!(
            "Repair: no existing master prefix at {} — nothing to back up",
            steam_cfg.wine_prefix.display()
        );
    }

    install_master_steam(config).await
}

pub fn stop_master_steam() -> bool {
    let steam_cfg = crate::core::utils::get_master_steam_config();
    let was_running = SteamClient::is_steam_running_in_prefix(&steam_cfg.wine_prefix);
    stop_prefix_processes(&steam_cfg.wine_prefix);
    was_running
}

pub async fn uninstall_master_steam() -> Result<()> {
    let steam_cfg = crate::core::utils::get_master_steam_config();

    stop_prefix_processes(&steam_cfg.wine_prefix);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    if steam_cfg.root_dir.exists() {
        std::fs::remove_dir_all(&steam_cfg.root_dir).with_context(|| {
            format!(
                "failed to remove the master Steam prefix at {}",
                steam_cfg.root_dir.display()
            )
        })?;
    }
    Ok(())
}

pub async fn relogin_master_steam(config: &LauncherConfig) -> Result<()> {
    let base_dir = config_dir()?;
    let steam_cfg = crate::core::utils::get_master_steam_config();

    let steam_exe = require_steam_exe(&steam_cfg)?;

    let wine = resolve_runtime_wine(config)?;

    stop_prefix_processes(&steam_cfg.wine_prefix);

    crate::core::utils::ensure_steam_runtime_prefix_libs(&wine, &steam_cfg.wine_prefix);

    launch_master_steam(&wine, &steam_exe, &steam_cfg, &base_dir)
}

pub async fn launch_game_via_master_steam(
    config: &LauncherConfig,
    app_id: u32,
    install_path: &Path,
    timeout: std::time::Duration,
    on_status: Option<&(dyn Fn(&str) + Send + Sync)>,
) -> Result<()> {
    let base_dir = config_dir()?;
    let steam_cfg = crate::core::utils::get_master_steam_config();
    let steam_exe = require_steam_exe(&steam_cfg)?;

    let wine = resolve_runtime_wine(config)?;

    crate::core::utils::ensure_steam_runtime_prefix_libs(&wine, &steam_cfg.wine_prefix);

    if !SteamClient::is_steam_running_in_prefix(&steam_cfg.wine_prefix) {
        tracing::info!("in-Wine Steam not running; starting it before -applaunch");
        launch_master_steam(&wine, &steam_exe, &steam_cfg, &base_dir)?;
        let mut ready = false;
        for _ in 0..40 {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            if SteamClient::is_steam_running_in_prefix(&steam_cfg.wine_prefix) {
                ready = true;
                break;
            }
        }
        if !ready {
            return Err(anyhow!(
                "the in-Wine Steam client did not come up in time — sign in first with \
                 `aurelia steam-runtime login`, then retry"
            ));
        }
        tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    }

    let mut cmd = Command::new(&wine);
    cmd.arg(&steam_exe);
    cmd.arg("-applaunch");
    cmd.arg(app_id.to_string());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    apply_master_steam_env(&mut cmd, &steam_cfg, &base_dir)?;
    cmd.spawn()
        .context("failed to dispatch -applaunch to the in-Wine Steam")?;

    let Some(installdir) = install_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
    else {
        return Ok(());
    };

    let prefix = steam_cfg.wine_prefix.clone();
    wait_for_launch(
        timeout,
        std::time::Duration::from_secs(1),
        || SteamClient::is_game_running_in_prefix(&prefix, &installdir),
        || SteamClient::is_shader_processing_in_prefix(&prefix),
        |message| {
            if let Some(cb) = on_status {
                cb(message);
            }
        },
    )
    .await
    .map_err(|waited| {
        anyhow!(crate::core::error::TypedError::new(
            crate::core::error::ErrorKind::Timeout,
            format!(
                "app {app_id} did not start within {}s of -applaunch; the in-Wine Steam may \
                 be updating it or showing a prompt (check its window), or raise \
                 --launch-timeout",
                waited.as_secs()
            ),
        ))
    })?;
    while SteamClient::is_game_running_in_prefix(&steam_cfg.wine_prefix, &installdir) {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    Ok(())
}

pub(crate) async fn wait_for_launch(
    timeout: std::time::Duration,
    poll: std::time::Duration,
    mut game_running: impl FnMut() -> bool,
    mut shaders_busy: impl FnMut() -> bool,
    status: impl Fn(&str),
) -> std::result::Result<(), std::time::Duration> {
    let mut waited = std::time::Duration::ZERO;
    let mut processing = false;
    loop {
        tokio::time::sleep(poll).await;
        if game_running() {
            return Ok(());
        }
        if shaders_busy() {
            if !processing {
                status(
                    "The in-Wine Steam is processing Vulkan shaders; waiting for it to finish ...",
                );
                processing = true;
            }
            continue;
        }
        if processing {
            processing = false;
            status("Shader processing finished; waiting for the game to start ...");
        }
        waited += poll;
        if waited >= timeout {
            return Err(waited);
        }
    }
}

pub fn has_mz_header(path: &Path) -> bool {
    use std::io::Read;
    let mut header = [0u8; 2];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut header))
        .is_ok()
        && &header == b"MZ"
}

pub fn is_valid_setup_exe(path: &Path) -> bool {
    has_mz_header(path)
}

async fn ensure_steam_setup(path: &Path) -> Result<()> {
    if is_valid_setup_exe(path) {
        tracing::info!("Using cached SteamSetup.exe at {}", path.display());
        return Ok(());
    }
    if path.exists() {
        tracing::warn!(
            "Cached {} is not a valid Windows executable — re-downloading",
            path.display()
        );
    }
    download_steam_setup(path).await
}

async fn download_steam_setup(path: &Path) -> Result<()> {
    tracing::info!("Downloading SteamSetup.exe...");
    let url = "https://cdn.akamai.steamstatic.com/client/installer/SteamSetup.exe";
    let bytes = reqwest::get(url)
        .await
        .context("Failed to reach the Steam CDN to download SteamSetup.exe")?
        .error_for_status()
        .context("Steam CDN rejected the SteamSetup.exe download")?
        .bytes()
        .await
        .context("Failed to read the SteamSetup.exe response body")?;

    if bytes.len() < 2 || &bytes[..2] != b"MZ" {
        return Err(anyhow!(
            "Downloaded SteamSetup.exe is not a Windows executable ({} bytes from {url})",
            bytes.len()
        ));
    }

    let tmp = path.with_extension("exe.part");
    std::fs::write(&tmp, &bytes).with_context(|| format!("failed writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| {
        format!(
            "failed moving {} into place at {}",
            tmp.display(),
            path.display()
        )
    })?;
    Ok(())
}
