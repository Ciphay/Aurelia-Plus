use super::*;

pub fn select_launch_entry(
    launch_options: &[LaunchInfo],
    app: &LibraryGame,
    prefer_windows_target: bool,
    prefer_native_target: bool,
) -> Option<LaunchInfo> {
    let on_disk = |rel: &str| on_disk_path(app, rel);
    pick_launch_entry(
        launch_options,
        app,
        prefer_windows_target,
        prefer_native_target,
        &on_disk,
    )
    .map(|mut entry| {
        if let Some(exe) = on_disk(&entry.executable) {
            entry.executable = exe;
        }
        if let Some(wd) = entry
            .workingdir
            .as_deref()
            .filter(|wd| !wd.is_empty())
            .and_then(on_disk)
        {
            entry.workingdir = Some(wd);
        }
        entry
    })
}

pub fn find_launch_option<'a>(options: &'a [LaunchInfo], id: &str) -> Result<&'a LaunchInfo> {
    options.iter().find(|o| o.id == id).ok_or_else(|| {
        let known: Vec<&str> = options.iter().map(|o| o.id.as_str()).collect();
        anyhow!(
            "launch option {id} can't be started: it doesn't exist or is macOS-only (available: {})",
            known.join(", ")
        )
    })
}

pub fn check_launch_option_target(
    entry: &LaunchInfo,
    required: Option<(LaunchTarget, &str)>,
) -> Result<()> {
    let Some((target, source)) = required else {
        return Ok(());
    };
    if entry.target != target {
        let (build, needed) = match entry.target {
            LaunchTarget::NativeLinux => ("native Linux", "Windows"),
            LaunchTarget::WindowsProton => ("Windows", "native Linux"),
        };
        bail!(
            "launch option {} starts the {build} build, but {source} requires the {needed} build",
            entry.id
        );
    }
    Ok(())
}

pub fn launch_executable_installed(app: &LibraryGame, executable: &str) -> bool {
    app.is_installed && !executable.is_empty() && on_disk_path(app, executable).is_some()
}

pub fn hide_uninstalled_launch_options(options: &mut Vec<LaunchOptionInfo>, app: &LibraryGame) {
    if app.is_installed {
        options.retain(|o| launch_executable_installed(app, &o.executable));
    }
}

pub(super) fn require_launch_option_installed(entry: &LaunchInfo, app: &LibraryGame) -> Result<()> {
    if !launch_executable_installed(app, &entry.executable) {
        bail!(
            "launch option {} needs {:?}, which isn't installed; see `aurelia launch-targets {}`",
            entry.id,
            entry.executable,
            app.app_id
        );
    }
    Ok(())
}

fn on_disk_path(app: &LibraryGame, rel: &str) -> Option<String> {
    if crate::core::utils::is_launch_url(rel) {
        return app.install_path.as_ref().map(|_| rel.to_string());
    }
    let dir = std::path::Path::new(app.install_path.as_deref()?);
    let found = crate::core::utils::resolve_path_ignore_case(dir, rel)?;
    let found = found
        .strip_prefix(dir)
        .map(|p| p.to_path_buf())
        .unwrap_or(found);
    Some(found.to_string_lossy().into_owned())
}

