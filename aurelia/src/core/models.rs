use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum SteamPrefixMode {
    #[default]
    Shared,
    PerGame,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum SteamRuntimePolicy {
    #[default]
    Auto,
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SteamLaunchConfig {
    #[serde(default = "default_true")]
    pub no_browser: bool,
    #[serde(default = "default_true")]
    pub no_friends_ui: bool,
    #[serde(default = "default_true")]
    pub no_overlay: bool,
    #[serde(default = "default_true")]
    pub no_chat_ui: bool,
    #[serde(default)]
    pub no_vr: bool,
    #[serde(default)]
    pub big_picture: bool,
}

pub fn default_true() -> bool {
    true
}

impl Default for SteamLaunchConfig {
    fn default() -> Self {
        Self {
            no_browser: true,
            no_friends_ui: true,
            no_overlay: true,
            no_chat_ui: true,
            no_vr: false,
            big_picture: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum GraphicsBackendPolicy {
    #[default]
    Auto,
    WineD3D,
    DXVK,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum D3D12ProviderPolicy {
    #[default]
    Auto,
    Vkd3dProton,
    Vkd3dWine,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphicsLayerConfig {
    #[serde(default)]
    pub dxvk_enabled: bool,
    #[serde(default)]
    pub vkd3d_proton_enabled: bool,
    #[serde(default)]
    pub vkd3d_enabled: bool,
    #[serde(default = "default_true")]
    pub nvapi_enabled: bool,
    #[serde(default)]
    pub graphics_backend_policy: GraphicsBackendPolicy,
    #[serde(default)]
    pub d3d12_policy: D3D12ProviderPolicy,
    #[serde(default)]
    pub use_symlinks_in_prefix: bool,
    #[serde(default)]
    pub custom_dxvk_path: Option<PathBuf>,
    #[serde(default)]
    pub custom_vkd3d_path: Option<PathBuf>,
    #[serde(default)]
    pub custom_vkd3d_proton_path: Option<PathBuf>,
}

impl Default for GraphicsLayerConfig {
    fn default() -> Self {
        Self {
            dxvk_enabled: false,
            vkd3d_proton_enabled: false,
            vkd3d_enabled: false,
            nvapi_enabled: true,
            graphics_backend_policy: GraphicsBackendPolicy::Auto,
            d3d12_policy: D3D12ProviderPolicy::Auto,
            use_symlinks_in_prefix: false,
            custom_dxvk_path: None,
            custom_vkd3d_path: None,
            custom_vkd3d_proton_path: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserAppConfig {
    pub launch_options: String,
    pub env_variables: HashMap<String, String>,
    pub use_steam_runtime: bool,
    #[serde(default)]
    pub steam_runtime_policy: SteamRuntimePolicy,
    #[serde(default)]
    pub steam_prefix_mode: SteamPrefixMode,
    #[serde(default)]
    pub steam_launch_config: SteamLaunchConfig,
    #[serde(default)]
    pub graphics_layers: GraphicsLayerConfig,
    #[serde(default)]
    pub gpu_preference: Option<String>,
    pub hidden: bool,
    pub favorite: bool,
}

pub type UserConfigStore = HashMap<u32, UserAppConfig>;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionState {
    pub account_name: Option<String>,
    #[serde(default)]
    pub persona_name: Option<String>,
    pub steam_id: Option<u64>,
    pub refresh_token: Option<crate::core::secret::Secret>,
    pub client_instance_id: Option<u64>,
    #[serde(default)]
    pub web_token: Option<crate::core::secret::Secret>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnedGame {
    pub app_id: u32,
    pub name: String,
    pub playtime_forever_minutes: u32,
    #[serde(default)]
    pub local_manifest_ids: HashMap<u64, u64>,
    #[serde(default)]
    pub update_available: bool,
}

fn default_branch() -> String {
    "public".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryGame {
    pub app_id: u32,
    pub name: String,
    pub playtime_forever_minutes: Option<u32>,
    pub is_installed: bool,
    pub install_path: Option<String>,
    #[serde(default)]
    pub local_manifest_ids: HashMap<u64, u64>,
    #[serde(default)]
    pub update_available: bool,
    #[serde(default)]
    pub update_queued: bool,
    #[serde(default = "default_branch")]
    pub active_branch: String,
    #[serde(default = "default_true")]
    pub is_owned: bool,
    #[serde(default)]
    pub is_family_shared: bool,
    #[serde(default)]
    pub online_required: Option<bool>,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub from_windows_steam: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GameLibrary {
    pub games: Vec<LibraryGame>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlcState {
    pub app_id: u32,
    pub owned: bool,
    pub installed: bool,
    pub disabled: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ExecutableArchitecture {
    #[default]
    Unknown,
    X86,
    X86_64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DepotPlatform {
    Linux,
    Windows,
}

pub struct ManifestSelection {
    pub app_id: u32,
    pub depot_id: u32,
    pub manifest_id: u64,
    pub appinfo_vdf: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SteamGuardReq {
    EmailCode { domain_hint: String },
    DeviceCode,
    DeviceConfirmation,
}

#[derive(Debug, Clone, Copy)]
pub enum QrEvent<'a> {
    Challenge(&'a str),
    Scanned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DownloadProgressState {
    #[default]
    Queued,
    Downloading,
    Verifying,
    Moving,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DownloadProgress {
    pub state: DownloadProgressState,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub current_file: String,
    pub depot_id: u32,
    pub depot_bytes_downloaded: u64,
    pub depot_total_bytes: u64,
    pub reused_bytes: u64,
    pub error_kind: Option<crate::core::error::ErrorKind>,
}

impl DownloadProgress {
    pub fn queued(msg: impl Into<String>) -> Self {
        Self {
            state: DownloadProgressState::Queued,
            current_file: msg.into(),
            ..Default::default()
        }
    }

    pub fn completed(msg: impl Into<String>) -> Self {
        Self {
            state: DownloadProgressState::Completed,
            bytes_downloaded: 1,
            total_bytes: 1,
            current_file: msg.into(),
            ..Default::default()
        }
    }

    pub fn failed(msg: impl Into<String>) -> Self {
        let msg = msg.into();
        Self {
            state: DownloadProgressState::Failed,
            error_kind: crate::core::error::message_kind(&msg),
            current_file: msg,
            ..Default::default()
        }
    }

    pub fn failed_with(context: &str, err: &anyhow::Error) -> Self {
        let kind = crate::core::error::classify(err).kind;
        Self {
            state: DownloadProgressState::Failed,
            current_file: format!("{context}: {err:#}"),
            error_kind: (kind != crate::core::error::ErrorKind::Unknown).then_some(kind),
            ..Default::default()
        }
    }
}

#[derive(Clone, Default)]
pub struct DownloadState {
    pub is_downloading: bool,
    pub is_paused: bool,
    pub app_id: u32,
    pub app_name: String,
    pub total_bytes: u64,
    pub downloaded_bytes: u64,
    pub reused_bytes: u64,
    pub status_text: String,
    pub depot_id: u32,
    pub depot_total_bytes: u64,
    pub depot_downloaded_bytes: u64,
    pub abort_signal: Arc<AtomicBool>,
    pub install_dir: Option<std::path::PathBuf>,
}

impl DownloadState {
    pub fn begin(&mut self, app_id: u32, app_name: String, total_bytes: u64, status_text: String) {
        self.is_downloading = true;
        self.app_id = app_id;
        self.app_name = app_name;
        self.downloaded_bytes = 0;
        self.reused_bytes = 0;
        self.total_bytes = total_bytes;
        self.depot_id = 0;
        self.depot_downloaded_bytes = 0;
        self.depot_total_bytes = 0;
        self.status_text = status_text;
    }

    pub fn finish(&mut self, status_text: &str) {
        self.is_downloading = false;
        self.status_text = status_text.to_string();
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct AppInfoRoot {
    #[serde(default)]
    pub appinfo: Option<AppInfoNode>,
    #[serde(default)]
    pub common: Option<CommonNode>,
    #[serde(default)]
    pub depots: HashMap<String, DepotNode>,
    #[serde(default)]
    pub branches: HashMap<String, BranchNode>,
    #[serde(default)]
    pub config: Option<ConfigNode>,
    #[serde(default)]
    pub extended: Option<ExtendedNode>,
}

#[derive(Debug, serde::Deserialize)]
pub struct AppInfoNode {
    #[serde(default)]
    pub common: Option<CommonNode>,
    #[serde(default)]
    pub depots: HashMap<String, DepotNode>,
    #[serde(default)]
    pub branches: HashMap<String, BranchNode>,
    #[serde(default)]
    pub config: Option<ConfigNode>,
    #[serde(default)]
    pub extended: Option<ExtendedNode>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ExtendedNode {
    #[serde(default)]
    pub dependantonapp: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ConfigNode {
    #[serde(default)]
    pub launch: HashMap<String, ProductLaunchEntry>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ProductLaunchEntry {
    #[serde(default)]
    pub executable: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub oslist: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct CommonNode {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub dlc: HashMap<String, String>,
    #[serde(default)]
    pub installdir: Option<String>,
    #[serde(rename = "type", default)]
    pub app_type: Option<String>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub category: HashMap<String, String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct DepotNode {
    #[serde(default)]
    pub config: Option<DepotConfig>,
    #[serde(default)]
    pub manifests: Option<DepotManifests>,
    #[serde(flatten)]
    pub _other: HashMap<String, serde_json::Value>,
}

#[derive(Debug, serde::Deserialize)]
pub struct BranchNode {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub pwdrequired: Option<String>,
    #[serde(default)]
    pub buildid: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct DepotConfig {
    #[serde(default)]
    pub oslist: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct DepotManifests {
    #[serde(default)]
    pub public: Option<String>,
}

#[derive(Debug, Clone, Copy, serde::Serialize, PartialEq)]
pub enum WorkshopItemKind {
    Item,
    Collection,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkshopItem {
    pub id: u64,
    pub app_id: u32,
    pub creator: u64,
    pub title: String,
    pub hcontent_file: u64,
    pub file_url: String,
    pub file_size: u64,
    pub time_updated: i64,
    pub kind: WorkshopItemKind,
    pub children: Vec<u64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkshopInstalledInfo {
    pub id: u64,
    pub manifest_id: u64,
    pub size: u64,
    pub time_updated: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkshopComment {
    pub id: u64,
    pub author: u64,
    pub timestamp: i64,
    pub text: String,
    pub upvotes: i32,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkshopQueryPage {
    pub items: Vec<WorkshopItem>,
    pub total: u32,
    pub next_cursor: String,
}
