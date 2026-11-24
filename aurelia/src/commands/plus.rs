use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tokio::io::AsyncBufReadExt;

const BRIDGE_READY: std::time::Duration = std::time::Duration::from_secs(180);
const BRIDGE_CLOSE: std::time::Duration = std::time::Duration::from_secs(120);
pub(crate) const WRAPPER_NAME: &str = "aurelia-plus-wrapper";
const PASS_THROUGH: &[&str] = &[
    "AURELIA_SHIM_LOG",
    "AURELIA_SHIM_TRACE",
    "AURELIA_SHIM_TICKET_FILE",
    "AURELIA_SHIM_OPEN_CHECKOUT",
    "AURELIA_SHIM_GNS",
    "AURELIA_SHIM_NO_BROWSER",
    "AURELIA_PLUS_PROBE_EXE",
    "AURELIA_PLUS_WINEDEBUG",
    "AURELIA_PLUS_GAME_ARGS",
    "AURELIA_PLUS_GAME_ENV",
];

#[cfg(aurelia_plus_shim)]
const SHIM: &[(&str, &[u8])] = &[
    ("steamclient64.dll", include_bytes!(concat!(env!("OUT_DIR"), "/plus_shim/steamclient64.dll"))),
    ("steamclient.dll", include_bytes!(concat!(env!("OUT_DIR"), "/plus_shim/steamclient.dll"))),
    ("windows.ui.core.textinput.dll", include_bytes!(concat!(env!("OUT_DIR"), "/plus_shim/windows.ui.core.textinput.dll"))),
    ("windows.ui.core.textinput32.dll", include_bytes!(concat!(env!("OUT_DIR"), "/plus_shim/windows.ui.core.textinput32.dll"))),
    ("Steam.exe", include_bytes!(concat!(env!("OUT_DIR"), "/plus_shim/Steam.exe"))),
];
#[cfg(not(aurelia_plus_shim))]
const SHIM: &[(&str, &[u8])] = &[];

fn plus_home() -> Result<PathBuf> {
    Ok(aurelia::core::config::data_dir()?.join("plus"))
}

fn install(home: &Path, exe: &Path) -> Result<(PathBuf, PathBuf)> {
    if SHIM.is_empty() {
        bail!("this Aurelia build does not include Aurelia Plus (its shim was not embedded when it was built)");
    }
    let steam = home.join("shimsteam");
    let legacy = steam.join("legacycompat");
    std::fs::create_dir_all(&legacy).with_context(|| format!("could not create {}", legacy.display()))?;
    for (name, bytes) in SHIM {
        let path = legacy.join(name);
        if std::fs::read(&path).ok().as_deref() != Some(*bytes) {
            let tmp = legacy.join(format!(".{name}.{}.tmp", std::process::id()));
            std::fs::write(&tmp, bytes).with_context(|| format!("could not write {}", tmp.display()))?;
            std::fs::rename(&tmp, &path)?;
        }
    }
    let link = home.join(WRAPPER_NAME);
    if std::fs::read_link(&link).ok().as_deref() != Some(exe) {
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(exe, &link).with_context(|| format!("could not link {}", link.display()))?;
    }
    Ok((steam, link))
}

