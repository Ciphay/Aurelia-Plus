use crate::core::models::{OwnedGame, SessionState, SteamPrefixMode, UserConfigStore};
use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs;

async fn write_json_pretty<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let body = serde_json::to_string_pretty(value)?;
    crate::core::atomic::write_atomic_async(path.to_path_buf(), body.into_bytes(), false).await
}

async fn write_json_private<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let body = serde_json::to_string_pretty(value)?;
    crate::core::atomic::write_atomic_async(path.to_path_buf(), body.into_bytes(), true).await
}

async fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let raw = fs::read_to_string(path)
        .await
        .with_context(|| format!("failed reading {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed parsing {}", path.display()))
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct GameConfig {
    pub forced_proton_version: Option<String>,
    pub platform_preference: Option<String>,
    #[serde(default)]
    pub runner: GameRunner,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_script: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub pinned_manifests: HashMap<u32, u64>,
    #[serde(default)]
    pub pinned: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum GameRunner {
    #[default]
    Auto,
    Luxtorpeda,
    Umu,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ProxyConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_proxy: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ChatPresence {
    #[default]
    Offline,
    Online,
}

impl ChatPresence {
    pub fn persona_state(self) -> u32 {
        match self {
            ChatPresence::Offline => 7,
            ChatPresence::Online => 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LauncherConfig {
    pub steam_library_path: String,
    pub proton_version: String,
    #[serde(default)]
    pub chat_presence: ChatPresence,
    #[serde(default)]
    pub steam_runtime_runner: PathBuf,
    #[serde(default)]
    pub steam_prefix_mode: SteamPrefixMode,
    pub enable_cloud_sync: bool,
    #[serde(default)]
    pub use_shared_compat_data: bool,
    #[serde(default = "crate::core::models::default_true")]
    pub windows_steam_discovery_enabled: bool,
    #[serde(default)]
    pub preferred_launch_options: HashMap<u32, String>,
    #[serde(default)]
    pub game_configs: HashMap<u32, GameConfig>,
    #[serde(default)]
    pub luxtorpeda_enabled: bool,
    #[serde(default)]
    pub luxtorpeda_path: Option<String>,
    #[serde(default)]
    pub umu_enabled: bool,
    #[serde(default)]
    pub umu_path: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub steam_runtime_policy: crate::core::models::SteamRuntimePolicy,
    #[serde(default)]
    pub install_resume: crate::library::backlog::InstallResume,
    #[serde(default)]
    pub shader_precache: crate::launch::shaders::ShaderPrecache,
    #[serde(default = "crate::core::models::default_true")]
    pub hide_uninstalled_launch_options: bool,
    #[serde(default = "crate::core::models::default_true")]
    pub anticheat_auto_install: bool,
    #[serde(default)]
    pub experimental: bool,
    #[serde(default)]
    pub encrypt_session: bool,
    #[serde(default = "default_persona_wait_secs")]
    pub persona_wait_secs: u64,
    #[serde(default)]
    pub download_limit_kbit: u64,
}

pub fn kbit_to_bytes(kbit: u64) -> u64 {
    kbit.saturating_mul(1000) / 8
}

pub const DEFAULT_PERSONA_WAIT_SECS: u64 = 30;

fn default_persona_wait_secs() -> u64 {
    DEFAULT_PERSONA_WAIT_SECS
}

impl LauncherConfig {
    pub fn download_limit_bytes(&self) -> u64 {
        kbit_to_bytes(self.download_limit_kbit)
    }

    pub async fn load() -> Result<Self> {
        load_launcher_config().await
    }

    pub async fn save(&self) -> Result<()> {
        save_launcher_config(self).await
    }
}

impl Default for LauncherConfig {
    fn default() -> Self {
        let steam_library_path = detect_steam_path()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_else(|| {
                home_dir()
                    .map(|home| home.join("Games").join("Aurelia"))
                    .unwrap_or_else(|_| PathBuf::from("~/Games/Aurelia"))
                    .to_string_lossy()
                    .into_owned()
            });

        Self {
            steam_library_path,
            proton_version: "experimental".to_string(),
            chat_presence: ChatPresence::default(),
            steam_runtime_runner: PathBuf::new(),
            steam_prefix_mode: SteamPrefixMode::default(),
            enable_cloud_sync: true,
            use_shared_compat_data: false,
            windows_steam_discovery_enabled: true,
            preferred_launch_options: HashMap::new(),
            game_configs: HashMap::new(),
            luxtorpeda_enabled: false,
            luxtorpeda_path: None,
            umu_enabled: false,
            umu_path: None,
            language: None,
            country: None,
            proxy: ProxyConfig::default(),
            steam_runtime_policy: crate::core::models::SteamRuntimePolicy::default(),
            install_resume: crate::library::backlog::InstallResume::default(),
            shader_precache: crate::launch::shaders::ShaderPrecache::default(),
            hide_uninstalled_launch_options: true,
            anticheat_auto_install: true,
            experimental: false,
            encrypt_session: false,
            persona_wait_secs: DEFAULT_PERSONA_WAIT_SECS,
            download_limit_kbit: 0,
        }
    }
}

pub async fn experimental_enabled() -> bool {
    if let Some(value) = std::env::var_os("AURELIA_EXPERIMENTAL") {
        let value = value.to_string_lossy();
        let value = value.trim();
        if !value.is_empty()
            && !value.eq_ignore_ascii_case("0")
            && !value.eq_ignore_ascii_case("false")
        {
            return true;
        }
    }
    load_launcher_config()
        .await
        .map(|config| config.experimental)
        .unwrap_or(false)
}

pub fn detect_steam_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let candidates = [PathBuf::from(r"C:\Program Files (x86)\Steam")];
        return candidates.into_iter().find(|path| path.exists());
    }

    #[cfg(not(target_os = "windows"))]
    {
        let home = std::env::var("HOME").ok()?;
        let mut candidates = vec![
            PathBuf::from(&home).join(".steam/steam"),
            PathBuf::from(&home).join(".local/share/Steam"),
            PathBuf::from(&home).join(".steam/root"),
        ];
        candidates.extend(crate::library::steam_root_candidates());
        pick_steam_root(&candidates)
    }
}

pub fn pick_steam_root(candidates: &[PathBuf]) -> Option<PathBuf> {
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut first: Option<PathBuf> = None;
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for path in candidates {
        if !path.exists() {
            continue;
        }
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        if seen.contains(&canonical) {
            continue;
        }
        seen.push(canonical);
        if first.is_none() {
            first = Some(path.clone());
        }
        let modified = std::fs::metadata(path.join("config/loginusers.vdf"))
            .and_then(|m| m.modified())
            .ok();
        if let Some(modified) = modified
            && best.as_ref().is_none_or(|(t, _)| modified > *t)
        {
            best = Some((modified, path.clone()));
        }
    }
    best.map(|(_, p)| p).or(first)
}

pub fn home_dir() -> Result<PathBuf> {
    let from = |var: &str| std::env::var_os(var).filter(|v| !v.is_empty());

    if let Some(home) = from("HOME").or_else(|| from("USERPROFILE")) {
        return Ok(PathBuf::from(home));
    }

    if let (Some(drive), Some(path)) = (from("HOMEDRIVE"), from("HOMEPATH")) {
        let mut combined = PathBuf::from(drive);
        combined.push(path);
        return Ok(combined);
    }

    anyhow::bail!(
        "could not determine home directory (none of HOME, USERPROFILE, or HOMEDRIVE/HOMEPATH are set)"
    )
}

pub fn config_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("AURELIA_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    Ok(home_dir()?.join(".config").join("Aurelia"))
}

pub async fn ensure_config_dirs() -> Result<()> {
    let config = config_dir()?;
    fs::create_dir_all(&config).await?;
    let images = opensteam_image_cache_dir()?;
    fs::create_dir_all(&images).await?;
    Ok(())
}

pub fn opensteam_image_cache_dir() -> Result<PathBuf> {
    Ok(config_dir()?.join("images"))
}

pub fn data_dir() -> Result<PathBuf> {
    config_dir()
}

pub fn session_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("session.json"))
}

pub async fn load_session() -> Result<SessionState> {
    let session_path = session_path()?;
    if !session_path.exists() {
        return Ok(SessionState::default());
    }
    let raw = fs::read_to_string(&session_path)
        .await
        .with_context(|| format!("failed reading {}", session_path.display()))?;
    if let Some(envelope) = crate::core::session_crypto::EncryptedSession::from_json(&raw) {
        if !envelope.keyring {
            anyhow::bail!(
                "session.json was encrypted with a legacy session password — \
                 run `aurelia login` again to re-key it to the OS keyring"
            );
        }
        let key = tokio::task::spawn_blocking(crate::core::session_crypto::keyring_secret)
            .await
            .ok()
            .flatten()
            .context(
                "session.json is keyed to the OS keyring, which is unavailable — \
                 run `aurelia login` again",
            )?;
        let plain = crate::core::session_crypto::decrypt(&envelope, &key)?;
        return serde_json::from_slice(&plain)
            .with_context(|| format!("failed parsing decrypted {}", session_path.display()));
    }
    serde_json::from_str(&raw).with_context(|| format!("failed parsing {}", session_path.display()))
}

async fn session_encryption_active(session_path: &Path) -> bool {
    if let Ok(config) = load_launcher_config().await {
        if config.encrypt_session {
            return true;
        }
    }
    match fs::read_to_string(session_path).await {
        Ok(raw) => crate::core::session_crypto::EncryptedSession::from_json(&raw).is_some(),
        Err(_) => false,
    }
}

pub async fn save_session(session: &SessionState) -> Result<()> {
    let session_path = session_path()?;
    if session_encryption_active(&session_path).await {
        let key =
            tokio::task::spawn_blocking(crate::core::session_crypto::keyring_secret_or_create)
                .await
                .ok()
                .flatten();
        match key {
            Some(key) => {
                let plain = serde_json::to_vec(session)?;
                let mut envelope = crate::core::session_crypto::encrypt(&plain, &key)?;
                envelope.keyring = true;
                envelope.display_name = session
                    .persona_name
                    .clone()
                    .or_else(|| session.account_name.clone());
                write_json_private(&session_path, &envelope).await?;
            }
            None => {
                tracing::warn!(
                    "no OS keyring available; session.json stays plaintext \
                     (owner-only permissions)"
                );
                write_json_private(&session_path, session).await?;
            }
        }
    } else {
        write_json_private(&session_path, session).await?;
    }

    Ok(())
}

pub async fn save_session_plaintext(session: &SessionState) -> Result<()> {
    let mut config = load_launcher_config().await?;
    if config.encrypt_session {
        config.encrypt_session = false;
        save_launcher_config(&config).await?;
    }
    write_json_private(&session_path()?, session).await
}

pub async fn delete_session() -> Result<()> {
    let session_path = session_path()?;
    if session_path.exists() {
        fs::remove_file(session_path).await?;
    }
    Ok(())
}

pub async fn load_launcher_config() -> Result<LauncherConfig> {
    let path = config_dir()?.join("config.json");
    if !path.exists() {
        let mut config = LauncherConfig::default();
        if let Some(detected) = detect_steam_path() {
            config.steam_library_path = detected.to_string_lossy().to_string();
        }
        return Ok(config);
    }
    read_json(&path).await.with_context(|| {
        format!(
            "invalid launcher config — fix the JSON in {}, or delete it to regenerate defaults",
            path.display()
        )
    })
}

pub async fn save_launcher_config(config: &LauncherConfig) -> Result<()> {
    let path = config_dir()?.join("config.json");
    write_json_pretty(&path, config).await
}

pub fn load_proxy_config_blocking() -> ProxyConfig {
    let Ok(path) = config_dir().map(|dir| dir.join("config.json")) else {
        return ProxyConfig::default();
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return ProxyConfig::default();
    };
    serde_json::from_str::<LauncherConfig>(&raw)
        .map(|config| config.proxy)
        .unwrap_or_default()
}

pub async fn set_game_pin(app_id: u32, manifests: HashMap<u32, u64>) -> Result<()> {
    let mut cfg = load_launcher_config().await?;
    let gc = cfg.game_configs.entry(app_id).or_default();
    gc.pinned = true;
    gc.pinned_manifests = manifests;
    save_launcher_config(&cfg).await
}

pub async fn clear_game_pin(app_id: u32) -> Result<()> {
    let mut cfg = load_launcher_config().await?;
    if let Some(gc) = cfg.game_configs.get_mut(&app_id) {
        gc.pinned = false;
        gc.pinned_manifests.clear();
    }
    save_launcher_config(&cfg).await
}

pub async fn game_pin_state(app_id: u32) -> (bool, HashMap<u32, u64>) {
    match load_launcher_config().await {
        Ok(cfg) => cfg
            .game_configs
            .get(&app_id)
            .map(|gc| (gc.pinned, gc.pinned_manifests.clone()))
            .unwrap_or((false, HashMap::new())),
        Err(_) => (false, HashMap::new()),
    }
}

pub fn library_cache_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("library_cache.json"))
}

