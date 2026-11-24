use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{Mutex, mpsc};

#[derive(Clone, Copy, Debug)]
pub enum Stream {
    Stdout,
    Stderr,
}

pub struct OutChunk {
    pub stream: Stream,
    pub bytes: Vec<u8>,
}

#[derive(Clone)]
pub struct OutputCtx {
    out: mpsc::UnboundedSender<OutChunk>,
    stdin: Arc<Mutex<DaemonStdin>>,
    cwd: Option<PathBuf>,
}

impl OutputCtx {
    pub fn new(
        out: mpsc::UnboundedSender<OutChunk>,
        stdin_rx: mpsc::UnboundedReceiver<Vec<u8>>,
        cwd: Option<PathBuf>,
    ) -> Self {
        Self {
            out,
            stdin: Arc::new(Mutex::new(DaemonStdin::new(stdin_rx))),
            cwd,
        }
    }
}

pub fn user_path(p: &Path) -> PathBuf {
    if p.is_absolute() || p.as_os_str().is_empty() {
        return p.to_path_buf();
    }
    let base = CTX
        .try_with(|ctx| ctx.cwd.clone())
        .ok()
        .flatten()
        .or_else(|| std::env::current_dir().ok());
    match base {
        Some(base) => base.join(p),
        None => p.to_path_buf(),
    }
}

pub fn user_path_str(s: String) -> String {
    user_path(Path::new(&s)).to_string_lossy().into_owned()
}

tokio::task_local! {
    static CTX: OutputCtx;
}

pub async fn scoped<F: Future>(ctx: OutputCtx, fut: F) -> F::Output {
    CTX.scope(ctx, fut).await
}

pub fn current() -> Option<OutputCtx> {
    CTX.try_with(|ctx| ctx.clone()).ok()
}

pub fn write(stream: Stream, s: &str) {
    let routed = CTX.try_with(|ctx| {
        let _ = ctx.out.send(OutChunk {
            stream,
            bytes: s.as_bytes().to_vec(),
        });
    });
    if routed.is_err() {
        use std::io::Write as _;
        let relay_stream = match stream {
            Stream::Stdout => aurelia::infra::logging::relay::RelayStream::Stdout,
            Stream::Stderr => aurelia::infra::logging::relay::RelayStream::Stderr,
        };
        aurelia::infra::logging::relay::broadcast_daemon_output(relay_stream, s.as_bytes());
        match stream {
            Stream::Stdout => {
                let _ = std::io::stdout().write_all(s.as_bytes());
            }
            Stream::Stderr => {
                let _ = std::io::stderr().write_all(s.as_bytes());
            }
        }
    }
}

pub async fn read_line() -> std::io::Result<String> {
    Ok(read_line_opt().await?.unwrap_or_default())
}

pub async fn read_line_opt() -> std::io::Result<Option<String>> {
    if let Ok(stdin) = CTX.try_with(|ctx| ctx.stdin.clone()) {
        let mut guard = stdin.lock().await;
        return guard.read_line_opt().await;
    }
    read_real_stdin_line().await
}

static REAL_STDIN: std::sync::OnceLock<Arc<Mutex<DaemonStdin>>> = std::sync::OnceLock::new();

fn real_stdin() -> Arc<Mutex<DaemonStdin>> {
    REAL_STDIN
        .get_or_init(|| {
            let (tx, rx) = mpsc::unbounded_channel();
            let _ = std::thread::Builder::new()
                .name("aurelia-stdin".to_string())
                .spawn(move || pump_reader(std::io::stdin(), tx));
            Arc::new(Mutex::new(DaemonStdin::new(rx)))
        })
        .clone()
}

fn pump_reader(mut reader: impl std::io::Read, tx: mpsc::UnboundedSender<Vec<u8>>) {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

async fn read_real_stdin_line() -> std::io::Result<Option<String>> {
    let stdin = real_stdin();
    let mut guard = stdin.lock().await;
    guard.read_line_opt().await
}

pub fn read_line_opt_blocking() -> std::io::Result<Option<String>> {
    use std::io::IsTerminal as _;
    use tokio::runtime::{Handle, RuntimeFlavor};
    let in_ctx = CTX.try_with(|_| ()).is_ok();
    if direct_tty_read(
        in_ctx,
        std::io::stdin().is_terminal(),
        REAL_STDIN.get().is_some(),
    ) {
        return read_std_line();
    }
    match Handle::try_current() {
        Ok(h) if h.runtime_flavor() == RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(|| h.block_on(read_line_opt()))
        }
        Ok(_) => read_std_line(),
        Err(_) => tokio::runtime::Builder::new_current_thread()
            .build()?
            .block_on(read_line_opt()),
    }
}

fn direct_tty_read(in_ctx: bool, stdin_is_tty: bool, pump_started: bool) -> bool {
    !in_ctx && stdin_is_tty && !pump_started
}

fn read_std_line() -> std::io::Result<Option<String>> {
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim_end_matches(['\n', '\r']).to_string()))
}

struct DaemonStdin {
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
    buf: Vec<u8>,
    eof: bool,
}

impl DaemonStdin {
    fn new(rx: mpsc::UnboundedReceiver<Vec<u8>>) -> Self {
        Self {
            rx,
            buf: Vec::new(),
            eof: false,
        }
    }

    async fn read_line_opt(&mut self) -> std::io::Result<Option<String>> {
        loop {
            if let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=pos).collect();
                return Ok(Some(
                    String::from_utf8_lossy(&line)
                        .trim_end_matches(['\n', '\r'])
                        .to_string(),
                ));
            }
            if self.eof {
                if self.buf.is_empty() {
                    return Ok(None);
                }
                let rest = String::from_utf8_lossy(&self.buf).trim().to_string();
                self.buf.clear();
                return Ok(Some(rest));
            }
            match self.rx.recv().await {
                Some(chunk) => self.buf.extend_from_slice(&chunk),
                None => self.eof = true,
            }
        }
    }
}

#[macro_export]
macro_rules! cli_println {
    () => { $crate::output::write($crate::output::Stream::Stdout, "\n") };
    ($($arg:tt)*) => {
        $crate::output::write($crate::output::Stream::Stdout, &format!("{}\n", format_args!($($arg)*)))
    };
}

#[macro_export]
macro_rules! cli_print {
    ($($arg:tt)*) => {
        $crate::output::write($crate::output::Stream::Stdout, &format!("{}", format_args!($($arg)*)))
    };
}

#[macro_export]
macro_rules! cli_eprintln {
    () => { $crate::output::write($crate::output::Stream::Stderr, "\n") };
    ($($arg:tt)*) => {
        $crate::output::write($crate::output::Stream::Stderr, &format!("{}\n", format_args!($($arg)*)))
    };
}

#[macro_export]
macro_rules! cli_eprint {
    ($($arg:tt)*) => {
        $crate::output::write($crate::output::Stream::Stderr, &format!("{}", format_args!($($arg)*)))
    };
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
