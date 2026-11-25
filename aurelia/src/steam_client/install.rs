use super::*;
use crate::core::error::{ErrorKind, TypedError, io_kind, message_kind};

pub(crate) const HOST_POOL_SIZE: usize = 4;

static DOWNLOAD_LIMITER: std::sync::LazyLock<Arc<steam_cdn::RateLimiter>> =
    std::sync::LazyLock::new(|| Arc::new(steam_cdn::RateLimiter::new(0)));

const LIMIT_REFRESH: Duration = Duration::from_secs(5);

async fn refresh_download_limit() {
    if let Ok(cfg) = load_launcher_config().await {
        DOWNLOAD_LIMITER.set_rate(cfg.download_limit_bytes());
    }
}

struct LimitWatcher(tokio::task::JoinHandle<()>);

impl LimitWatcher {
    async fn start() -> Self {
        refresh_download_limit().await;
        Self(tokio::spawn(async {
            loop {
                tokio::time::sleep(LIMIT_REFRESH).await;
                refresh_download_limit().await;
            }
        }))
    }
}

impl Drop for LimitWatcher {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(crate) fn cdn_server_for(
    content: &ContentHost,
    cell_id: u32,
    auth_token: Option<String>,
) -> steam_cdn::web_api::content_service::CDNServer {
    let default_port = if content.https { 443 } else { 80 };
    let (host_name, port) = match content.host.rsplit_once(':') {
        Some((name, port)) => (name, port.parse::<u16>().unwrap_or(default_port)),
        None => (content.host.as_str(), default_port),
    };
    steam_cdn::web_api::content_service::CDNServer {
        r#type: "CDN".to_string(),
        https: content.https || port == 443,
        host: host_name.to_string(),
        vhost: host_name.to_string(),
        port,
        cell_id,
        load: 0,
        weighted_load: 0,
        auth_token,
    }
}

fn aborted_error() -> anyhow::Error {
    anyhow!(TypedError::new(ErrorKind::Cancelled, "download aborted"))
}

pub(crate) fn cdn_error_kind(err: &steam_cdn::Error) -> Option<ErrorKind> {
    match err {
        steam_cdn::Error::Io(io) => io_kind(io),
        other => message_kind(&other.to_string()),
    }
}

#[derive(Default)]
pub struct InstallOptions {
    pub cached_vdf: Option<Vec<u8>>,
    pub filter_depots: Option<Vec<u64>>,
    pub library_override: Option<String>,
    pub manifest_overrides: Option<std::collections::HashMap<u32, u64>>,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchDetail {
    pub name: String,
    pub description: String,
    pub build_id: u64,
    pub time_updated: u32,
    pub private: bool,
}

impl SteamClient {
    pub async fn fetch_branch_details(&self, appid: u32) -> Result<Vec<BranchDetail>> {
        let buffer = self.pics_buffer(appid, "failed requesting appinfo product info for branches").await?;
        let appinfo_vdf_text = String::from_utf8_lossy(&buffer);
        let vdf = steam_vdf_parser::parse_binary(&buffer)
            .or_else(|_| steam_vdf_parser::parse_text(&appinfo_vdf_text).map(|v| v.into_owned()))
            .context("failed parsing appinfo VDF")?;
        let mut out = Vec::new();
        if let Some(branches) = pics_depots_value(&vdf).and_then(|v| v.as_obj()).and_then(|d| d.get("branches")).and_then(|b| b.as_obj()) {
            for (name, node) in branches.iter() {
                let Some(node) = node.as_obj() else { continue };
                let text = |key: &str| node.get(key).and_then(|v| v.as_str()).map(|v| v.to_string()).unwrap_or_default();
                out.push(BranchDetail {
                    name: name.to_string(),
                    description: text("description"),
                    build_id: text("buildid").parse().unwrap_or(0),
                    time_updated: text("timeupdated").parse().unwrap_or(0),
                    private: node.get("pwdrequired").and_then(|v| v.as_str()).is_some_and(|v| v != "0"),
                });
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub async fn fetch_branches(&self, appid: u32) -> Result<Vec<String>> {
        let buffer = self
            .pics_buffer(appid, "failed requesting appinfo product info for branches")
            .await?;
        let appinfo_vdf_text = String::from_utf8_lossy(&buffer);
        let vdf = steam_vdf_parser::parse_binary(&buffer)
            .or_else(|_| steam_vdf_parser::parse_text(&appinfo_vdf_text).map(|v| v.into_owned()))
            .context("failed parsing appinfo VDF")?;
        vdf.as_obj().context("appinfo VDF root is not an object")?;

        let depots = pics_depots_value(&vdf);

        let mut names: Vec<String> = Vec::new();
        if let Some(branches) = depots
            .and_then(|v| v.as_obj())
            .and_then(|d| d.get("branches"))
            .and_then(|b| b.as_obj())
        {
            for (name, node) in branches.iter() {
                let private = node
                    .as_obj()
                    .and_then(|o| o.get("pwdrequired"))
                    .and_then(|v| v.as_str())
                    .is_some_and(|v| v != "0");
                if !private {
                    names.push(name.to_string());
                }
            }
        }

        if !names.iter().any(|n| n == "public") {
            names.push("public".to_string());
        }

        names.sort();
        names.dedup();
        Ok(names)
    }

    pub async fn get_available_platforms(
        &mut self,
        appid: u32,
    ) -> Result<(Vec<DepotPlatform>, Vec<u8>)> {
        let buffer = self
            .pics_buffer(appid, "failed requesting appinfo product info")
            .await?;
        let appinfo_vdf_text = String::from_utf8_lossy(&buffer);

        let mut has_linux = false;
        let mut has_windows = false;

        let vdf_res = steam_vdf_parser::parse_binary(&buffer)
            .or_else(|_| steam_vdf_parser::parse_text(&appinfo_vdf_text).map(|v| v.into_owned()));

        if let Ok(vdf) = vdf_res {
            let depots_val = pics_depots_value(&vdf);

            if let Some(depots) = depots_val.and_then(|v| v.as_obj()) {
                for value in depots.values() {
                    let oslist = value
                        .get_obj(&["config"])
                        .and_then(|c| c.get("oslist"))
                        .and_then(|o| o.as_str());

                    if let Some(os) = oslist {
                        let os = os.to_lowercase();
                        if os.contains("linux") {
                            has_linux = true;
                        }
                        if os.contains("windows") {
                            has_windows = true;
                        }
                    }
                }
            }
        } else {
            tracing::warn!(
                "get_available_platforms: VDF parse failed for {appid}, using fallback discovery"
            );
            return Ok((vec![DepotPlatform::Windows, DepotPlatform::Linux], buffer));
        }

        let mut platforms = Vec::new();
        if has_windows {
            platforms.push(DepotPlatform::Windows);
        }
        if has_linux {
            platforms.push(DepotPlatform::Linux);
        }

        if platforms.is_empty() {
            platforms.push(DepotPlatform::Windows);
        }

        Ok((platforms, buffer))
    }

    pub async fn install_game(
        &self,
        appid: u32,
        platform: DepotPlatform,
        opts: InstallOptions,
        shared_state: Arc<std::sync::RwLock<crate::core::models::DownloadState>>,
    ) -> Result<Receiver<DownloadProgress>> {
        let InstallOptions {
            cached_vdf,
            filter_depots,
            library_override,
            manifest_overrides,
            branch,
        } = opts;
        let connection = self.require_connection_owned()?;

        let cfg = load_launcher_config().await?;
        let library_root = library_override.unwrap_or_else(|| cfg.steam_library_path.clone());
        let (mut game_name, pics_installdir) = self.resolve_install_game_info(appid).await;
        let mut existing_installdir = None;

        let existing_manifest = Path::new(&library_root)
            .join("steamapps")
            .join(format!("appmanifest_{appid}.acf"));
        if let Ok(raw) = std::fs::read_to_string(&existing_manifest) {
            let parsed = crate::core::acf::parse_app_manifest(&raw);
            if let Some(dir) = parsed.install_dir.clone() {
                let existing_dir = Path::new(&library_root)
                    .join("steamapps")
                    .join("common")
                    .join(&dir);
                if existing_dir.is_dir() && crate::core::utils::is_safe_dir_name(&dir) {
                    existing_installdir = Some(dir);
                }
            }
            if game_name.starts_with("App ") {
                if let Some(name) = parsed.display_name().filter(|n| !n.starts_with("App ")) {
                    game_name = name;
                }
            }
        }

        let dlc_parent = self.resolve_dlc_parent(appid).await;
        let dlc_appid = dlc_parent.map(|_| appid);

        let (install_dir, manifest_path, installdir) = if let Some(base_appid) = dlc_parent {
            let base_manifest = self.appmanifest_path(base_appid).await?;
            if !base_manifest.exists() {
                bail!(
                    "cannot install DLC {appid}: its base game (app {base_appid}) is not installed — install it first"
                );
            }
            let base_raw = std::fs::read_to_string(&base_manifest)
                .with_context(|| format!("failed reading {}", base_manifest.display()))?;
            let base_installdir = parse_installdir_from_acf(&base_raw).ok_or_else(|| {
                anyhow!("could not determine base game install dir for app {base_appid}")
            })?;
            let steamapps = base_manifest
                .parent()
                .ok_or_else(|| anyhow!("invalid base manifest path for app {base_appid}"))?;
            let dir = steamapps.join("common").join(&base_installdir);
            tracing::info!(
                "DLC {appid} -> installing into base game {base_appid} at {}",
                dir.display()
            );
            (dir, base_manifest, base_installdir)
        } else {
            let pics_installdir = match pics_installdir {
                Some(dir) => Some(dir),
                None => self.retry_pics_installdir(appid).await,
            };
            let installdir = choose_install_dir(appid, pics_installdir, existing_installdir)?;
            let dir = Path::new(&library_root)
                .join("steamapps")
                .join("common")
                .join(&installdir);
            let mp = Path::new(&library_root)
                .join("steamapps")
                .join(format!("appmanifest_{appid}.acf"));
            (dir, mp, installdir)
        };

        std::fs::create_dir_all(&install_dir)
            .with_context(|| format!("failed creating {}", install_dir.display()))?;
        if let Ok(mut state) = shared_state.write() {
            state.install_dir = Some(install_dir.clone());
        }

        let (tx, rx) = tokio::sync::mpsc::channel(128);
        let client_clone = self.clone();
        let shared_state_clone = shared_state.clone();
        let pin_updates = manifest_overrides.is_some();

        tokio::task::spawn(async move {
            emit_queued(&tx, "").await;

            let appinfo_vdf_bytes_owned;
            let appinfo_vdf_bytes = if let Some(cached) = cached_vdf {
                appinfo_vdf_bytes_owned = cached;
                &appinfo_vdf_bytes_owned
            } else {
                match super::pics_app_buffer(&connection, appid, "failed requesting appinfo").await
                {
                    Ok(buffer) => {
                        appinfo_vdf_bytes_owned = buffer;
                        &appinfo_vdf_bytes_owned
                    }
                    Err(e) => {
                        emit_failed_err(&tx, "appinfo", &e).await;
                        return;
                    }
                }
            };

            let mut selections = Vec::new();
            let mut build_id: Option<String> = None;
            let mut grand_total_bytes: u64 = 0;

            let mut has_windows = false;
            if let Ok(map) = parse_pics_product_info(appinfo_vdf_bytes) {
                if let Ok(vdf) = find_vdf_in_pics(appinfo_vdf_bytes) {
                    let depots_val = pics_depots_value(&vdf);

                    build_id = depots_val
                        .and_then(|d| d.get_obj(&["branches", "public"]))
                        .and_then(|b| b.get("buildid"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());

                    if let Some(depots) = depots_val.and_then(|v| v.as_obj()) {
                        for (key, value) in depots.iter() {
                            if let Ok(d_id) = key.parse::<u32>() {
                                let oslist = value
                                    .get_obj(&["config"])
                                    .and_then(|c| c.get("oslist"))
                                    .and_then(|o| o.as_str());

                                if oslist.is_some_and(|os| os.to_lowercase().contains("windows")) {
                                    has_windows = true;
                                }

                                let override_mid = manifest_overrides
                                    .as_ref()
                                    .and_then(|m| m.get(&d_id).copied());

                                let manifest_id = if let Some(mid) = override_mid {
                                    Some(mid)
                                } else {
                                    let mut match_os = should_keep_depot(oslist, platform);

                                    if match_os {
                                        let lang = value
                                            .get_obj(&["config"])
                                            .and_then(|c| c.get("language"))
                                            .and_then(|l| l.as_str());
                                        if let Some(lang) = lang {
                                            if lang != "english" && !lang.is_empty() {
                                                match_os = false;
                                            }
                                        }
                                    }

                                    let depot_id_u64 = d_id as u64;
                                    let is_allowed = filter_depots
                                        .as_ref()
                                        .is_none_or(|list| list.contains(&depot_id_u64));

                                    if match_os && is_allowed {
                                        map.get(&depot_id_u64).copied()
                                    } else {
                                        None
                                    }
                                };

                                if let Some(manifest_id) = manifest_id {
                                    grand_total_bytes += value
                                        .get_obj(&["manifests", "public"])
                                        .and_then(|m| m.get("size"))
                                        .and_then(|v| v.as_str())
                                        .and_then(|s| s.parse::<u64>().ok())
                                        .or_else(|| {
                                            value
                                                .get("maxsize")
                                                .and_then(|v| v.as_str())
                                                .and_then(|s| s.parse::<u64>().ok())
                                        })
                                        .unwrap_or(0);
                                    selections.push(ManifestSelection {
                                        app_id: appid,
                                        depot_id: d_id,
                                        manifest_id,
                                        appinfo_vdf: String::new(),
                                    });
                                }
                            }
                        }
                    }
                }
            } else {
                tracing::error!("VDF parse failed for app {appid}");
            }

            if selections.is_empty() {
                let msg = if has_windows && matches!(platform, DepotPlatform::Linux) {
                    "No native Linux depots found. This game may only support Windows (Proton)."
                } else {
                    "No matching depots found for the selected platform."
                };

                emit_failed(&tx, msg.to_string()).await;
                return;
            }

            if let Some(branch_name) = branch.as_deref() {
                if let Some(bid) =
                    SteamClient::remote_buildid_static(&connection, appid, branch_name).await
                {
                    build_id = Some(bid);
                }
            }

            let _ = tx
                .send(DownloadProgress {
                    state: DownloadProgressState::Downloading,
                    total_bytes: grand_total_bytes,
                    current_file: format!("starting download of {} depots", selections.len()),
                    ..Default::default()
                })
                .await;

            if let Ok(mut state) = shared_state_clone.write() {
                state.begin(
                    appid,
                    game_name.clone(),
                    grand_total_bytes,
                    format!("Initializing download for {}...", game_name),
                );
            }

            if dlc_appid.is_none() {
                if let Err(e) = SteamClient::write_appmanifest(
                    &manifest_path,
                    appid,
                    &game_name,
                    &installdir,
                    Vec::new(),
                    build_id.as_deref(),
                    false,
                    pin_updates,
                ) {
                    tracing::warn!("failed writing initial appmanifest for app {appid}: {e}");
                } else {
                    tracing::info!(
                        "Registered install start with Steam for app {appid} (buildid {})",
                        build_id.as_deref().unwrap_or("0")
                    );
                }
            }

            spawn_progress_reporter(
                tx.clone(),
                shared_state_clone.clone(),
                DownloadProgressState::Downloading,
            );

            let outcome = match fetch_content_hosts(&client_clone, &connection, &tx, appid).await {
                Some(hosts) => {
                    run_depot_loop(
                        &client_clone,
                        &connection,
                        &tx,
                        &shared_state_clone,
                        appid,
                        selections,
                        &install_dir,
                        &hosts,
                        &DepotLoopOpts {
                            verify_mode: false,
                            grand_total_bytes,
                            manifest_overrides,
                        },
                    )
                    .await
                }
                None => None,
            };
            let Some(successful_depots) = outcome else {
                if let Ok(mut state) = shared_state_clone.write() {
                    state.finish("Download failed");
                }
                return;
            };

            if let Ok(mut state) = shared_state_clone.write() {
                state.is_downloading = false;
                state.status_text = "Download complete".to_string();
            }

            let manifest_result = if let Some(dlc) = dlc_appid {
                SteamClient::enable_dlc_in_appmanifest(&manifest_path, dlc, &successful_depots)
            } else {
                SteamClient::write_appmanifest(
                    &manifest_path,
                    appid,
                    &game_name,
                    &installdir,
                    successful_depots,
                    build_id.as_deref(),
                    true,
                    pin_updates,
                )
            };
            if let Err(err) = manifest_result {
                tracing::warn!("failed updating appmanifest for {}: {}", appid, err);
            } else if dlc_appid.is_none() {
                tracing::info!(
                    "Wrote appmanifest for app {appid}: fully installed, buildid {}",
                    build_id.as_deref().unwrap_or("0")
                );
            }
            emit_completed(&tx, "completed").await;
        });

        Ok(rx)
    }

    pub(crate) async fn download_depot_to(
        &self,
        app_id: u32,
        depot_id: u32,
        manifest_id: u64,
        dest: &std::path::Path,
        shared_state: Arc<std::sync::RwLock<crate::core::models::DownloadState>>,
    ) -> anyhow::Result<u64> {
        let connection = self.require_connection_owned()?;

        let hosts = self.get_content_servers(connection.cell_id()).await?;
        let key = self
            .get_depot_key(app_id, depot_id)
            .await
            .with_context(|| format!("failed to get depot key for depot {depot_id}"))?;

        self.download_depot_to_with_hosts(
            app_id,
            depot_id,
            manifest_id,
            dest,
            shared_state,
            &hosts,
            0,
            &connection,
            key,
            false,
        )
        .await
    }

    pub(crate) async fn download_depot_to_with_hosts(
        &self,
        app_id: u32,
        depot_id: u32,
        manifest_id: u64,
        dest: &std::path::Path,
        shared_state: Arc<std::sync::RwLock<crate::core::models::DownloadState>>,
        hosts: &[ContentHost],
        grand_total_bytes: u64,
        connection: &Connection,
        key: Vec<u8>,
        verify_mode: bool,
    ) -> anyhow::Result<u64> {
        tracing::debug!(
            "Depot {} key: {} bytes, all_zero={}",
            depot_id,
            key.len(),
            key.iter().all(|&b| b == 0)
        );

        let manifest_code = match self
            .get_manifest_request_code(app_id, depot_id, manifest_id)
            .await
        {
            Ok(code) => Some(code),
            Err(e) => {
                tracing::warn!(
                    "Failed to get manifest request code for depot {}: {}",
                    depot_id,
                    e
                );
                None
            }
        };

        let depot_size = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mut depot_success = false;
        let mut last_error: Option<steam_cdn::Error> = None;

        let (base_downloaded, base_reused) = shared_state
            .read()
            .map(|s| (s.downloaded_bytes, s.reused_bytes))
            .unwrap_or_default();

        let _limit = LimitWatcher::start().await;

        for pool in hosts.chunks(HOST_POOL_SIZE) {
            let servers = self
                .cdn_servers(app_id, depot_id, pool, connection.cell_id())
                .await?;
            let host = pool
                .iter()
                .map(|h| h.host.as_str())
                .collect::<Vec<_>>()
                .join(", ");

            if let Ok(mut state) = shared_state.write() {
                state.downloaded_bytes = base_downloaded;
                state.reused_bytes = base_reused;
                state.depot_downloaded_bytes = 0;
            }

            let cdn_client =
                steam_cdn::CDNClient::with_servers(Arc::new(connection.clone()), servers)
                    .with_rate_limiter(DOWNLOAD_LIMITER.clone());

            let state_for_closure = shared_state.clone();
            let on_progress: steam_cdn::ProgressFn = Arc::new(move |bytes: u64, reused: bool| {
                if let Ok(mut state) = state_for_closure.write() {
                    state.downloaded_bytes += bytes;
                    state.depot_downloaded_bytes += bytes;
                    if reused {
                        state.reused_bytes += bytes;
                    }
                }
            });

            let state_for_manifest = shared_state.clone();
            let size_clone = depot_size.clone();
            let grand_total_fallback = grand_total_bytes;
            let on_manifest = Arc::new(move |total_bytes: u64| {
                size_clone.store(total_bytes, std::sync::atomic::Ordering::SeqCst);
                if let Ok(mut state) = state_for_manifest.write() {
                    state.depot_total_bytes = total_bytes;
                    if grand_total_fallback == 0 {
                        state.total_bytes += total_bytes;
                    }
                }
            });

            let abort_signal = shared_state.read().ok().map(|s| s.abort_signal.clone());

            match cdn_client
                .download_depot(
                    app_id,
                    depot_id,
                    manifest_id,
                    &key,
                    dest,
                    manifest_code,
                    verify_mode,
                    abort_signal,
                    Some(on_progress),
                    Some(on_manifest.clone()),
                    Some(Arc::new(move |files: Vec<String>| {
                        crate::launch::install_scripts::save_depot(app_id, depot_id, files)
                    })),
                )
                .await
            {
                Ok(_) => {
                    if download_aborted(&shared_state) {
                        return Err(aborted_error());
                    }

                    tracing::info!("Depot {} download complete from {}!", depot_id, host);
                    depot_success = true;
                    break;
                }
                Err(e) => {
                    tracing::error!("CDN Error from {}: {}", host, e);
                    if download_aborted(&shared_state) {
                        return Err(aborted_error());
                    }
                    if cdn_error_kind(&e) == Some(ErrorKind::DiskFull) {
                        return Err(anyhow!(TypedError::new(
                            ErrorKind::DiskFull,
                            format!("writing depot {depot_id} to {}: {e}", dest.display()),
                        )));
                    }
                    last_error = Some(e);
                }
            }
        }

        if depot_success {
            Ok(depot_size.load(std::sync::atomic::Ordering::SeqCst))
        } else {
            let detail = last_error
                .as_ref()
                .map(|e| format!(" (last: {e})"))
                .unwrap_or_default();
            let message =
                format!("Failed to download depot {depot_id} from all available servers{detail}");
            match last_error.as_ref().and_then(cdn_error_kind) {
                Some(kind) => Err(anyhow!(TypedError::new(kind, message))),
                None => Err(anyhow!(message)),
            }
        }
    }
}