async fn shortcut_command(sc: &crate::commands::shortcut::Shortcut, wrapper: &Path, id: &str, proton: Option<&str>, json: bool) -> Result<tokio::process::Command> {
    let cfg = aurelia::core::config::load_launcher_config().await?;
    let wanted = proton.map(str::to_string).or_else(|| sc.proton.clone()).unwrap_or_else(|| cfg.proton_version.clone());
    let installed = aurelia::compat::proton::list_installed(Path::new(&cfg.steam_library_path));
    let lower = wanted.to_ascii_lowercase();
    let runner = installed
        .iter()
        .find(|i| i.name.eq_ignore_ascii_case(&wanted))
        .or_else(|| installed.iter().find(|i| i.name.to_ascii_lowercase().contains(&lower)))
        .with_context(|| format!("Proton '{wanted}' is not installed (see `aurelia proton list --installed`)"))?;
    let data = sc.compat_data()?;
    std::fs::create_dir_all(&data).with_context(|| format!("could not create {}", data.display()))?;
    let dir = sc.dir();
    let mut cmd = tokio::process::Command::new(wrapper);
    cmd.arg(runner.path.join("proton")).arg("run").arg(&sc.exe).args(&sc.args).current_dir(&dir);
    let api = sc.api_app_id().map_or_else(|| id.to_string(), |a| a.to_string());
    cmd.env("AURELIA_APP_ID", id)
        .env("SteamGameId", id)
        .env("SteamAppId", &api)
        .env("STEAM_COMPAT_APP_ID", &api)
        .env("STEAM_COMPAT_DATA_PATH", &data)
        .env("STEAM_COMPAT_INSTALL_PATH", &dir)
        .env("STEAM_COMPAT_MOUNTS", &dir)
        .env("SteamDeck", "1");
    if json {
        crate::commands::common::eprint_json_line(&serde_json::json!({ "event": "message", "message": format!("Launching shortcut {} with {}", sc.name, runner.name) }));
    }
    Ok(cmd)
}

fn launch_file(home: &Path, app_id: &str) -> PathBuf {
    home.join(format!("launch-{app_id}.json"))
}

fn steam_language(lang: &str) -> &'static str {
    let lang = lang.to_ascii_lowercase();
    let pairs: &[(&str, &str)] = &[
        ("pt_br", "brazilian"),
        ("pt", "portuguese"),
        ("de", "german"),
        ("fr", "french"),
        ("es_es", "spanish"),
        ("es", "latam"),
        ("it", "italian"),
        ("ru", "russian"),
        ("pl", "polish"),
        ("ja", "japanese"),
    ];
    pairs.iter().find(|(p, _)| lang.starts_with(p)).map_or("english", |(_, l)| l)
}

fn launch_vars(port: u16, token: &str, app_id: u32, steam_dir: &Path, extra: &[(String, String)]) -> BTreeMap<String, String> {
    let mut vars: BTreeMap<String, String> = extra.iter().cloned().collect();
    vars.insert("AURELIA_BRIDGE_PORT".into(), port.to_string());
    vars.insert("AURELIA_BRIDGE_TOKEN".into(), token.into());
    vars.insert("SteamOverlayGameId".into(), app_id.to_string());
    vars.insert("SteamClientLaunch".into(), "1".into());
    vars.insert("SteamEnv".into(), "1".into());
    vars.insert("AURELIA_PLUS_STEAM_DIR".into(), steam_dir.to_string_lossy().into_owned());
    vars
}

