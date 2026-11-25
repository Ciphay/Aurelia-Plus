use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use aurelia::core::models::DepotPlatform;

#[derive(Parser)]
#[command(
    name = "aurelia",
    version,
    about,
    long_about = None,
    help_template = "{before-help}{about-with-newline}Version: {version}\n\n{usage-heading} {usage}\n\n{all-args}{after-help}"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
    #[arg(short = 'j', long, global = true)]
    pub(crate) json: bool,
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub(crate) verbose: u8,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    Login(LoginArgs),
    Capabilities {
        path: Vec<String>,
    },
    List {
        #[arg(short, long)]
        installed: bool,
        #[arg(short, long)]
        search: Option<String>,
        #[arg(short = 'c', long)]
        collection: Option<String>,
        #[arg(short = 'o', long)]
        online: bool,
        #[arg(short = 'u', long)]
        check_updates: bool,
    },
    Info {
        #[arg(required = true)]
        app_ids: Vec<u32>,
        #[arg(short = 'e', long)]
        extended: bool,
        #[arg(short = 'n', long)]
        no_cache: bool,
        #[arg(short = 'l', long = "lang", alias = "language")]
        lang: Option<String>,
        #[arg(short = 'c', long, alias = "cc")]
        country: Option<String>,
    },
    Price {
        app_id: u32,
        #[arg(short = 'r', long, value_delimiter = ',')]
        compare: Vec<String>,
        #[arg(short = 'c', long, alias = "cc")]
        country: Option<String>,
        #[arg(short = 'l', long = "lang", alias = "language")]
        lang: Option<String>,
    },
    Search {
        term: String,
        #[arg(short = 'n', long, default_value_t = 20)]
        count: u32,
        #[arg(short = 'c', long, alias = "cc")]
        country: Option<String>,
        #[arg(short = 'l', long = "lang", alias = "language")]
        lang: Option<String>,
    },
    Deals {
        #[arg(short = 's', long, value_enum, default_value_t = DealsScopeArg::DiscountedTopSellers)]
        scope: DealsScopeArg,
        #[arg(short = 'm', long, default_value_t = 1)]
        min_discount: i32,
        #[arg(short = 'n', long, default_value_t = 20)]
        count: i32,
        #[arg(short = 'o', long, default_value_t = 0)]
        start: i32,
        #[arg(short = 'c', long, alias = "cc")]
        country: Option<String>,
        #[arg(short = 'l', long = "lang", alias = "language")]
        lang: Option<String>,
    },
    Players { app_id: u32 },
    Events {
        #[arg(short = 'c', long, alias = "cc")]
        country: Option<String>,
    },
    News {
        app_id: u32,
        #[arg(short = 'n', long, default_value_t = 10)]
        count: u32,
        #[arg(short = 'm', long, default_value_t = 300)]
        max_length: u32,
    },
    Reviews(ReviewsArgs),
    User { user: String },
    #[command(args_conflicts_with_subcommands = true)]
    Wishlist {
        user: Option<String>,
        #[arg(short = 'n', long, default_value_t = 50)]
        count: usize,
        #[arg(short = 'o', long, default_value_t = 0)]
        offset: usize,
        #[arg(short = 'c', long, alias = "cc")]
        country: Option<String>,
        #[arg(short = 'l', long = "lang", alias = "language")]
        lang: Option<String>,
        #[command(subcommand)]
        command: Option<WishlistCommand>,
    },
    Similar {
        app_id: u32,
        #[arg(short = 'n', long, default_value_t = 10)]
        count: i32,
        #[arg(short = 'c', long, alias = "cc")]
        country: Option<String>,
        #[arg(short = 'l', long = "lang", alias = "language")]
        lang: Option<String>,
    },
    Install(InstallArgs),
    Libraries,
    Update {
        app_id: Option<u32>,
        #[arg(short = 'f', long)]
        force: bool,
        /// Only check whether this game has an update, quickly and without downloading anything (needs an app id).
        #[arg(long, requires = "app_id", conflicts_with = "force")]
        check: bool,
    },
    Play {
        app_id: u32,
        #[arg(short, long)]
        proton: Option<String>,
        #[arg(short, long)]
        windows: bool,
        #[arg(short = 'n', long, conflicts_with_all = ["proton", "windows"])]
        native_engine: bool,
        #[arg(short = 'u', long, conflicts_with_all = ["windows", "native_engine"])]
        umu: bool,
        #[arg(short = 'x', long, value_name = "PATH")]
        script: Option<PathBuf>,
        #[arg(short = 'X', long, conflicts_with = "script")]
        no_script: bool,
        #[arg(short = 's', long)]
        steam: bool,
        #[arg(short = 't', long, conflicts_with = "native_engine")]
        platform: Option<PlatformArg>,
        #[arg(short = 'm', long)]
        steam_mode: Option<SteamModeArg>,
        #[arg(long, value_name = "ID")]
        launch_option: Option<String>,
        #[arg(short = 'N', long = "no-update", alias = "noupdate")]
        no_update: bool,
        #[arg(short = 'k', long)]
        skip_shaders: bool,
        #[arg(short = 'L', long, value_name = "SECS", value_parser = clap::value_parser!(u64).range(1..))]
        launch_timeout: Option<u64>,
        #[arg(long, conflicts_with_all = ["windows", "native_engine", "umu", "script", "no_script", "steam", "platform", "steam_mode"])]
        plus: bool,
    },
    Shaders {
        app_id: u32,
        #[arg(short, long)]
        force: bool,
    },
    LaunchTargets {
        app_id: u32,
        #[arg(short = 's', long)]
        steam: bool,
    },
    Stop {
        app_id: Option<u32>,
        #[arg(short = 'f', long)]
        force: bool,
    },
    Running,
    Uninstall {
        app_id: u32,
        #[arg(short = 'd', long)]
        delete_prefix: bool,
    },
    Verify { app_id: u32 },
    Duplicates,
    Available { app_id: u32 },
    Watch,
    Bridge {
        #[command(subcommand)]
        command: BridgeCommand,
    },
    Shortcut {
        #[command(subcommand)]
        command: ShortcutCommand,
    },
    Drm { app_id: u32 },
    #[command(hide = true)]
    DebugBurst { emsg: Option<i32> },
    #[command(hide = true)]
    DebugCert {
        app_id: u32,
        #[arg(long)]
        key: Option<String>,
    },
    #[command(hide = true)]
    DebugServers {
        app_id: u32,
        #[arg(long, default_value = "")]
        filter: String,
    },
    #[command(hide = true)]
    DebugListen {
        app_id: u32,
        #[arg(long, default_value = "60")]
        secs: u64,
        #[arg(long = "emsg")]
        emsgs: Vec<i32>,
    },
    #[command(hide = true)]
    DebugSignal {
        app_id: u32,
        #[arg(long, default_value = "1")]
        sender: String,
        #[arg(long, default_value = "both")]
        playing: String,
        #[arg(long)]
        dest: Option<u64>,
    },
    Account,
    #[command(hide = true)]
    WebSession,
    Dlc { app_id: u32 },
    Enable {
        app_id: u32,
        #[arg(short = 'r', long)]
        restart_steam: bool,
    },
    Disable {
        app_id: u32,
        #[arg(short = 'r', long)]
        restart_steam: bool,
    },
    Achievements {
        app_id: u32,
        #[arg(short, long, alias = "language")]
        lang: Option<String>,
    },
    Image {
        app_id: u32,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(short, long)]
        force: bool,
        #[arg(short = 'l', long)]
        list: bool,
        #[arg(short = 'p', long, requires = "list")]
        probe: bool,
    },
    #[command(hide = true)]
    Appinfo { app_id: u32 },
    #[command(hide = true)]
    Tags {
        #[arg(short = 'd', long)]
        dump: bool,
    },
    Move {
        app_id: u32,
        library: PathBuf,
        #[arg(short = 'r', long)]
        restart_steam: bool,
    },
    Relink {
        app_id: u32,
        library: PathBuf,
        #[arg(short = 'r', long)]
        restart_steam: bool,
    },
    Import {
        app_id: u32,
        library: PathBuf,
        #[arg(short, long)]
        platform: Option<PlatformArg>,
        #[arg(short = 'r', long)]
        restart_steam: bool,
    },
    Branches { app_id: u32 },
    SetBranch { app_id: u32, branch: String },
    Depots { app_id: u32 },
    Manifests {
        app_id: u32,
        #[arg(short = 'd', long)]
        depot: Option<u32>,
    },
    Downgrade(DowngradeArgs),
    Pin { app_id: u32 },
    Unpin { app_id: u32 },
    LaunchOptions { app_id: u32 },
    Cloud {
        #[command(subcommand)]
        command: CloudCommand,
    },
    Workshop {
        #[command(subcommand)]
        command: WorkshopCommand,
    },
    Proton {
        #[command(subcommand)]
        command: ProtonCommand,
    },
    SteamRuntime {
        #[command(subcommand)]
        command: SteamRuntimeCommand,
    },
    Luxtorpeda {
        #[command(subcommand)]
        command: LuxtorpedaCommand,
    },
    Umu {
        #[command(subcommand)]
        command: UmuCommand,
    },
    Scripts {
        #[command(subcommand)]
        command: ScriptsCommand,
    },
    Collections {
        #[command(subcommand)]
        command: CollectionsCommand,
    },
    Friends {
        #[command(subcommand)]
        command: Option<FriendsCommand>,
    },
    Chat {
        #[command(subcommand)]
        command: ChatCommand,
    },
    Inventory {
        app_id: u32,
        #[arg(short = 'c', long, default_value_t = 2)]
        context: u32,
    },
    Wallet,
    Market {
        #[command(subcommand)]
        command: MarketCommand,
    },
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    Logout,
    SessionPlain,
    Daemon {
        #[arg(short = 's', long)]
        socket: Option<String>,
        #[command(subcommand)]
        command: Option<DaemonCommand>,
    },
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    Kill,
}

