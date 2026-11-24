use anyhow::{Context, Result};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::transport;
use super::{Header, proto};

const SPAWN_WAIT: std::time::Duration = std::time::Duration::from_millis(100);
const SPAWN_ATTEMPTS: u32 = 50;

pub const CONNECTION_LOST_EXIT: i32 = 75;

pub enum Forwarded {
    Done(i32),
    OutputClosed,
    Unavailable,
}

pub async fn try_forward() -> Result<Forwarded> {
    let Some(stream) = connect_or_spawn().await else {
        return Ok(Forwarded::Unavailable);
    };
    let argv: Vec<String> = std::env::args().collect();
    forward(stream, argv).await
}

async fn connect_or_spawn() -> Option<impl AsyncRead + AsyncWrite + Unpin + Send> {
    let managed = transport::systemd_managed();
    if let Ok(stream) = transport::connect().await {
        let current = env!("CARGO_PKG_VERSION");
        let mut info = super::read_daemon_info();
        if managed && info.is_none() {
            info = wait_for_marker().await;
        }
        if !daemon_needs_restart(info.as_ref(), current) {
            return Some(stream);
        }
        let running = aurelia::compat::running::list_active();
        if !running.is_empty() {
            tracing::warn!(
                "the aurelia daemon is from another build, but {} game(s) still run through it; keeping it until they exit",
                running.len()
            );
            return Some(stream);
        }
        drop(stream);
        let reported = info.as_ref().map_or("unknown", |i| i.version.as_str());
        tracing::info!(
            "restarting aurelia daemon: it is running v{reported} but this binary is v{current}"
        );
        if managed {
            restart_managed(info.as_ref().map(|i| i.pid));
            for _ in 0..(SPAWN_ATTEMPTS * 2) {
                if let Ok(stream) = transport::connect().await {
                    wait_for_marker().await;
                    return Some(stream);
                }
                tokio::time::sleep(SPAWN_WAIT).await;
            }
            return None;
        }
        restart_daemon(info.as_ref().map(|i| i.pid));

        for _ in 0..SPAWN_ATTEMPTS {
            if transport::connect().await.is_err() {
                break;
            }
            tokio::time::sleep(SPAWN_WAIT).await;
        }
    }

    if std::env::var_os("AURELIA_NO_SPAWN").is_some_and(|v| !v.is_empty()) {
        return None;
    }

    spawn_daemon().ok()?;
    for _ in 0..SPAWN_ATTEMPTS {
        tokio::time::sleep(SPAWN_WAIT).await;
        if let Ok(stream) = transport::connect().await {
            return Some(stream);
        }
    }
    None
}

async fn wait_for_marker() -> Option<super::DaemonInfo> {
    for _ in 0..150 {
        if let Some(info) = super::read_daemon_info() {
            return Some(info);
        }
        tokio::time::sleep(SPAWN_WAIT).await;
    }
    None
}

fn restart_managed(pid: Option<u32>) {
    let restarted = std::process::Command::new("systemctl")
        .args(["--user", "restart", "aurelia-daemon.service"])
        .status()
        .is_ok_and(|s| s.success());
    if !restarted {
        restart_daemon(pid);
    } else {
        super::clear_daemon_info();
    }
}

fn daemon_needs_restart(info: Option<&super::DaemonInfo>, current: &str) -> bool {
    let Some(info) = info else { return true };
    if info.version != current {
        return true;
    }
    match (&info.build_id, super::current_build_id()) {
        (Some(daemon), Some(client)) => *daemon != client,
        (None, _) => true,
        (_, None) => false,
    }
}

fn restart_daemon(pid: Option<u32>) {
    let killed = match pid {
        Some(pid) => crate::proc_admin::kill_pids(&[pid]),
        None => {
            let daemons: Vec<u32> = crate::proc_admin::find_aurelia_processes()
                .into_iter()
                .filter(|p| p.is_daemon)
                .map(|p| p.pid)
                .collect();
            crate::proc_admin::kill_pids(&daemons)
        }
    };
    tracing::info!("stopped {killed} stale aurelia daemon(s)");
    super::clear_daemon_info();
}

pub async fn start_background() -> Result<bool> {
    if transport::connect().await.is_ok() {
        return Ok(false);
    }
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("daemon").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::setpriority(libc::PRIO_PROCESS, 0, 10);
                libc::setsid();
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    detach(&mut cmd);
    cmd.spawn().context("could not start the daemon")?;
    Ok(true)
}

fn spawn_daemon() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("daemon")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    detach(&mut cmd);
    cmd.spawn()?;
    Ok(())
}

