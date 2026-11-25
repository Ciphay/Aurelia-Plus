use async_trait::async_trait;
use std::path::{Path, PathBuf};

use crate::launch::pipeline::{LaunchError, PipelineContext, PipelineStage};

/// What the Steam client does when a game starts (Steamworks "Install Scripts"): runs the install scripts of the game's depots, which are
/// the files the depot manifest flags with 0x100 (`install_scripts` keeps that list from the download; installs made before it fall back
/// to the `installscript` of the appinfo). `Registry` values are written, `Run Process` entries run in the game's prefix (VC++ and DirectX
/// redistributables, the game's own prerequisites). Proton leaves this to the client.
///
/// Like Steam: a `Run Process` with a `HasRunKey` runs while the DWORD named after the entry at that key is missing or below
/// `MinimumHasRunValue`, and is set to 1 when the program exits with 0; `Requirement_OS` filters apply; the variables are expanded.
/// Unlike Steam, the signature (`kvsignatures`) is not checked (Valve's key is not available), and a marker in the prefix (the hash of
/// the scripts) stops the evaluation from repeating at every launch once everything went through.
///
/// Never halts the launch, and reports what it is doing through `ctx.on_setup_note` so the launcher can show "Aurelia is installing ...".
pub struct ApplyInstallScriptStage;

const MARKER_PREFIX: &str = "aurelia-installscript-";
const MARKER_VERSION: u32 = 2;
const PROCESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

#[derive(Debug, PartialEq)]
pub(crate) enum Node {
    Str(String),
    Obj(Vec<(String, Node)>),
}

impl Node {
    fn get(&self, key: &str) -> Option<&Node> {
        match self {
            Node::Obj(items) => items.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v),
            Node::Str(_) => None,
        }
    }
    fn text(&self) -> Option<&str> {
        match self {
            Node::Str(s) => Some(s),
            Node::Obj(_) => None,
        }
    }
    fn items(&self) -> &[(String, Node)] {
        match self {
            Node::Obj(items) => items,
            Node::Str(_) => &[],
        }
    }
}

/// Text VDF into a tree (quoted or bare words, `{ }`, `//` comments). Unbalanced input gives what was read so far.
pub(crate) fn parse_vdf(text: &str) -> Node {
    fn tokens(text: &str) -> Vec<(bool, String)> {
        // (is_quoted, text); braces are (false, "{") / (false, "}")
        let mut out = Vec::new();
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c.is_whitespace() {
                i += 1;
            } else if c == '/' && chars.get(i + 1) == Some(&'/') {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            } else if c == '{' || c == '}' {
                out.push((false, c.to_string()));
                i += 1;
            } else if c == '"' {
                let mut s = String::new();
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1;
                        s.push(match chars[i] {
                            'n' => '\n',
                            't' => '\t',
                            other => other,
                        });
                    } else {
                        s.push(chars[i]);
                    }
                    i += 1;
                }
                i += 1;
                out.push((true, s));
            } else {
                let mut s = String::new();
                while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '{' && chars[i] != '}' {
                    s.push(chars[i]);
                    i += 1;
                }
                out.push((true, s));
            }
        }
        out
    }
    fn block(toks: &[(bool, String)], pos: &mut usize) -> Node {
        let mut items = Vec::new();
        while *pos < toks.len() {
            let (quoted, key) = toks[*pos].clone();
            if !quoted && key == "}" {
                *pos += 1;
                break;
            }
            *pos += 1;
            match toks.get(*pos) {
                Some((false, b)) if b == "{" => {
                    *pos += 1;
                    items.push((key, block(toks, pos)));
                }
                Some((true, value)) => {
                    items.push((key, Node::Str(value.clone())));
                    *pos += 1;
                }
                _ => break,
            }
        }
        Node::Obj(items)
    }
    let toks = tokens(text);
    let mut pos = 0;
    block(&toks, &mut pos)
}

/// One value of the `Registry` section.
#[derive(Debug, PartialEq)]
pub(crate) struct RegValue {
    pub key: String,
    /// `None`: the key's default value (`(Default)`).
    pub name: Option<String>,
    pub reg_type: &'static str,
    pub data: String,
}