#[derive(Subcommand)]
pub(crate) enum ServiceCommand {
    Install,
    Uninstall,
    Status,
}

#[derive(Subcommand)]
pub(crate) enum DaemonCommand {
    Stop {
        pid: Option<u32>,
    },
    List,
    Start,
}

#[derive(Subcommand)]
pub(crate) enum ConfigCommand {
    Show,
    Protons,
    Presence {
        mode: Option<ChatPresenceArg>,
    },
    #[command(alias = "lang")]
    Language {
        lang: Option<String>,
    },
    PersonaWait {
        #[arg(value_parser = clap::value_parser!(u64).range(1..=300))]
        seconds: Option<u64>,
    },
    #[command(alias = "cc")]
    Country {
        country: Option<String>,
    },
    Experimental {
        enabled: Option<bool>,
    },
    SteamRuntimeRunner {
        runner: Option<String>,
    },
    SteamRuntimePolicy {
        policy: Option<SteamRuntimeArg>,
    },
    InstallResume {
        mode: Option<InstallResumeArg>,
    },
    DownloadLimit {
        #[arg(value_parser = parse_download_limit)]
        kbit: Option<u64>,
    },
    ShaderPrecache {
        mode: Option<ShaderPrecacheArg>,
    },
    HideUninstalledLaunchOptions {
        mode: Option<OnOffArg>,
    },
    AnticheatAutoInstall {
        mode: Option<OnOffArg>,
    },
    Proxy {
        url: Option<String>,
        #[arg(short = 'n', long, value_name = "LIST")]
        no_proxy: Option<String>,
        #[arg(short = 'c', long, conflicts_with_all = ["url", "no_proxy"])]
        clear: bool,
    },
    Game {
        #[command(flatten)]
        args: GameConfigArgs,
    },
    ClearGames {
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum ProtonCommand {
    List {
        #[arg(short = 'i', long)]
        installed: bool,
    },
    Install {
        version: String,
    },
    Uninstall { version: String },
    Default { version: String },
    Anticheat {
        runtime: AntiCheatArg,
    },
}

#[derive(Subcommand)]
pub(crate) enum SteamRuntimeCommand {
    Install {
        #[arg(short = 'r', long)]
        reinstall: bool,
    },
    Repair,
    Uninstall,
    #[command(visible_alias = "relogin")]
    Login,
    Stop,
    Status,
}

#[derive(Subcommand)]
pub(crate) enum LuxtorpedaCommand {
    Enable,
    Disable,
    Install,
    Update,
    Status,
    Path {
        path: Option<String>,
        #[arg(short = 'c', long, conflicts_with = "path")]
        clear: bool,
    },
    Uninstall,
}

#[derive(Subcommand)]
pub(crate) enum UmuCommand {
    Enable,
    Disable,
    Install,
    Update,
    Status,
    Path {
        path: Option<String>,
        #[arg(short = 'c', long, conflicts_with = "path")]
        clear: bool,
    },
    Uninstall,
}

#[derive(Subcommand)]
pub(crate) enum ScriptsCommand {
    Dir,
    List,
    New {
        app_id: u32,
        #[arg(short = 'f', long)]
        force: bool,
    },
    Show { app_id: u32 },
    Remove { app_id: u32 },
}

#[derive(Subcommand)]
pub(crate) enum CollectionsCommand {
    List,
    Show { name: String },
    Create { name: String },
    Delete { name: String },
    Rename { name: String, new_name: String },
    Add {
        name: String,
        #[arg(required = true)]
        app_ids: Vec<u32>,
    },
    Remove {
        name: String,
        #[arg(required = true)]
        app_ids: Vec<u32>,
    },
    Pull,
    Push {
        #[arg(short, long)]
        yes: bool,
        #[arg(long)]
        force: bool,
    },
    Sync {
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum FriendsCommand {
    List,
    Search { query: String },
    Add { query: String },
    Remove { steamid: u64 },
}

#[derive(Subcommand)]
pub(crate) enum MarketCommand {
    Price {
        app_id: u32,
        name: String,
        #[arg(short = 'c', long, default_value_t = 1)]
        currency: u32,
    },
    Search {
        query: Option<String>,
        #[arg(short = 'a', long)]
        app_id: Option<u32>,
        #[arg(short = 'n', long, default_value_t = 20)]
        count: u32,
    },
    Listings,
}

#[derive(Subcommand)]
pub(crate) enum ChatCommand {
    Send {
        steamid: u64,
        #[arg(required = true, trailing_var_arg = true)]
        message: Vec<String>,
    },
    Invite {
        steamid: u64,
        #[arg(short = 'a', long = "app")]
        app_id: u32,
        #[arg(short = 'c', long)]
        connect: Option<String>,
    },
    History {
        steamid: u64,
        #[arg(short = 'n', long, default_value_t = 20)]
        count: u32,
    },
    Open { steamid: u64 },
}

#[derive(Subcommand)]
pub(crate) enum ShortcutCommand {
    Add {
        name: String,
        exe: std::path::PathBuf,
        #[arg(long = "arg", allow_hyphen_values = true)]
        args: Vec<String>,
        #[arg(long)]
        start_dir: Option<std::path::PathBuf>,
        #[arg(long)]
        proton: Option<String>,
        #[arg(long)]
        steam_appid: Option<u32>,
    },
    List,
    Remove { app_id: u32 },
}

#[derive(Subcommand)]
pub(crate) enum BridgeCommand {
    Serve {
        #[arg(long)]
        app: u32,
    },
    Prewarm {
        #[arg(long)]
        app: u32,
    },
    Status,
    Simulate {
        #[arg(long, default_value_t = 480)]
        app: u32,
        #[arg(long, default_value_t = 400)]
        interval_ms: u64,
    },
}

#[derive(Subcommand)]
pub(crate) enum CloudCommand {
    Sync {
        app_id: u32,
        #[arg(short = 'u', long, conflicts_with = "down")]
        up: bool,
        #[arg(short = 'd', long, conflicts_with = "up")]
        down: bool,
        #[arg(short = 'p', long)]
        path: Option<PathBuf>,
        #[arg(short = 'r', long, value_enum)]
        resolve: Option<CloudResolve>,
    },
    List { app_id: u32 },
}

#[derive(Subcommand)]
pub(crate) enum WorkshopCommand {
    Browse {
        app_id: u32,
        #[arg(short, long)]
        search: Option<String>,
        #[arg(short = 'o', long, value_enum, default_value_t = WorkshopSort::Trend)]
        sort: WorkshopSort,
        #[arg(short = 'n', long, default_value_t = 20)]
        count: u32,
        #[arg(short = 'c', long, default_value = "*")]
        cursor: String,
        #[arg(short = 't', long = "tag")]
        tags: Vec<String>,
    },
    Info {
        #[arg(required = true)]
        ids: Vec<u64>,
    },
    List { app_id: u32 },
    Install {
        #[arg(required = true)]
        ids: Vec<u64>,
        #[arg(short = 'R', long)]
        no_recurse: bool,
    },
    Uninstall {
        #[arg(required = true)]
        ids: Vec<u64>,
        #[arg(short = 'R', long)]
        no_recurse: bool,
    },
    Subscribe {
        #[arg(required = true)]
        ids: Vec<u64>,
        #[arg(short = 'i', long)]
        install: bool,
        #[arg(short = 'R', long)]
        no_recurse: bool,
    },
    Unsubscribe {
        #[arg(required = true)]
        ids: Vec<u64>,
        #[arg(short = 'R', long)]
        no_recurse: bool,
    },
    Status { app_id: u32 },
    Rate {
        id: u64,
        vote: VoteArg,
    },
    Comments {
        id: u64,
        #[arg(short = 'n', long, default_value_t = 20)]
        count: i32,
        #[arg(short = 's', long, default_value_t = 0)]
        start: i32,
    },
    Comment {
        id: u64,
        text: String,
    },
    Publish {
        app_id: u32,
        #[arg(short, long)]
        content: Option<std::path::PathBuf>,
        #[arg(short, long)]
        item: Option<u64>,
        #[arg(short, long)]
        title: Option<String>,
        #[arg(short, long)]
        description: Option<String>,
        #[arg(long, value_enum)]
        visibility: Option<VisibilityArg>,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(short = 'n', long)]
        change_note: Option<String>,
        #[arg(long)]
        preview: Option<std::path::PathBuf>,
    },
    Mine { app_id: u32 },
    #[command(hide = true)]
    DebugManifest { app_id: u32, depot_id: u32, manifest_id: u64, out: std::path::PathBuf },
    Delete {
        app_id: u32,
        id: u64,
    },
}

#[derive(clap::Args)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
pub(crate) struct InstallArgs {
    #[command(subcommand)]
    pub(crate) action: Option<InstallAction>,
    pub(crate) app_id: Option<u32>,
    #[arg(short, long)]
    pub(crate) platform: Option<PlatformArg>,
    #[arg(short = 'r', long)]
    pub(crate) restart_steam: bool,
    #[arg(short = 'd', long)]
    pub(crate) dry_run: bool,
    #[arg(short = 'l', long)]
    pub(crate) library: Option<String>,
}

#[derive(clap::Subcommand)]
pub(crate) enum InstallAction {
    List,
    Stop { app_id: u32 },
    Pause { app_id: u32 },
    Resume {
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        app_id: Option<u32>,
        #[arg(short, long)]
        all: bool,
    },
    Forget {
        app_id: u32,
        #[arg(short, long)]
        delete_files: bool,
    },
}

#[derive(clap::Args)]
pub(crate) struct DowngradeArgs {
    pub(crate) app_id: u32,
    #[arg(short = 'd', long = "depot", value_name = "DEPOT_ID")]
    pub(crate) depots: Vec<u32>,
    #[arg(short = 'm', long = "manifest", value_name = "MANIFEST_ID")]
    pub(crate) manifests: Vec<String>,
    #[arg(short = 'b', long)]
    pub(crate) branch: Option<String>,
    #[arg(short = 'w', long)]
    pub(crate) branch_password: Option<String>,
    #[arg(short = 'l', long)]
    pub(crate) library: Option<String>,
    #[arg(short = 'V', long)]
    pub(crate) verify: bool,
    #[arg(short = 'N', long)]
    pub(crate) no_pin: bool,
}

#[derive(clap::Args)]
pub(crate) struct LoginArgs {
    #[arg(short, long)]
    pub(crate) username: Option<String>,
    #[arg(short, long)]
    pub(crate) password: Option<String>,
    #[arg(short, long)]
    pub(crate) guard: Option<String>,
    #[arg(short = 'q', long, conflicts_with_all = ["username", "password", "guard", "code"])]
    pub(crate) qr: bool,
    #[arg(short = 't', long, value_name = "SECS", requires = "qr", value_parser = clap::value_parser!(u64).range(1..))]
    pub(crate) timeout: Option<u64>,
    #[arg(short = 'c', long, visible_alias = "pin", conflicts_with = "guard")]
    pub(crate) code: bool,
    #[arg(short = 'o', long, hide = true, conflicts_with_all = ["username", "password", "guard", "qr", "code"])]
    pub(crate) openid: bool,
    #[arg(short = 'w', long, hide = true, num_args = 0..=1, conflicts_with_all = ["username", "password", "guard", "qr", "code", "openid"])]
    pub(crate) web_token: Option<Option<String>>,
    #[arg(short = 'H', long, conflicts_with_all = ["username", "password", "guard", "qr", "code", "openid", "web_token", "reconnect"])]
    pub(crate) health: bool,
    #[arg(short = 'r', long, conflicts_with_all = ["username", "password", "guard", "qr", "code", "openid", "web_token", "health"])]
    pub(crate) reconnect: bool,
}

#[derive(clap::Args)]
pub(crate) struct ReviewsArgs {
    pub(crate) app_id: u32,
    #[arg(short = 'f', long, default_value = "recent")]
    pub(crate) filter: String,
    #[arg(short = 't', long = "type", default_value = "all")]
    pub(crate) review_type: String,
    #[arg(short = 'p', long, default_value = "all")]
    pub(crate) purchase: String,
    #[arg(short = 'n', long, default_value_t = 10)]
    pub(crate) count: u32,
    #[arg(short = 'u', long)]
    pub(crate) cursor: Option<String>,
    #[arg(short = 'l', long = "lang", alias = "language")]
    pub(crate) lang: Option<String>,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum PlatformArg {
    Windows,
    Linux,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum SteamModeArg {
    Runtime,
    Host,
    None,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum AntiCheatArg {
    Eac,
    Battleye,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum OnOffArg {
    On,
    Off,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum ShaderPrecacheArg {
    Auto,
    Off,
}

impl From<ShaderPrecacheArg> for aurelia::launch::shaders::ShaderPrecache {
    fn from(value: ShaderPrecacheArg) -> Self {
        match value {
            ShaderPrecacheArg::Auto => Self::Auto,
            ShaderPrecacheArg::Off => Self::Off,
        }
    }
}

pub(crate) fn parse_download_limit(raw: &str) -> Result<u64, String> {
    let value = raw.trim();
    if ["off", "none", "unlimited"]
        .iter()
        .any(|w| value.eq_ignore_ascii_case(w))
    {
        return Ok(0);
    }
    value
        .parse::<u64>()
        .map_err(|_| format!("`{raw}` is not a number of kbit/s or `off`"))
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum InstallResumeArg {
    Manual,
    Auto,
}

impl From<InstallResumeArg> for aurelia::library::backlog::InstallResume {
    fn from(value: InstallResumeArg) -> Self {
        match value {
            InstallResumeArg::Manual => Self::Manual,
            InstallResumeArg::Auto => Self::Auto,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum SteamRuntimeArg {
    Auto,
    On,
    Off,
}

impl From<SteamRuntimeArg> for aurelia::core::models::SteamRuntimePolicy {
    fn from(value: SteamRuntimeArg) -> Self {
        use aurelia::core::models::SteamRuntimePolicy;
        match value {
            SteamRuntimeArg::Auto => SteamRuntimePolicy::Auto,
            SteamRuntimeArg::On => SteamRuntimePolicy::Enabled,
            SteamRuntimeArg::Off => SteamRuntimePolicy::Disabled,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum SteamPrefixModeArg {
    Shared,
    PerGame,
}

impl From<SteamPrefixModeArg> for aurelia::core::models::SteamPrefixMode {
    fn from(value: SteamPrefixModeArg) -> Self {
        use aurelia::core::models::SteamPrefixMode;
        match value {
            SteamPrefixModeArg::Shared => SteamPrefixMode::Shared,
            SteamPrefixModeArg::PerGame => SteamPrefixMode::PerGame,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum ChatPresenceArg {
    Online,
    Offline,
}

impl From<ChatPresenceArg> for aurelia::core::config::ChatPresence {
    fn from(value: ChatPresenceArg) -> Self {
        match value {
            ChatPresenceArg::Online => aurelia::core::config::ChatPresence::Online,
            ChatPresenceArg::Offline => aurelia::core::config::ChatPresence::Offline,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum CloudResolve {
    Cloud,
    Local,
}

impl From<CloudResolve> for aurelia::library::cloud_sync::ConflictPolicy {
    fn from(value: CloudResolve) -> Self {
        match value {
            CloudResolve::Cloud => aurelia::library::cloud_sync::ConflictPolicy::TakeCloud,
            CloudResolve::Local => aurelia::library::cloud_sync::ConflictPolicy::TakeLocal,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum WorkshopSort {
    Trend,
    Popular,
    Recent,
    Updated,
    Subscriptions,
    Text,
}

impl WorkshopSort {
    pub(crate) fn query_type(self) -> u32 {
        match self {
            WorkshopSort::Trend => 3,
            WorkshopSort::Popular => 0,
            WorkshopSort::Recent => 1,
            WorkshopSort::Updated => 21,
            WorkshopSort::Subscriptions => 9,
            WorkshopSort::Text => 12,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum VisibilityArg {
    Public,
    FriendsOnly,
    Private,
    Unlisted,
}

impl VisibilityArg {
    pub(crate) fn code(self) -> i32 {
        match self {
            VisibilityArg::Public => 0,
            VisibilityArg::FriendsOnly => 1,
            VisibilityArg::Private => 2,
            VisibilityArg::Unlisted => 3,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum VoteArg {
    Up,
    Down,
}

impl From<PlatformArg> for DepotPlatform {
    fn from(value: PlatformArg) -> Self {
        match value {
            PlatformArg::Windows => DepotPlatform::Windows,
            PlatformArg::Linux => DepotPlatform::Linux,
        }
    }
}

#[derive(Subcommand)]
pub(crate) enum WishlistCommand {
    Add { app_id: u32 },
    Remove { app_id: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum DealsScopeArg {
    DiscountedTopSellers,
    TopSellers,
    Specials,
}

#[derive(clap::Args)]
pub(crate) struct GameConfigArgs {
    pub(crate) app_id: u32,
    #[arg(short = 'p', long)]
    pub(crate) proton: Option<String>,
    #[arg(short = 'P', long, conflicts_with = "proton")]
    pub(crate) clear_proton: bool,
    #[arg(short = 't', long)]
    pub(crate) platform: Option<PlatformArg>,
    #[arg(short = 'T', long, conflicts_with = "platform")]
    pub(crate) no_platform: bool,
    #[arg(long, conflicts_with_all = ["proton", "clear_proton", "platform", "no_platform",
        "native_engine", "no_native_engine", "umu", "no_umu", "launch_script",
        "no_launch_script", "steam_runtime", "steam_prefix_mode"])]
    pub(crate) clear: bool,
    #[arg(short = 'n', long)]
    pub(crate) native_engine: bool,
    #[arg(short = 'N', long, conflicts_with = "native_engine")]
    pub(crate) no_native_engine: bool,
    #[arg(short = 'u', long, conflicts_with_all = ["native_engine", "no_native_engine"])]
    pub(crate) umu: bool,
    #[arg(short = 'U', long, conflicts_with = "umu")]
    pub(crate) no_umu: bool,
    #[arg(short = 's', long, value_name = "PATH")]
    pub(crate) launch_script: Option<PathBuf>,
    #[arg(short = 'S', long, conflicts_with = "launch_script")]
    pub(crate) no_launch_script: bool,
    #[arg(short = 'r', long, value_name = "auto|on|off")]
    pub(crate) steam_runtime: Option<SteamRuntimeArg>,
    #[arg(short = 'm', long, value_name = "shared|per-game")]
    pub(crate) steam_prefix_mode: Option<SteamPrefixModeArg>,
}
