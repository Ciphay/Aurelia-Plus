// Modified by Aurelia; see README.
use futures::{stream::FuturesOrdered, StreamExt};
use itertools::Itertools;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing;
use std::{fmt::Write, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::Semaphore,
};

use crate::{cdn::inner::InnerClient, Error};

/// Steam EDepotFileFlag::Executable bit.
pub const FLAG_EXECUTABLE: u32 = 32;

/// Steam EDepotFileFlag::Directory bit.
pub const FLAG_DIRECTORY: u32 = 64;

/// Steam EDepotFileFlag::InstallScript bit (0x100): the file is an install script that the client runs when the game starts.
pub const FLAG_INSTALL_SCRIPT: u32 = 256;

/// Steam EDepotFileFlag::Symlink bit.
pub const FLAG_SYMLINK: u32 = 512;

#[derive(Debug)]
pub struct ChunkData {
    pub(super) sha: Vec<u8>,
    pub(super) crc: u32,
    pub(super) offset: u64,
    pub(super) original_size: u32,
    pub(super) compressed_size: u32,
}

impl ChunkData {
    pub fn sha(&self) -> Vec<u8> {
        self.sha.clone()
    }

    pub fn id(&self) -> String {
        self.sha.iter().fold(String::new(), |mut output, b| {
            let _ = write!(output, "{b:02x}");
            output
        })
    }

    pub fn crc(&self) -> u32 {
        self.crc
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }

    pub fn original_size(&self) -> u32 {
        self.original_size
    }

    pub fn compressed_size(&self) -> u32 {
        self.compressed_size
    }
}

#[derive(Debug)]
pub struct ManifestFile {
    pub(super) inner: Arc<InnerClient>,
    pub(super) app_id: u32,
    pub(super) depot_id: u32,
    pub(super) filename: String,
    pub(super) size: u64,
    pub(super) flags: u32,
    pub(super) sha_filename: Vec<u8>,
    pub(super) sha_content: Vec<u8>,
    pub(super) chunks: Vec<ChunkData>,
    pub(super) linktarget: String,
}

impl ManifestFile {
    pub fn full_path(&self) -> String {
        self.filename
            .trim_matches(|c: char| c.is_whitespace() || c == '\0')
            .replace("\\", "/")
    }

    pub fn path(&self) -> String {
        let full_path = self.full_path();
        if let Some(pos) = full_path.rfind('/') {
            full_path[..pos].to_string()
        } else {
            full_path
        }
    }

