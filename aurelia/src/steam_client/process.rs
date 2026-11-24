use super::*;

impl SteamClient {
    #[cfg(target_os = "windows")]
    pub fn steam_is_running() -> bool {
        use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
        let Some(pid) = read_registry(r"HKCU\Software\Valve\Steam\ActiveProcess", "pid")
            .or_else(|| read_steam_registry("SteamPID"))
            .and_then(|v| parse_reg_dword(&v))
            .filter(|&pid| pid != 0)
        else {
            return false;
        };
        let pid = Pid::from_u32(pid);
        let mut sys = System::new();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing(),
        );
        sys.process(pid)
            .is_some_and(|p| p.name().to_string_lossy().eq_ignore_ascii_case("steam.exe"))
    }

    #[cfg(target_os = "windows")]
    pub fn steam_is_logged_on() -> bool {
        read_registry(r"HKCU\Software\Valve\Steam\ActiveProcess", "ActiveUser")
            .and_then(|v| parse_reg_dword(&v))
            .is_some_and(|user| user != 0)
    }

    #[cfg(target_os = "windows")]
    pub async fn ensure_host_steam_ready(timeout: std::time::Duration) {
        if SteamClient::steam_is_running() {
            tracing::info!("Steam client already running; not starting it");
        } else {
            tracing::info!("Steam client not running; starting `steam.exe -silent` (tray)");
            if let Err(e) = SteamClient::start_steam() {
                tracing::warn!("could not start Steam client: {e:#}");
                return;
            }
        }
        let start = std::time::Instant::now();
        while !(SteamClient::steam_is_running() && SteamClient::steam_is_logged_on()) {
            if start.elapsed() >= timeout {
                tracing::warn!(
                    "Steam not logged on within {}s; launching anyway (Steamworks auth may fail)",
                    timeout.as_secs()
                );
                return;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        tracing::info!("host Steam logged on; proceeding with launch");
    }

    #[cfg(target_os = "linux")]
    pub fn steam_is_running() -> bool {
        crate::core::utils::is_steam_running()
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    pub fn steam_is_running() -> bool {
        false
    }

    #[cfg(target_os = "windows")]
    pub fn shutdown_steam() -> Result<()> {
        if !SteamClient::steam_is_running() {
            return Ok(());
        }
        let exe = steam_exe_path().context("could not locate steam.exe to stop Steam")?;
        Command::new(&exe)
            .arg("-shutdown")
            .spawn()
            .context("failed to signal Steam shutdown")?;
        for _ in 0..60 {
            if !SteamClient::steam_is_running() {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        bail!("Steam did not shut down within 30s")
    }

    #[cfg(target_os = "windows")]
    pub fn start_steam() -> Result<()> {
        let exe = steam_exe_path().context("could not locate steam.exe to start Steam")?;
        Command::new(&exe)
            .arg("-silent")
            .spawn()
            .context("failed to start Steam")?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub fn shutdown_steam() -> Result<()> {
        if !SteamClient::steam_is_running() {
            return Ok(());
        }
        crate::core::utils::start_steam_detached(&["-shutdown"])
            .context("failed to signal Steam shutdown")?;
        let stopped = crate::core::utils::wait_until(
            || !SteamClient::steam_is_running(),
            std::time::Duration::from_secs(30),
            std::time::Duration::from_millis(250),
        );
        if !stopped {
            bail!("Steam did not shut down within 30s");
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub fn start_steam() -> Result<()> {
        crate::core::utils::start_steam_detached(&["-silent"]).context("failed to start Steam")
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    pub fn shutdown_steam() -> Result<()> {
        bail!("automatic Steam control is only supported on Windows and Linux")
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    pub fn start_steam() -> Result<()> {
        bail!("automatic Steam control is only supported on Windows and Linux")
    }

    pub fn stop_game(app_id: u32, force: bool) -> Result<crate::compat::running::RunningGame> {
        let record = crate::compat::running::load(app_id).ok_or_else(|| {
            anyhow!("app {app_id} is not running (no launch was recorded by Aurelia)")
        })?;
        let record = crate::compat::running::reconcile(record).ok_or_else(|| {
            anyhow!("app {app_id} is not running (cleared a stale launch record)")
        })?;
        let owns_pid = record.owns_pid();

        #[cfg(unix)]
        if let Some(prefix) = record.wineprefix.as_deref() {
            Self::kill_wine_processes_in_prefix(prefix, force);
        }

        #[cfg(unix)]
        Self::kill_processes_for_app(app_id, force);

        if owns_pid {
            kill_process_tree(record.pid, force);
        }
        crate::compat::running::clear(app_id);
        Ok(record)
    }

    #[cfg(unix)]
    pub fn kill_processes_for_app(app_id: u32, force: bool) {
        let needle = format!("STEAM_COMPAT_APP_ID={app_id}");

        let pids = Self::collect_proc_pids(|pid_path| {
            std::fs::read(pid_path.join("environ")).is_ok_and(|environ| {
                environ
                    .split(|&b| b == 0)
                    .any(|entry| entry == needle.as_bytes())
            })
        });
        kill_or_escalate(&pids, force);
    }

    #[cfg(unix)]
    fn scan_proc_pids<T>(mut f: impl FnMut(&Path, &str) -> Option<T>) -> Option<T> {
        let proc_dir = std::fs::read_dir("/proc").ok()?;
        for entry in proc_dir.flatten() {
            let pid_path = entry.path();
            let Some(pid_str) = pid_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !pid_str.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            if let Some(out) = f(&pid_path, pid_str) {
                return Some(out);
            }
        }
        None
    }

    #[cfg(unix)]
    fn proc_environ(pid_path: &Path) -> Option<String> {
        let bytes = std::fs::read(pid_path.join("environ")).ok()?;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    #[cfg(unix)]
    fn proc_cmdline(pid_path: &Path) -> Option<String> {
        let bytes = std::fs::read(pid_path.join("cmdline")).ok()?;
        Some(String::from_utf8_lossy(&bytes).replace('\0', " "))
    }

    #[cfg(unix)]
    fn collect_proc_pids(mut pred: impl FnMut(&Path) -> bool) -> Vec<i32> {
        let mut pids = Vec::new();
        Self::scan_proc_pids(|pid_path, pid_str| {
            if pred(pid_path) {
                if let Ok(pid) = pid_str.parse::<i32>() {
                    pids.push(pid);
                }
            }
            None::<()>
        });
        pids
    }

    #[cfg(unix)]
    pub fn kill_wine_processes_in_prefix(wineprefix: &Path, force: bool) {
        let prefix_str = wineprefix.to_string_lossy().to_string();

        let pids = Self::collect_proc_pids(|pid_path| {
            Self::proc_environ(pid_path).is_some_and(|env| env.contains(&prefix_str))
        });
        kill_or_escalate(&pids, force);
    }

    pub fn kill_steam_in_prefix(wineprefix: &Path) {
        #[cfg(unix)]
        {
            let prefix_str = wineprefix.to_string_lossy().to_string();

            let pids = Self::collect_proc_pids(|pid_path| {
                let Some(cmdline) = Self::proc_cmdline(pid_path) else {
                    return false;
                };
                let cmdline = cmdline.to_lowercase();
                (cmdline.contains("steam.exe")
                    || cmdline.contains("steamwebhelper.exe")
                    || cmdline.contains("steamservice.exe"))
                    && Self::proc_environ(pid_path).is_some_and(|env| env.contains(&prefix_str))
            });

            terminate_pids_with_escalation(&pids);
        }
        #[cfg(not(unix))]
        {
            let _ = wineprefix;
        }
    }

    pub fn kill_stale_wineservers_in_prefix(
        wineprefix: &Path,
        allowed_runner_roots: &[PathBuf],
    ) -> usize {
        #[cfg(unix)]
        {
            if allowed_runner_roots.is_empty() {
                return 0;
            }
            let mut stale: Vec<i32> = Vec::new();
            Self::scan_proc_pids(|pid_path, pid_str| {
                let exe = std::fs::read_link(pid_path.join("exe")).ok()?;
                let environ = std::fs::read(pid_path.join("environ")).ok()?;
                if is_stale_cross_runner_wineserver(&exe, &environ, wineprefix, allowed_runner_roots)
                {
                    if let Ok(pid) = pid_str.parse::<i32>() {
                        tracing::warn!(
                            pid,
                            exe = %exe.display(),
                            prefix = %wineprefix.display(),
                            "killing stale cross-runner wineserver holding the prefix"
                        );
                        stale.push(pid);
                    }
                }
                None::<()>
            });
            if !stale.is_empty() {
                terminate_pids_with_escalation(&stale);
            }
            stale.len()
        }
        #[cfg(not(unix))]
        {
            let _ = (wineprefix, allowed_runner_roots);
            0
        }
    }

    pub fn enforce_disabled_steam_features_in_prefix(
        wineprefix: &Path,
        no_browser: bool,
        no_friends_ui: bool,
        no_overlay: bool,
        no_chat_ui: bool,
    ) {
        #[cfg(unix)]
        {
            if !(no_browser || no_friends_ui || no_overlay || no_chat_ui) {
                return;
            }
            let prefix_str = wineprefix.to_string_lossy().to_string();
            let mut doomed: Vec<i32> = Vec::new();

            Self::scan_proc_pids(|pid_path, pid_str| {
                let argv = match std::fs::read(pid_path.join("cmdline")) {
                    Ok(b) => cmdline_argv(&b),
                    Err(_) => return None,
                };
                if !disabled_helper_kill_match(&argv, no_browser, no_friends_ui, no_overlay, no_chat_ui)
                {
                    return None;
                }
                let environ = match std::fs::read(pid_path.join("environ")) {
                    Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                    Err(_) => return None,
                };
                if !environ.contains(&prefix_str) {
                    return None;
                }
                if let Ok(pid) = pid_str.parse::<i32>() {
                    doomed.push(pid);
                }
                None::<()>
            });

            for pid in doomed {
                tracing::info!(pid, "enforcing disabled Steam feature: killing helper process");
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (wineprefix, no_browser, no_friends_ui, no_overlay, no_chat_ui);
        }
    }

    pub fn is_shader_processing_in_prefix(wineprefix: &Path) -> bool {
        #[cfg(unix)]
        {
            let prefix_str = wineprefix.to_string_lossy().to_string();
            Self::scan_proc_pids(|pid_path, _pid_str| {
                let cmdline = Self::proc_cmdline(pid_path)?.to_lowercase();
                if !(cmdline.contains("fossilize-replay") || cmdline.contains("fossilize_replay")) {
                    return None;
                }
                Self::proc_environ(pid_path)?
                    .contains(&prefix_str)
                    .then_some(true)
            })
            .unwrap_or(false)
        }
        #[cfg(not(unix))]
        {
            let _ = wineprefix;
            false
        }
    }

    pub fn is_steam_running_in_prefix(wineprefix: &Path) -> bool {
        #[cfg(unix)]
        {
            let prefix_str = wineprefix.to_string_lossy().to_string();

            Self::scan_proc_pids(|pid_path, _pid_str| {
                if !Self::proc_cmdline(pid_path)?
                    .to_lowercase()
                    .contains("steam.exe")
                {
                    return None;
                }

                Self::proc_environ(pid_path)?
                    .contains(&prefix_str)
                    .then_some(true)
            })
            .unwrap_or(false)
        }
        #[cfg(not(unix))]
        {
            let _ = wineprefix;
            false
        }
    }

    pub fn is_game_running_in_prefix(wineprefix: &Path, installdir: &str) -> bool {
        #[cfg(unix)]
        {
            let prefix_str = wineprefix.to_string_lossy().to_string();
            let needle = installdir.to_lowercase();
            if needle.is_empty() {
                return false;
            }
            Self::scan_proc_pids(|pid_path, _pid_str| {
                let cmdline_str = Self::proc_cmdline(pid_path)?.to_lowercase();
                if !(cmdline_str.contains(&needle) && cmdline_str.contains(".exe")) {
                    return None;
                }
                if cmdline_str.contains("steam.exe") || cmdline_str.contains("steamwebhelper") {
                    return None;
                }
                Self::proc_environ(pid_path)?
                    .contains(&prefix_str)
                    .then_some(true)
            })
            .unwrap_or(false)
        }
        #[cfg(not(unix))]
        {
            let _ = (wineprefix, installdir);
            false
        }
    }

    pub fn write_headless_steam_cfg(steam_dir: &Path) {
        let cfg_path = steam_dir.join("steam.cfg");
        if cfg_path.exists() {
            return;
        }
        let content = "\
BootStrapperForceSelfUpdate=disable
SteamDefaultDialog=Friends
NoSavePersonalInfo=1
";
        let _ = std::fs::write(&cfg_path, content);
    }

    async fn resolve_exec_plan(
        &self,
        app: &LibraryGame,
        launch_info: &LaunchInfo,
        user_config: Option<&crate::core::models::UserAppConfig>,
    ) -> Result<(PathBuf, PathBuf, Vec<String>, PathBuf)> {
        let install_dir = match app.install_path.as_ref().map(PathBuf::from) {
            Some(p) if p.exists() => p,
            _ => self.install_root_for_app(app.app_id).await?,
        };

        let exe_relative = launch_info.executable.replace('\\', "/");
        let executable = install_dir.join(&exe_relative);
        let mut args = split_args(&launch_info.arguments);

        if let Some(config) = user_config {
            if !config.launch_options.trim().is_empty() {
                args.extend(split_args(&config.launch_options));
            }
        }

        // Workingdir, exe parent, install dir.
        let game_working_dir: PathBuf = launch_info
            .workingdir
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|wd| install_dir.join(wd.replace('\\', "/")))
            .or_else(|| executable.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| install_dir.clone());

        Ok((install_dir, executable, args, game_working_dir))
    }

    pub(crate) async fn spawn_windows_native(
        &self,
        app: &LibraryGame,
        launch_info: &LaunchInfo,
        user_config: Option<&crate::core::models::UserAppConfig>,
    ) -> Result<crate::launch::game_process::GameProcess> {
        let (_install_dir, executable, args, game_working_dir) = self
            .resolve_exec_plan(app, launch_info, user_config)
            .await?;

        crate::core::utils::ensure_steam_appid_logged(&game_working_dir, app.app_id);

        let mut cmd = Command::new(&executable);
        cmd.args(&args);
        cmd.current_dir(&game_working_dir);
        cmd.env("SteamAppId", app.app_id.to_string());

        if let Some(config) = user_config {
            for (key, val) in &config.env_variables {
                cmd.env(key, val);
            }
        }

        cmd.stdout(crate::infra::logging::relay::child_stdio());
        cmd.stderr(crate::infra::logging::relay::child_stdio());

        tracing::info!(
            "Launching game (Native Windows): {:?} with args {:?}",
            executable,
            args
        );
        match cmd.spawn() {
            Ok(child) => Ok(crate::infra::logging::relay::attach(child).into()),
            #[cfg(windows)]
            Err(e) if crate::launch::game_process::needs_elevation(&e) => {
                self.spawn_windows_elevated(app, executable, args, game_working_dir, user_config)
                    .await
            }
            Err(e) => Err(e)
                .with_context(|| format!("failed to spawn windows game {}", executable.display())),
        }
    }

    #[cfg(windows)]
    async fn spawn_windows_elevated(
        &self,
        app: &LibraryGame,
        executable: PathBuf,
        args: Vec<String>,
        game_working_dir: PathBuf,
        user_config: Option<&crate::core::models::UserAppConfig>,
    ) -> Result<crate::launch::game_process::GameProcess> {
        tracing::info!(
            appid = app.app_id,
            "{} requires administrator rights; requesting elevation (UAC prompt)",
            executable.display()
        );
        let custom_env = user_config.is_some_and(|c| !c.env_variables.is_empty());
        let dropped = if custom_env {
            "SteamAppId and per-game environment variables"
        } else {
            "SteamAppId"
        };
        tracing::warn!(
            appid = app.app_id,
            "Elevated launch: {dropped} do not reach the game; it relies on steam_appid.txt"
        );
        let exe = executable.clone();
        let process = tokio::task::spawn_blocking(move || {
            crate::launch::game_process::elevated::spawn(&exe, &args, &game_working_dir)
        })
        .await
        .context("elevated launch task failed")?
        .with_context(|| format!("failed to launch {} as administrator", executable.display()))?;
        Ok(crate::launch::game_process::GameProcess::Elevated(process))
    }

    pub(crate) async fn spawn_game_process(
        &self,
        app: &LibraryGame,
        launch_info: &LaunchInfo,
        proton_path: Option<&str>,
        launcher_config: &crate::core::config::LauncherConfig,
        user_config: Option<&crate::core::models::UserAppConfig>,
        force_native_engine: bool,
        force_umu: bool,
        launch_script_override: Option<PathBuf>,
        disable_launch_script: bool,
        steam_enabled: bool,
        shaders: crate::launch::shaders::ShaderLaunch,
    ) -> Result<std::process::Child> {
        use crate::infra::logging::{EventLogger, LaunchSession};
        use crate::launch::pipeline::{LaunchPipeline, PipelineContext};

        let mut ctx = PipelineContext::new(app.app_id);
        ctx.app = Some(app.clone());
        ctx.launch_info = Some(launch_info.clone());
        ctx.launcher_config = Some(launcher_config.clone());
        ctx.user_config = user_config.cloned();
        ctx.proton_path = proton_path.map(|s| s.to_string());
        ctx.force_native_engine = force_native_engine;
        ctx.force_umu = force_umu;
        ctx.launch_script_override = launch_script_override;
        ctx.disable_launch_script = disable_launch_script;
        ctx.steam_enabled = steam_enabled;
        ctx.skip_shaders = shaders.skip;
        ctx.on_shader_event = shaders.on_event;

        if let Ok(config_dir) = crate::core::config::config_dir() {
            let session = LaunchSession::new(&config_dir.join("logs"));
            if let Ok(logger) = EventLogger::new(&session) {
                ctx.session = Some(session);
                ctx.logger = Some(logger);
            }
        }

        let pipeline = LaunchPipeline::with_default_stages();
        pipeline.run(&mut ctx).await.map_err(|e| anyhow!(e))?;

        ctx.child
            .ok_or_else(|| anyhow!("Pipeline finished without spawning a process"))
    }

    pub async fn internal_legacy_launch_adhoc(
        &self,
        app: &LibraryGame,
        launch_info: &LaunchInfo,
        _proton_path: Option<&str>,
        _launcher_config: &crate::core::config::LauncherConfig,
        user_config: Option<&crate::core::models::UserAppConfig>,
    ) -> Result<std::process::Child> {
        let (install_dir, executable, args, game_working_dir) = self
            .resolve_exec_plan(app, launch_info, user_config)
            .await?;

        match launch_info.target {
            LaunchTarget::NativeLinux => {
                crate::core::utils::ensure_steam_appid_logged(&game_working_dir, app.app_id);

                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(metadata) = std::fs::metadata(&executable) {
                        let mut perms = metadata.permissions();
                        perms.set_mode(0o755);
                        let _ = std::fs::set_permissions(&executable, perms);
                    }
                }

                let mut cmd = Command::new(&executable);
                cmd.args(&args);
                cmd.current_dir(&install_dir);

                let bin_dir = executable.parent().unwrap_or_else(|| Path::new("."));
                let existing_ld = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
                let existing_path = std::env::var("PATH").unwrap_or_default();

                cmd.env(
                    "LD_LIBRARY_PATH",
                    format!("{}:{}", bin_dir.display(), existing_ld),
                );
                cmd.env("PATH", format!("{}:{}", bin_dir.display(), existing_path));
                cmd.env("SteamAppId", app.app_id.to_string());

                if let Some(config) = user_config {
                    for (key, val) in &config.env_variables {
                        cmd.env(key, val);
                    }
                }

                cmd.stdout(crate::infra::logging::relay::child_stdio());
                cmd.stderr(crate::infra::logging::relay::child_stdio());

                tracing::info!(
                    "Launching game (Native): {:?} with args {:?}",
                    executable,
                    args
                );
                cmd.spawn()
                    .map(crate::infra::logging::relay::attach)
                    .context("failed to spawn native linux game")
            }
            LaunchTarget::WindowsProton => {
                bail!(
                    "WindowsProton targets must be launched via the Pipeline and Runner abstraction. Ad-hoc bypass is prohibited."
                );
            }
        }
    }
}

#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn cmdline_argv(raw: &[u8]) -> Vec<String> {
    raw.split(|&b| b == 0)
        .filter(|e| !e.is_empty())
        .map(|e| String::from_utf8_lossy(e).into_owned())
        .collect()
}

#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn is_stale_cross_runner_wineserver(
    exe_path: &Path,
    environ: &[u8],
    target_prefix: &Path,
    allowed_runner_roots: &[PathBuf],
) -> bool {
    let is_wineserver = exe_path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("wineserver"));
    if !is_wineserver {
        return false;
    }

    let target = target_prefix.to_string_lossy();
    let target = target.trim_end_matches('/');
    let prefix_matches = environ.split(|&b| b == 0).any(|entry| {
        entry
            .strip_prefix(&b"WINEPREFIX="[..])
            .is_some_and(|v| String::from_utf8_lossy(v).trim_end_matches('/') == target)
    });
    if !prefix_matches {
        return false;
    }

    !allowed_runner_roots.iter().any(|root| exe_path.starts_with(root))
}

#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn disabled_helper_kill_match(
    argv: &[String],
    no_browser: bool,
    no_friends_ui: bool,
    no_overlay: bool,
    no_chat_ui: bool,
) -> bool {
    if (no_browser || no_friends_ui || no_chat_ui)
        && argv.iter().any(|a| a.to_lowercase().contains("steamwebhelper.exe"))
    {
        return true;
    }
    if no_overlay {
        if let Some(first) = argv.first() {
            let base = first.replace('\\', "/");
            let base = base.rsplit('/').next().unwrap_or("");
            if base.eq_ignore_ascii_case("gameoverlayui.exe") {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environ(entries: &[&str]) -> Vec<u8> {
        let mut buf = Vec::new();
        for e in entries {
            buf.extend_from_slice(e.as_bytes());
            buf.push(0);
        }
        buf
    }

    #[test]
    fn stale_wineserver_requires_wineserver_binary() {
        let env = environ(&["WINEPREFIX=/home/u/pfx"]);
        assert!(!is_stale_cross_runner_wineserver(
            Path::new("/opt/other-wine/bin/wine64"),
            &env,
            Path::new("/home/u/pfx"),
            &[PathBuf::from("/opt/wine-tkg")],
        ));
    }

    #[test]
    fn stale_wineserver_requires_exact_prefix_match() {
        let env = environ(&["WINEPREFIX=/home/u/pfx2"]);
        assert!(!is_stale_cross_runner_wineserver(
            Path::new("/opt/other-wine/bin/wineserver"),
            &env,
            Path::new("/home/u/pfx"),
            &[PathBuf::from("/opt/wine-tkg")],
        ));
        let env = environ(&["HOME=/home/u"]);
        assert!(!is_stale_cross_runner_wineserver(
            Path::new("/opt/other-wine/bin/wineserver"),
            &env,
            Path::new("/home/u/pfx"),
            &[PathBuf::from("/opt/wine-tkg")],
        ));
    }

    #[test]
    fn stale_wineserver_tolerates_trailing_slash() {
        let env = environ(&["WINEPREFIX=/home/u/pfx/"]);
        assert!(is_stale_cross_runner_wineserver(
            Path::new("/opt/other-wine/bin/wineserver"),
            &env,
            Path::new("/home/u/pfx"),
            &[PathBuf::from("/opt/wine-tkg")],
        ));
    }

    #[test]
    fn same_runner_wineserver_is_not_stale() {
        let env = environ(&["WINEPREFIX=/home/u/pfx"]);
        assert!(!is_stale_cross_runner_wineserver(
            Path::new("/opt/wine-tkg/bin/wineserver"),
            &env,
            Path::new("/home/u/pfx"),
            &[PathBuf::from("/opt/wine-tkg")],
        ));
        assert!(!is_stale_cross_runner_wineserver(
            Path::new("/opt/proton/files/bin/wineserver"),
            &env,
            Path::new("/home/u/pfx"),
            &[PathBuf::from("/opt/wine-tkg"), PathBuf::from("/opt/proton/files")],
        ));
    }

    #[test]
    fn cross_runner_wineserver_is_stale() {
        let env = environ(&["WINEPREFIX=/home/u/pfx", "DISPLAY=:0"]);
        assert!(is_stale_cross_runner_wineserver(
            Path::new("/opt/other-wine/bin/wineserver"),
            &env,
            Path::new("/home/u/pfx"),
            &[PathBuf::from("/opt/wine-tkg")],
        ));
        assert!(is_stale_cross_runner_wineserver(
            Path::new("/opt/other-wine/bin/wineserver (deleted)"),
            &env,
            Path::new("/home/u/pfx"),
            &[PathBuf::from("/opt/wine-tkg")],
        ));
    }

    #[test]
    fn webhelper_matched_by_cmdline_substring() {
        let argv = vec![
            "C:\\Program Files (x86)\\Steam\\bin\\cef\\cef.win7x64\\steamwebhelper.exe".to_string(),
            "--type=renderer".to_string(),
        ];
        assert!(disabled_helper_kill_match(&argv, true, false, false, false));
        assert!(disabled_helper_kill_match(&argv, false, true, false, false));
        assert!(disabled_helper_kill_match(&argv, false, false, false, true));
        assert!(!disabled_helper_kill_match(&argv, false, false, true, false));
        assert!(!disabled_helper_kill_match(&argv, false, false, false, false));
    }

    #[test]
    fn overlay_matched_by_exact_argv0_basename_only() {
        let overlay = vec!["C:\\Steam\\gameoverlayui.exe".to_string(), "-pid".to_string()];
        assert!(disabled_helper_kill_match(&overlay, false, false, true, false));
        assert!(!disabled_helper_kill_match(&overlay, true, true, false, true));

        let game = vec![
            "C:\\game\\game.exe".to_string(),
            "-watchdog=gameoverlayui.exe".to_string(),
        ];
        assert!(!disabled_helper_kill_match(&game, false, false, true, false));

        let steam = vec!["C:\\Steam\\steam.exe".to_string(), "-silent".to_string()];
        assert!(!disabled_helper_kill_match(&steam, true, true, true, true));
    }

    #[test]
    fn cmdline_argv_splits_on_nul() {
        let raw = b"C:\\Steam\\steam.exe\0-silent\0\0";
        assert_eq!(cmdline_argv(raw), vec!["C:\\Steam\\steam.exe", "-silent"]);
    }
}
