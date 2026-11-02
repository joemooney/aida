//! Host owns one bwrap process and its namespace init until teardown finishes.
//! No detached receipt, vendor daemon or authority transport is exposed here.
// trace:TASK-2705 | ai:codex
use super::IsolationReason;
use anyhow::{Context, Result};
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
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
import os, json, errno, time, fcntl, stat, sys
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
            # Publish only a fixed system-library origin, never arbitrary paths.
            if target.startswith('/usr/lib/') and target.rsplit('/',1)[-1].startswith('libffi.so.'):
                label='system-libffi-trampoline'
            elif target.startswith('/usr/lib/locale/'):
                label='system-locale'
            elif target == '/proc/'+str(os.getpid())+'/status':
                label='own-proc-status'
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
if startup_fds:
    print(json.dumps(dict(fd_diagnostic_version=1, boundary=startup_fds)), file=sys.stderr, flush=True)
ok('exec_boundary_only_standard_fds_present', not startup_fds)
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
denied('inherited_file_fd_write', lambda: os.write(int(os.sys.argv[7]), b'escape'))
denied('inherited_directory_fd_create', lambda: os.open('fd-escape', os.O_WRONLY|os.O_CREAT, 0o600, dir_fd=int(os.sys.argv[8])))
# Nested user namespaces are retained for native layers. They must not peel
# off inherited protected mounts (MNT_LOCKED), even with new CAP_SYS_ADMIN.
nested=os.fork()
if nested==0:
    # This worker alone needs FFI. Python 3.14/libffi static trampolines open
    # an O_RDONLY|O_CLOEXEC library descriptor on import. Create it AFTER fork,
    # so it never enters the admitted lane or the bwrap namespace reaper.
    # Nothing closes or waives an inherited FD: both parent assertions stay.
    import ctypes
    libc=ctypes.CDLL(None, use_errno=True)
    def nested_refusal(stage, code):
        print(json.dumps(dict(nested_stage=stage, errno=ctypes.get_errno())), file=sys.stderr, flush=True)
        os._exit(code)
    if libc.unshare(0x10000000 | 0x00020000) != 0: nested_refusal('unshare', 10)
    os.mkdir('/tmp/nested-control')
    if libc.mount(b'tmpfs', b'/tmp/nested-control', b'tmpfs', 0, None) != 0: nested_refusal('mount_positive', 11)
    if libc.umount2(b'/tmp/nested-control', 2) != 0: nested_refusal('unmount_positive', 12)
    rc=libc.umount2(b'/lane/workspace/.aida', 2)
    if rc != -1 or ctypes.get_errno() not in (errno.EPERM, errno.EINVAL): nested_refusal('protected_unmount', 13)
    os._exit(0)
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

/// Typed tracer outcome. A host that confines bwrap so a nested user namespace
/// cannot be created (e.g. Ubuntu's AppArmor `unpriv_bwrap` child profile,
/// `audit deny capability`) is a host property, not a tracer defect: it is
/// reported as isolation unavailable with its own typed reason, never a pass.
// trace:TASK-2705 | ai:claude
#[derive(Debug)]
pub(super) enum TracerOutcome {
    Passed(Vec<String>),
    IsolationUnavailable {
        reason: IsolationReason,
        phase: String,
        errno: i32,
    },
}

/// Fixed nested-userns capability probe, run inside the same bwrap boundary
/// as the tracer lane. Only reports the phase and errno; no other output.
const NESTED_PREFLIGHT: &str = r#"
import os, json, ctypes
libc=ctypes.CDLL(None, use_errno=True)
def report(phase, rc):
    print(json.dumps(dict(phase=phase, errno=0 if rc==0 else ctypes.get_errno())), flush=True)
    if rc!=0: os._exit(0)
report('unshare', libc.unshare(0x10000000 | 0x00020000))
os.mkdir('/tmp/nested-preflight')
report('mount_positive', libc.mount(b'tmpfs', b'/tmp/nested-preflight', b'tmpfs', 0, None))
report('unmount_positive', libc.umount2(b'/tmp/nested-preflight', 2))
"#;

/// Host prerequisites, nested-userns preflight, then the full lane.
/// `on_available` fires once the preflight proved nested userns usable, so a
/// caller can record that before the lane's own outcome is known.
pub(super) fn run_probe(on_available: impl FnOnce()) -> Result<TracerOutcome> {
    run_probe_inner(false, on_available)
}

#[cfg(test)]
pub(super) fn timeout_probe() -> Result<TracerOutcome> {
    run_probe_inner(true, || {})
}

fn host_prerequisites() -> Result<()> {
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
    Ok(())
}

/// Minimal synthetic layout `Boundary::synthetic` pins; no controls needed.
fn synthetic_layout() -> Result<tempfile::TempDir> {
    let root = tempfile::Builder::new()
        .prefix("aida-containment-probe-")
        .tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    for dir in ["workspace", "store", "cache", "state"] {
        std::fs::create_dir(root.path().join(dir))?;
    }
    for name in [".aida", ".git", ".agents", ".codex", ".claude"] {
        std::fs::create_dir(root.path().join("workspace").join(name))?;
    }
    for name in ["AGENTS.md", "CLAUDE.md", ".mcp.json"] {
        std::fs::write(root.path().join("workspace").join(name), "protected")?;
    }
    std::fs::write(root.path().join("outside"), "outside-control")?;
    Ok(root)
}