    pub fn filename(&self) -> String {
        self.full_path()
            .rsplit("/")
            .next()
            .unwrap_or("")
            .to_string()
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn flags(&self) -> u32 {
        self.flags
    }

    pub fn sha_filename(&self) -> Vec<u8> {
        self.sha_filename.clone()
    }

    pub fn sha_content(&self) -> Vec<u8> {
        self.sha_content.clone()
    }

    pub fn chunks(&self) -> &Vec<ChunkData> {
        self.chunks.as_ref()
    }

    pub fn linktarget(&self) -> String {
        self.linktarget.clone()
    }

    /// Fetch this file's full content into memory. Symlink entries store
    /// their link target as chunk data, so they need content without a
    /// destination file on disk.
    pub async fn download_to_memory(&self, depot_key: [u8; 32]) -> Result<Vec<u8>, Error> {
        let mut buffer = vec![0u8; self.size as usize];
        for chunk in self.chunks().iter().sorted_by(|a, b| a.offset.cmp(&b.offset)) {
            let data = self
                .inner
                .get_chunk(
                    self.app_id,
                    self.depot_id,
                    depot_key,
                    chunk.id(),
                    &chunk.sha,
                    chunk.original_size,
                )
                .await?;
            let offset = chunk.offset as usize;
            let end = offset.saturating_add(data.len()).min(buffer.len());
            if offset < end {
                buffer[offset..end].copy_from_slice(&data[..end - offset]);
            }
        }
        Ok(buffer)
    }

    /// Download into `target_path`, reusing matching chunks.
    ///
    /// `on_progress(bytes, reused)` fires per chunk.
    pub async fn download(
        &self,
        depot_key: [u8; 32],
        target_path: &std::path::Path,
        verify_mode: bool,
        abort_signal: Option<Arc<AtomicBool>>,
        limit: Option<Arc<Semaphore>>,
        on_progress: Option<ProgressFn>,
    ) -> Result<(), Error> {
        let _ = verify_mode;
        let semaphore = limit.unwrap_or_else(|| Arc::new(Semaphore::new(4)));

        let metadata_before = tokio::fs::metadata(target_path).await;
        let file_exists_before = metadata_before.is_ok();
        let current_len_before = metadata_before.map(|m| m.len()).unwrap_or(0);

        let mut out = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(target_path)
            .await
            .map_err(Error::Io)?;

        if current_len_before != self.size {
            out.set_len(self.size).await.map_err(Error::Io)?;
        }

        let mut tasks = self
            .chunks()
            .iter()
            .sorted_by(|&a, &b| a.offset.cmp(&b.offset))
            .map(|chunk_data| {
                let job = ChunkJob {
                    inner: self.inner.clone(),
                    app_id: self.app_id,
                    depot_id: self.depot_id,
                    depot_key,
                    id: chunk_data.id(),
                    sha: chunk_data.sha.clone(),
                    offset: chunk_data.offset,
                    size: chunk_data.original_size,
                    target_path: target_path.to_path_buf(),
                    // Only existing files hold chunks.
                    check_disk: file_exists_before,
                };
                AbortOnDrop(tokio::spawn(job.run(
                    semaphore.clone(),
                    abort_signal.clone(),
                    on_progress.clone(),
                )))
            })
            .collect::<FuturesOrdered<_>>();

        while let Some(result) = tasks.next().await {
            if let Some(signal) = &abort_signal {
                if signal.load(Ordering::Relaxed) {
                    break;
                }
            }
            let (offset, data) = result??;
            // Counted by the task on arrival.
            if let Some(data) = data {
                out.seek(tokio::io::SeekFrom::Start(offset))
                    .await
                    .map_err(Error::Io)?;
                out.write_all(&data).await.map_err(Error::Io)?;
            }
        }
        out.flush().await.map_err(Error::Io)?;

        #[cfg(unix)]
        self.apply_unix_mode(target_path).await?;

        Ok(())
    }

    #[cfg(unix)]
    pub(crate) async fn apply_unix_mode(&self, target_path: &std::path::Path) -> Result<(), Error> {
        use std::os::unix::fs::PermissionsExt;

        if self.flags & FLAG_EXECUTABLE == 0 {
            // Flagless depots: detect ELF/shebang content.
            let mut head = [0u8; 4];
            let looks_executable = match tokio::fs::File::open(target_path).await {
                Ok(mut f) => match f.read(&mut head).await {
                    Ok(n) => (n >= 4 && head == *b"\x7fELF") || (n >= 2 && head[..2] == *b"#!"),
                    Err(_) => false,
                },
                Err(_) => false,
            };
            if !looks_executable {
                return Ok(());
            }
        }
        let metadata = tokio::fs::metadata(target_path)
            .await
            .map_err(Error::Io)?;
        let mut permissions = metadata.permissions();
        if permissions.mode() & 0o111 != 0 {
            return Ok(());
        }
        permissions.set_mode(permissions.mode() | 0o755);
        tokio::fs::set_permissions(target_path, permissions)
            .await
            .map_err(Error::Io)?;
        Ok(())
    }
}

/// `(bytes, reused_from_disk)` progress callback.
pub type ProgressFn = Arc<dyn Fn(u64, bool) + Send + Sync + 'static>;

/// Spawned task, aborted when dropped.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl<T> std::future::Future for AbortOnDrop<T> {
    type Output = Result<T, tokio::task::JoinError>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::pin::Pin::new(&mut self.0).poll(cx)
    }
}

/// One chunk: reuse or fetch.
struct ChunkJob {
    inner: Arc<InnerClient>,
    app_id: u32,
    depot_id: u32,
    depot_key: [u8; 32],
    id: String,
    sha: Vec<u8>,
    offset: u64,
    size: u32,
    target_path: std::path::PathBuf,
    check_disk: bool,
}

