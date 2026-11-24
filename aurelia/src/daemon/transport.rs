pub fn endpoint() -> String {
    imp::endpoint()
}

fn config_dir_tag() -> Option<String> {
    let dir = std::env::var_os("AURELIA_CONFIG_DIR").filter(|v| !v.is_empty())?;
    let default = aurelia::core::config::home_dir()
        .ok()
        .map(|h| canonical(&h.join(".config").join("Aurelia")));
    config_tag_for(&canonical(std::path::Path::new(&dir)), default.as_deref())
}

pub(crate) fn canonical(p: &std::path::Path) -> std::path::PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

pub(super) fn config_tag_for(
    dir: &std::path::Path,
    default: Option<&std::path::Path>,
) -> Option<String> {
    if default == Some(dir) {
        return None;
    }
    let key = dir.to_string_lossy();
    Some(format!(
        "{:08x}",
        aurelia::steam_client::stable_logon_id(&key)
    ))
}

#[cfg_attr(not(unix), allow(dead_code))]
pub(super) fn unix_endpoint_for(
    socket_override: Option<String>,
    tag: Option<&str>,
    base: &std::path::Path,
    uid: u32,
) -> String {
    if let Some(p) = socket_override {
        return p;
    }
    let name = match tag {
        Some(t) => format!("aurelia-{uid}-{t}.sock"),
        None => format!("aurelia-{uid}.sock"),
    };
    base.join(name).to_string_lossy().into_owned()
}

pub fn version_marker_path() -> std::path::PathBuf {
    imp::version_marker_path()
}

pub fn socket_activated() -> bool {
    imp::socket_activated()
}

pub fn systemd_managed() -> bool {
    imp::systemd_managed()
}

pub fn remove_endpoint() {
    imp::remove_endpoint()
}

pub fn location_trusted() -> bool {
    imp::location_trusted()
}

#[cfg(all(unix, test))]
#[path = "transport_tests.rs"]
mod tests;

#[cfg(unix)]
mod imp {
    use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use tokio::net::{UnixListener, UnixStream};

    fn uid() -> u32 {
        unsafe { libc::getuid() }
    }