/// Map one failed preflight phase to a typed host property, or `None` when it
/// is an ordinary failure. Only the measured case is a policy denial:
/// `unshare(CLONE_NEWUSER)` returning EPERM/EACCES. ENOSPC there is an
/// exhausted user-namespace limit, a distinct reason. A nested mount/unmount
/// failure after a successful unshare is never classified as a host
/// property: it could be a boundary regression. trace:TASK-2705 | ai:claude
pub(super) fn classify_preflight_failure(phase: &str, errno: i32) -> Option<IsolationReason> {
    match (phase, errno) {
        ("unshare", libc::EPERM | libc::EACCES) => Some(IsolationReason::HostPolicyDenied),
        ("unshare", libc::ENOSPC) => Some(IsolationReason::HostResourceLimit),
        _ => None,
    }
}

/// Probe nested-userns capability ONCE, before the full lane. `Ok(None)` means
/// available; `Ok(Some(..))` is a typed host property (see
/// `classify_preflight_failure`). Every other failure stays an error.
fn nested_userns_preflight() -> Result<Option<TracerOutcome>> {
    let root = synthetic_layout()?;
    let boundary = super::filesystem::Boundary::synthetic(root.path())?;
    let mut cmd = Command::new("/usr/bin/bwrap");
    boundary.append_to(&mut cmd);
    cmd.args(["--", "/usr/bin/python3", "-I", "-S", "-c", NESTED_PREFLIGHT])
        .env_clear();
    let finished = match run_owned(
        cmd,
        boundary.fds(),
        None,
        DEADLINE,
        "nested user namespace preflight unavailable",
        |_| {},
    )? {
        Ended::Exited(finished) => finished,
        Ended::TimedOut => {
            anyhow::bail!("nested user namespace preflight timeout; wrapper reaped")
        }
    };
    anyhow::ensure!(
        finished.status.success(),
        "fixed probe refused (bwrap/userns/filesystem): {}",
        String::from_utf8_lossy(&finished.stderr)
    );
    #[derive(serde::Deserialize)]
    struct Phase {
        phase: String,
        errno: i32,
    }
    let phases = String::from_utf8_lossy(&finished.stdout)
        .lines()
        .map(serde_json::from_str::<Phase>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("invalid nested user namespace preflight channel")?;
    match phases.iter().find(|p| p.errno != 0) {
        Some(p) => match classify_preflight_failure(&p.phase, p.errno) {
            Some(reason) => Ok(Some(TracerOutcome::IsolationUnavailable {
                reason,
                phase: p.phase.clone(),
                errno: p.errno,
            })),
            None => anyhow::bail!(
                "nested user namespace preflight failed at {} (errno {})",
                p.phase,
                p.errno
            ),
        },
        None => {
            anyhow::ensure!(
                phases.len() == 3,
                "nested user namespace preflight incomplete"
            );
            Ok(None)
        }
    }
}

// The boolean selects only a fixed stalled test lane, never executable input.
fn run_probe_inner(timeout_fixture: bool, on_available: impl FnOnce()) -> Result<TracerOutcome> {
    host_prerequisites()?;
    // trace:TASK-2705 | ai:claude — typed host result, never a pass.
    if let Some(unavailable) = nested_userns_preflight()? {
        return Ok(unavailable);
    }
    on_available();
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
    // Seed every untyped host-handle class at the actual bwrap exec boundary,
    // not just a socket. All payloads and paths are owned synthetic controls.
    // trace:TASK-2705 | ai:codex
    let host_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(root.path().join("fd-host-file"))?;
    let host_directory = std::fs::File::open(root.path())?;
    unsafe {
        anyhow::ensure!(
            libc::write(host_file.as_raw_fd(), b"F".as_ptr().cast(), 1) == 1,
            "host file FD positive control"
        );
        let control_fd = libc::openat(
            host_directory.as_raw_fd(),
            c"fd-directory-control".as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC,
            0o600,
        );
        anyhow::ensure!(control_fd >= 0, "host directory FD positive control");
        libc::close(control_fd);
        for fd in [host_file.as_raw_fd(), host_directory.as_raw_fd()] {
            anyhow::ensure!(
                libc::fcntl(fd, libc::F_SETFD, 0) == 0,
                "host handle non-CLOEXEC positive control"
            );
        }
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
        .arg(host_file.as_raw_fd().to_string())
        .arg(host_directory.as_raw_fd().to_string());
    // Typed readiness channel reports the actual namespace-init host PID.
    let mut pipe = [-1; 2];
    unsafe {
        anyhow::ensure!(
            libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) == 0,
            "probe readiness pipe unavailable"
        );
    }
    let info = unsafe { std::fs::File::from_raw_fd(pipe[0]) };
    let info_writer = unsafe { std::fs::File::from_raw_fd(pipe[1]) };
    // Insert before the fixed command (all bwrap options must precede --).
    let cmd = {
        let args = cmd.get_args().map(|a| a.to_os_string()).collect::<Vec<_>>();
        let mut result = Command::new("/usr/bin/bwrap");
        result
            .args(["--info-fd", &info_writer.as_raw_fd().to_string()])
            .args(args)
            .env_clear();
        // Synthetic authority values set after the clear: the lane proves
        // none is forwarded.
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
    let mut namespace = None;
    let ended = run_owned(
        cmd,
        fds,
        Some((info, info_writer)),
        if timeout_fixture {
            Duration::from_secs(2)
        } else {
            DEADLINE
        },
        "fixed probe exec/FD hygiene unavailable (close_range and typed descriptors required)",
        |readiness| {
            if namespace.is_some() {
                return;
            }
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(readiness) {
                if let Some(pid) = value.get("child-pid").and_then(|p| p.as_u64()) {
                    let path = format!("/proc/{pid}/ns/pid");
                    if let Ok(identity) = std::fs::read_link(&path) {
                        namespace = Some((path, identity));
                    }
                }
            }
        },
    )?;
    let finished = match ended {
        Ended::Exited(finished) => finished,
        Ended::TimedOut => {
            let (path, identity) =
                namespace.context("timeout without namespace readiness; wrapper reaped")?;
            require_namespace_gone(&path, &identity)?;
            anyhow::bail!(
                "fixed probe timeout: owned wrapper and namespace init reaped; no fallback"
            );
        }
    };
    anyhow::ensure!(
        finished.status.success(),
        "fixed probe refused (bwrap/userns/filesystem): {}",
        String::from_utf8_lossy(&finished.stderr)
    );
    let mut checks: Vec<String> = serde_json::from_slice(&finished.stdout)
        .context("invalid fixed probe readiness channel")?;
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
    anyhow::ensure!(
        std::fs::read(root.path().join("fd-host-file"))? == b"F"
            && root.path().join("fd-directory-control").is_file()
            && !root.path().join("fd-escape").exists(),
        "host FD controls changed"
    );
    checks.push("host_controls_unchanged".into());
    checks.push("wrapper_reaped".into());
    let (path, identity) = namespace.context("namespace identity readiness unavailable")?;
    require_namespace_gone(&path, &identity)?;
    checks.push("namespace_init_reaped".into());
    drop((leaked, peer, host_file, host_directory));
    Ok(TracerOutcome::Passed(checks))
}

struct Finished {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

enum Ended {
    Exited(Finished),
    /// Deadline passed; the owned wrapper has already been killed and reaped.
    TimedOut,
}

/// Shared spawn/supervise path for every fixed bwrap command (the caller has
/// already cleared its environment): close every descriptor except the typed
/// `fds` at exec, own the child, and drain stdout/stderr (plus the optional
/// `--info-fd` reader) without blocking until it exits or `deadline` passes.
/// `on_info` sees the readiness bytes each tick while the child may still be
/// alive. trace:TASK-2705 | ai:claude
fn run_owned(
    mut cmd: Command,
    fds: Vec<std::os::fd::RawFd>,
    info: Option<(std::fs::File, std::fs::File)>,
    deadline: Duration,
    spawn_context: &'static str,
    mut on_info: impl FnMut(&[u8]),
) -> Result<Ended> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
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
        child: cmd.spawn().context(spawn_context)?,
        reaped: false,
    };
    // The child holds its own writer copy; ours must close for EOF.
    let mut info = info.map(|(reader, writer)| {
        drop(writer);
        reader
    });
    let mut stdout = run.child.stdout.take().unwrap();
    let mut stderr = run.child.stderr.take().unwrap();
    let mut channels = vec![stdout.as_raw_fd(), stderr.as_raw_fd()];
    channels.extend(info.as_ref().map(|i| i.as_raw_fd()));
    for fd in channels {
        unsafe {
            anyhow::ensure!(
                libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) == 0,
                "nonblocking probe channel unavailable"
            );
        }
    }
    let (mut output, mut error, mut readiness) = (Vec::new(), Vec::new(), Vec::new());
    let (mut out_eof, mut err_eof, mut info_eof) = (false, false, info.is_none());
    let mut status = None;
    let deadline = Instant::now() + deadline;
    loop {
        if !out_eof {
            out_eof = read_available(&mut stdout, &mut output)?;
        }
        if !err_eof {
            err_eof = read_available(&mut stderr, &mut error)?;
        }
        if let (false, Some(reader)) = (info_eof, info.as_mut()) {
            info_eof = read_available(reader, &mut readiness)?;
        }
        on_info(&readiness);
        if status.is_none() {
            status = run.child.try_wait()?;
            run.reaped = status.is_some();
        }
        if let (Some(status), true, true, true) = (status, out_eof, err_eof, info_eof) {
            return Ok(Ended::Exited(Finished {
                status,
                stdout: output,
                stderr: error,
            }));
        }
        if Instant::now() >= deadline {
            run.stop_and_reap()?;
            return Ok(Ended::TimedOut);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
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
