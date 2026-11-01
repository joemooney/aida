//! Host owns one bwrap process and its namespace init until teardown finishes.
//! No detached receipt, vendor daemon or authority transport is exposed here.
// trace:TASK-2705 | ai:codex
use anyhow::{Context, Result};
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(12);
const LIMIT: u64 = 16 * 1024;

/// No interactive lane is admitted by this slice. Kept explicit for the shared
/// boundary: an interactive launcher must pass the kernel TIOCSTI preflight.
pub(super) fn terminal_preflight(interactive: bool, legacy_tiocsti: Option<&str>) -> Result<()> {
    anyhow::ensure!(
        !interactive || legacy_tiocsti.map(str::trim) == Some("0"),
        "interactive terminal injection protection unavailable: require dev.tty.legacy_tiocsti=0"
    );
    Ok(())
}

struct OwnedRun {
    child: Child,
    reaped: bool,
}
impl OwnedRun {
    fn stop_and_reap(&mut self) -> Result<()> {
        if !self.reaped {
            // This PID remains our unreaped child, never an ambient registry PID.
            self.child.kill().ok();
            let end = Instant::now() + Duration::from_secs(2);
            while self.child.try_wait()?.is_none() {
                anyhow::ensure!(
                    Instant::now() < end,
                    "owned wrapper teardown deadline exceeded"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            self.reaped = true;
        }
        Ok(())
    }
}
impl Drop for OwnedRun {
    fn drop(&mut self) {
        let _ = self.stop_and_reap();
    }
}

// Fixed code, never read from the spec, user config, PATH or project files.
// Each denial has a writable synthetic host-side positive control prepared
// before confinement. No credentials, model executable or grant selector.
const LANE: &str = r#"
import os, json, errno, ctypes, time, fcntl, stat, sys
# Metadata only: bounded counts, no payloads, paths, socket endpoints or credentials.
def fd_metadata():
    result=[]
    for s in sorted(os.listdir('/proc/self/fd'), key=int):
        n=int(s)
        if n<=2: continue
        try:
            mode=os.fstat(n).st_mode
            target=os.readlink('/proc/self/fd/'+s)
            kind=('directory' if stat.S_ISDIR(mode) else 'file' if stat.S_ISREG(mode)
                  else 'socket' if stat.S_ISSOCK(mode) else 'pipe' if stat.S_ISFIFO(mode) else 'other')
            # Only fixed kernel-generated labels are safe to publish verbatim.
            label=target if target in ('anon_inode:[eventfd]', 'anon_inode:[signalfd]', 'anon_inode:[eventpoll]') else '<redacted>'
            result.append(dict(fd=n, kind=kind, target=label,
                               fd_flags=fcntl.fcntl(n,fcntl.F_GETFD),
                               status_flags=fcntl.fcntl(n,fcntl.F_GETFL)))
        except FileNotFoundError: continue
        except OSError as e:
            if e.errno==errno.EBADF: continue
            raise
        if len(result)==32: break
    return result
startup_fds=fd_metadata()
time.sleep(0.1)
checks=[]
def ok(name, condition):
    if not condition: raise RuntimeError(name)
    checks.append(name)
def denied(name, action):
    try: action()
    except OSError as e:
        ok(name, e.errno in (errno.EROFS, errno.EACCES, errno.EPERM, errno.ENOENT, errno.EBUSY, errno.EBADF))
    else: raise RuntimeError(name + ': unexpectedly allowed')
def write(p):
    with open(p, 'w') as f: f.write('synthetic-write')
for root in ['workspace', 'store', 'cache', 'state']:
    p='/lane/'+root+'/allowed'
    write(p)
    ok('write_'+root, open(p).read() == 'synthetic-write')
write('/tmp/private-scratch')
ok('private_scratch', True)
denied('outside_write', lambda: write('/outside'))
denied('outside_create', lambda: write('/outside-new'))
for name in ['.aida', '.git', '.agents', '.codex', '.claude']:
    p='/lane/workspace/'+name
    denied('protected_modify_'+name, lambda: write(p+'/existing'))
    denied('protected_create_'+name, lambda: write(p+'/missing'))
    denied('protected_unlink_'+name, lambda: os.unlink(p+'/existing'))
    denied('protected_rename_'+name, lambda: os.rename(p, p+'-moved'))
    denied('protected_child_rename_'+name, lambda: os.rename(p+'/existing', p+'/renamed'))
for name in ['AGENTS.md', 'CLAUDE.md', '.mcp.json']:
    p='/lane/workspace/'+name
    denied('protected_file_modify_'+name, lambda: write(p))
    denied('protected_file_unlink_'+name, lambda: os.unlink(p))
    denied('protected_file_rename_'+name, lambda: os.rename(p, p+'-moved'))
status=dict(line.split(':',1) for line in open('/proc/self/status') if ':' in line)
ok('no_new_privs', status['NoNewPrivs'].strip() == '1')
ok('capabilities_dropped', all(int(status[k].strip(),16) == 0 for k in ['CapEff','CapPrm','CapBnd','CapInh']))
ok('no_authority_env_forwarded', not any(k.startswith('AIDA_') for k in os.environ))
ok('identity_mapping', os.getuid() == int(os.sys.argv[1]))
host_ns=os.sys.argv[3]
for name, identity in json.loads(os.sys.argv[6]).items():
    ok('private_namespace_'+name, os.readlink('/proc/self/ns/'+name) != identity)
local_ns=os.readlink('/proc/self/ns/pid')
ok('no_host_pid_visible', local_ns != host_ns and all(os.readlink('/proc/'+p+'/ns/pid') != host_ns for p in os.listdir('/proc') if p.isdigit()))
denied('host_proc_root_unreachable', lambda: write('/proc/'+os.sys.argv[2]+'/root/outside'))
denied('private_proc_root_readonly', lambda: write('/proc/1/root/outside'))
ok('host_proc_environ_unreachable', not os.path.exists('/proc/'+os.sys.argv[2]) or os.readlink('/proc/'+os.sys.argv[2]+'/ns/pid') != host_ns)
fds=[]
for s in os.listdir('/proc/self/fd'):
    n=int(s)
    if n>2:
        try: os.fstat(n); fds.append(n)
        except OSError: pass
if fds:
    print(json.dumps(dict(fd_diagnostic_version=1, python=sys.version_info[:3],
                          startup=startup_fds, lane=fd_metadata())), file=sys.stderr, flush=True)
ok('only_standard_fds_present', not fds)
# The namespace reaper must not retain mount/proc/socket handles either: a
# source directory FD there could bypass protected overmounts via /proc/1/fd.
for p in os.listdir('/proc'):
    if not p.isdigit(): continue
    try: entries=os.listdir('/proc/'+p+'/fd')
    except PermissionError: continue
    for f in entries:
        if int(f)<=2: continue
        try: target=os.readlink('/proc/'+p+'/fd/'+f)
        except FileNotFoundError: continue
        ok('reaper_fd_'+p+'_'+f, p == '1' and target == 'anon_inode:[eventfd]')
ok('reaper_has_no_host_handles', True)
ok('only_typed_fds_present', not fds)
denied('inherited_fd_write', lambda: os.write(int(os.sys.argv[4]), b'escape'))
# Nested user namespaces are retained for native layers. They must not peel
# off inherited protected mounts (MNT_LOCKED), even with new CAP_SYS_ADMIN.
nested=os.fork()
if nested==0:
    libc=ctypes.CDLL(None, use_errno=True)
    if libc.unshare(0x10000000 | 0x00020000) != 0: os._exit(10)
    os.mkdir('/tmp/nested-control')
    if libc.mount(b'tmpfs', b'/tmp/nested-control', b'tmpfs', 0, None) != 0: os._exit(11)
    if libc.umount2(b'/tmp/nested-control', 2) != 0: os._exit(12)
    rc=libc.umount2(b'/lane/workspace/.aida', 2)
    os._exit(0 if rc == -1 and ctypes.get_errno() in (errno.EPERM, errno.EINVAL) else 13)
_,nested_status=os.waitpid(nested,0)
ok('nested_userns_cannot_unmount_protected', nested_status == 0)
# Leave a descendant alive with SIGTERM ignored. Namespace-init exit must reap
# it; the host verifies namespace disappearance after the wrapper settles.
ready_r,ready_w=os.pipe()
pid=os.fork()
if pid==0:
    import signal
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    os.close(ready_r)
    os.write(ready_w,b'R')
    os.close(ready_w)
    time.sleep(30)
    os._exit(0)
os.close(ready_w)
ok('descendant_started', pid > 0 and os.read(ready_r,1) == b'R')
os.close(ready_r)
print(json.dumps(checks), flush=True)
if os.sys.argv[5] == 'timeout': time.sleep(30)
"#;

pub(super) fn run_probe() -> Result<Vec<String>> {
    run_probe_inner(false)
}

#[cfg(test)]
pub(super) fn timeout_probe() -> Result<Vec<String>> {
    run_probe_inner(true)
}

// The boolean selects only a fixed stalled test lane, never executable input.
fn run_probe_inner(timeout_fixture: bool) -> Result<Vec<String>> {
    terminal_preflight(false, None)?;
    anyhow::ensure!(
        cfg!(target_os = "linux"),
        "Linux containment backend unavailable"
    );
    anyhow::ensure!(
        std::path::Path::new("/usr/bin/bwrap").is_file(),
        "bubblewrap unavailable: install bubblewrap; no uncontained fallback"
    );
    anyhow::ensure!(
        std::path::Path::new("/usr/bin/python3").exists(),
        "fixed model-free Python probe unavailable"
    );
    let root = tempfile::Builder::new()
        .prefix("aida-containment-probe-")
        .tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    for dir in ["workspace", "store", "cache", "state"] {
        std::fs::create_dir(root.path().join(dir))?;
    }
    std::fs::set_permissions(
        root.path().join("state"),
        std::fs::Permissions::from_mode(0o700),
    )?;
    for name in [".aida", ".git", ".agents", ".codex", ".claude"] {
        let p = root.path().join("workspace").join(name);
        std::fs::create_dir(&p)?;
        std::fs::write(p.join("existing"), "protected")?;
        // Positive control: these operations work before mounts enforce denial.
        std::fs::write(p.join("missing"), "control")?;
        std::fs::rename(p.join("missing"), p.join("renamed"))?;
        std::fs::remove_file(p.join("renamed"))?;
    }
    for name in ["AGENTS.md", "CLAUDE.md", ".mcp.json"] {
        std::fs::write(
            root.path().join("workspace").join(name),
            "protected-instructions",
        )?;
    }
    std::fs::write(root.path().join("outside"), "outside-control")?;
    // Non-CLOEXEC inherited host socket: actual launch must close it. A socket
    // descriptor would permit host service access despite namespace separation.
    let (leaked, peer) = std::os::unix::net::UnixStream::pair()?;
    // Positive control proves this inherited descriptor is a usable channel.
    unsafe {
        anyhow::ensure!(
            libc::write(leaked.as_raw_fd(), b"C".as_ptr().cast(), 1) == 1,
            "host FD write positive control"
        );
    }
    let mut control = [0; 1];
    (&peer).read_exact(&mut control)?;
    anyhow::ensure!(&control == b"C", "host FD read positive control");
    unsafe {
        anyhow::ensure!(
            libc::fcntl(leaked.as_raw_fd(), libc::F_SETFD, 0) == 0,
            "FD positive control"
        );
    }
    let boundary = super::filesystem::Boundary::synthetic(root.path())?;
    let mut cmd = Command::new("/usr/bin/bwrap");
    boundary.append_to(&mut cmd);
    cmd.args(["--", "/usr/bin/python3", "-I", "-S", "-c", LANE])
        .arg(unsafe { libc::getuid() }.to_string())
        .arg(std::process::id().to_string())
        .arg(std::fs::read_link("/proc/self/ns/pid")?)
        .arg(leaked.as_raw_fd().to_string())
        .arg(if timeout_fixture { "timeout" } else { "normal" })
        .arg(serde_json::to_string(
            &["pid", "ipc", "net", "uts", "cgroup", "user", "mnt"]
                .into_iter()
                .map(|name| Ok((name, std::fs::read_link(format!("/proc/self/ns/{name}"))?)))
                .collect::<std::io::Result<std::collections::BTreeMap<_, _>>>()?,
        )?)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Typed readiness channel reports the actual namespace-init host PID.
    let mut pipe = [-1; 2];
    unsafe {
        anyhow::ensure!(
            libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) == 0,
            "probe readiness pipe unavailable"
        );
    }
    let mut info = unsafe { std::fs::File::from_raw_fd(pipe[0]) };
    let info_writer = unsafe { std::fs::File::from_raw_fd(pipe[1]) };
    // Insert before the fixed command (all bwrap options must precede --).
    let mut cmd = {
        let args = cmd.get_args().map(|a| a.to_os_string()).collect::<Vec<_>>();
        let mut result = Command::new("/usr/bin/bwrap");
        result
            .args(["--info-fd", &info_writer.as_raw_fd().to_string()])
            .args(args)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(test)]
        result
            .env(
                crate::gate::seat_authority::GRANT_ENV,
                "synthetic-parent-selector",
            )
            .env("AIDA_HANDLE_LAUNCH_TOKEN", "synthetic-handle-token")
            .env("AIDA_AGENT_REGISTRY_TOKEN", "synthetic-registry-token");
        result
    };
    let mut fds = boundary.fds();
    fds.push(info_writer.as_raw_fd());
    unsafe {
        cmd.pre_exec(move || {
            // Only async-signal-safe syscalls after fork. No authority/keyring
            // logic. Refuse rather than inherit unknown descriptors.
            if libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 4u32) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            for fd in &fds {
                if libc::fcntl(*fd, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    let mut run = OwnedRun {
        child: cmd.spawn().context(
            "fixed probe exec/FD hygiene unavailable (close_range and typed descriptors required)",
        )?,
        reaped: false,
    };
    drop(info_writer);
    let mut stdout = run.child.stdout.take().unwrap();
    let mut stderr = run.child.stderr.take().unwrap();
    for fd in [stdout.as_raw_fd(), stderr.as_raw_fd(), info.as_raw_fd()] {
        unsafe {
            anyhow::ensure!(
                libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) == 0,
                "nonblocking probe channel unavailable"
            );
        }
    }
    let (mut output, mut error, mut readiness) = (Vec::new(), Vec::new(), Vec::new());
    let (mut out_eof, mut err_eof, mut info_eof) = (false, false, false);
    let mut status = None;
    let mut namespace = None;
    let deadline = Instant::now()
        + if timeout_fixture {
            Duration::from_secs(2)
        } else {
            DEADLINE
        };
    loop {
        if !out_eof {
            out_eof = read_available(&mut stdout, &mut output)?;
        }
        if !err_eof {
            err_eof = read_available(&mut stderr, &mut error)?;
        }
        if !info_eof {
            info_eof = read_available(&mut info, &mut readiness)?;
        }
        if namespace.is_none() {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&readiness) {
                if let Some(pid) = value.get("child-pid").and_then(|p| p.as_u64()) {
                    let path = format!("/proc/{pid}/ns/pid");
                    if let Ok(identity) = std::fs::read_link(&path) {
                        namespace = Some((path, identity));
                    }
                }
            }
        }
        if status.is_none() {
            status = run.child.try_wait()?;
            if status.is_some() {
                run.reaped = true;
            }
        }
        if status.is_some() && out_eof && err_eof && info_eof {
            break;
        }
        if Instant::now() >= deadline {
            run.stop_and_reap()?;
            let (path, identity) =
                namespace.context("timeout without namespace readiness; wrapper reaped")?;
            require_namespace_gone(&path, &identity)?;
            anyhow::bail!(
                "fixed probe timeout: owned wrapper and namespace init reaped; no fallback"
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let status = status.unwrap();
    anyhow::ensure!(
        status.success(),
        "fixed probe refused (bwrap/userns/filesystem): {}",
        String::from_utf8_lossy(&error)
    );
    let mut checks: Vec<String> =
        serde_json::from_slice(&output).context("invalid fixed probe readiness channel")?;
    for dir in ["workspace", "store", "cache", "state"] {
        anyhow::ensure!(
            std::fs::read_to_string(root.path().join(dir).join("allowed"))? == "synthetic-write",
            "permitted write not persisted"
        );
    }
    anyhow::ensure!(
        std::fs::read_to_string(root.path().join("outside"))? == "outside-control",
        "outside control changed"
    );
    checks.push("host_controls_unchanged".into());
    checks.push("wrapper_reaped".into());
    let (path, identity) = namespace.context("namespace identity readiness unavailable")?;
    require_namespace_gone(&path, &identity)?;
    checks.push("namespace_init_reaped".into());
    drop((leaked, peer));
    Ok(checks)
}

fn read_available(reader: &mut impl Read, bytes: &mut Vec<u8>) -> Result<bool> {
    let mut buffer = [0; 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(n) => {
                anyhow::ensure!(
                    bytes.len() + n <= LIMIT as usize,
                    "probe channel exceeds bound"
                );
                bytes.extend_from_slice(&buffer[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
}

#[cfg(test)]
pub(super) fn channel_limit_fixture() -> bool {
    let data = vec![b'x'; LIMIT as usize + 1];
    read_available(&mut data.as_slice(), &mut Vec::new()).is_err()
}

fn require_namespace_gone(path: &str, identity: &std::path::Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match std::fs::read_link(path) {
            Ok(current) if current.as_path() != identity => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error).context("verify owned namespace teardown"),
            _ => {}
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "namespace init survived owned teardown"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
