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

pub(super) struct Private {
    dir: File,
    path: PathBuf,
}

impl Private {
    pub(super) fn open(path: &Path) -> Result<Self> {
        let dir = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|_| anyhow::anyhow!("Transport private directory unavailable"))?;
        let meta = dir.metadata()?;
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o022 != 0 {
            bail!("Transport private directory has unsafe ownership or permissions");
        }
        Ok(Self {
            dir,
            path: path.to_owned(),
        })
    }

    pub(super) fn revalidate(&self) -> Result<()> {
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
        if fd < 0 {
            if !create && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
            bail!("Transport private file unavailable or unsafe");
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let m = file.metadata()?;
        if !m.is_file()
            || m.nlink() != 1
            || m.uid() != unsafe { libc::geteuid() }
            || m.mode() & 0o777 != 0o600
        {
            bail!("Transport private file requires singly linked owner-only regular storage");
        }
        Ok(Some(file))
    }

    pub(super) fn read(&self, name: &str) -> Result<Option<Vec<u8>>> {
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

    pub(super) fn lock(&self) -> Result<File> {
        self.revalidate()?;
        let file = self.file("aida-transport.lock", true)?.unwrap();
        file.try_lock_exclusive()
            .map_err(|_| anyhow::anyhow!("Transport configuration is busy"))?;
        // Ensure the path still names the locked inode. Never unlink it.
        let named = self.file("aida-transport.lock", false)?.unwrap();
        if named.metadata()?.ino() != file.metadata()?.ino() {
            bail!("Transport lock identity changed");
        }
        Ok(file)
    }

    pub(super) fn remove(&self, name: &str) -> Result<()> {
        if self.read(name)?.is_some() {
            let name = CString::new(name)?;
            if unsafe { libc::unlinkat(self.dir.as_raw_fd(), name.as_ptr(), 0) } != 0 {
                bail!("Transport private removal failed");
            }
            self.dir.sync_all()?;
        }
        Ok(())
    }

    pub(super) fn write(&self, name: &str, bytes: &[u8]) -> Result<()> {
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
