//! Descriptor-relative durable publication in an already initialized directory.
//!
//! The publication owner supplies an exclusively guarded, validated directory.
//! This layer neither discovers roots nor creates infrastructure. It cannot
//! decide authorization, recovery, or whether a visible decision may roll back.
// trace:BUG-1808 | ai:codex

#![cfg(target_os = "linux")]

use std::ffi::{CStr, CString};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

/// The named target's status, not the status of disposable staging bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Visibility {
    Unchanged,
    /// An installation syscall was attempted but its outcome is not confirmed.
    Uncertain,
    /// Rename succeeded; directory durability was not confirmed.
    Visible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WriteStage {
    ValidateName,
    OpenStaging,
    WritePayload,
    SetMode,
    SyncPayload,
    Rename,
    Remove,
    SyncDirectory,
}

#[derive(Debug, thiserror::Error)]
#[error("publication write failed at {stage:?} ({visibility:?}): {source}")]
pub(super) struct DurableWriteError {
    pub stage: WriteStage,
    pub visibility: Visibility,
    #[source]
    pub source: io::Error,
    /// Failed cleanup is retained, never hidden by the original write error.
    pub cleanup_error: Option<io::Error>,
}

type Result<T> = std::result::Result<T, DurableWriteError>;

#[derive(Debug, Clone, Copy)]
enum Install {
    Replace,
    Immutable,
}

/// Open directory FD anchors every operation against pathname replacement.
/// The owner still validates this identity against the journal/root binding.
#[derive(Debug)]
pub(super) struct DurableDirectory {
    file: File,
    device: u64,
    inode: u64,
}

impl DurableDirectory {
    pub fn open_existing(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let metadata = file.metadata()?;
        Ok(Self {
            file,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    pub fn identity(&self) -> (u64, u64) {
        (self.device, self.inode)
    }

    /// Each traversed component is opened relative to the retained parent FD.
    /// No symlink, implicit mkdir or pathname rediscovery is allowed here.
    pub fn open_child(&self, name: &str) -> io::Result<Self> {
        let name = component(name)?;
        let file = self.open_at(&name, libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
        let metadata = file.metadata()?;
        Ok(Self {
            file,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    pub fn duplicate(&self) -> io::Result<Self> {
        Ok(Self {
            file: self.file.try_clone()?,
            device: self.device,
            inode: self.inode,
        })
    }

    /// Finite regular-file installer used only under the owner's resource
    /// locks and already durable journal. Evidence always uses `create`.
    pub fn replace_file(&self, name: &str, bytes: &[u8], mode: u32) -> Result<()> {
        self.write_mode_with(name, bytes, mode, Install::Replace, &mut NoFault)
    }

    pub fn remove_file(&self, name: &str) -> Result<()> {
        self.remove_with(name, &mut NoFault)
    }

    /// Recovery must confirm durability even when bytes already equal the
    /// desired image: a previous rename may have failed at directory fsync.
    pub fn sync_entry(&self, name: &str) -> Result<()> {
        let mut stage = WriteStage::ValidateName;
        let operation = (|| -> io::Result<()> {
            let name = component(name)?;
            stage = WriteStage::SyncPayload;
            match self.open_at(&name, libc::O_RDONLY | libc::O_NONBLOCK, 0) {
                Ok(file) => {
                    let metadata = file.metadata()?;
                    if !metadata.is_file() || metadata.nlink() != 1 {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "unsupported publication file image",
                        ));
                    }
                    file.sync_all()?;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            stage = WriteStage::SyncDirectory;
            self.file.sync_all()
        })();
        operation.map_err(|source| DurableWriteError {
            stage,
            visibility: Visibility::Visible,
            source,
            cleanup_error: None,
        })
    }

    fn remove_with(&self, name: &str, faults: &mut impl FaultBoundary) -> Result<()> {
        let mut stage = WriteStage::ValidateName;
        let mut visibility = Visibility::Unchanged;
        let operation = (|| -> io::Result<()> {
            let name = component(name)?;
            stage = WriteStage::Remove;
            faults.before(stage)?;
            visibility = Visibility::Uncertain;
            // SAFETY: held directory and validated component. The caller has
            // verified the exact current file under the resource locks.
            if unsafe { libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), 0) } != 0 {
                return Err(io::Error::last_os_error());
            }
            visibility = Visibility::Visible;
            stage = WriteStage::SyncDirectory;
            faults.before(stage)?;
            self.file.sync_all()
        })();
        operation.map_err(|source| DurableWriteError {
            stage,
            visibility,
            source,
            cleanup_error: None,
        })
    }

    /// Capture bytes and exact permission bits from the same opened object.
    /// Hard links and privilege bits need a different finite-effect adapter;
    /// replacing one name must not silently change their semantics.
    pub fn read_regular(&self, name: &str, limit: usize) -> io::Result<Option<(Vec<u8>, u32)>> {
        let name = component(name)?;
        let mut file = match self.open_at(&name, libc::O_RDONLY | libc::O_NONBLOCK, 0) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let before = file.metadata()?;
        if !before.is_file()
            || before.nlink() != 1
            || before.mode() & 0o7000 != 0
            || before.len() > limit as u64
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported publication file image",
            ));
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take((limit as u64).saturating_add(1))
            .read_to_end(&mut bytes)?;
        let after = file.metadata()?;
        if bytes.len() > limit
            || bytes.len() as u64 != before.len()
            || before.len() != after.len()
            || before.mode() != after.mode()
            || before.nlink() != after.nlink()
            || (before.ctime(), before.ctime_nsec()) != (after.ctime(), after.ctime_nsec())
            || (before.mtime(), before.mtime_nsec()) != (after.mtime(), after.mtime_nsec())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "publication file changed during capture",
            ));
        }
        Ok(Some((bytes, before.mode() & 0o777)))
    }

