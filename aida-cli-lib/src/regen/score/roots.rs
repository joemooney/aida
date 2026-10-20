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

/// The parent PID from `/proc/<pid>/status`, if the process is still readable.
// trace:BUG-2160 | ai:claude
fn parent_pid(proc_dir: &Path) -> Option<i32> {
    std::fs::read_to_string(proc_dir.join("status"))
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("PPid:"))?
        .trim()
        .parse()
        .ok()
}

/// A live child of this process still executing this process's own binary:
/// a fork that has not reached its exec yet.
// trace:BUG-2160 | ai:claude
fn is_own_pre_exec_fork(proc_dir: &Path) -> bool {
    if parent_pid(proc_dir) != Some(std::process::id() as i32) {
        return false;
    }
    let identity = |path: &Path| std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()));
    match (
        identity(&proc_dir.join("exe")),
        identity(Path::new("/proc/self/exe")),
    ) {
        (Some(child), Some(own)) => child == own,
        _ => false,
    }
}

/// The process group from `/proc/<pid>/stat` (the field after the state, which
/// follows the parenthesized command name).
// trace:TASK-1956 | ai:claude
fn process_group(proc_dir: &Path) -> Option<i32> {
    let stat = std::fs::read_to_string(proc_dir.join("stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(2)?
        .parse()
        .ok()
}

/// A child of this process caught mid-exec by a concurrent spawn: its parent
/// is the scorer and it sits either in the scorer's own process group (a plain
/// `Command`) or in a group it leads (`command::run`'s `process_group(0)`).
/// A clone(CLONE_PARENT) sibling of a scored command also has the scorer as
/// parent, but inherits the scored command's group, which it neither leads nor
/// shares with the scorer, so it is never mistaken for one.
// trace:TASK-1956 | ai:claude trace:BUG-2155 | ai:claude
fn is_own_spawn(proc_dir: &Path, pid: i32) -> bool {
    if parent_pid(proc_dir) != Some(std::process::id() as i32) {
        return false;
    }
    let own_group = unsafe { libc::getpgrp() };
    process_group(proc_dir).is_some_and(|group| group == own_group || group == pid)
}

/// Whether descriptor `fd` of the process at `proc_dir` is close-on-exec, from
/// the `flags:` line of its fdinfo (octal, with O_CLOEXEC reported there).
/// Unreadable means "no": the descriptor still counts as held.
// trace:TASK-1956 | ai:claude
fn close_on_exec(proc_dir: &Path, fd: &OsStr) -> bool {
    std::fs::read_to_string(proc_dir.join("fdinfo").join(fd))
        .ok()
        .and_then(|info| {
            info.lines()
                .find_map(|line| line.strip_prefix("flags:"))
                .and_then(|flags| i64::from_str_radix(flags.trim(), 8).ok())
        })
        .is_some_and(|flags| flags & i64::from(libc::O_CLOEXEC) != 0)
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
        // A child forked by another scorer thread holds copies of all our
        // descriptors (the guard pipe included) until its exec, and matching
        // it here SIGKILLed unrelated concurrent spawns, such as a `git
        // cat-file` dying with empty stderr (BUG-2160). Skip only that case:
        // a live child that is still running this very binary. Parentage
        // alone is not enough: an unconfined installer (our child) can
        // clone(CLONE_PARENT) a sibling whose parent is us, and that sibling
        // must still be counted and killed. Read after the pidfd pins the
        // identity, so a reused PID is never signalled.
        // trace:BUG-2160 | ai:claude
        if is_own_pre_exec_fork(&entry.path()) {
            continue;
        }
        let under_root = ["cwd", "root", "exe"].iter().any(|name| {
            std::fs::read_link(entry.path().join(name))
                .ok()
                .is_some_and(|path| roots.iter().any(|root| path.starts_with(root)))
        });
        // A child of this process that has already switched its image (so the
        // check above no longer matches it) still holds close-on-exec copies
        // of our descriptors until execve reaches do_close_on_exec. Counting
        // that copy SIGKILLed concurrent `git` spawns mid-exec, which then
        // failed with empty stderr in parallel regen tests (BUG-2155). A
        // scored command holds the guard with close-on-exec cleared (see
        // `run_with_bundle`), so it still matches, and so does a
        // clone(CLONE_PARENT) sibling of one that sets close-on-exec on its
        // copy: it stays in the scored command's process group. Only a sibling
        // that also leaves that group evades, no more than closing the guard.
        // trace:TASK-1956 | ai:claude trace:BUG-2155 | ai:claude
        let own_child = is_own_spawn(&entry.path(), pid);
        let holds_guard = guard.as_ref().is_some_and(|guard| {
            std::fs::read_dir(entry.path().join("fd"))
                .ok()
                .is_some_and(|entries| {
                    entries.filter_map(Result::ok).any(|fd| {
                        std::fs::read_link(fd.path()).ok().as_ref() == Some(guard)
                            && !(own_child && close_on_exec(&entry.path(), &fd.file_name()))
                    })
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

    /// Reap our child `pid` within `secs`; `Some(signal)` if a signal ended it.
    // trace:BUG-2160 | ai:claude
    fn reap(pid: i32, secs: u64) -> Option<Option<i32>> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
        loop {
            let mut status = 0;
            let r = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
            if r == pid {
                return Some(libc::WIFSIGNALED(status).then(|| libc::WTERMSIG(status)));
            }
            if r < 0 || std::time::Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    // trace:BUG-2160 | ai:claude
    #[test]
    fn live_children_of_the_scorer_are_never_survivors() {
        use std::process::{Command, Stdio};
        let temp = TempDir::new().unwrap();
        let root = temp.path().to_path_buf();
        // A fork of this binary that has not exec'd yet (as a concurrent
        // thread's spawn is mid-fork), sitting under the root: neither counted
        // nor killed. The child only makes async-signal-safe calls.
        let dir = CString::new(root.as_os_str().as_bytes()).unwrap();
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            unsafe {
                // Drop the copies of other tests' descriptors (locks, pipes)
                // that this long-lived fork would otherwise keep open.
                libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 0u32);
                if libc::chdir(dir.as_ptr()) != 0 {
                    libc::_exit(1);
                }
                libc::sleep(60);
                libc::_exit(0);
            }
        }
        let in_root = |pid: i32| {
            std::fs::read_link(format!("/proc/{pid}/cwd")).is_ok_and(|cwd| cwd.starts_with(&root))
        };
        while !in_root(pid) {
            assert!(reap(pid, 0).is_none(), "the fork exited before chdir");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // An escaped descendant: its parent exited, so it was reparented.
        let out = Command::new("/bin/sh")
            .args(["-c", "sleep 60 </dev/null >/dev/null 2>&1 & echo $!"])
            .current_dir(&root)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success());
        let count = survivors(std::slice::from_ref(&root), None, true).unwrap();
        // Still our unreaped child, so its PID cannot have been reused.
        let fork_alive = reap(pid, 0).is_none();
        unsafe { libc::kill(pid, libc::SIGKILL) };
        reap(pid, 30);
        assert!(fork_alive, "the scan killed a pre-exec fork of the scorer");
        assert_eq!(count, 1, "only the reparented descendant is a survivor");
    }

    // A concurrent spawn caught inside execve: our child, no longer running
    // this binary, still holding a close-on-exec copy of the guard. It is not
    // a survivor and must not be killed (BUG-2155). The same child holding the
    // guard with close-on-exec cleared, as a scored command does, still is.
    // trace:TASK-1956 | ai:claude
    #[test]
    fn close_on_exec_guard_copy_in_a_mid_exec_child_is_not_a_survivor() {
        use std::io::BufRead;
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
        let (read_end, write_end) =
            unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) };
        let guard = write_end.as_raw_fd();
        let unrelated_root = TempDir::new().unwrap();
        for inheritable in [false, true] {
            // python3 receives the guard, then sets close-on-exec on it as
            // requested and reports ready: the mid-exec descriptor state.
            let script = format!(
                "import os,sys,time\nos.set_inheritable({guard},{})\nprint('ready',flush=True)\ntime.sleep(60)\n",
                if inheritable { "True" } else { "False" }
            );
            let mut cmd = Command::new("/usr/bin/python3");
            cmd.args(["-c", &script])
                .stdin(Stdio::null())
                .stdout(Stdio::piped());
            unsafe {
                cmd.pre_exec(move || {
                    if libc::fcntl(guard, libc::F_SETFD, 0) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let Ok(mut child) = cmd.spawn() else {
                eprintln!("SKIP close_on_exec_guard_copy_in_a_mid_exec_child_is_not_a_survivor: /usr/bin/python3 cannot be spawned");
                return;
            };
            let pid = child.id() as i32;
            let mut ready = String::new();
            std::io::BufReader::new(child.stdout.take().unwrap())
                .read_line(&mut ready)
                .unwrap();
            assert_eq!(ready.trim(), "ready");
            let count = survivors(
                std::slice::from_ref(&unrelated_root.path().to_path_buf()),
                Some(guard),
                true,
            )
            .unwrap();
            let ended = reap(pid, if inheritable { 30 } else { 0 });
            if ended.is_none() {
                unsafe { libc::kill(pid, libc::SIGKILL) };
                reap(pid, 30);
            }
            if inheritable {
                assert_eq!(count, 1, "a scored-command guard holder was not counted");
                assert_eq!(ended, Some(Some(libc::SIGKILL)), "it was not killed");
            } else {
                assert_eq!(count, 0, "a mid-exec close-on-exec copy was counted");
                assert_eq!(ended, None, "a mid-exec spawn was killed");
            }
        }
        drop((read_end, write_end));
    }

    // A scored command (our child) clones a sibling with CLONE_PARENT, so the
    // sibling's parent is the scorer too. It is not this binary, so it must
    // be counted and killed (BUG-2160 review).
    // trace:BUG-2160 | ai:claude
    #[test]
    fn clone_parent_sibling_of_a_scored_command_is_counted_and_killed() {
        use std::process::{Command, Stdio};
        let temp = TempDir::new().unwrap();
        let root = temp.path().to_path_buf();
        let script = r#"
import ctypes,os,time
libc=ctypes.CDLL(None,use_errno=True)
CLONE_PARENT,SIGCHLD,SYS_clone=0x8000,17,{'x86_64':56,'aarch64':220}[os.uname().machine]
pid=libc.syscall(SYS_clone,CLONE_PARENT|SIGCHLD,0,0,0,0)
if pid==0:
 n=os.open('/dev/null',os.O_RDWR)
 for fd in (0,1,2): os.dup2(n,fd)
 time.sleep(60); os._exit(0)
print(pid,flush=True)
"#;
        let out = match Command::new("/usr/bin/python3")
            .args(["-c", script])
            .current_dir(&root)
            .stdin(Stdio::null())
            .output()
        {
            Ok(out) if out.status.success() => out,
            // No python3, or an architecture without a known clone number.
            _ => {
                eprintln!("SKIP clone_parent_sibling_of_a_scored_command_is_counted_and_killed: /usr/bin/python3 could not clone a sibling");
                return;
            }
        };
        let sibling: i32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
        assert!(sibling > 0, "clone failed");
        let parent = parent_pid(Path::new(&format!("/proc/{sibling}")));
        let count = survivors(std::slice::from_ref(&root), None, true).unwrap();
        // CLONE_PARENT made it our child: reaping it is safe and tells us how
        // it ended.
        let ended = reap(sibling, 30);
        if ended.is_none() {
            unsafe { libc::kill(sibling, libc::SIGKILL) };
            reap(sibling, 30);
        }
        assert_eq!(
            parent,
            Some(std::process::id() as i32),
            "not a CLONE_PARENT sibling"
        );
        assert_eq!(count, 1, "the sibling under the root was not counted");
        assert_eq!(
            ended,
            Some(Some(libc::SIGKILL)),
            "the sibling was not killed"
        );
    }

    // A scored command in its own process group, holding the guard as
    // inheritable, clones a CLONE_PARENT sibling that marks its guard copy
    // close-on-exec. Its parent is the scorer, like a mid-exec spawn, but it is
    // in the scored command's group, so it is still counted and killed.
    // trace:TASK-1956 | ai:claude
    #[test]
    fn close_on_exec_guard_copy_in_a_clone_parent_sibling_is_a_survivor() {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
        let (read_end, write_end) =
            unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) };
        let guard = write_end.as_raw_fd();
        let unrelated_root = TempDir::new().unwrap();
        let script = format!(
            r#"
import ctypes,os,time
libc=ctypes.CDLL(None,use_errno=True)
CLONE_PARENT,SIGCHLD,SYS_clone=0x8000,17,{{'x86_64':56,'aarch64':220}}[os.uname().machine]
pid=libc.syscall(SYS_clone,CLONE_PARENT|SIGCHLD,0,0,0,0)
if pid==0:
 os.set_inheritable({guard},False)
 print('ready',flush=True)
 n=os.open('/dev/null',os.O_RDWR)
 for fd in (0,1,2): os.dup2(n,fd)
 time.sleep(60); os._exit(0)
print(pid,flush=True)
"#
        );
        let mut cmd = Command::new("/usr/bin/python3");
        cmd.args(["-c", &script])
            .process_group(0)
            .stdin(Stdio::null());
        unsafe {
            cmd.pre_exec(move || {
                if libc::fcntl(guard, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        // Output waits for the sibling's stdout too, so it is ready by then.
        let out = match cmd.output() {
            Ok(out) if out.status.success() => out,
            _ => {
                eprintln!("SKIP close_on_exec_guard_copy_in_a_clone_parent_sibling_is_a_survivor: /usr/bin/python3 could not clone a sibling");
                return;
            }
        };
        let stdout = String::from_utf8_lossy(&out.stdout);
        let sibling: i32 = stdout
            .lines()
            .find_map(|line| line.trim().parse().ok())
            .expect("the sibling pid");
        assert!(sibling > 0, "clone failed");
        assert!(stdout.lines().any(|line| line == "ready"), "{stdout}");
        let parent = parent_pid(Path::new(&format!("/proc/{sibling}")));
        let count = survivors(
            std::slice::from_ref(&unrelated_root.path().to_path_buf()),
            Some(guard),
            true,
        )
        .unwrap();
        let ended = reap(sibling, 30);
        if ended.is_none() {
            unsafe { libc::kill(sibling, libc::SIGKILL) };
            reap(sibling, 30);
        }
        drop((read_end, write_end));
        assert_eq!(
            parent,
            Some(std::process::id() as i32),
            "not a CLONE_PARENT sibling"
        );
        assert_eq!(count, 1, "the close-on-exec sibling was not counted");
        assert_eq!(
            ended,
            Some(Some(libc::SIGKILL)),
            "the sibling was not killed"
        );
    }

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
        // Survivor fd scans must not observe unrelated libtest forks.
        // trace:STORY-1677 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        if !matches!(
            crate::session::bwrap_availability(),
            crate::session::BwrapAvailability::Ok
        ) {
            return;
        }
        let config = RegenConfig {
            reference_repeats: 3,
            crash_failure_share_percent: 50,
            min_bytes: 64,
            components: BTreeMap::new(),
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