/// HKLM\SOFTWARE of a 64-bit Windows lives under Wow6432Node for a 32-bit process, where Steam writes it.
pub(crate) fn wow_key(key: &str) -> String {
    let upper = key.to_ascii_uppercase();
    let prefix = "HKEY_LOCAL_MACHINE\\SOFTWARE\\";
    if upper.starts_with(prefix) && !upper.contains("WOW6432NODE") {
        format!("{}Wow6432Node\\{}", &key[..prefix.len()], &key[prefix.len()..])
    } else {
        key.to_string()
    }
}

/// `%NAME%` variables of the scripts, case-insensitive; the ones that depend on the prefix are fixed Windows locations of a Proton user.
pub(crate) fn expand(text: &str, vars: &[(&str, String)]) -> String {
    let mut out = text.to_string();
    for (name, value) in vars {
        let needle = format!("%{name}%").to_ascii_lowercase();
        let mut result = String::new();
        let mut rest = out.as_str();
        while let Some(at) = rest.to_ascii_lowercase().find(&needle) {
            result.push_str(&rest[..at]);
            result.push_str(value);
            rest = &rest[at + needle.len()..];
        }
        result.push_str(rest);
        out = result;
    }
    out
}

pub(crate) fn variables(install_win: &str) -> Vec<(&'static str, String)> {
    let drive = install_win.get(..2).filter(|d| d.ends_with(':')).unwrap_or("C:").to_string();
    vec![
        ("INSTALLDIR", install_win.to_string()),
        ("ROOTDRIVE", drive),
        ("STEAMPATH", "C:\\Program Files (x86)\\Steam".to_string()),
        ("APPDATA", "C:\\users\\steamuser\\AppData\\Roaming".to_string()),
        ("LOCALAPPDATA", "C:\\users\\steamuser\\AppData\\Local".to_string()),
        ("USER_MYDOCS", "C:\\users\\steamuser\\Documents".to_string()),
        ("COMMON_MYDOCS", "C:\\users\\Public\\Documents".to_string()),
        ("WinDir", "C:\\windows".to_string()),
    ]
}

const LANGUAGES: &[&str] = &[
    "english", "german", "french", "italian", "koreana", "spanish", "schinese", "tchinese", "russian", "thai", "japanese", "portuguese",
    "polish", "danish", "dutch", "finnish", "norwegian", "swedish", "hungarian", "czech", "romanian", "turkish", "brazilian", "bulgarian",
    "greek", "ukrainian", "latam", "vietnamese", "arabic",
];

fn is_language(name: &str) -> bool {
    LANGUAGES.iter().any(|l| name.eq_ignore_ascii_case(l))
}

pub(crate) fn registry_values(script: &Node, vars: &[(&str, String)], language: &str) -> Vec<RegValue> {
    let mut out = Vec::new();
    let Some(root) = script.get("InstallScript").and_then(|s| s.get("Registry")) else {
        return out;
    };
    for (key, body) in root.items() {
        for (kind, reg_type) in [("string", "REG_SZ"), ("dword", "REG_DWORD")] {
            let items = body.get(kind).map(Node::items).unwrap_or(&[]);
            let has = |lang: &str| items.iter().any(|(n, v)| n.eq_ignore_ascii_case(lang) && matches!(v, Node::Obj(_)));
            let chosen = if has(language) { language } else { "english" };
            let mut push = |name: &str, data: &str| {
                out.push(RegValue {
                    key: expand(key, vars),
                    name: (!name.eq_ignore_ascii_case("(Default)")).then(|| name.to_string()),
                    reg_type,
                    data: expand(data, vars),
                });
            };
            for (name, value) in items {
                match value {
                    Node::Str(text) => push(name, text),
                    Node::Obj(inner) if is_language(name) => {
                        if name.eq_ignore_ascii_case(chosen) {
                            for (n, v) in inner {
                                if let Node::Str(text) = v {
                                    push(n, text);
                                }
                            }
                        }
                    }
                    Node::Obj(_) => {
                        if let Some(text) = value.get(chosen).or_else(|| value.get("english")).and_then(Node::text) {
                            push(name, text);
                        }
                    }
                }
            }
        }
    }
    out
}