pub async fn save_library_cache(owned_games: &[OwnedGame]) -> Result<()> {
    write_json_pretty(&library_cache_path()?, &owned_games).await
}

pub async fn load_library_cache() -> Result<Vec<OwnedGame>> {
    let path = library_cache_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    read_json(&path).await
}

const INFO_CACHE_DEFAULT_TTL_SECS: u64 = 6 * 60 * 60;

pub fn info_cache_ttl() -> std::time::Duration {
    let secs = std::env::var("AURELIA_INFO_CACHE_TTL")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(INFO_CACHE_DEFAULT_TTL_SECS);
    std::time::Duration::from_secs(secs)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedAppInfo {
    pub fetched_at: u64,
    pub details: crate::steam_client::StoreAppInfo,
    pub dlc: Vec<(u32, Option<String>)>,
}

fn info_cache_dir() -> Result<PathBuf> {
    Ok(data_dir()?.join("info_cache"))
}

fn info_cache_path(app_id: u32, language: &str, country: &str) -> Result<PathBuf> {
    let sanitize = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect()
    };
    let (lang, cc) = (sanitize(language), sanitize(country));
    Ok(info_cache_dir()?.join(format!("{app_id}.{lang}.{cc}.json")))
}

use crate::core::utils::now_unix;

pub async fn load_info_cache(
    app_id: u32,
    language: &str,
    country: &str,
    ttl: std::time::Duration,
) -> Option<CachedAppInfo> {
    if ttl.is_zero() {
        return None;
    }
    let path = info_cache_path(app_id, language, country).ok()?;
    let raw = fs::read_to_string(&path).await.ok()?;
    let cached: CachedAppInfo = serde_json::from_str(&raw).ok()?;
    let age = now_unix().saturating_sub(cached.fetched_at);
    (age <= ttl.as_secs()).then_some(cached)
}

