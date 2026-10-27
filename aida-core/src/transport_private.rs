//! Anchored private transport files. No caller-controlled diagnostic text.
// trace:STORY-1742 | ai:codex
use anyhow::{bail, Result};
use fs2::FileExt;
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// How long a WORK write or transport operation waits for another holder of
/// the transport lock before refusing, so concurrent writers serialize like
/// ordinary store writes instead of failing at once.
// trace:STORY-1742 | ai:claude
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

thread_local! {
    static LOCK_WAIT_BOUND: std::cell::Cell<Option<std::time::Duration>> =
        const { std::cell::Cell::new(None) };
}

/// Run `f` with transport-lock waits on this thread bounded to `bound` (never
/// longer than the default). A server holding its in-memory store lock uses
/// this so a contended write refuses quickly instead of stalling readers.
// trace:STORY-1742 | ai:claude
pub fn with_lock_wait<T>(bound: std::time::Duration, f: impl FnOnce() -> T) -> T {
    let previous = LOCK_WAIT_BOUND.with(|cell| cell.replace(Some(bound)));
    struct Restore(Option<std::time::Duration>);
    impl Drop for Restore {
        fn drop(&mut self) {
            LOCK_WAIT_BOUND.with(|cell| cell.set(self.0));
        }
    }
    let _restore = Restore(previous);
    f()
}

/// Names that predate owner-only private storage (see `Private::file`).
const LEGACY_NAMES: &[&str] = &["aida-transport.json", "aida-transport.lock"];

fn unsafe_hint(name: &str) -> String {
    format!("Transport private file {name} in the Git common directory is unsafe: it must be a regular file you own, with one link and owner-only permissions. If it is yours, restore it with `chmod 600`; otherwise move it aside and re-enroll. Nothing was changed.")
}

pub struct Private {
    dir: File,
    path: PathBuf,
}

impl Private {
    pub fn open(path: &Path) -> Result<Self> {
        let dir = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|_| anyhow::anyhow!("Transport private directory unavailable"))?;
        let meta = dir.metadata()?;
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o022 != 0 {
            bail!("Transport private directory (the clone's Git common directory) must be owned by you and not group- or other-writable; correct its ownership or permissions (for example `chmod go-w`) before retrying.");
        }
        Ok(Self {
            dir,
            path: path.to_owned(),
        })
    }

    pub fn revalidate(&self) -> Result<()> {
        let fresh = Self::open(&self.path)?;
        let a = self.dir.metadata()?;
        let b = fresh.dir.metadata()?;
        if a.dev() != b.dev() || a.ino() != b.ino() {
            bail!("Transport private directory changed; refused");
        }
        Ok(())
    }

    fn file(&self, name: &str, create: bool) -> Result<Option<File>> {
        let name = CString::new(name)?;
        let flags = libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if create {
                libc::O_RDWR | libc::O_CREAT
            } else {
                libc::O_RDONLY
            };
        let fd = unsafe { libc::openat(self.dir.as_raw_fd(), name.as_ptr(), flags, 0o600) };
        let display = name.to_string_lossy().into_owned();
        if fd < 0 {
            if !create && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
            bail!("{}", unsafe_hint(&display));
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let m = file.metadata()?;
        // Profiles and locks created before private storage existed were
        // written with the umask (0644). Tolerate exactly that legacy shape
        // for those two names: still a singly linked regular file you own,
        // with no group/other write or special bits. Policy and receipt
        // stay owner-only; link, type and owner checks are never relaxed.
        // trace:STORY-1742 | ai:claude
        let mode = m.mode() & 0o7777;
        let legacy = LEGACY_NAMES.contains(&display.as_str()) && mode & 0o7022 == 0;
        if !m.is_file()
            || m.nlink() != 1
            || m.uid() != unsafe { libc::geteuid() }
            || (mode != 0o600 && !legacy)
        {
            bail!("{}", unsafe_hint(&display));
        }
        Ok(Some(file))
    }

    pub fn read(&self, name: &str) -> Result<Option<Vec<u8>>> {
        self.revalidate()?;
        let Some(file) = self.file(name, false)? else {
            return Ok(None);
        };
        if file.metadata()?.len() > 65536 {
            bail!("Transport private file exceeds size limit");
        }
        let mut bytes = Vec::new();
        file.take(65537).read_to_end(&mut bytes)?;
        if bytes.len() > 65536 {
            bail!("Transport private file exceeds size limit");
        }
        Ok(Some(bytes))
    }

    pub fn lock(&self) -> Result<File> {
        self.revalidate()?;
        let file = self.file("aida-transport.lock", true)?.unwrap();
        let wait = LOCK_WAIT_BOUND
            .with(|cell| cell.get())
            .map_or(LOCK_WAIT, |bound| bound.min(LOCK_WAIT));
        let deadline = std::time::Instant::now() + wait;
        // Only contention is retried; any other lock error fails at once.
        // trace:STORY-1742 | ai:claude
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => break,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => bail!("Transport lock unavailable. Nothing was changed."),
            }
            if std::time::Instant::now() >= deadline {
                bail!(
                    "Transport configuration is busy: another AIDA write or transport operation \
                     on this clone held its lock for {}s. Nothing was changed; retry once it \
                     finishes.",
                    wait.as_secs()
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // Migrate a legacy lock in place: same inode, never unlinked.
        if file.metadata()?.mode() & 0o777 != 0o600
            && unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } != 0
        {
            bail!("{}", unsafe_hint("aida-transport.lock"));
        }
        // Ensure the path still names the locked inode. Never unlink it.
        let named = self.file("aida-transport.lock", false)?.unwrap();
        if named.metadata()?.ino() != file.metadata()?.ino() {
            bail!("Transport lock identity changed");
        }
        Ok(file)
    }

    pub fn remove(&self, name: &str) -> Result<()> {
        if self.read(name)?.is_some() {
            let name = CString::new(name)?;
            if unsafe { libc::unlinkat(self.dir.as_raw_fd(), name.as_ptr(), 0) } != 0 {
                bail!("Transport private removal failed");
            }
            self.dir.sync_all()?;
        }
        Ok(())
    }

    pub fn write(&self, name: &str, bytes: &[u8]) -> Result<()> {
        if bytes.len() > 65536 {
            bail!("Transport private file exceeds size limit");
        }
        self.read(name)?;
        let temp = CString::new(format!(".aida-transport-{}", uuid::Uuid::new_v4()))?;
        let target = CString::new(name)?;
        let fd = unsafe {
            libc::openat(
                self.dir.as_raw_fd(),
                temp.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            bail!("Transport private temporary file unavailable");
        }
        let result = (|| -> Result<()> {
            let mut file = unsafe { File::from_raw_fd(fd) };
            file.write_all(bytes)?;
            file.sync_all()?;
            self.revalidate()?;
            self.read(name)?;
            if unsafe {
                libc::renameat(
                    self.dir.as_raw_fd(),
                    temp.as_ptr(),
                    self.dir.as_raw_fd(),
                    target.as_ptr(),
                )
            } != 0
            {
                bail!("Transport private file replacement failed");
            }
            self.dir.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            unsafe {
                libc::unlinkat(self.dir.as_raw_fd(), temp.as_ptr(), 0);
            }
        }
        result.map_err(|_| anyhow::anyhow!("Transport private persistence failed"))
    }
}