    /// Coordinator replacement only. Immutable evidence uses `create`.
    pub fn replace_coordinator(&self, bytes: &[u8]) -> Result<()> {
        self.write_with("coordinator.json", bytes, Install::Replace, &mut NoFault)
    }

    /// No-replace rename prevents overwriting COMMIT or an observation, even
    /// with identical bytes. Idempotent recovery must first validate evidence.
    pub fn create(&self, name: &str, bytes: &[u8]) -> Result<()> {
        self.write_with(name, bytes, Install::Immutable, &mut NoFault)
    }

    /// Missing is distinct from every other I/O error. Symlinks and nonregular
    /// objects are errors, never evidence of absence. Reads have a finite cap.
    pub fn read(&self, name: &str, limit: usize) -> io::Result<Option<Vec<u8>>> {
        let name = component(name)?;
        let file = match self.open_at(&name, libc::O_RDONLY | libc::O_NONBLOCK, 0) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > limit as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "publication evidence is not a bounded regular file",
            ));
        }
        let mut bytes = Vec::new();
        file.take((limit as u64).saturating_add(1))
            .read_to_end(&mut bytes)?;
        if bytes.len() > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "publication evidence exceeded its read budget",
            ));
        }
        Ok(Some(bytes))
    }

    fn open_at(&self, name: &CStr, flags: i32, mode: libc::mode_t) -> io::Result<File> {
        // SAFETY: live directory FD and NUL-terminated single component; the
        // successful new FD is immediately transferred to exactly one File.
        let fd = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                mode,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    fn unlink_owned_staging(&self, name: &CStr) -> io::Result<()> {
        // SAFETY: single-component name allocated by this write; no recursive
        // deletion, target deletion or permanent sidecar removal is possible.
        let result = unsafe { libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), 0) };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        self.file.sync_all()
    }

    fn write_with(
        &self,
        name: &str,
        bytes: &[u8],
        install: Install,
        faults: &mut impl FaultBoundary,
    ) -> Result<()> {
        self.write_mode_with(name, bytes, 0o600, install, faults)
    }

    fn write_mode_with(
        &self,
        name: &str,
        bytes: &[u8],
        mode: u32,
        install: Install,
        faults: &mut impl FaultBoundary,
    ) -> Result<()> {
        let mut stage = WriteStage::ValidateName;
        let mut visibility = Visibility::Unchanged;
        let mut staging_created = false;
        let mut renamed = false;
        let staging = CString::new(format!(".publication-{}.tmp", uuid::Uuid::new_v4()))
            .expect("UUID staging name contains no NUL");
        let operation = (|| -> io::Result<()> {
            let destination = component(name)?;
            // Owner-read is required so the installed image stays verifiable
            // by read_regular/sync_entry; refused before staging exists.
            // trace:BUG-1808 | ai:claude
            if mode & !0o777 != 0 || mode & 0o400 == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "unsupported publication file mode",
                ));
            }
            stage = WriteStage::OpenStaging;
            faults.before(stage)?;
            let mut file = self.open_at(
                &staging,
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )?;
            staging_created = true;
            stage = WriteStage::WritePayload;
            faults.before(stage)?;
            file.write_all(bytes)?;
            stage = WriteStage::SetMode;
            faults.before(stage)?;
            file.set_permissions(std::fs::Permissions::from_mode(mode))?;
            stage = WriteStage::SyncPayload;
            faults.before(stage)?;
            file.sync_all()?;
            stage = WriteStage::Rename;
            faults.before(stage)?;
            // A syscall failure is conservatively uncertain except the
            // no-replace collision, which guarantees the old target remains.
            visibility = Visibility::Uncertain;
            let flags = match install {
                Install::Replace => 0,
                Install::Immutable => libc::RENAME_NOREPLACE,
            };
            // SAFETY: both names are live CStrings, both dirfds are held,
            // flags select only replace or atomic no-replace semantics.
            let rc = unsafe {
                libc::renameat2(
                    self.file.as_raw_fd(),
                    staging.as_ptr(),
                    self.file.as_raw_fd(),
                    destination.as_ptr(),
                    flags,
                )
            };
            if rc != 0 {
                let error = io::Error::last_os_error();
                if matches!(install, Install::Immutable)
                    && error.kind() == io::ErrorKind::AlreadyExists
                {
                    visibility = Visibility::Unchanged;
                }
                return Err(error);
            }
            renamed = true;
            visibility = Visibility::Visible;
            stage = WriteStage::SyncDirectory;
            faults.before(stage)?;
            self.file.sync_all()?;
            Ok(())
        })();
        operation.map_err(|source| {
            let cleanup_error = if staging_created && !renamed {
                self.unlink_owned_staging(&staging).err()
            } else {
                None
            };
            DurableWriteError {
                stage,
                visibility,
                source,
                cleanup_error,
            }
        })
    }
}