pub async fn save_info_cache(
    app_id: u32,
    language: &str,
    country: &str,
    details: &crate::steam_client::StoreAppInfo,
    dlc: &[(u32, Option<String>)],
) -> Result<()> {
    let record = CachedAppInfo {
        fetched_at: now_unix(),
        details: details.clone(),
        dlc: dlc.to_vec(),
    };
    write_json_pretty(&info_cache_path(app_id, language, country)?, &record).await
}

fn tag_name_cache_path(language: &str) -> Result<PathBuf> {
    let lang: String = language
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    Ok(data_dir()?.join(format!("tag_names.{lang}.json")))
}

pub async fn load_tag_name_cache(language: &str) -> std::collections::HashMap<u32, String> {
    let Ok(path) = tag_name_cache_path(language) else {
        return Default::default();
    };
    read_json(&path).await.unwrap_or_default()
}

pub async fn save_tag_name_cache(
    language: &str,
    names: &std::collections::HashMap<u32, String>,
) -> Result<()> {
    write_json_pretty(&tag_name_cache_path(language)?, names).await
}

pub async fn load_user_configs() -> Result<UserConfigStore> {
    let path = config_dir()?.join("user_apps.json");
    if !path.exists() {
        return Ok(UserConfigStore::new());
    }
    read_json(&path).await.with_context(|| {
        format!(
            "invalid per-game config — fix the JSON in {}, or delete it to start fresh",
            path.display()
        )
    })
}

pub async fn save_user_configs(configs: &UserConfigStore) -> Result<()> {
    let path = config_dir()?.join("user_apps.json");
    write_json_pretty(&path, configs).await
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