    fn base_dir() -> PathBuf {
        let uid = uid();
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| std::env::temp_dir().join(format!("aurelia-{uid}")))
    }

    pub fn endpoint() -> String {
        let socket = std::env::var("AURELIA_DAEMON_SOCKET").ok();
        let tag = socket.is_none().then(super::config_dir_tag).flatten();
        super::unix_endpoint_for(socket, tag.as_deref(), &base_dir(), uid())
    }

    pub fn version_marker_path() -> PathBuf {
        PathBuf::from(endpoint()).with_extension("info")
    }

    pub(super) fn dir_is_private(dir: &Path) -> bool {
        std::fs::symlink_metadata(dir)
            .is_ok_and(|m| m.file_type().is_dir() && m.uid() == uid() && m.mode() & 0o022 == 0)
    }

    pub(super) fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
        match std::fs::symlink_metadata(dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(dir)?;
            }
            Err(e) => return Err(e),
            Ok(_) => {}
        }
        let meta = std::fs::symlink_metadata(dir)?;
        if !meta.file_type().is_dir() || meta.uid() != uid() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("{} is not a directory owned by this user", dir.display()),
            ));
        }
        if meta.mode() & 0o022 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    pub fn location_trusted() -> bool {
        let path = PathBuf::from(endpoint());
        let parent_ok = path.parent().is_some_and(|dir| {
            dir_is_private(dir) || std::env::var_os("AURELIA_DAEMON_SOCKET").is_some()
        });
        let socket_ok = std::fs::symlink_metadata(&path).map_or(true, |m| m.uid() == uid());
        parent_ok && socket_ok
    }

    static ACTIVATED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    pub fn activation_offered() -> bool {
        let pid_ok = std::env::var("LISTEN_PID").ok().and_then(|v| v.trim().parse::<u32>().ok()) == Some(std::process::id());
        let fds = std::env::var("LISTEN_FDS").ok().and_then(|v| v.trim().parse::<i32>().ok()).unwrap_or(0);
        pid_ok && fds >= 1
    }

    pub fn socket_activated() -> bool {
        ACTIVATED.load(std::sync::atomic::Ordering::SeqCst) || activation_offered()
    }

    pub fn systemd_managed() -> bool {
        if std::env::var_os("AURELIA_DAEMON_SOCKET").is_some() {
            return false;
        }
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| aurelia::core::config::home_dir().ok().map(|h| h.join(".config")));
        base.is_some_and(|b| b.join("systemd/user/aurelia-daemon.socket").exists())
    }

    pub fn remove_endpoint() {
        if ACTIVATED.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let path = PathBuf::from(endpoint());
        let ours = std::fs::symlink_metadata(&path)
            .is_ok_and(|m| m.file_type().is_socket() && m.uid() == uid());
        if ours {
            let _ = std::fs::remove_file(&path);
        }
    }

    pub struct Listener(UnixListener);

    impl Listener {
        pub async fn bind() -> std::io::Result<Self> {
            if activation_offered() {
                use std::os::fd::FromRawFd;
                const SD_LISTEN_FDS_START: i32 = 3;
                let listener = unsafe {
                    libc::fcntl(SD_LISTEN_FDS_START, libc::F_SETFD, libc::FD_CLOEXEC);
                    let std_listener = std::os::unix::net::UnixListener::from_raw_fd(SD_LISTEN_FDS_START);
                    std_listener.set_nonblocking(true)?;
                    UnixListener::from_std(std_listener)?
                };
                unsafe {
                    std::env::remove_var("LISTEN_PID");
                    std::env::remove_var("LISTEN_FDS");
                    std::env::remove_var("LISTEN_FDNAMES");
                }
                ACTIVATED.store(true, std::sync::atomic::Ordering::SeqCst);
                return Ok(Self(listener));
            }
            let path = PathBuf::from(endpoint());
            if std::env::var_os("AURELIA_DAEMON_SOCKET").is_none() {
                if let Some(parent) = path.parent() {
                    ensure_private_dir(parent)?;
                }
            }
            if let Ok(meta) = std::fs::symlink_metadata(&path) {
                if !meta.file_type().is_socket() || meta.uid() != uid() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        format!("{} exists and is not our socket", path.display()),
                    ));
                }
                std::fs::remove_file(&path)?;
            }
            let listener = UnixListener::bind(&path)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            Ok(Self(listener))
        }

        pub async fn next(&mut self) -> std::io::Result<UnixStream> {
            loop {
                let (stream, _addr) = self.0.accept().await?;
                match stream.peer_cred() {
                    Ok(cred) if cred.uid() == uid() => return Ok(stream),
                    Ok(cred) => {
                        tracing::warn!("daemon: rejected connection from uid {}", cred.uid());
                    }
                    Err(e) => tracing::warn!("daemon: unreadable peer credentials: {e}"),
                }
            }
        }
    }

    pub async fn connect() -> std::io::Result<UnixStream> {
        if !location_trusted() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("untrusted daemon endpoint {}", endpoint()),
            ));
        }
        UnixStream::connect(endpoint()).await
    }
}

#[cfg(windows)]
mod imp {
    use tokio::net::windows::named_pipe::{
        ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
    };

    const ERROR_PIPE_BUSY: i32 = 231;

    pub fn endpoint() -> String {
        if let Ok(p) = std::env::var("AURELIA_DAEMON_SOCKET") {
            return p;
        }
        let user = std::env::var("USERNAME").unwrap_or_else(|_| "default".to_string());
        match super::config_dir_tag() {
            Some(tag) => format!(r"\\.\pipe\aurelia-{user}-{tag}"),
            None => format!(r"\\.\pipe\aurelia-{user}"),
        }
    }

    pub fn location_trusted() -> bool {
        true
    }

    pub fn socket_activated() -> bool {
        false
    }

    pub fn systemd_managed() -> bool {
        false
    }

    pub fn remove_endpoint() {}

    pub fn version_marker_path() -> std::path::PathBuf {
        let name: String = endpoint()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        std::env::temp_dir().join(format!("{name}.info"))
    }

    pub struct Listener {
        name: String,
        server: NamedPipeServer,
    }

    impl Listener {
        pub async fn bind() -> std::io::Result<Self> {
            let name = endpoint();
            let server = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&name)?;
            Ok(Self { name, server })
        }

        pub async fn next(&mut self) -> std::io::Result<NamedPipeServer> {
            self.server.connect().await?;
            let next = ServerOptions::new().create(&self.name)?;
            Ok(std::mem::replace(&mut self.server, next))
        }
    }

    pub async fn connect() -> std::io::Result<NamedPipeClient> {
        let name = endpoint();
        loop {
            match ClientOptions::new().open(&name) {
                Ok(client) => return Ok(client),
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

pub use imp::{Listener, connect};