/// A `Run Process` entry.
#[derive(Debug, PartialEq)]
pub(crate) struct Redist {
    pub name: String,
    pub program: String,
    pub args: String,
    /// Registry key holding the DWORD (named like the entry) that says whether it already ran.
    pub has_run_key: Option<String>,
    pub minimum_has_run: u32,
    /// `Requirement_OS` allows it in this prefix.
    pub applies: bool,
}

/// `prefix_is_64bit`: the prefix has a 32-bit layer (Windows 64). Wine in Proton reports Windows 10.
pub(crate) fn run_processes(script: &Node, vars: &[(&str, String)], prefix_is_64bit: bool) -> Vec<Redist> {
    let mut out = Vec::new();
    let Some(root) = script.get("InstallScript").and_then(|s| s.get("Run Process")) else {
        return out;
    };
    for (name, body) in root.items() {
        let Some(program) = body.get("Process 1").and_then(Node::text) else { continue };
        let requirement = body.get("Requirement_OS");
        let wants_64 = requirement.and_then(|r| r.get("Is64BitWindows")).and_then(Node::text);
        let os_type = requirement.and_then(|r| r.get("OSType")).and_then(Node::text);
        let applies = wants_64.is_none_or(|v| (v.trim() == "1") == prefix_is_64bit)
            && os_type.is_none_or(|v| v.trim().eq_ignore_ascii_case("Windows 10"));
        out.push(Redist {
            name: name.clone(),
            program: expand(program, vars),
            args: expand(body.get("Command 1").and_then(Node::text).unwrap_or(""), vars),
            has_run_key: body.get("HasRunKey").and_then(Node::text).map(|k| expand(k, vars)),
            minimum_has_run: body.get("MinimumHasRunValue").and_then(Node::text).and_then(|v| v.trim().parse().ok()).unwrap_or(1),
            applies,
        });
    }
    out
}

/// Arguments as the command line of CreateProcess reads them: split at spaces, double quotes group.
pub(crate) fn split_args(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any || !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                    any = false;
                }
            }
            c => current.push(c),
        }
    }
    if any || !current.is_empty() {
        out.push(current);
    }
    out
}

/// The DWORD of `reg query ... /v NAME`: its last `REG_DWORD 0x..` field.
pub(crate) fn parse_reg_dword(stdout: &str) -> Option<u32> {
    let line = stdout.lines().find(|l| l.contains("REG_DWORD"))?;
    let value = line.split_whitespace().last()?;
    match value.strip_prefix("0x").or_else(|| value.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => value.parse().ok(),
    }
}