impl ChunkJob {
    /// `(offset, None)` when reused or aborted.
    async fn run(
        self,
        semaphore: Arc<Semaphore>,
        abort_signal: Option<Arc<AtomicBool>>,
        on_progress: Option<ProgressFn>,
    ) -> Result<(u64, Option<Vec<u8>>), Error> {
        let aborted = || abort_signal.as_ref().is_some_and(|s| s.load(Ordering::Relaxed));
        if aborted() {
            return Ok((self.offset, None));
        }
        let _permit = semaphore.acquire_owned().await?;
        if aborted() {
            return Ok((self.offset, None));
        }
        if self.check_disk && self.on_disk().await? {
            tracing::debug!("Verified chunk {}", self.id);
            if let Some(cb) = &on_progress {
                cb(self.size as u64, true);
            }
            return Ok((self.offset, None));
        }
        let data = self
            .inner
            .get_chunk(
                self.app_id,
                self.depot_id,
                self.depot_key,
                self.id.clone(),
                &self.sha,
                self.size,
            )
            .await?;
        // On arrival, not in file order.
        if let Some(cb) = &on_progress {
            cb(data.len() as u64, false);
        }
        Ok((self.offset, Some(data)))
    }

    /// Whether the bytes on disk match. The read and the SHA-1 run on the few low-priority verify threads (see `verify_pool`), not on
    /// the async runtime: a resume checks the whole game, and on the runtime's own threads that starved everything else in the process.
    async fn on_disk(&self) -> Result<bool, Error> {
        // Oversized chunks: refetch, then reject.
        if self.size as usize > crate::utils::lzma::MAX_CHUNK_LEN {
            return Ok(false);
        }
        let (path, offset, size) = (self.target_path.clone(), self.offset, self.size as usize);
        let (sha, id) = (self.sha.clone(), format!("{}", self.id));
        let (tx, rx) = tokio::sync::oneshot::channel();
        verify_pool::run(Box::new(move || {
            verify_pool::yield_to_others();
            let _ = tx.send(verify_pool::chunk_matches(&path, offset, size, sha.as_slice(), &id));
        }));
        Ok(rx.await.unwrap_or(false))
    }
}

/// Threads that read and hash chunks already on disk when a download is resumed or verified. Their number follows the cores of the PC
/// (`AURELIA_VERIFY_THREADS` overrides it, 1..=32) and each runs at the lowest CPU priority and idle disk priority of its own thread only: the rest of the process (the runtime
/// serving the Steam connection, the bridge of the games) and the games themselves keep their priority.
mod verify_pool {
    use sha1::Digest;
    use std::io::{Read, Seek, SeekFrom};
    use std::sync::{mpsc, Arc, Mutex, OnceLock};

    type Job = Box<dyn FnOnce() + Send>;

