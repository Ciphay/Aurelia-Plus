use crate::commands::common::*;

use anyhow::{Context, Result, bail};
use aurelia::core::config::load_launcher_config;
use aurelia::core::models::{DepotPlatform, DownloadState};
use aurelia::steam_client::InstallOptions;
use std::sync::{Arc, RwLock};

pub(crate) async fn cmd_proton_list(installed_only: bool, json: bool) -> Result<()> {
    let cfg = load_launcher_config().await?;
    let installed =
        aurelia::compat::proton::list_installed(std::path::Path::new(&cfg.steam_library_path));
    let default = cfg.proton_version;
    let is_installed = |name: &str| installed.iter().any(|i| i.name.eq_ignore_ascii_case(name));
    if !json && !aurelia::core::utils::PROTON_SUPPORTED {
        cli_eprintln!(
            "Note: {}",
            aurelia::core::utils::linux_only_msg("Proton/Wine")
        );
    }

    if installed_only {
        if json {
            print_json(&serde_json::json!({ "default": default, "installed": installed }));
            return Ok(());
        }
        if installed.is_empty() {
            cli_println!("No Proton/Wine runtimes installed.");
            return Ok(());
        }
        cli_println!("Installed runtimes:");
        for i in &installed {
            let star = if i.name == default { "  * default" } else { "" };
            cli_println!("  {:<28} ({}){star}", i.name, i.location);
        }
        return Ok(());
    }

    let available = aurelia::compat::proton::list_available()
        .await
        .unwrap_or_default();
    if json {
        print_json(&serde_json::json!({
            "default": default,
            "installed": installed,
            "available": available,
        }));
        return Ok(());
    }

    cli_println!("{:<10}  {:<28}  {:>10}  STATUS", "SOURCE", "NAME", "SIZE");
    for pkg in &available {
        let size = if pkg.size > 0 {
            human_bytes(pkg.size)
        } else {
            "-".to_string()
        };
        let mut status = String::new();
        if is_installed(&pkg.name) {
            status.push_str("installed");
        }
        if pkg.name == default {
            status.push_str(" *default");
        }
        cli_println!(
            "{:<10}  {:<28}  {:>10}  {}",
            pkg.label,
            pkg.name,
            size,
            status.trim()
        );
    }
    for i in &installed {
        if !available
            .iter()
            .any(|p| p.name.eq_ignore_ascii_case(&i.name))
        {
            let star = if i.name == default { " *default" } else { "" };
            cli_println!(
                "{:<10}  {:<28}  {:>10}  installed{star}",
                "(local)",
                i.name,
                "-"
            );
        }
    }
    cli_println!("\nInstall with `aurelia proton install <NAME>`.");
    Ok(())
}

pub(crate) async fn cmd_proton_install(version: String, json: bool) -> Result<()> {
    if !aurelia::core::utils::PROTON_SUPPORTED {
        bail!(
            "{} Nothing to install.",
            aurelia::core::utils::linux_only_msg("Proton/Wine")
        );
    }
    let pkg = aurelia::compat::proton::resolve_package(&version).await?;

    match &pkg.source {
        aurelia::compat::proton::ProtonSource::Valve { app_id } => {
            let app_id = *app_id;
            if !json {
                cli_println!("Installing {} via Steam (app {app_id}) ...", pkg.name);
            }
            let client = authed_client().await?;
            let state = Arc::new(RwLock::new(DownloadState::default()));
            let rx = client
                .install_game(
                    app_id,
                    DepotPlatform::Linux,
                    InstallOptions::default(),
                    state,
                )
                .await
                .with_context(|| format!("failed to start installing {}", pkg.name))?;
            drive_progress(rx, json).await?;
        }
        aurelia::compat::proton::ProtonSource::Github { .. } => {
            if !json {
                cli_println!("Downloading {} ({}) ...", pkg.name, pkg.label);
            }
            let mut on_progress = byte_progress_printer(json);
            let path =
                aurelia::compat::proton::install_github_package(&pkg, &mut on_progress).await?;
            if !json {
                cli_println!("\n  Extracted to {}", path.display());
            }
        }
    }

    let mut cfg = load_launcher_config().await?;
    cfg.proton_version = pkg.name.clone();
    cfg.save()
        .await
        .context("failed saving the default Proton version")?;

    if json {
        print_json(&serde_json::json!({
            "name": pkg.name,
            "status": "installed",
            "default": true,
        }));
    } else {
        cli_println!("Installed {} and set it as the global default.", pkg.name);
    }
    Ok(())
}

