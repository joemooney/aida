//! Frozen fallback guards and an AIDA-owned pipe evidence channel.
// trace:TASK-1730 | ai:codex
use super::*;
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::JoinHandle;

pub(super) struct GuardChannel {
    marker_writer: File,
    _marker_reader: File,
    _keepalive: File,
    pub(super) root: PathBuf,
    pub(super) pipe: PathBuf,
    stop: Arc<AtomicBool>,
    collector: Option<JoinHandle<Result<Vec<String>>>>,
    hits: Vec<String>,
}

impl GuardChannel {
    pub(super) fn new(cache_parent: &Path) -> Result<Self> {
        let root = cache_parent.join(format!(".guard-channel-{}", Uuid::new_v4()));
        std::fs::DirBuilder::new().mode(0o700).create(&root)?;
        let pipe = root.join("events");
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::OpenOptionsExt;
        let name = std::ffi::CString::new(pipe.as_os_str().as_bytes())?;
        anyhow::ensure!(
            unsafe { libc::mkfifo(name.as_ptr(), 0o600) } == 0,
            "create guard FIFO: {}",
            std::io::Error::last_os_error()
        );
        let mut reader = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&pipe)?;
        let keepalive = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&pipe)?;
        // The child mount is RO. Write-only inode permissions additionally
        // prevent a child from consuming unread evidence as a competing reader.
        std::fs::set_permissions(&pipe, std::fs::Permissions::from_mode(0o200))?;
        let mut fds = [-1; 2];
        anyhow::ensure!(
            unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } == 0,
            "create guard evidence pipe: {}",
            std::io::Error::last_os_error()
        );
        let marker_reader = unsafe { File::from_raw_fd(fds[0]) };
        let marker_writer = unsafe { File::from_raw_fd(fds[1]) };
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::clone(&stop);
        let collector = std::thread::spawn(move || -> Result<Vec<String>> {
            let mut bytes = Vec::new();
            let mut overflow = false;
            let mut eof = false;
            let mut buffer = [0; 4096];
            loop {
                let finishing = done.load(Ordering::Acquire);
                // Bound each drain, even if an escaped descendant holds the
                // writer forever or floods it. Such data cannot erase hits.
                for _ in 0..64 {
                    match reader.read(&mut buffer) {
                        Ok(0) => {
                            eof = true;
                            break;
                        }
                        Ok(n) => {
                            if bytes.len() + n <= 65536 {
                                bytes.extend_from_slice(&buffer[..n]);
                            } else {
                                overflow = true;
                            }
                        }
                        Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                        Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                        Err(e) => return Err(e).context("collect guard evidence"),
                    }
                }
                if finishing || eof {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            let mut hits: Vec<_> = String::from_utf8_lossy(&bytes)
                .lines()
                .map(str::to_owned)
                .collect();
            if eof {
                hits.push("guard_channel_eof".into());
            }
            if overflow {
                hits.push("guard_channel_overflow".into());
            }
            Ok(hits)
        });
        Ok(Self {
            marker_writer,
            _marker_reader: marker_reader,
            _keepalive: keepalive,
            root,
            pipe,
            stop,
            collector: Some(collector),
            hits: Vec::new(),
        })
    }

    pub(super) fn fd(&self) -> RawFd {
        self.marker_writer.as_raw_fd()
    }

    pub(super) fn collect(&mut self) -> Result<Vec<String>> {
        self.stop.store(true, Ordering::Release);
        if let Some(collector) = self.collector.take() {
            self.hits = collector
                .join()
                .map_err(|_| anyhow::anyhow!("guard evidence collector panicked"))??;
        }
        Ok(self.hits.clone())
    }
}
impl Drop for GuardChannel {
    fn drop(&mut self) {
        let _ = self.collect();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests {
    use super::*;
    #[test]
    fn unexpected_fifo_eof_is_recorded_and_collection_never_waits_for_descendants() {
        let cache = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let mut channel = GuardChannel::new(cache.path()).unwrap();
        channel._keepalive = File::open("/dev/null").unwrap();
        // A child forked by a concurrent test thread holds a copy of the
        // writer until its exec, so EOF can lag the drop. The collector stops
        // on its own once it sees EOF; wait for that (bounded) instead of
        // racing it with collect(), which never waits (BUG-2160).
        // trace:BUG-2160 | ai:claude
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !channel.collector.as_ref().unwrap().is_finished() {
            assert!(
                std::time::Instant::now() < deadline,
                "collector never observed the writer's EOF"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(channel.collect().unwrap(), ["guard_channel_eof"]);
        let mut channel = GuardChannel::new(cache.path()).unwrap();
        use std::io::Write;
        channel._keepalive.write_all(b"prior-hit\n").unwrap();
        // An escaped descendant inherits the FIFO writer and outlives the run.
        // It becomes the only writer, so the FIFO cannot reach EOF until it
        // exits. Collection must return while it is still alive: a structural
        // check, not a wall-clock bound that load can break (BUG-2160).
        // trace:BUG-2160 | ai:claude
        let writer = channel._keepalive.try_clone().unwrap();
        let mut descendant = std::process::Command::new("sleep")
            .arg("120")
            .stdin(std::process::Stdio::null())
            .stdout(writer)
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        channel._keepalive = File::open("/dev/null").unwrap();
        let hits = channel.collect();
        let descendant_alive = descendant.try_wait().unwrap().is_none();
        let _ = descendant.kill();
        let _ = descendant.wait();
        assert!(
            descendant_alive,
            "collection waited for the descendant holding the writer to exit"
        );
        // No EOF: the descendant still held the writer when collection ended.
        assert_eq!(hits.unwrap(), ["prior-hit"]);
    }
}