#[cfg(unix)]
fn detach(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

#[cfg(windows)]
fn detach(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    clear_std_handle_inheritance();
}

#[cfg(windows)]
fn clear_std_handle_inheritance() {
    use windows_sys::Win32::Foundation::{
        HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        unsafe {
            let handle = GetStdHandle(id);
            if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

async fn write_and_flush<W: AsyncWrite + Unpin>(w: &mut W, data: &[u8]) -> std::io::Result<()> {
    w.write_all(data).await?;
    w.flush().await
}

fn is_output_closed(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::UnexpectedEof
    )
}

async fn forward<S>(stream: S, argv: Vec<String>) -> Result<Forwarded>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut reader, writer) = proto::split_shared(stream);

    let header = serde_json::to_vec(&Header {
        argv,
        config_dir: super::own_config_dir(),
        cwd: std::env::current_dir().ok(),
        env: aurelia::core::client_env::collect(),
    })?;
    proto::send_frame(&writer, proto::C_HEADER, &header)
        .await
        .context("failed sending request to daemon")?;

    let writer_in = writer.clone();
    let stdin_task = tokio::spawn(async move {
        let mut stdin = tokio::io::stdin();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf).await {
                Ok(0) => {
                    let _ = proto::send_frame(&writer_in, proto::C_STDIN_EOF, &[]).await;
                    break;
                }
                Ok(n) => {
                    if proto::send_frame(&writer_in, proto::C_STDIN, &buf[..n])
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut stdout = tokio::io::stdout();
    let mut stderr = tokio::io::stderr();
    let mut code = CONNECTION_LOST_EXIT;
    loop {
        let relayed = match proto::read_frame(&mut reader).await? {
            Some((proto::C_STDOUT, data)) => write_and_flush(&mut stdout, &data).await,
            Some((proto::C_STDERR, data)) => write_and_flush(&mut stderr, &data).await,
            Some((proto::C_EXIT, data)) => {
                code = i32::from_be_bytes(
                    data.get(..4)
                        .and_then(|b| b.try_into().ok())
                        .unwrap_or([0; 4]),
                );
                break;
            }
            Some(_) => Ok(()),
            None => {
                let line = format!(
                    "{{\"error\":\"the daemon connection was lost before the command finished\",\"type\":\"daemon_connection_lost\"}}\n"
                );
                let _ = write_and_flush(&mut stderr, line.as_bytes()).await;
                break;
            }
        };
        if let Err(e) = relayed {
            stdin_task.abort();
            if is_output_closed(&e) {
                return Ok(Forwarded::OutputClosed);
            }
            return Err(e).context("failed relaying daemon output");
        }
    }
    stdin_task.abort();
    Ok(Forwarded::Done(code))
}

pub struct Session {
    pub stdout: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    writer: Box<dyn Fn(u8, Vec<u8>) -> std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<()>> + Send>> + Send + Sync>,
    exit: tokio::sync::oneshot::Receiver<i32>,
}

impl Session {
    pub async fn write_stdin(&self, data: &[u8]) -> std::io::Result<()> {
        (self.writer)(proto::C_STDIN, data.to_vec()).await
    }

    pub async fn wait(self) -> Option<i32> {
        let Session { exit, writer, stdout } = self;
        let code = exit.await.ok();
        drop((writer, stdout));
        code
    }
}

pub async fn open(args: &[&str]) -> Result<Option<Session>> {
    let Some(stream) = connect_or_spawn().await else {
        return Ok(None);
    };
    let (mut reader, writer) = proto::split_shared(stream);
    let mut argv = vec![std::env::args().next().unwrap_or_else(|| "aurelia".into())];
    argv.extend(args.iter().map(|a| a.to_string()));
    let header = serde_json::to_vec(&Header { argv, config_dir: super::own_config_dir(), cwd: std::env::current_dir().ok(), env: aurelia::core::client_env::collect() })?;
    proto::send_frame(&writer, proto::C_HEADER, &header).await.context("failed sending request to daemon")?;
    let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel();
    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        loop {
            match proto::read_frame(&mut reader).await {
                Ok(Some((proto::C_STDOUT, data))) => {
                    let _ = out_tx.send(data);
                }
                Ok(Some((proto::C_STDERR, data))) => {
                    let _ = write_and_flush(&mut tokio::io::stderr(), &data).await;
                }
                Ok(Some((proto::C_EXIT, data))) => {
                    let _ = exit_tx.send(i32::from_be_bytes(data.get(..4).and_then(|b| b.try_into().ok()).unwrap_or([0; 4])));
                    break;
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break,
            }
        }
    });
    let writer = Box::new(move |channel: u8, data: Vec<u8>| {
        let writer = writer.clone();
        Box::pin(async move { proto::send_frame(&writer, channel, &data).await }) as std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<()>> + Send>>
    });
    Ok(Some(Session { stdout: out_rx, writer, exit: exit_rx }))
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