pub(crate) async fn cmd_play_plus(app_id: u32, proton: Option<String>, launch_option: Option<String>, json: bool) -> Result<()> {
    if aurelia::core::utils::is_steam_running() {
        bail!("Steam is running; close it before `play --plus` (it takes the game's Steam authorization)");
    }
    let exe = std::env::current_exe().context("could not locate the aurelia binary")?;
    let home = plus_home()?;
    let (steam_dir, wrapper) = install(&home, &exe)?;

    let id = app_id.to_string();
    let Some(mut bridge) = crate::daemon::client::open(&["--json", "bridge", "serve", "--app", &id]).await? else {
        bail!("the Aurelia daemon is not available");
    };
    let mut out_rx = std::mem::replace(&mut bridge.stdout, tokio::sync::mpsc::unbounded_channel().1);
    let mut pending = Vec::new();
    let first = tokio::time::timeout(BRIDGE_READY, async {
        loop {
            if let Some(n) = pending.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = pending.drain(..=n).collect();
                return Some(line);
            }
            pending.extend(out_rx.recv().await?);
        }
    })
    .await
    .context("the bridge did not start in time")?
    .context("the bridge ended before starting")?;
    let hello: serde_json::Value = serde_json::from_slice(&first).context("unexpected first line from the bridge")?;
    let (Some(port), Some(token)) = (hello["port"].as_u64(), hello["token"].as_str()) else {
        bail!("the bridge failed: {}", String::from_utf8_lossy(&first).trim());
    };

    let mut extra: Vec<(String, String)> = PASS_THROUGH.iter().filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v))).collect();
    let language = std::env::var("AURELIA_SHIM_LANGUAGE").unwrap_or_else(|_| steam_language(&std::env::var("LANG").unwrap_or_default()).to_string());
    extra.push(("AURELIA_SHIM_LANGUAGE".into(), language));
    let launch_path = launch_file(&home, &id);
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let vars = launch_vars(port as u16, token, app_id, &steam_dir, &extra);
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&launch_path).with_context(|| format!("could not write {}", launch_path.display()))?;
        f.write_all(&serde_json::to_vec(&vars)?)?;
    }

    let relay_out = tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let mut out = tokio::io::stdout();
        if json && !pending.is_empty() {
            let _ = out.write_all(&pending).await;
        }
        while let Some(chunk) = out_rx.recv().await {
            if json {
                let _ = out.write_all(&chunk).await;
                let _ = out.flush().await;
            }
        }
    });
    let bridge = std::sync::Arc::new(bridge);
    let relay_in = json.then(|| {
        let bridge = std::sync::Arc::clone(&bridge);
        tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if matches!(aurelia::bridge::overlay::parse_command(&line), Ok(aurelia::bridge::overlay::Command::Stop)) {
                    continue;
                }
                if bridge.write_stdin(format!("{line}\n").as_bytes()).await.is_err() {
                    break;
                }
            }
        })
    });

    let mut play = if let Some(sc) = crate::commands::shortcut::find(app_id) {
        shortcut_command(&sc, &wrapper, &id, proton.as_deref(), json).await?
    } else {
        let mut play = tokio::process::Command::new(&exe);
        if json {
            play.arg("--json");
        }
        play.args(["play", &id, "-x"]).arg(&wrapper).args(["-N", "-k", "--platform", "windows"]);
        if let Some(p) = proton {
            play.args(["--proton", &p]);
        }
        if let Some(o) = launch_option {
            play.args(["--launch-option", &o]);
        }
        play
    };
    let played = play.stdin(std::process::Stdio::null()).status().await;

    if let Some(task) = relay_in {
        task.abort();
    }
    let _ = bridge.write_stdin(b"{\"cmd\":\"stop\"}\n").await;
    let closed = match std::sync::Arc::try_unwrap(bridge) {
        Ok(bridge) => tokio::time::timeout(BRIDGE_CLOSE, bridge.wait()).await.ok().flatten(),
        Err(_) => None,
    };
    let _ = relay_out.await;
    let _ = std::fs::remove_file(&launch_path);
    if closed != Some(0) {
        tracing::warn!("plus: the bridge did not close cleanly ({closed:?})");
    }
    let played = played.context("could not start the game")?;
    if !played.success() {
        bail!("the game launch failed ({played})");
    }
    Ok(())
}

fn wrap(inherited: &BTreeMap<String, String>, vars: &BTreeMap<String, String>, command: &[String]) -> Result<(Vec<String>, BTreeMap<String, String>, Vec<String>)> {
    let steam_dir = vars.get("AURELIA_PLUS_STEAM_DIR").context("launch file without AURELIA_PLUS_STEAM_DIR")?;
    let unset: Vec<String> = PASS_THROUGH.iter().filter(|k| inherited.contains_key(**k) && !vars.contains_key(**k)).map(|k| k.to_string()).collect();
    let mut set = vars.clone();
    if let Some(debug) = vars.get("AURELIA_PLUS_WINEDEBUG").filter(|d| !d.is_empty()) {
        set.insert("WINEDEBUG".into(), debug.clone());
    }
    for kv in vars.get("AURELIA_PLUS_GAME_ENV").map(|s| s.split_whitespace().collect::<Vec<_>>()).unwrap_or_default() {
        if let Some((k, v)) = kv.split_once('=') {
            set.insert(k.into(), v.into());
        }
    }
    set.insert("STEAM_COMPAT_CLIENT_INSTALL_PATH".into(), steam_dir.clone());
    let overrides = inherited.get("WINEDLLOVERRIDES").filter(|o| !o.is_empty()).map_or("lsteamclient=d".to_string(), |o| format!("{o};lsteamclient=d"));
    set.insert("WINEDLLOVERRIDES".into(), overrides);
    if !vars.contains_key("AURELIA_SHIM_LOG") {
        set.insert("AURELIA_SHIM_LOG".into(), r"Z:\tmp\aurelia-shim.log".into());
    }
    let mut command = command.to_vec();
    if let (Some(probe), Some(last)) = (vars.get("AURELIA_PLUS_PROBE_EXE").filter(|p| !p.is_empty()), command.last_mut()) {
        *last = probe.clone();
    } else if let Some(args) = vars.get("AURELIA_PLUS_GAME_ARGS") {
        command.extend(args.split_whitespace().map(str::to_string));
    }
    Ok((command, set, unset))
}