fn target_installed(
    launch_options: &[LaunchInfo],
    app: &LibraryGame,
    target: LaunchTarget,
) -> bool {
    launch_options.iter().any(|o| {
        o.target == target && !o.executable.is_empty() && on_disk_path(app, &o.executable).is_some()
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchTargetChoice {
    pub id: &'static str,
    pub platform: &'static str,
    pub steam_mode: Option<&'static str>,
    pub flags: Vec<String>,
    pub available: bool,
    pub missing: Option<&'static str>,
    pub default: bool,
}

pub fn launch_targets(
    launch_options: &[LaunchInfo],
    app: &LibraryGame,
    runtime_installed: bool,
    host_installed: bool,
    steam_flag: bool,
    default_entry: Option<LaunchTarget>,
    default_mode: crate::infra::runners::SteamMode,
) -> Vec<LaunchTargetChoice> {
    use crate::infra::runners::SteamMode;
    let flags = |extra: &[&str]| extra.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let offers = |target: LaunchTarget| launch_options.iter().any(|o| o.target == target);
    let mut targets = Vec::new();

    if offers(LaunchTarget::NativeLinux) {
        let installed = target_installed(launch_options, app, LaunchTarget::NativeLinux);
        let mut f = flags(&["--platform", "linux"]);
        if steam_flag {
            f.push("--steam".to_string());
        }
        targets.push(LaunchTargetChoice {
            id: "linux",
            platform: "linux",
            steam_mode: None,
            flags: f,
            available: installed,
            missing: (!installed).then_some("build"),
            default: installed && default_entry == Some(LaunchTarget::NativeLinux),
        });
    }

    if offers(LaunchTarget::WindowsProton) {
        let installed = target_installed(launch_options, app, LaunchTarget::WindowsProton);
        let default_windows = installed && default_entry == Some(LaunchTarget::WindowsProton);
        let modes = [
            (
                "windows-runtime",
                "runtime",
                SteamMode::InWineRuntime,
                (!runtime_installed).then_some("steam-runtime"),
            ),
            (
                "windows-host",
                "host",
                SteamMode::HostBridge,
                (!host_installed).then_some("host-steam"),
            ),
            ("windows-standalone", "none", SteamMode::Standalone, None),
        ];
        for (id, mode, steam_mode, steam_missing) in modes {
            let missing = if installed {
                steam_missing
            } else {
                Some("build")
            };
            targets.push(LaunchTargetChoice {
                id,
                platform: "windows",
                steam_mode: Some(mode),
                flags: flags(&["--platform", "windows", "--steam-mode", mode]),
                available: missing.is_none(),
                missing,
                default: default_windows && missing.is_none() && default_mode == steam_mode,
            });
        }
    }

    if !targets.iter().any(|t| t.default) {
        if let Some(first) = targets.iter_mut().find(|t| t.available) {
            first.default = true;
        }
    }
    targets
}

fn pick_launch_entry(
    launch_options: &[LaunchInfo],
    app: &LibraryGame,
    prefer_windows_target: bool,
    prefer_native_target: bool,
    on_disk: &dyn Fn(&str) -> Option<String>,
) -> Option<LaunchInfo> {
    let exe_exists =
        |o: &LaunchInfo| -> bool { !o.executable.is_empty() && on_disk(&o.executable).is_some() };
    let installed_target = app.platform.as_deref().and_then(|p| match p {
        "linux" => Some(LaunchTarget::NativeLinux),
        "windows" => Some(LaunchTarget::WindowsProton),
        _ => None,
    });

    if prefer_native_target {
        if let Some(entry) = launch_options
            .iter()
            .find(|o| o.target == LaunchTarget::NativeLinux && exe_exists(o))
        {
            return Some(entry.clone());
        }
    }

    if prefer_windows_target {
        launch_options
            .iter()
            .find(|o| o.target == LaunchTarget::WindowsProton && exe_exists(o))
            .or_else(|| {
                launch_options
                    .iter()
                    .find(|o| installed_target.is_some_and(|t| o.target == t) && exe_exists(o))
            })
            .or_else(|| launch_options.iter().find(|o| exe_exists(o)))
            .or_else(|| {
                launch_options
                    .iter()
                    .find(|o| o.target == LaunchTarget::WindowsProton)
            })
            .or_else(|| launch_options.first())
            .cloned()
    } else {
        launch_options
            .iter()
            .find(|o| installed_target.is_some_and(|t| o.target == t) && exe_exists(o))
            .or_else(|| launch_options.iter().find(|o| exe_exists(o)))
            .or_else(|| launch_options.first())
            .cloned()
    }
}

pub(crate) fn needs_host_steam(requested: bool, is_owned: bool, owns_by_license: bool) -> bool {
    requested || (!is_owned && !owns_by_license)
}

impl SteamClient {
    pub async fn play_game(
        &mut self,
        app: &LibraryGame,
        proton_path: Option<&str>,
        user_config: Option<&crate::core::models::UserAppConfig>,
        force_windows: bool,
        force_native_engine: bool,
        force_umu: bool,
        launch_script_override: Option<PathBuf>,
        disable_launch_script: bool,
        steam_enabled: bool,
        prefer_native_target: bool,
        launch_option: Option<&str>,
        on_target_resolved: Option<&(dyn Fn(Option<&str>) + Send + Sync)>,
        shaders: crate::launch::shaders::ShaderLaunch,
    ) -> Result<LaunchInfo> {
        let owns_by_license = if steam_enabled || app.is_owned { true } else { self.get_app_ticket(app.app_id).await.is_ok() };
        let steam_enabled = needs_host_steam(steam_enabled, app.is_owned, owns_by_license);

        let launch_options = self.get_product_info(app.app_id).await?;
        let launch_options = match launch_option {
            Some(id) => {
                let entry = find_launch_option(&launch_options, id)?;
                require_launch_option_installed(entry, app)?;
                vec![entry.clone()]
            }
            None => launch_options,
        };

        #[cfg(target_os = "windows")]
        if steam_enabled {
            SteamClient::ensure_host_steam_ready(std::time::Duration::from_secs(30)).await;
        }
        #[cfg(target_os = "linux")]
        if steam_enabled {
            crate::core::utils::ensure_steam_running();
            let timeout = std::time::Duration::from_secs(30);
            if crate::core::utils::wait_for_steam_logged_on(timeout).await {
                tracing::info!("host Steam logged on; proceeding with launch");
            } else {
                tracing::warn!(
                    "host Steam not logged on within {}s; launching anyway (Steamworks auth may fail)",
                    timeout.as_secs()
                );
            }
        }

        let prefer_windows_target = force_windows || proton_path.is_some();
        let launch_info = select_launch_entry(
            &launch_options,
            app,
            prefer_windows_target,
            prefer_native_target,
        )
        .ok_or_else(|| anyhow!("no launch options"))?;

        let launcher_config = load_launcher_config().await?;

        let native_windows = force_windows
            || (cfg!(target_os = "windows") && launch_info.target == LaunchTarget::WindowsProton);

        match launch_info.target {
            _ if native_windows => {
                tracing::info!(
                    app_id = app.app_id,
                    "resolved launch platform: Windows (native)"
                );
            }
            LaunchTarget::NativeLinux => {
                tracing::info!(
                    app_id = app.app_id,
                    "resolved launch platform: native Linux"
                );
            }
            LaunchTarget::WindowsProton => {
                tracing::info!(
                    app_id = app.app_id,
                    proton = proton_path.unwrap_or(&launcher_config.proton_version),
                    "resolved launch platform: Windows via Proton"
                );
            }
        }

        if let Some(cb) = on_target_resolved {
            cb(match launch_info.target {
                LaunchTarget::WindowsProton if !native_windows => {
                    Some(proton_path.unwrap_or(&launcher_config.proton_version))
                }
                _ => None,
            });
        }

        let chosen_proton_path = if native_windows {
            None
        } else {
            match launch_info.target {
                LaunchTarget::NativeLinux => None,
                LaunchTarget::WindowsProton => {
                    proton_path.or(Some(launcher_config.proton_version.as_str()))
                }
            }
        };

        if chosen_proton_path.is_some() && launcher_config.anticheat_auto_install {
            self.ensure_anticheat_runtimes(app).await;
        }

        let cloud_enabled = launcher_config.enable_cloud_sync && !self.is_offline();
        let mut cloud_client = None;
        let mut cloud_resolver = None;
        let mut cloud_specs: Vec<crate::library::cloud_sync::UfsSaveSpec> = Vec::new();

        if cloud_enabled {
            let client = CloudClient::new(self.require_connection_owned()?);
            let remote_root = default_cloud_root(client.steam_id(), app.app_id)?.join("remote");
            let cloud_user_configs = crate::core::config::load_user_configs().await?;
            let ufs = self.fetch_ufs_config(app.app_id).await.unwrap_or_default();
            let runs_windows = native_windows || chosen_proton_path.is_some();
            let target_os = if runs_windows { "windows" } else { "linux" };
            let wine_prefix = chosen_proton_path.is_some().then(|| {
                crate::core::utils::game_save_prefix(
                    &launcher_config,
                    app.app_id,
                    &cloud_user_configs,
                    app.install_path.as_deref().map(std::path::Path::new),
                )
            });
            let resolver =
                CloudPathResolver::new(remote_root, app.install_path.as_ref().map(PathBuf::from))
                    .with_wine_prefix(wine_prefix)
                    .with_root_overrides(&ufs.root_overrides, target_os)
                    .with_steam_id(client.steam_id());
            tracing::info!(appid = app.app_id, "Syncing Cloud...");
            match client.sync_down(app.app_id, &resolver).await {
                Ok(outcome) => {
                    if outcome.has_conflicts() {
                        tracing::warn!(
                            appid = app.app_id,
                            "{} Cloud save(s) diverged from local — left untouched; resolve with `aurelia cloud sync`",
                            outcome.conflicts.len()
                        );
                    }
                    if outcome.has_skips() {
                        tracing::warn!(
                            appid = app.app_id,
                            "{} Cloud save(s) could not be placed on disk (unmapped root token(s): {}) — the game may start without them; see `aurelia cloud sync {}`",
                            outcome.skipped.len(),
                            outcome
                                .skipped_tokens()
                                .iter()
                                .map(|t| format!("%{t}%"))
                                .collect::<Vec<_>>()
                                .join(", "),
                            app.app_id
                        );
                    }
                    if outcome.has_failures() {
                        tracing::error!(
                            appid = app.app_id,
                            "{} Cloud save(s) FAILED to download — the save set on disk is incomplete and the game may show no progress. Run `aurelia cloud sync {} --down` before playing. First error: {}",
                            outcome.failed.len(),
                            app.app_id,
                            outcome.failed.first().map_or("", |f| f.error.as_str())
                        );
                    }
                }
                Err(e) => tracing::error!(
                    appid = app.app_id,
                    "Cloud sync-down failed, launching anyway — saves may be missing or incomplete: {e:#}"
                ),
            }
            cloud_client = Some(client);
            cloud_resolver = Some(resolver);
            cloud_specs = ufs.savefiles;
        }

        let mut child = if native_windows {
            self.spawn_windows_native(app, &launch_info, user_config)
                .await?
        } else {
            self.spawn_game_process(
                app,
                &launch_info,
                chosen_proton_path,
                &launcher_config,
                user_config,
                force_native_engine,
                force_umu,
                launch_script_override,
                disable_launch_script,
                steam_enabled,
                shaders,
            )
            .await?
            .into()
        };

        let wineprefix = if native_windows {
            None
        } else {
            let user_configs = crate::core::config::load_user_configs().await?;
            let pfx = crate::core::utils::steam_wineprefix_for_game(
                &launcher_config,
                app.app_id,
                &user_configs,
                app.install_path.as_deref().map(std::path::Path::new),
            );
            pfx.to_string_lossy().contains("compatdata").then_some(pfx)
        };
        let record = crate::compat::running::RunningGame::new(
            app.app_id,
            app.name.clone(),
            child.id(),
            wineprefix,
        );
        if let Err(e) = crate::compat::running::record_launch(&record) {
            tracing::warn!(appid = app.app_id, "could not record running game: {e:#}");
        }

        let wait_result = tokio::task::spawn_blocking(move || child.wait())
            .await
            .context("game wait task failed")?
            .context("failed waiting for game process exit");
        loop {
            let survivors = crate::compat::running::processes_for_app(app.app_id);
            let Some(&pid) = survivors.first() else { break };
            let mut record = record.clone();
            record.set_pid(pid);
            let _ = crate::compat::running::record_launch(&record);
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
        crate::compat::running::clear(app.app_id);
        wait_result?;

        if cloud_enabled {
            if let (Some(client), Some(resolver)) = (cloud_client.as_ref(), cloud_resolver.as_ref())
            {
                match client.sync_up(app.app_id, resolver, &cloud_specs).await {
                    Ok(outcome) if outcome.has_conflicts() => tracing::warn!(
                        appid = app.app_id,
                        "{} Cloud save(s) diverged on upload — left untouched; resolve with `aurelia cloud sync`",
                        outcome.conflicts.len()
                    ),
                    Ok(_) => tracing::info!(appid = app.app_id, "Upload Complete"),
                    Err(e) => {
                        tracing::warn!(
                            appid = app.app_id,
                            "Cloud upload failed (continuing): {e:#}"
                        )
                    }
                }
            }
        }

        Ok(launch_info)
    }

    pub async fn update_game(
        &self,
        appid: u32,
        shared_state: Arc<std::sync::RwLock<crate::core::models::DownloadState>>,
    ) -> Result<Receiver<DownloadProgress>> {
        self.start_manifest_download(appid, false, shared_state)
            .await
    }

    pub async fn verify_game(
        &self,
        appid: u32,
        shared_state: Arc<std::sync::RwLock<crate::core::models::DownloadState>>,
    ) -> Result<Receiver<DownloadProgress>> {
        self.start_manifest_download(appid, true, shared_state)
            .await
    }

    pub(crate) async fn start_manifest_download(
        &self,
        appid: u32,
        verify_mode: bool,
        shared_state: Arc<std::sync::RwLock<crate::core::models::DownloadState>>,
    ) -> Result<Receiver<DownloadProgress>> {
        let connection = self.require_connection_owned()?;

        let install_root = self.install_root_for_app(appid).await?;
        let manifest_path = self.appmanifest_path(appid).await?;
        let (tx, rx) = tokio::sync::mpsc::channel(128);

        let (local_manifests, active_branch) = self
            .local_manifest_info_for_appid(appid)
            .await
            .unwrap_or_else(|_| (HashMap::new(), "public".to_string()));

        let client_clone = self.clone();
        let shared_state_clone = shared_state.clone();
        let recorded_name = std::fs::read_to_string(&manifest_path)
            .ok()
            .and_then(|raw| parse_name_from_acf(&raw))
            .filter(|n| !n.starts_with("App "));
        let game_name = match recorded_name.clone() {
            Some(name) => name,
            None => self.resolve_install_game_name(appid).await,
        };
        tokio::task::spawn(async move {
            if let Ok(mut state) = shared_state_clone.write() {
                state.begin(
                    appid,
                    game_name.clone(),
                    0,
                    format!("Preparing operation for {}...", game_name),
                );
            }

            emit_queued(
                &tx,
                if verify_mode {
                    "verifying installed chunks"
                } else {
                    "resolving latest manifest"
                },
            )
            .await;

            let remote_manifests = if verify_mode {
                local_manifests.clone()
            } else {
                let mut remote =
                    SteamClient::remote_manifest_ids_static(&connection, appid, &active_branch)
                        .await
                        .unwrap_or_default();
                if !local_manifests.is_empty() {
                    remote.retain(|depot_id, _| local_manifests.contains_key(depot_id));
                }
                remote
            };

            let selections: Vec<ManifestSelection> = remote_manifests
                .iter()
                .map(|(depot_id, manifest_id)| ManifestSelection {
                    app_id: appid,
                    depot_id: *depot_id as u32,
                    manifest_id: *manifest_id,
                    appinfo_vdf: String::new(),
                })
                .collect();

            if selections.is_empty() {
                let message = if verify_mode {
                    format!(
                        "app {appid} has no installed depots to verify — it is not fully \
                         installed (its appmanifest lists no completed depots, e.g. a \
                         staged or partial download). Run `aurelia install {appid}` to \
                         complete the installation."
                    )
                } else {
                    format!(
                        "no manifest/depot available for app {appid} (no downloadable \
                         depot was resolved for the active branch)"
                    )
                };
                emit_failed(&tx, message).await;
                return;
            };

            let Some(hosts) = fetch_content_hosts(&client_clone, &connection, &tx, appid).await
            else {
                if let Ok(mut state) = shared_state_clone.write() {
                    state.finish("Operation failed or paused");
                }
                return;
            };

            spawn_progress_reporter(
                tx.clone(),
                shared_state_clone.clone(),
                if verify_mode {
                    DownloadProgressState::Verifying
                } else {
                    DownloadProgressState::Downloading
                },
            );

            // The whole-app total up front, from the sizes the appmanifest records for the installed depots (an update or a verify
            // touches the same depots): the bar then has one total from the start instead of growing as each depot begins. If a depot
            // has no recorded size the total stays 0 and the per-depot accumulation applies.
            let grand_total_bytes = {
                let sizes = std::fs::read_to_string(&manifest_path)
                    .map(|raw| crate::steam_client::parse_installed_depot_sizes_from_acf(&raw))
                    .unwrap_or_default();
                let per_depot: Vec<u64> = selections.iter().map(|s| sizes.get(&(s.depot_id as u64)).copied().unwrap_or(0)).collect();
                if per_depot.iter().all(|&n| n > 0) { per_depot.iter().sum() } else { 0 }
            };
            // With a total given, the per-depot callbacks no longer add to it, so it has to be set here.
            if grand_total_bytes > 0 {
                if let Ok(mut state) = shared_state_clone.write() {
                    state.total_bytes = grand_total_bytes;
                }
            }

            // Zero grand total: per-depot accumulation.
            let Some(successful_depots) = run_depot_loop(
                &client_clone,
                &connection,
                &tx,
                &shared_state_clone,
                appid,
                selections,
                &install_root,
                &hosts,
                &DepotLoopOpts {
                    verify_mode,
                    grand_total_bytes,
                    manifest_overrides: None,
                },
            )
            .await
            else {
                if let Ok(mut state) = shared_state_clone.write() {
                    state.finish("Operation failed or paused");
                }
                return;
            };

            if let Ok(mut state) = shared_state_clone.write() {
                state.is_downloading = false;
                state.status_text = "Operation complete".to_string();
            }

            let game_name = match recorded_name {
                Some(name) => name,
                None => client_clone.resolve_install_game_name(appid).await,
            };
            let installdir = install_root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| sanitize_install_dir(&game_name));
            let build_id =
                SteamClient::remote_buildid_static(&connection, appid, &active_branch).await;

            if let Err(err) = SteamClient::write_appmanifest(
                &manifest_path,
                appid,
                &game_name,
                &installdir,
                successful_depots,
                build_id.as_deref(),
                true,
                false,
            ) {
                tracing::warn!("failed writing appmanifest for {}: {}", appid, err);
            }
            emit_completed(
                &tx,
                if verify_mode {
                    "verify completed"
                } else {
                    "update completed"
                },
            )
            .await;
        });

        Ok(rx)
    }
}

#[cfg(test)]
mod host_steam_tests {
    use super::needs_host_steam;

    #[test]
    fn a_game_the_account_has_never_opens_the_host_steam_on_its_own() {
        assert!(!needs_host_steam(false, true, true), "in the games list");
        assert!(!needs_host_steam(false, false, true), "off the list (tool or free game), but licensed: Spacewar");
    }

    #[test]
    fn family_sharing_and_an_explicit_request_still_need_it() {
        assert!(needs_host_steam(false, false, false), "no account license: Family Sharing");
        assert!(needs_host_steam(true, true, true), "requested by the user");
    }
}
