//! Independent bind roots and no-follow quarantine snapshots.
// trace:TASK-1730 | ai:codex
use super::*;
use std::ffi::{CString, OsStr};
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{symlink, MetadataExt, OpenOptionsExt};

/// A mounted ancestor exposes replacement directory entries to an old install
/// namespace, even when that ancestor is read-only. /tmp is covered by the
/// helper's private tmpfs; essential host roots are not safe quarantine roots.
pub(super) fn validate_mounts(roots: &[PathBuf]) -> Result<()> {
    let roots = roots
        .iter()
        .map(|p| p.canonicalize())
        .collect::<std::io::Result<Vec<_>>>()?;
    for (i, a) in roots.iter().enumerate() {
        anyhow::ensure!(
            a != Path::new("/"),
            "cannot bind filesystem root for scoring"
        );
        for b in &roots[i + 1..] {
            anyhow::ensure!(
                !a.starts_with(b) && !b.starts_with(a),
                "scoring writable roots must not overlap: {} and {}",
                a.display(),
                b.display()
            );
        }
    }
    Ok(())
}

/// An open descriptor pins every input inode. Traversal never resolves a
/// caller-controlled path below the initial directory. Reopening /proc/self/fd
/// accesses that pinned inode rather than following a raced directory entry.
fn open_child(parent: RawFd, name: &OsStr) -> Result<File> {
    let name = CString::new(name.as_bytes())?;
    let fd = unsafe {
        libc::openat(
            parent,
            name.as_ptr(),
            libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error())
            .context("open snapshot entry without following links");
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn descriptor_path(file: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

fn copy_directory(source: &File, destination: &Path, skip_git: bool) -> Result<()> {
    for child in std::fs::read_dir(descriptor_path(source))? {
        let child = child?;
        let name = child.file_name();
        if skip_git && name == ".git" {
            continue;
        }
        let source = open_child(source.as_raw_fd(), &name)?;
        let metadata = source.metadata()?;
        let target = destination.join(name);
        if metadata.is_dir() {
            std::fs::DirBuilder::new().mode(0o700).create(&target)?;
            copy_directory(&source, &target, false)?;
            // Ignore set-id/sticky bits supplied by the untrusted installer.
            std::fs::set_permissions(
                &target,
                std::fs::Permissions::from_mode(metadata.mode() & 0o777),
            )?;
        } else if metadata.file_type().is_symlink() {
            // Linux readlinkat with an empty pathname reads the O_PATH link
            // itself; neither a replaced parent nor the link target is followed.
            let mut bytes = vec![0u8; 65536];
            let n = unsafe {
                libc::readlinkat(
                    source.as_raw_fd(),
                    c"".as_ptr(),
                    bytes.as_mut_ptr().cast(),
                    bytes.len(),
                )
            };
            anyhow::ensure!(
                n >= 0,
                "cannot read pinned snapshot link: {}",
                std::io::Error::last_os_error()
            );
            anyhow::ensure!((n as usize) < bytes.len(), "snapshot link target too long");
            symlink(OsStr::from_bytes(&bytes[..n as usize]), target)?;
        } else {
            anyhow::ensure!(
                metadata.is_file(),
                "snapshot refuses special file: {}",
                target.display()
            );
            let input = File::open(descriptor_path(&source))?;
            let mut output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&target)?;
            // A surviving writer cannot make an append-only file stream forever.
            use std::io::Read;
            let copied = std::io::copy(&mut input.take(metadata.len()), &mut output)?;
            anyhow::ensure!(
                copied == metadata.len(),
                "snapshot file changed size while copying"
            );
            output.set_permissions(std::fs::Permissions::from_mode(metadata.mode() & 0o777))?;
        }
    }
    Ok(())
}

pub(super) fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    let source = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(source)?;
    copy_directory(&source, destination, false)
}

// trace:TASK-1731 | ai:codex
pub(super) fn copy_trial_tree(source: &Path, destination: &Path) -> Result<()> {
    let source = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(source)?;
    copy_directory(&source, destination, true)
}

/// The original bind mount pins the renamed inode. The replacement at the same
/// host path preserves absolute paths baked into an installed executable.
pub(super) struct Quarantine {
    paths: Vec<(PathBuf, PathBuf)>,
    complete: bool,
}

impl Quarantine {
    pub(super) fn snapshot(roots: &[PathBuf]) -> Result<Self> {
        validate_mounts(roots)?;
        for path in roots {
            anyhow::ensure!(
                path.starts_with("/tmp")
                    && path != Path::new("/tmp")
                    && path.canonicalize()? == *path,
                "quarantine roots must be under private /tmp: {}",
                path.display()
            );
        }
        let mut quarantine = Self {
            paths: Vec::new(),
            complete: false,
        };
        for path in roots {
            let old = path.with_file_name(format!(
                "{}.quarantine-{}",
                path.file_name()
                    .context("snapshot root has no name")?
                    .to_string_lossy(),
                Uuid::new_v4()
            ));
            // Open before rename so a replaced root cannot redirect traversal.
            let source = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(path)?;
            std::fs::rename(path, &old)?;
            quarantine.paths.push((path.clone(), old));
            std::fs::DirBuilder::new().mode(0o700).create(path)?;
            copy_directory(&source, path, false)?;
        }
        quarantine.complete = true;
        Ok(quarantine)
    }
}
impl Drop for Quarantine {
    fn drop(&mut self) {
        for (path, old) in self.paths.iter().rev() {
            if self.complete {
                let _ = std::fs::remove_dir_all(old);
            } else {
                // A failed copy must not destroy the caller's original data.
                let _ = std::fs::remove_dir_all(path);
                let _ = std::fs::rename(old, path);
            }
        }
    }
}

/// Advisory only: namespace/process races prevent this from being a universal
/// no-survivors guarantee. Open pidfds keep best-effort signals bound to the
/// observed process identity instead of a subsequently reused numeric PID.
pub(super) fn survivors(roots: &[PathBuf], guard_fd: Option<RawFd>, kill: bool) -> Result<usize> {
    let guard = guard_fd.and_then(|fd| std::fs::read_link(format!("/proc/self/fd/{fd}")).ok());
    let mut count = 0;
    for entry in std::fs::read_dir("/proc")? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<i32>().ok())
        else {
            continue;
        };
        if pid == std::process::id() as i32 {
            continue;
        }
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
        // A process may vanish, be inaccessible, or predate pidfd support. It
        // can still be counted; never signal an unpinned PID in that case.
        let identity = (fd >= 0).then(|| unsafe { File::from_raw_fd(fd) });
        let under_root = ["cwd", "root", "exe"].iter().any(|name| {
            std::fs::read_link(entry.path().join(name))
                .ok()
                .is_some_and(|path| roots.iter().any(|root| path.starts_with(root)))
        });
        let holds_guard = guard.as_ref().is_some_and(|guard| {
            std::fs::read_dir(entry.path().join("fd"))
                .ok()
                .is_some_and(|entries| {
                    entries
                        .filter_map(Result::ok)
                        .any(|entry| std::fs::read_link(entry.path()).ok().as_ref() == Some(guard))
                })
        });
        if under_root || holds_guard {
            count += 1;
            if kill {
                if let Some(identity) = identity {
                    unsafe {
                        libc::syscall(
                            libc::SYS_pidfd_send_signal,
                            identity.as_raw_fd(),
                            libc::SIGKILL,
                            std::ptr::null::<libc::siginfo_t>(),
                            0,
                        );
                    }
                }
            }
        }
    }
    Ok(count)
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn overlapping_rw_roots_and_aliases_refuse() {
        let temp = TempDir::new().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        std::fs::create_dir_all(a.join("nested")).unwrap();
        symlink(&a, &b).unwrap();
        assert!(validate_mounts(&[a.clone(), a.join("nested")]).is_err());
        assert!(validate_mounts(&[a, b]).is_err());
    }

    #[test]
    fn snapshot_preserves_bytes_modes_links_and_empty_dirs_without_shared_inodes() {
        let temp = TempDir::new_in("/tmp").unwrap();
        let root = temp.path().join("trial");
        std::fs::create_dir_all(root.join("empty")).unwrap();
        std::fs::write(root.join("tool"), "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(root.join("tool"), std::fs::Permissions::from_mode(0o751))
            .unwrap();
        symlink("/outside/missing", root.join("link")).unwrap();
        let old_inode = std::fs::metadata(root.join("tool")).unwrap().ino();
        let snapshot = Quarantine::snapshot(std::slice::from_ref(&root)).unwrap();
        assert_ne!(
            old_inode,
            std::fs::metadata(root.join("tool")).unwrap().ino()
        );
        assert_eq!(
            std::fs::metadata(root.join("tool")).unwrap().mode() & 0o777,
            0o751
        );
        assert!(root.join("empty").is_dir());
        assert_eq!(
            std::fs::read_link(root.join("link")).unwrap(),
            Path::new("/outside/missing")
        );
        std::fs::write(snapshot.paths[0].1.join("tool"), "changed").unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("tool")).unwrap(),
            "#!/bin/sh\nexit 0\n"
        );
    }

    #[test]
    fn snapshot_refuses_special_files_and_essential_mounted_ancestors() {
        let temp = TempDir::new_in("/tmp").unwrap();
        let root = temp.path().join("trial");
        std::fs::create_dir(&root).unwrap();
        let fifo = CString::new(root.join("fifo").as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(Quarantine::snapshot(std::slice::from_ref(&root)).is_err());
        assert!(
            root.join("fifo").exists(),
            "failed snapshots restore the original root"
        );
        let outside = TempDir::new_in("/var/tmp").unwrap();
        assert!(Quarantine::snapshot(&[outside.path().to_path_buf()]).is_err());
    }
    #[test]
    fn real_scorer_mounts_pin_old_trial_and_prefix_after_quarantine() {
        if !matches!(
            crate::session::bwrap_availability(),
            crate::session::BwrapAvailability::Ok
        ) {
            return;
        }
        let config = RegenConfig {
            reference_repeats: 3,
            oracle_command: vec!["oracle.sh".into()],
            oracle_paths: vec!["oracle.sh".into()],
            install_command: None,
            required_tools: vec![],
        };
        let env = ScoreEnv::new(&config, false).unwrap();
        let trial = env.run_root.join("trial");
        std::fs::create_dir(&trial).unwrap();
        for root in [&trial, &env.prefix] {
            std::fs::write(root.join("counter"), "0").unwrap();
        }
        let script = r#"import os,time
trial=os.getcwd(); prefix=os.environ['PREFIX']
pid=os.fork()
if pid:
 print(pid, flush=True); os._exit(0)
os.setsid()
os.close(1); os.close(2)
while not os.path.exists(prefix+'/start'): time.sleep(.01)
for n in range(1,1000):
 for root in [trial,prefix]:
  with open(root+'/counter','w') as f: f.write(str(n))
 time.sleep(.01)
"#;
        let output = env
            .run(
                &trial,
                &["/usr/bin/python3".into(), "-c".into(), script.into()],
            )
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let pid: i32 = std::str::from_utf8(&output.stdout)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        struct Kill(i32);
        impl Drop for Kill {
            fn drop(&mut self) {
                unsafe {
                    libc::kill(self.0, libc::SIGKILL);
                }
            }
        }
        let _cleanup = Kill(pid);
        assert!(
            survivors(
                &[trial.clone(), env.prefix.clone(), env.root.clone()],
                None,
                false
            )
            .unwrap()
                >= 1
        );
        let snapshot = Quarantine::snapshot(&[trial.clone(), env.prefix.clone()]).unwrap();
        std::fs::write(snapshot.paths[1].1.join("start"), "go").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if std::fs::read_to_string(snapshot.paths[0].1.join("counter")).unwrap() != "0"
                && std::fs::read_to_string(snapshot.paths[1].1.join("counter")).unwrap() != "0"
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "daemon did not keep writing the quarantined roots"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        for root in [&trial, &env.prefix] {
            assert_eq!(std::fs::read_to_string(root.join("counter")).unwrap(), "0");
        }
        // Exercise absolute embedded paths through a new real scorer sandbox.
        let output = env
            .run(
                &trial,
                &[
                    "/bin/sh".into(),
                    "-c".into(),
                    "test \"$(cat counter)\" = 0 && test \"$(cat \"$PREFIX/counter\")\" = 0".into(),
                ],
            )
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[test]
    fn concurrent_directory_link_swaps_never_copy_external_file_bytes() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        let temp = TempDir::new_in("/tmp").unwrap();
        let source = temp.path().join("source");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(source.join("entry")).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(source.join("entry/value"), "inside").unwrap();
        std::fs::write(outside.join("value"), "external secret").unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_child = Arc::clone(&stop);
        let source_child = source.clone();
        let writer = std::thread::spawn(move || {
            while !stop_child.load(Ordering::Relaxed) {
                let entry = source_child.join("entry");
                let saved = source_child.join("saved");
                std::fs::rename(&entry, &saved).unwrap();
                symlink(&outside, &entry).unwrap();
                std::thread::yield_now();
                std::fs::remove_file(&entry).unwrap();
                std::fs::rename(saved, entry).unwrap();
            }
        });
        let mut leaked = false;
        for n in 0..80 {
            let destination = temp.path().join(format!("copy-{n}"));
            std::fs::create_dir(&destination).unwrap();
            let _ = copy_tree(&source, &destination); // races may safely refuse
            for child in ["entry", "saved"] {
                let path = destination.join(child);
                if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                    leaked |= std::fs::read_to_string(path.join("value"))
                        .is_ok_and(|s| s == "external secret");
                }
            }
        }
        stop.store(true, Ordering::Relaxed);
        writer.join().unwrap();
        assert!(
            !leaked,
            "snapshot followed a concurrently swapped directory symlink"
        );
    }
}