fn vdf_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let quoted: Vec<&str> = line.split('"').skip(1).step_by(2).collect();
        (quoted.len() >= 2 && quoted[0] == key).then(|| quoted[1].to_string())
    })
}

fn in_runtime(command: &[String], steamapps: &[PathBuf]) -> Option<Vec<String>> {
    let proton = Path::new(command.first()?);
    if proton.file_name()? != "proton" || command.get(1).map(String::as_str) != Some("run") {
        return None;
    }
    let tool = proton.parent()?;
    let runtime_id = vdf_value(&std::fs::read_to_string(tool.join("toolmanifest.vdf")).ok()?, "require_tool_appid")?;
    let own = tool.parent().and_then(Path::parent).map(Path::to_path_buf);
    let runtime = own.iter().chain(steamapps).find_map(|lib| {
        let manifest = std::fs::read_to_string(lib.join(format!("appmanifest_{runtime_id}.acf"))).ok()?;
        Some(lib.join("common").join(vdf_value(&manifest, "installdir")?))
    })?;
    let entry = vdf_value(&std::fs::read_to_string(runtime.join("toolmanifest.vdf")).ok()?, "commandline")?;
    let mut parts = entry.split_whitespace();
    let program = runtime.join(parts.next()?.trim_start_matches('/'));
    if !program.is_file() {
        return None;
    }
    let mut out = vec![program.to_string_lossy().into_owned()];
    out.extend(parts.map(|p| p.replace("%verb%", "waitforexitandrun")));
    out.push(command[0].clone());
    out.push("waitforexitandrun".into());
    out.extend(command[2..].iter().cloned());
    Some(out)
}

fn steamapps_dirs(env: &BTreeMap<String, String>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = env
        .get("STEAM_COMPAT_LIBRARY_PATHS")
        .map(|v| v.split(':').filter(|p| !p.is_empty()).map(|p| Path::new(p).join("steamapps")).collect())
        .unwrap_or_default();
    if let Some(home) = env.get("HOME") {
        dirs.push(Path::new(home).join(".local/share/Steam/steamapps"));
    }
    dirs
}

fn put_file(target: &Path, bytes: &[u8]) -> bool {
    if std::fs::read(target).ok().as_deref() == Some(bytes) {
        return true;
    }
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = target.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    if std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, target)).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    true
}

fn install_textinput(env: &BTreeMap<String, String>, steam_dir: &str) -> bool {
    let Some(data) = env.get("STEAM_COMPAT_DATA_PATH") else { return false };
    let drive_c = Path::new(data).join("pfx/drive_c");
    let system32 = drive_c.join("windows/system32");
    let legacy = Path::new(steam_dir).join("legacycompat");
    let steam_in_prefix = drive_c.join("Program Files (x86)/Steam");
    if steam_in_prefix.is_dir() {
        if let Ok(bytes) = std::fs::read(legacy.join("Steam.exe")) {
            put_file(&steam_in_prefix.join("Steam.exe"), &bytes);
        }
    }
    let syswow64 = drive_c.join("windows/syswow64");
    if syswow64.is_dir() {
        if let Ok(bytes) = std::fs::read(legacy.join("windows.ui.core.textinput32.dll")) {
            put_file(&syswow64.join("windows.ui.core.textinput.dll"), &bytes);
        }
    }
    let Ok(bytes) = std::fs::read(legacy.join("windows.ui.core.textinput.dll")) else { return false };
    system32.is_dir() && put_file(&system32.join("windows.ui.core.textinput.dll"), &bytes)
}