pub(crate) async fn cmd_proton_anticheat(
    kind: aurelia::compat::anticheat::AntiCheat,
    json: bool,
) -> Result<()> {
    if !aurelia::core::utils::PROTON_SUPPORTED {
        bail!(
            "{} Nothing to install.",
            aurelia::core::utils::linux_only_msg("Proton anti-cheat runtimes")
        );
    }
    let app_id = kind.runtime_app_id();
    let installed = aurelia::compat::anticheat::installed_runtimes().await;
    let already = installed.contains_key(&app_id);
    if !already {
        if !json {
            cli_println!(
                "Installing {} via Steam (app {app_id}) ...",
                kind.runtime_name()
            );
        }
        let client = authed_client().await?;
        let state = Arc::new(RwLock::new(DownloadState::default()));
        let rx = client
            .install_game(
                app_id,
                DepotPlatform::Linux,
                InstallOptions::default(),
                state,
            )
            .await
            .with_context(|| format!("failed to start installing {}", kind.runtime_name()))?;
        drive_progress(rx, json).await?;
    }
    let path = aurelia::compat::anticheat::installed_runtimes()
        .await
        .remove(&app_id)
        .with_context(|| format!("{} finished but was not found on disk", kind.runtime_name()))?;
    let status = if already {
        "already_installed"
    } else {
        "installed"
    };
    if json {
        print_json(&serde_json::json!({
            "name": kind.runtime_name(),
            "app_id": app_id,
            "env": kind.env_var(),
            "path": path,
            "status": status,
        }));
    } else {
        let verb = if already {
            "Already installed"
        } else {
            "Installed"
        };
        cli_println!("{verb}: {} at {}", kind.runtime_name(), path.display());
        cli_println!("Proton launches set {} automatically.", kind.env_var());
    }
    Ok(())
}

pub(crate) async fn cmd_proton_uninstall(version: String, json: bool) -> Result<()> {
    aurelia::compat::proton::remove(&version)
        .with_context(|| format!("failed to uninstall {version}"))?;

    let cfg = load_launcher_config().await?;
    let was_default = cfg.proton_version.eq_ignore_ascii_case(&version);

    if json {
        print_json(&serde_json::json!({
            "name": version,
            "status": "uninstalled",
            "was_default": was_default,
        }));
    } else {
        cli_println!("Uninstalled {version}.");
        if was_default {
            cli_eprintln!(
                "Note: {version} was the global default — set a new one with \
                 `aurelia proton default <NAME>`."
            );
        }
    }
    Ok(())
}

pub(crate) async fn cmd_proton_default(version: String, json: bool) -> Result<()> {
    let mut cfg = load_launcher_config().await?;
    let installed =
        aurelia::compat::proton::list_installed(std::path::Path::new(&cfg.steam_library_path));
    let present = installed
        .iter()
        .any(|i| i.name.eq_ignore_ascii_case(&version));

    cfg.proton_version = version.clone();
    cfg.save()
        .await
        .context("failed saving the default Proton version")?;

    if json {
        print_json(&serde_json::json!({
            "default": version,
            "installed_present": present,
        }));
    } else {
        cli_println!("Global default Proton set to {version}.");
        if !aurelia::core::utils::PROTON_SUPPORTED {
            cli_eprintln!(
                "Note: {}",
                aurelia::core::utils::linux_only_msg("Proton/Wine")
            );
        } else if !present {
            cli_eprintln!(
                "Note: '{version}' is not installed — run `aurelia proton install {version}`."
            );
        }
    }
    Ok(())
}

async fn steam_runtime_config(gerund: &str) -> Result<aurelia::core::config::LauncherConfig> {
    let config = load_launcher_config().await?;
    aurelia::core::utils::ensure_steam_runtime_supported()?;
    if config.steam_runtime_runner.as_os_str().is_empty() {
        bail!(
            "{}",
            aurelia::core::utils::steam_runtime_runner_unset_msg(gerund)
        );
    }
    Ok(config)
}

pub(crate) async fn cmd_steam_runtime_install(reinstall: bool, json: bool) -> Result<()> {
    let config = steam_runtime_config("installing").await?;
    if reinstall {
        if !json {
            cli_println!("Removing the existing Windows Steam runtime ...");
        }
        aurelia::launch::uninstall_master_steam().await?;
    }
    aurelia::launch::install_master_steam(&config).await?;
    let steam_cfg = aurelia::core::utils::get_master_steam_config();
    if json {
        print_json(&serde_json::json!({
            "status": if reinstall { "reinstalled" } else { "installed" },
            "steam_exe": steam_cfg.steam_exe,
        }));
    } else {
        let verb = if reinstall {
            "reinstalled"
        } else {
            "installed"
        };
        cli_println!("Master Steam runtime {verb}; Steam client started.");
        if let Some(exe) = &steam_cfg.steam_exe {
            cli_println!("steam.exe path    : {}", exe.display());
        }
    }
    Ok(())
}