    pub(super) fn run(job: Job) {
        static POOL: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
        let tx = POOL.get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Job>();
            let rx = Arc::new(Mutex::new(rx));
            let threads = std::env::var("AURELIA_VERIFY_THREADS")
                .ok()
                .and_then(|v| v.trim().parse::<usize>().ok())
                .map(|n| n.clamp(1, 32))
                .unwrap_or_else(default_threads);
            for n in 0..threads {
                let rx = rx.clone();
                let _ = std::thread::Builder::new().name(format!("verify-{n}")).spawn(move || {
                    lower_priority();
                    loop {
                        let job = match rx.lock() {
                            Ok(guard) => guard.recv(),
                            Err(_) => return,
                        };
                        match job {
                            Ok(job) => job(),
                            Err(_) => return,
                        }
                    }
                });
            }
            tx
        });
        let _ = tx.send(job);
    }

    /// Half of the cores the process may use (a PC with 4 threads gets 2, one with 32 gets 16), never fewer than 2 or more than 16. More
    /// threads cost the rest nothing: they run at the lowest priority, so whatever else wants the CPU or the disk goes first.
    fn default_threads() -> usize {
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        (cores / 2).clamp(2, 16)
    }

    /// The I/O class `idle` only counts under the BFQ scheduler; on an SSD with mq-deadline (or none) the verification reads at full speed
    /// next to everything else, and a game that starts meanwhile waits for the disk (its launcher took minutes). So the threads also watch
    /// the system's I/O pressure (PSI) and step aside while it is high, whoever causes it: the check then takes what the disk has to spare.
    pub(super) fn yield_to_others() {
        #[cfg(target_os = "linux")]
        {
            use std::sync::atomic::{AtomicU64, Ordering};
            use std::time::{Duration, Instant};
            // (millis since START of the last read, pressure x100)
            static START: OnceLock<Instant> = OnceLock::new();
            static LAST_AT: AtomicU64 = AtomicU64::new(u64::MAX);
            static LAST_VAL: AtomicU64 = AtomicU64::new(0);
            const LIMIT_X100: u64 = 1500; // "some" avg10 of 15 %
            let start = *START.get_or_init(Instant::now);
            for _ in 0..20 {
                let now = start.elapsed().as_millis() as u64;
                let at = LAST_AT.load(Ordering::Relaxed);
                let value = if at != u64::MAX && now.saturating_sub(at) < 300 {
                    LAST_VAL.load(Ordering::Relaxed)
                } else {
                    let v = std::fs::read_to_string("/proc/pressure/io")
                        .ok()
                        .and_then(|t| {
                            let line = t.lines().find(|l| l.starts_with("some"))?;
                            let avg = line.split_whitespace().find_map(|f| f.strip_prefix("avg10="))?;
                            avg.parse::<f64>().ok()
                        })
                        .map(|f| (f * 100.0) as u64)
                        .unwrap_or(0);
                    LAST_VAL.store(v, Ordering::Relaxed);
                    LAST_AT.store(now, Ordering::Relaxed);
                    v
                };
                if value < LIMIT_X100 {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100)); // at most 2 s per chunk, so a busy disk slows the check but never stops it
            }
        }
    }

    /// Only the calling thread: on Linux `setpriority` and `ioprio_set` with `who = 0` act on the thread, not on the process.
    #[cfg(target_os = "linux")]
    fn lower_priority() {
        const IOPRIO_WHO_PROCESS: libc::c_long = 1;
        const IOPRIO_CLASS_IDLE: libc::c_long = 3;
        // SAFETY: both calls only change the scheduling of the calling thread.
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, 0, 19);
            libc::syscall(libc::SYS_ioprio_set, IOPRIO_WHO_PROCESS, 0 as libc::c_long, IOPRIO_CLASS_IDLE << 13);
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn lower_priority() {}

    pub(super) fn chunk_matches(path: &std::path::Path, offset: u64, size: usize, sha: &[u8], id: &str) -> bool {
        let Ok(mut file) = std::fs::File::open(path) else {
            return false;
        };
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        if len < offset + size as u64 {
            return false;
        }
        if file.seek(SeekFrom::Start(offset)).is_err() {
            return false;
        }
        let mut buffer = vec![0u8; size];
        if file.read_exact(&mut buffer).is_err() {
            return false;
        }
        let matches = sha1::Sha1::digest(&buffer).as_slice() == sha;
        if !matches {
            tracing::warn!("Corruption detected in chunk {}", id);
        }
        matches
    }
}

#[cfg(test)]
mod verify_pool_tests {
    use super::verify_pool;
    use sha1::Digest;

    #[test]
    fn matches_and_runs_at_low_priority() {
        let dir = std::env::temp_dir().join(format!("verify-pool-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.bin");
        std::fs::write(&path, b"0123456789abcdef").unwrap();
        let sha = sha1::Sha1::digest(b"456789").to_vec();
        let (tx, rx) = std::sync::mpsc::channel();
        let p = path.clone();
        verify_pool::run(Box::new(move || {
            let ok = verify_pool::chunk_matches(&p, 4, 6, &sha, "c");
            let bad = verify_pool::chunk_matches(&p, 5, 6, &sha, "c");
            let nice = std::fs::read_to_string("/proc/thread-self/stat").ok().and_then(|s| {
                let rest = s.rsplit(')').next()?.to_string();
                rest.split_whitespace().nth(16).map(str::to_string)
            });
            tx.send((ok, bad, nice)).unwrap();
        }));
        let (ok, bad, nice) = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(ok && !bad);
        if cfg!(target_os = "linux") {
            assert_eq!(nice.as_deref(), Some("19"));
        }
        std::fs::remove_dir_all(dir).ok();
    }
}