pub(crate) fn wrapper_main() {
    let mut args = std::env::args_os();
    let Some(argv0) = args.next() else { return };
    if Path::new(&argv0).file_name().and_then(|n| n.to_str()) != Some(WRAPPER_NAME) {
        return;
    }
    let fail = |msg: String| -> ! {
        eprintln!("aurelia plus wrapper: {msg}");
        std::process::exit(127);
    };
    let command: Vec<String> = args.map(|a| a.to_string_lossy().into_owned()).collect();
    if command.is_empty() {
        fail("no launch command".into());
    }
    let app_id = std::env::var("AURELIA_APP_ID").or_else(|_| std::env::var("SteamAppId")).unwrap_or_default();
    let home = plus_home().unwrap_or_else(|e| fail(format!("{e:#}")));
    let file = launch_file(&home, &app_id);
    let vars: BTreeMap<String, String> = std::fs::read(&file)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| fail(format!("no launch file {}", file.display())));
    let inherited: BTreeMap<String, String> = std::env::vars().collect();
    let (command, mut set, unset) = wrap(&inherited, &vars, &command).unwrap_or_else(|e| fail(format!("{e:#}")));
    if vars.get("AURELIA_PLUS_STEAM_DIR").is_some_and(|d| install_textinput(&inherited, d)) {
        let overrides = set.get("WINEDLLOVERRIDES").cloned().unwrap_or_default();
        set.insert("WINEDLLOVERRIDES".into(), if overrides.is_empty() { "windows.ui.core.textinput=n".into() } else { format!("{overrides};windows.ui.core.textinput=n") });
    }
    let command = in_runtime(&command, &steamapps_dirs(&inherited)).unwrap_or_else(|| {
        eprintln!("aurelia plus wrapper: no Steam Linux Runtime for this launch; running it on the host");
        command
    });
    let mut cmd = std::process::Command::new(&command[0]);
    cmd.args(&command[1..]);
    for k in &unset {
        cmd.env_remove(k);
    }
    cmd.envs(&set);
    use std::os::unix::process::CommandExt;
    let err = cmd.exec();
    fail(format!("could not start {}: {err}", command[0]));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn language_follows_the_locale() {
        assert_eq!(steam_language("pt_BR.UTF-8"), "brazilian");
        assert_eq!(steam_language("pt_PT.UTF-8"), "portuguese");
        assert_eq!(steam_language("es_ES.UTF-8"), "spanish");
        assert_eq!(steam_language("es_MX.UTF-8"), "latam");
        assert_eq!(steam_language(""), "english");
    }

    #[test]
    fn launch_vars_carry_the_bridge_and_steam_markers() {
        let v = launch_vars(4242, "ab12", 550, Path::new("/x/shimsteam"), &[("AURELIA_SHIM_LANGUAGE".into(), "brazilian".into())]);
        assert_eq!(v["AURELIA_BRIDGE_PORT"], "4242");
        assert_eq!(v["AURELIA_BRIDGE_TOKEN"], "ab12");
        assert_eq!(v["SteamOverlayGameId"], "550");
        assert_eq!(v["AURELIA_PLUS_STEAM_DIR"], "/x/shimsteam");
        assert_eq!(v["AURELIA_SHIM_LANGUAGE"], "brazilian");
    }

    #[test]
    fn wrapper_points_proton_at_the_shim() {
        let vars = map(&[("AURELIA_PLUS_STEAM_DIR", "/d/shimsteam"), ("AURELIA_BRIDGE_PORT", "1")]);
        let inherited = map(&[("WINEDLLOVERRIDES", "dxgi=n"), ("AURELIA_SHIM_TRACE", "1")]);
        let (cmd, set, unset) = wrap(&inherited, &vars, &["proton".into(), "run".into(), "game.exe".into()]).unwrap();
        assert_eq!(cmd, ["proton", "run", "game.exe"]);
        assert_eq!(set["STEAM_COMPAT_CLIENT_INSTALL_PATH"], "/d/shimsteam");
        assert_eq!(set["WINEDLLOVERRIDES"], "dxgi=n;lsteamclient=d");
        assert_eq!(set["AURELIA_SHIM_LOG"], r"Z:\tmp\aurelia-shim.log");
        assert_eq!(unset, ["AURELIA_SHIM_TRACE"]);
    }

    #[test]
    fn wrapper_applies_game_args_env_and_probe() {
        let vars = map(&[("AURELIA_PLUS_STEAM_DIR", "/d"), ("AURELIA_PLUS_GAME_ARGS", "-novid -console"), ("AURELIA_PLUS_GAME_ENV", "A=1 B=x=y bad"), ("AURELIA_PLUS_WINEDEBUG", "+seh")]);
        let (cmd, set, _) = wrap(&BTreeMap::new(), &vars, &["proton".into(), "game.exe".into()]).unwrap();
        assert_eq!(cmd, ["proton", "game.exe", "-novid", "-console"]);
        assert_eq!(set["A"], "1");
        assert_eq!(set["B"], "x=y");
        assert_eq!(set["WINEDEBUG"], "+seh");
        assert_eq!(set["WINEDLLOVERRIDES"], "lsteamclient=d");
        let probe = map(&[("AURELIA_PLUS_STEAM_DIR", "/d"), ("AURELIA_PLUS_PROBE_EXE", "/p.exe"), ("AURELIA_PLUS_GAME_ARGS", "-x")]);
        let (cmd, _, _) = wrap(&BTreeMap::new(), &probe, &["proton".into(), "game.exe".into()]).unwrap();
        assert_eq!(cmd, ["proton", "/p.exe"]);
    }

    #[test]
    fn wrapper_needs_the_steam_dir() {
        assert!(wrap(&BTreeMap::new(), &BTreeMap::new(), &["x".into()]).is_err());
    }

    #[cfg(aurelia_plus_shim)]
    #[test]
    fn install_writes_the_shim_and_links_the_wrapper() {
        let home = std::env::temp_dir().join(format!("aurelia-plus-home-{}", std::process::id()));
        let exe = std::env::current_exe().unwrap();
        let (steam, link) = install(&home, &exe).unwrap();
        for (name, bytes) in SHIM {
            assert_eq!(std::fs::read(steam.join("legacycompat").join(name)).unwrap(), *bytes);
        }
        assert_eq!(std::fs::read_link(&link).unwrap(), exe);
        install(&home, &exe).unwrap();
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn proton_runs_inside_the_runtime_it_requires() {
        let lib = std::env::temp_dir().join(format!("aurelia-plus-runtime-{}", std::process::id()));
        let proton = lib.join("common/Proton X");
        let runtime = lib.join("common/SteamLinuxRuntime_4");
        std::fs::create_dir_all(&proton).unwrap();
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::write(proton.join("toolmanifest.vdf"), "\"manifest\"\n{\n  \"commandline\" \"/proton %verb%\"\n  \"require_tool_appid\" \"4183110\"\n}\n").unwrap();
        std::fs::write(lib.join("appmanifest_4183110.acf"), "\"AppState\"\n{\n\t\"appid\"\t\t\"4183110\"\n\t\"installdir\"\t\t\"SteamLinuxRuntime_4\"\n}\n").unwrap();
        std::fs::write(runtime.join("toolmanifest.vdf"), "\"manifest\"\n{\n\t\"commandline\" \"/_v2-entry-point --verb=%verb% --\"\n}\n").unwrap();
        std::fs::write(runtime.join("_v2-entry-point"), "").unwrap();
        let p = proton.join("proton").to_string_lossy().into_owned();
        let cmd = vec![p.clone(), "run".into(), "/g/game.exe".into(), "-steam".into()];
        let entry = runtime.join("_v2-entry-point").to_string_lossy().into_owned();
        assert_eq!(
            in_runtime(&cmd, &[]).unwrap(),
            vec![entry, "--verb=waitforexitandrun".into(), "--".into(), p.clone(), "waitforexitandrun".into(), "/g/game.exe".into(), "-steam".into()]
        );
        assert!(in_runtime(&["/usr/bin/wine".into(), "game.exe".into()], &[]).is_none());
        std::fs::remove_file(lib.join("appmanifest_4183110.acf")).unwrap();
        assert!(in_runtime(&cmd, &[]).is_none());
        std::fs::remove_dir_all(&lib).unwrap();
    }
}