pub(crate) async fn cmd_steam_runtime_uninstall(json: bool) -> Result<()> {
    let steam_cfg = aurelia::core::utils::get_master_steam_config();
    let existed = steam_cfg.root_dir.exists();
    aurelia::launch::uninstall_master_steam().await?;
    if json {
        print_json(&serde_json::json!({
            "status": if existed { "uninstalled" } else { "not_installed" },
            "removed_path": steam_cfg.root_dir,
        }));
    } else if existed {
        cli_println!(
            "Removed the master Windows Steam runtime ({}).",
            steam_cfg.root_dir.display()
        );
        cli_println!("Reinstall with `aurelia steam-runtime install`.");
    } else {
        cli_println!("No master Windows Steam runtime was installed — nothing to remove.");
    }
    Ok(())
}

pub(crate) async fn cmd_steam_runtime_repair(json: bool) -> Result<()> {
    let config = steam_runtime_config("repairing").await?;
    aurelia::launch::repair_master_steam(&config).await?;
    let steam_cfg = aurelia::core::utils::get_master_steam_config();
    if json {
        print_json(&serde_json::json!({
            "status": "repaired",
            "steam_exe": steam_cfg.steam_exe,
        }));
    } else {
        cli_println!("Master Steam runtime repaired (backed up old prefix, reinstalled).");
    }
    Ok(())
}

pub(crate) async fn cmd_steam_runtime_login(json: bool) -> Result<()> {
    let config = steam_runtime_config("signing in").await?;
    aurelia::launch::relogin_master_steam(&config).await?;
    let steam_cfg = aurelia::core::utils::get_master_steam_config();
    if json {
        print_json(&serde_json::json!({
            "status": "started",
            "wine_prefix": steam_cfg.wine_prefix,
        }));
    } else {
        cli_println!("Started the Windows Steam client for the master runtime.");
        cli_println!("Sign in (username/password + Steam Guard) in the Steam window that appears.");
        cli_println!("The login is saved in the master prefix and reused by `play --steam`.");
        cli_println!("You can close Steam once it shows you as signed in.");
    }
    Ok(())
}

pub(crate) async fn cmd_steam_runtime_stop(json: bool) -> Result<()> {
    let was_running = aurelia::launch::stop_master_steam();
    if json {
        print_json(&serde_json::json!({
            "status": if was_running { "stopped" } else { "not_running" },
        }));
    } else if was_running {
        cli_println!("Stopped the Windows Steam client running in the master prefix.");
    } else {
        cli_println!("No Windows Steam client is running in the master prefix.");
    }
    Ok(())
}

pub(crate) async fn cmd_steam_runtime_status(json: bool) -> Result<()> {
    let config = load_launcher_config().await?;
    let steam_cfg = aurelia::core::utils::get_master_steam_config();
    let steam_exe_present = steam_cfg.steam_exe.is_some();
    let runner = config.steam_runtime_runner.to_string_lossy().to_string();
    let runner_configured = !runner.is_empty();

    if json {
        print_json(&serde_json::json!({
            "root_dir": steam_cfg.root_dir,
            "wine_prefix": steam_cfg.wine_prefix,
            "layout_kind": steam_cfg.layout_kind,
            "steam_exe": steam_cfg.steam_exe,
            "steam_exe_present": steam_exe_present,
            "steam_runtime_runner": runner_configured.then_some(runner.clone()),
            "steam_runtime_runner_configured": runner_configured,
            "supported": aurelia::core::utils::STEAM_RUNTIME_SUPPORTED,
        }));
    } else {
        if !aurelia::core::utils::STEAM_RUNTIME_SUPPORTED {
            cli_println!(
                "Supported here    : no — {}",
                aurelia::core::utils::steam_runtime_unsupported_msg()
            );
        }
        cli_println!("Master Steam root : {}", steam_cfg.root_dir.display());
        cli_println!("Wine prefix       : {}", steam_cfg.wine_prefix.display());
        cli_println!("Layout kind       : {}", steam_cfg.layout_kind);
        cli_println!(
            "steam.exe present : {}",
            if steam_exe_present { "yes" } else { "no" }
        );
        if let Some(exe) = &steam_cfg.steam_exe {
            cli_println!("steam.exe path    : {}", exe.display());
        }
        cli_println!(
            "Runtime runner    : {}",
            if runner_configured {
                runner.as_str()
            } else {
                "(unset)"
            }
        );
    }
    Ok(())
}