fn component(name: &str) -> io::Result<CString> {
    if name.is_empty() || matches!(name, "." | "..") || name.contains('/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "publication filename must be one nonempty component",
        ));
    }
    CString::new(name).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "publication filename contains NUL",
        )
    })
}

// Only tests can select a fault; production always executes the actual syscalls.
trait FaultBoundary {
    fn before(&mut self, stage: WriteStage) -> io::Result<()>;
}

struct NoFault;
impl FaultBoundary for NoFault {
    fn before(&mut self, _: WriteStage) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    struct FailAt(WriteStage);
    impl FaultBoundary for FailAt {
        fn before(&mut self, stage: WriteStage) -> io::Result<()> {
            if stage == self.0 {
                Err(io::Error::from_raw_os_error(libc::EIO))
            } else {
                Ok(())
            }
        }
    }

    // trace:BUG-1808 | ai:codex
    #[test]
    fn durable_create_replace_and_collision_preserve_immutable_evidence() {
        let root = tempfile::tempdir().unwrap();
        let dir = DurableDirectory::open_existing(root.path()).unwrap();
        dir.create("COMMIT.json", b"decision").unwrap();
        let collision = dir.create("COMMIT.json", b"different").unwrap_err();
        assert_eq!(collision.visibility, Visibility::Unchanged);
        assert_eq!(collision.stage, WriteStage::Rename);
        assert!(collision.cleanup_error.is_none());
        assert_eq!(dir.read("COMMIT.json", 100).unwrap().unwrap(), b"decision");
        dir.replace_coordinator(b"active").unwrap();
        dir.replace_coordinator(b"settled").unwrap();
        assert_eq!(
            dir.read("coordinator.json", 100).unwrap().unwrap(),
            b"settled"
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
        assert_eq!(
            std::fs::metadata(root.path().join("COMMIT.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    // Real filesystem operations surround each deterministic fault, paired
    // with the complete success above. This is a primitive witness only.
    // trace:BUG-1808 | ai:codex
    #[test]
    fn every_write_boundary_retains_visibility_and_peer_bytes() {
        for stage in [
            WriteStage::OpenStaging,
            WriteStage::WritePayload,
            WriteStage::SetMode,
            WriteStage::SyncPayload,
            WriteStage::Rename,
            WriteStage::SyncDirectory,
        ] {
            let root = tempfile::tempdir().unwrap();
            let dir = DurableDirectory::open_existing(root.path()).unwrap();
            dir.create("peer", b"peer history").unwrap();
            dir.create("authority-publication.lock", b"").unwrap();
            let lock_inode = std::fs::metadata(root.path().join("authority-publication.lock"))
                .unwrap()
                .ino();
            let error = dir
                .write_with(
                    "COMMIT.json",
                    b"exact C",
                    Install::Immutable,
                    &mut FailAt(stage),
                )
                .unwrap_err();
            assert_eq!(error.stage, stage);
            assert!(error.cleanup_error.is_none());
            let expected = if stage == WriteStage::SyncDirectory {
                assert_eq!(error.visibility, Visibility::Visible);
                Some(b"exact C".to_vec())
            } else {
                assert_eq!(error.visibility, Visibility::Unchanged);
                None
            };
            assert_eq!(dir.read("COMMIT.json", 100).unwrap(), expected);
            assert_eq!(dir.read("peer", 100).unwrap().unwrap(), b"peer history");
            assert_eq!(
                std::fs::metadata(root.path().join("authority-publication.lock"))
                    .unwrap()
                    .ino(),
                lock_inode
            );
            assert!(!std::fs::read_dir(root.path()).unwrap().any(|entry| {
                entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            }));
        }
    }

    // trace:BUG-1808 | ai:codex
    #[test]
    fn directory_fd_does_not_follow_replaced_path() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("original");
        let moved = root.path().join("moved");
        let peer = root.path().join("peer");
        std::fs::create_dir(&original).unwrap();
        std::fs::create_dir(&peer).unwrap();
        let dir = DurableDirectory::open_existing(&original).unwrap();
        let identity = dir.identity();
        std::fs::rename(&original, &moved).unwrap();
        symlink(&peer, &original).unwrap();
        dir.create("COMMIT.json", b"anchored").unwrap();
        assert_eq!(
            std::fs::read(moved.join("COMMIT.json")).unwrap(),
            b"anchored"
        );
        assert!(!peer.join("COMMIT.json").exists());
        assert_eq!(std::fs::metadata(&moved).unwrap().ino(), identity.1);
        assert!(DurableDirectory::open_existing(&original).is_err());
    }

    // trace:BUG-1808 | ai:claude
    #[test]
    fn unreadable_or_privileged_install_modes_never_stage() {
        let root = tempfile::tempdir().unwrap();
        let dir = DurableDirectory::open_existing(root.path()).unwrap();
        for mode in [0o000, 0o200, 0o070, 0o4600, 0o1600] {
            let error = dir.replace_file("target", b"bytes", mode).unwrap_err();
            assert_eq!(error.stage, WriteStage::ValidateName, "{mode:o}");
            assert_eq!(error.visibility, Visibility::Unchanged);
            assert_eq!(error.source.kind(), io::ErrorKind::InvalidInput);
        }
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
        dir.replace_file("target", b"bytes", 0o400).unwrap();
        assert_eq!(
            dir.read_regular("target", 5).unwrap(),
            Some((b"bytes".to_vec(), 0o400))
        );
    }

    // trace:BUG-1808 | ai:codex
    #[test]
    fn invalid_names_and_missing_infrastructure_have_no_effects() {
        let root = tempfile::tempdir().unwrap();
        assert!(DurableDirectory::open_existing(&root.path().join("missing")).is_err());
        let dir = DurableDirectory::open_existing(root.path()).unwrap();
        for name in ["", ".", "..", "../escape", "/absolute", "a/b", "nul\0name"] {
            let error = dir.create(name, b"not written").unwrap_err();
            assert_eq!(error.stage, WriteStage::ValidateName);
            assert_eq!(error.visibility, Visibility::Unchanged);
        }
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    // trace:BUG-1808 | ai:codex
    #[test]
    fn missing_corrupt_kind_and_oversized_evidence_are_distinct() {
        let root = tempfile::tempdir().unwrap();
        let dir = DurableDirectory::open_existing(root.path()).unwrap();
        assert!(dir.read("absent", 4).unwrap().is_none());
        dir.create("payload", b"12345").unwrap();
        assert!(dir.read("payload", 4).is_err());
        assert_eq!(dir.read("payload", 5).unwrap().unwrap(), b"12345");
        symlink("payload", root.path().join("link")).unwrap();
        assert!(dir.read("link", 10).is_err());
        std::fs::create_dir(root.path().join("directory")).unwrap();
        assert!(dir.read("directory", 10).is_err());
        let fifo = CString::new(root.path().join("fifo").as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(dir.read("fifo", 10).is_err());
    }

    // trace:BUG-1808 | ai:codex
    #[test]
    fn replacement_and_removal_faults_preserve_visible_outcome() {
        for stage in [
            WriteStage::OpenStaging,
            WriteStage::WritePayload,
            WriteStage::SetMode,
            WriteStage::SyncPayload,
            WriteStage::Rename,
            WriteStage::SyncDirectory,
        ] {
            let root = tempfile::tempdir().unwrap();
            let dir = DurableDirectory::open_existing(root.path()).unwrap();
            dir.replace_file("effect", b"before", 0o640).unwrap();
            let error = dir
                .write_mode_with(
                    "effect",
                    b"after",
                    0o700,
                    Install::Replace,
                    &mut FailAt(stage),
                )
                .unwrap_err();
            assert_eq!(error.stage, stage);
            let expected = if stage == WriteStage::SyncDirectory {
                assert_eq!(error.visibility, Visibility::Visible);
                (b"after".to_vec(), 0o700)
            } else {
                assert_eq!(error.visibility, Visibility::Unchanged);
                (b"before".to_vec(), 0o640)
            };
            assert_eq!(dir.read_regular("effect", 20).unwrap(), Some(expected));
            dir.sync_entry("effect").unwrap();
            dir.replace_file("effect", b"after", 0o700).unwrap();
            assert_eq!(
                dir.read_regular("effect", 20).unwrap(),
                Some((b"after".to_vec(), 0o700))
            );
        }
        for stage in [WriteStage::Remove, WriteStage::SyncDirectory] {
            let root = tempfile::tempdir().unwrap();
            let dir = DurableDirectory::open_existing(root.path()).unwrap();
            dir.create("effect", b"owned").unwrap();
            let error = dir.remove_with("effect", &mut FailAt(stage)).unwrap_err();
            assert_eq!(error.stage, stage);
            if stage == WriteStage::Remove {
                assert_eq!(error.visibility, Visibility::Unchanged);
                assert_eq!(dir.read("effect", 10).unwrap().unwrap(), b"owned");
                dir.remove_file("effect").unwrap();
            } else {
                assert_eq!(error.visibility, Visibility::Visible);
            }
            assert!(dir.read("effect", 10).unwrap().is_none());
            dir.sync_entry("effect").unwrap();
        }
    }
}