/// The scripts to run: the depots' flagged files, or, for an install made before they were recorded, the appinfo's `installscript` (Steam's
/// own default name last).
fn script_files(app_id: u32, install: &Path) -> Vec<PathBuf> {
    let relative: Vec<String> = match crate::launch::install_scripts::load(app_id) {
        Some(list) => list,
        None => {
            let declared = crate::steam_client::pics_cache::load(app_id)
                .and_then(|buf| crate::steam_client::find_vdf_in_pics(&buf).ok())
                .and_then(|vdf| {
                    crate::steam_client::pics_app_section(vdf.value())
                        .get("config")
                        .and_then(|c| c.get("installscript"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                });
            declared.into_iter().chain(std::iter::once("installscript.vdf".to_string())).collect()
        }
    };
    let mut found: Vec<PathBuf> = Vec::new();
    for rel in relative {
        let path = install.join(rel.replace('\\', "/"));
        if path.is_file() && !found.contains(&path) {
            found.push(path);
        }
    }
    found
}

/// The game's folder as the prefix sees it. Proton maps the library holding the game to `S:` (`dosdevices/s:`), which is what the Steam
/// client's own scripts and the game's launcher record (`S:\steamapps\common\<game>`): used when the prefix has it, so a value the launcher
/// already wrote is not changed. Otherwise `Z:` + the Unix path, valid in every prefix. No trailing backslash (like `%INSTALLDIR%`).
fn windows_path(install: &Path, prefix: &Path) -> String {
    let common = install.parent();
    let steamapps = common.and_then(Path::parent);
    let library = steamapps.and_then(Path::parent);
    if let (Some(common), Some(steamapps), Some(library), Some(name)) = (common, steamapps, library, install.file_name()) {
        let is_layout = common.file_name().is_some_and(|n| n == "common") && steamapps.file_name().is_some_and(|n| n == "steamapps");
        let mapped = std::fs::canonicalize(prefix.join("dosdevices").join("s:")).ok();
        if is_layout && mapped.is_some() && mapped == std::fs::canonicalize(library).ok() {
            return format!("S:\\steamapps\\common\\{}", name.to_string_lossy());
        }
    }
    format!("Z:{}", install.to_string_lossy().replace('/', "\\"))
}

fn script_hash(parts: &[Vec<u8>]) -> String {
    use sha1::Digest;
    let mut hasher = sha1::Sha1::new();
    for part in parts {
        hasher.update(part);
        hasher.update([0u8]);
    }
    hex::encode(hasher.finalize())
}

#[async_trait]
impl PipelineStage for ApplyInstallScriptStage {
    fn name(&self) -> &str {
        "ApplyInstallScript"
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        let Some(install) = ctx.resolved_install_dir.clone() else { return Ok(()) };
        let prefix = match ctx.command_spec.as_ref().and_then(|s| s.env.get("STEAM_COMPAT_DATA_PATH")) {
            Some(data) => {
                let pfx = PathBuf::from(data).join("pfx");
                if !pfx.join("system.reg").is_file() {
                    return Ok(());
                }
                pfx.to_string_lossy().into_owned()
            }
            None => match ctx.command_spec.as_ref().and_then(|s| s.env.get("WINEPREFIX")).cloned() {
                Some(p) => p,
                None => return Ok(()),
            },
        };
        let files = script_files(ctx.app_id, &install);
        if files.is_empty() {
            return Ok(());
        }
        let contents: Vec<Vec<u8>> = files.iter().filter_map(|p| std::fs::read(p).ok()).collect();
        let hash = script_hash(&contents);
        let hash = format!("{hash}:{MARKER_VERSION}");
        let language = ctx
            .command_spec
            .as_ref()
            .and_then(|s| s.env.get("AURELIA_SHIM_LANGUAGE").cloned())
            .or_else(|| std::env::var("AURELIA_SHIM_LANGUAGE").ok())
            .unwrap_or_else(|| "english".to_string());
        let marker = PathBuf::from(&prefix).join(format!("{MARKER_PREFIX}{}", ctx.app_id));
        if std::fs::read_to_string(&marker).is_ok_and(|done| done.trim() == hash) {
            return Ok(());
        }
        let install_win = windows_path(&install, Path::new(&prefix));
        let vars = variables(&install_win);
        let prefix_is_64bit = Path::new(&prefix).join("drive_c/windows/syswow64").is_dir();
        let mut regs = Vec::new();
        let mut procs = Vec::new();
        for bytes in &contents {
            let script = parse_vdf(&String::from_utf8_lossy(bytes));
            regs.extend(registry_values(&script, &vars, &language));
            procs.extend(run_processes(&script, &vars, prefix_is_64bit));
        }
        if regs.is_empty() && procs.is_empty() {
            let _ = std::fs::write(&marker, &hash);
            return Ok(());
        }

        let name = ctx.app.as_ref().map(|a| a.name.clone()).unwrap_or_default();
        let note = |text: String| {
            tracing::info!("{text}");
            if let Some(cb) = &ctx.on_setup_note {
                cb(text);
            }
        };
        note(format!("Aurelia is setting up what {name} needs..."));

        #[cfg(unix)]
        {
            let library_root = ctx.launcher_config.as_ref().map(|c| PathBuf::from(&c.steam_library_path)).unwrap_or_default();
            let proton = ctx
                .launcher_config
                .as_ref()
                .and_then(|c| c.game_configs.get(&ctx.app_id))
                .and_then(|c| c.forced_proton_version.clone())
                .or_else(|| ctx.proton_path.clone().filter(|p| !p.is_empty()))
                .unwrap_or_else(|| "wine".to_string());
            let runner = crate::core::utils::resolve_runner(&proton, &library_root);
            let wine = crate::launch::stages::apply_registry_fixups::resolve_prefix_wine(&runner);
            // (exit status ok, stdout)
            let run = |args: Vec<String>, wait: std::time::Duration| -> Option<(bool, String)> {
                let mut child = std::process::Command::new(&wine)
                    .args(&args)
                    .env("WINEPREFIX", &prefix)
                    .env("WINEDEBUG", "-all")
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .ok()?;
                let mut stdout = child.stdout.take();
                let reader = std::thread::spawn(move || {
                    let mut text = String::new();
                    if let Some(out) = stdout.as_mut() {
                        use std::io::Read;
                        let _ = out.read_to_string(&mut text);
                    }
                    text
                });
                let end = std::time::Instant::now() + wait;
                let ok = loop {
                    match child.try_wait() {
                        Ok(Some(status)) => break status.success(),
                        Ok(None) if std::time::Instant::now() < end => std::thread::sleep(std::time::Duration::from_millis(200)),
                        _ => {
                            let _ = child.kill();
                            let _ = child.wait();
                            break false;
                        }
                    }
                };
                Some((ok, reader.join().unwrap_or_default()))
            };
            let mut all_ok = true;
            for r in &regs {
                let mut args: Vec<String> = ["reg", "add"].map(String::from).to_vec();
                args.push(wow_key(&r.key));
                match &r.name {
                    Some(n) => args.extend(["/v".to_string(), n.clone()]),
                    None => args.push("/ve".to_string()),
                }
                args.extend(["/t", r.reg_type, "/d"].map(String::from));
                args.push(r.data.clone());
                args.push("/f".to_string());
                if !matches!(run(args, std::time::Duration::from_secs(30)), Some((true, _))) {
                    tracing::warn!(key = %r.key, name = ?r.name, "install script: registry value not written");
                    all_ok = false;
                }
            }
            for p in procs.iter().filter(|p| p.applies) {
                let tracked = p.has_run_key.as_deref().map(wow_key);
                if let Some(key) = &tracked {
                    let query = ["reg", "query", key, "/v", &p.name].map(String::from).to_vec();
                    let done = run(query, std::time::Duration::from_secs(30))
                        .and_then(|(ok, out)| if ok { parse_reg_dword(&out) } else { None })
                        .unwrap_or(0);
                    if done >= p.minimum_has_run.max(1) {
                        continue; // already ran
                    }
                }
                note(format!("Aurelia is installing {} for {name}...", p.name));
                let mut args = vec![p.program.clone()];
                args.extend(split_args(&p.args));
                match run(args, PROCESS_TIMEOUT) {
                    Some((true, _)) => {
                        if let Some(key) = &tracked {
                            let set = ["reg", "add", key, "/v", &p.name, "/t", "REG_DWORD", "/d", "1", "/f"].map(String::from).to_vec();
                            let _ = run(set, std::time::Duration::from_secs(30));
                        }
                    }
                    _ => {
                        tracing::warn!(process = %p.name, "install script: process did not exit with 0");
                        all_ok = false;
                    }
                }
            }
            if all_ok {
                let _ = std::fs::write(&marker, &hash);
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (&regs, &procs, &marker, &hash);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GTA: &str = r#""InstallScript"
{
	"Registry"
	{
		"HKEY_LOCAL_MACHINE\\SOFTWARE\\Rockstar Games\\GTA V Enhanced\\"
		{
			"string"
			{
				"InstallFolderSteam"		"%INSTALLDIR%\\"
				"(Default)"		"x"
				"Lang"
				{
					"english"	"en"
					"german"	"de"
				}
			}
			"dword"
			{
				"Count"		"3"
			}
		}
	}
	"Run Process"
	{
		"vcredist"
		{
			"HasRunKey"		"HKEY_LOCAL_MACHINE\\Software\\Valve\\Steam\\Apps\\1"
			"MinimumHasRunValue"		"2"
			"Process 1"		"%INSTALLDIR%\\_CommonRedist\\vc.exe"
			"Command 1"		"/q /norestart \"a b\""
		}
		"only32"
		{
			"Process 1"		"x.exe"
			"Requirement_OS"
			{
				"Is64BitWindows"		"0"
			}
		}
		"win7only"
		{
			"Process 1"		"y.exe"
			"Requirement_OS"
			{
				"OSType"		"Windows 7"
			}
		}
	}
	"Run Process On Uninstall"
	{
		"BattlEye_Uninstall"
		{
			"Process 1"		"x.exe"
		}
	}
}
"kvsignatures"
{
	"InstallScript"		"abc"
}
"#;

    fn vars() -> Vec<(&'static str, String)> {
        variables("S:\\steamapps\\common\\G")
    }

    #[test]
    fn language_blocks_write_their_values_in_the_chosen_language() {
        let script = parse_vdf(r#""InstallScript" { "Registry" { "HKEY_LOCAL_MACHINE\\Software\\X" { "string" {
            "installdir" "%INSTALLDIR%"
            "english" { "language" "English(US)" }
            "brazilian" { "language" "Portuguese(Brazil)" }
        } } } }"#);
        let en = registry_values(&script, &vars(), "english");
        assert_eq!(en.len(), 2);
        assert_eq!((en[1].name.as_deref(), en[1].data.as_str()), (Some("language"), "English(US)"));
        let br = registry_values(&script, &vars(), "brazilian");
        assert_eq!(br[1].data, "Portuguese(Brazil)");
        let fr = registry_values(&script, &vars(), "french");
        assert_eq!(fr[1].data, "English(US)", "a language the game has no block for falls back to English");
    }

    #[test]
    fn registry_values_names_types_languages_and_variables() {
        let regs = registry_values(&parse_vdf(GTA), &vars(), "english");
        assert_eq!(regs.len(), 4);
        assert_eq!(regs[0].name.as_deref(), Some("InstallFolderSteam"));
        assert_eq!(regs[0].data, "S:\\steamapps\\common\\G\\");
        assert_eq!(regs[1].name, None);
        assert_eq!(regs[2].data, "en");
        assert_eq!((regs[3].reg_type, regs[3].data.as_str()), ("REG_DWORD", "3"));
    }

    #[test]
    fn run_process_has_run_key_minimum_and_os_filters() {
        let procs = run_processes(&parse_vdf(GTA), &vars(), true);
        assert_eq!(procs.len(), 3);
        assert_eq!(procs[0].program, "S:\\steamapps\\common\\G\\_CommonRedist\\vc.exe");
        assert_eq!(procs[0].minimum_has_run, 2);
        assert!(procs[0].has_run_key.is_some() && procs[0].applies);
        assert!(!procs[1].applies, "Is64BitWindows 0 does not apply to a 64-bit prefix");
        assert!(!procs[2].applies, "OSType Windows 7 does not apply: Wine reports Windows 10");
        assert!(run_processes(&parse_vdf(GTA), &vars(), false)[1].applies);
    }

    #[test]
    fn wow6432node_for_hklm_software_only() {
        assert_eq!(wow_key("HKEY_LOCAL_MACHINE\\SOFTWARE\\A\\B"), "HKEY_LOCAL_MACHINE\\SOFTWARE\\Wow6432Node\\A\\B");
        assert_eq!(wow_key("HKEY_LOCAL_MACHINE\\SOFTWARE\\Wow6432Node\\A"), "HKEY_LOCAL_MACHINE\\SOFTWARE\\Wow6432Node\\A");
        assert_eq!(wow_key("HKEY_CURRENT_USER\\Software\\A"), "HKEY_CURRENT_USER\\Software\\A");
    }

    #[test]
    fn variables_are_case_insensitive() {
        assert_eq!(expand("%installdir%\\x %WINDIR%", &vars()), "S:\\steamapps\\common\\G\\x C:\\windows");
    }

    #[test]
    fn splits_arguments_like_a_command_line() {
        assert_eq!(split_args("/q /norestart \"a b\""), vec!["/q", "/norestart", "a b"]);
        assert!(split_args("   ").is_empty());
    }

    #[test]
    fn reads_the_dword_of_reg_query() {
        let out = "\r\nHKEY_LOCAL_MACHINE\\X\r\n    vcredist    REG_DWORD    0x1\r\n";
        assert_eq!(parse_reg_dword(out), Some(1));
        assert_eq!(parse_reg_dword("nothing"), None);
    }

    #[test]
    fn broken_input_gives_nothing() {
        assert!(registry_values(&parse_vdf("garbage {"), &vars(), "english").is_empty());
        assert!(run_processes(&parse_vdf(""), &vars(), true).is_empty());
    }
}
