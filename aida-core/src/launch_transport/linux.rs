// trace:TASK-1612 | ai:codex
// trace:BUG-1808 | ai:codex
use super::image::{dup_high, read_bounded, sealed_bytes, verify_image, verify_sealed};
use super::{ImageIdentity, SealedExecImage};
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::fs::File;
use std::marker::PhantomData;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::rc::Rc;
use std::time::Duration;

const LEAF: RawFd = 3;
const DESCRIPTION: RawFd = 4;
const CONTROL: RawFd = 5;
const ERROR: RawFd = 6;
const HELPER: RawFd = 7;
const CWD: RawFd = 8;
const MAX_DESCRIPTION: u64 = 64 * 1024;
const FRAME_LEN: usize = 96;
const READY: u8 = 1;
#[cfg(test)]
const FIXTURE_RELEASE: u8 = 2;
const CANCEL: u8 = 3;
const MAX_STARTUP: Duration = Duration::from_secs(30);
const SETTLEMENT: Duration = Duration::from_millis(500);

/// An observed identity, not a signaling right. Only WaitingChild owns a pidfd.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: i32,
    pub boot_id: String,
    pub start_ticks: u64,
}
impl ProcessIdentity {
    fn capture(pid: i32) -> Result<Self> {
        ensure!(pid > 0, "invalid process identity");
        let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?
            .trim()
            .to_owned();
        ensure!(
            uuid::Uuid::parse_str(&boot_id).is_ok(),
            "unknown boot identity"
        );
        let stat = std::fs::read(format!("/proc/{pid}/stat"))?;
        let start_ticks = stat_start(&stat).context("unknown or dead process identity")?;
        Ok(Self {
            pid,
            boot_id,
            start_ticks,
        })
    }
    fn is_current(&self) -> bool {
        Self::capture(self.pid)
            .as_ref()
            .is_ok_and(|now| now == self)
    }
}

// No allocation, locks, or formatting: also used in the post-fork child.
fn stat_start(stat: &[u8]) -> Option<u64> {
    let end = stat.iter().rposition(|b| *b == b')')?;
    let mut fields = stat.get(end + 2..)?.split(|b| *b == b' ');
    let state = fields.next()?;
    if matches!(state, b"Z" | b"X" | b"x") {
        return None;
    }
    let digits = fields.nth(18)?; // state is field 3; starttime is field 22
    let mut n = 0u64;
    for &b in digits {
        if !b.is_ascii_digit() {
            return None;
        }
        n = n.checked_mul(10)?.checked_add(u64::from(b - b'0'))?;
    }
    (n > 0).then_some(n)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Credentials {
    uid: Vec<u32>,
    gid: Vec<u32>,
    groups: Vec<u32>,
    capabilities: Vec<String>,
}
impl Credentials {
    fn capture() -> Result<Self> {
        // /proc/self/status can describe the group leader rather than a caller
        // with thread-local credentials. Read this actual thread instead.
        let status = std::fs::read_to_string("/proc/thread-self/status")?;
        let field = |name: &str| -> Result<&str> {
            status
                .lines()
                .find_map(|l| l.strip_prefix(name))
                .context("missing credential identity")
        };
        let numbers = |name: &str| -> Result<Vec<u32>> {
            field(name)?
                .split_whitespace()
                .map(|v| v.parse().map_err(Into::into))
                .collect()
        };
        let uid = numbers("Uid:")?;
        let gid = numbers("Gid:")?;
        ensure!(
            uid.len() == 4 && uid.iter().all(|v| *v == uid[0]) && uid[0] != 0,
            "root or transitioning uid profile is unsupported"
        );
        ensure!(
            gid.len() == 4 && gid.iter().all(|v| *v == gid[0]),
            "transitioning gid profile is unsupported"
        );
        let mut capabilities = Vec::new();
        for name in ["CapInh:", "CapPrm:", "CapEff:", "CapBnd:", "CapAmb:"] {
            let value = field(name)?.trim().to_owned();
            let n = u64::from_str_radix(&value, 16)?;
            ensure!(
                name == "CapBnd:" || n == 0,
                "active process capabilities are unsupported"
            );
            capabilities.push(value);
        }
        Ok(Self {
            uid,
            gid,
            groups: numbers("Groups:")?,
            capabilities,
        })
    }
}

/// The only profile in this slice: same host PID namespace, procfs, native ELF,
/// unprivileged credentials, no wrapper/containment/pane adapter. The orphan
/// adopter is observed, not declared: the nearest preexisting subreaper or
/// namespace init that actually adopted and reaped an owned probe orphan.
#[derive(Debug)]
pub struct HostProfile {
    adopter: ProcessIdentity,
    credentials: Credentials,
}
impl HostProfile {
    pub fn probe() -> Result<Self> {
        super::require_supported_build()?;
        // /proc/1/ns/pid is ptrace-gated for an unprivileged host user.
        // Instead require procfs to expose this exact namespace PID (no
        // host-visible/inner-PID translation). PID 1 in that view is the
        // declared namespace-init adopter; no wrapper/containment is added.
        ensure!(
            std::fs::read_link("/proc/self")?.to_str()
                == Some(unsafe { libc::getpid() }.to_string().as_str()),
            "procfs PID translation requires a containment adapter"
        );
        let clone_probe = unsafe { libc::syscall(libc::SYS_clone3, std::ptr::null::<u8>(), 0) };
        ensure!(
            clone_probe == -1
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINVAL),
            "atomic child/pidfd creation requires clone3"
        );
        let me = unsafe { libc::getpid() };
        let fd = pidfd_open(me)?;
        ensure!(
            unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    fd.as_raw_fd(),
                    0,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                )
            } == 0,
            "pidfd signaling is unavailable"
        );
        // Empty close_range probes syscall support without touching any FD.
        let rc = unsafe { libc::syscall(libc::SYS_close_range, u32::MAX, u32::MAX, 0) };
        ensure!(rc == 0, "close_range is required by the descriptor profile");
        let rc = unsafe {
            libc::syscall(
                libc::SYS_execveat,
                -1,
                c"".as_ptr(),
                std::ptr::null::<*const libc::c_char>(),
                std::ptr::null::<*const libc::c_char>(),
                libc::AT_EMPTY_PATH,
            )
        };
        ensure!(
            rc == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EBADF),
            "descriptor exec is unavailable"
        );
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        ensure!(
            unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), &mut action) } == 0
                && action.sa_sigaction == libc::SIG_DFL
                && action.sa_flags & libc::SA_NOCLDWAIT == 0,
            "waiting-child requires exclusive wait ownership and default SIGCHLD"
        );
        Ok(Self {
            adopter: qualify_adopter()?,
            credentials: Credentials::capture()?,
        })
    }
    /// The qualified orphan adopter. An observation, not a signaling right.
    pub fn adopter(&self) -> &ProcessIdentity {
        &self.adopter
    }
}

const ADOPTER_QUALIFICATION: Duration = Duration::from_secs(2);

// trace:TASK-1612 | ai:claude
// trace:BUG-1808 | ai:claude
// When this supervisor dies, the kernel reparents its children to the nearest
// live ancestor child-subreaper, else to namespace init. Neither is observable
// in procfs, and liveness of PID 1 proves neither. So orphan an owned probe the
// same way and observe who adopts it: owned C (pidfd from clone3) forks G and
// reports its PID while G is C's unreaped child, pinning G's PID for our
// pidfd. C exits; G's new parent is the actual adopter. G then exits, and the
// adopter must reap it within the bound. No probe is signaled except C through
// its own pidfd, and nothing is guessed: an unqualified adopter refuses.
fn qualify_adopter() -> Result<ProcessIdentity> {
    let mut subreaper = 0i32;
    ensure!(
        unsafe { libc::prctl(libc::PR_GET_CHILD_SUBREAPER, &mut subreaper, 0, 0, 0) } == 0
            && subreaper == 0,
        "a child-subreaper supervisor adopts its own orphans; outer orphan adopter unqualified"
    );
    let me = unsafe { libc::getpid() };
    let until = monotonic_ns()?.saturating_add(ADOPTER_QUALIFICATION.as_nanos() as u64);
    // Bounded even if this supervisor is lost mid-probe and no byte arrives.
    let linger_ms = (ADOPTER_QUALIFICATION.as_millis() as i32) * 2;
    let (report_r, report_w) = error_pipe()?;
    let (hold_r, hold_w) = error_pipe()?;
    let (release_r, release_w) = error_pipe()?;
    let (report_fd, hold_fd, release_fd) = (
        report_w.as_raw_fd(),
        hold_r.as_raw_fd(),
        release_r.as_raw_fd(),
    );
    let parent_ends = [
        report_r.as_raw_fd(),
        hold_w.as_raw_fd(),
        release_w.as_raw_fd(),
    ];
    let mut owned_pidfd = -1i32;
    let c = unsafe { raw_clone(&mut owned_pidfd) };
    ensure!(
        c >= 0,
        "orphan adopter probe fork failed: {}",
        std::io::Error::last_os_error()
    );
    if c == 0 {
        // Only raw syscalls here, as in the waiting-child post-fork path.
        unsafe {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) != 0
                || libc::getppid() != me
            {
                libc::_exit(1);
            }
            // Supervisor loss then reaches both probes as EOF.
            for fd in parent_ends {
                libc::close(fd);
            }
            let g = raw_clone(std::ptr::null_mut());
            if g == 0 {
                libc::close(report_fd);
                libc::close(hold_fd);
                linger(release_fd, linger_ms);
                libc::_exit(0);
            }
            if g < 0 || libc::write(report_fd, (&g as *const i32).cast(), 4) != 4 {
                libc::_exit(1);
            }
            linger(hold_fd, linger_ms);
            libc::_exit(0);
        }
    }
    let mut probe = ProbeChild {
        pidfd: unsafe { File::from_raw_fd(owned_pidfd) },
        reaped: false,
    };
    drop((report_w, hold_r, release_r));
    let observed = observe_orphan(me, c, &mut probe, &report_r, &hold_w, &release_w, until);
    // Always unblock both probes, then settle the owned one. A lingering G is
    // the adopter's to collect; it is never signaled.
    let _ = write_byte(&hold_w);
    let _ = write_byte(&release_w);
    let settled = probe.settle();
    let adopter = observed?;
    settled?;
    Ok(adopter)
}

struct ProbeChild {
    pidfd: File,
    reaped: bool,
}
impl ProbeChild {
    fn exited(&self) -> Result<bool> {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let rc = unsafe {
            libc::waitid(
                libc::P_PIDFD,
                self.pidfd.as_raw_fd() as u32,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        ensure!(rc == 0, "orphan adopter probe wait failed");
        Ok(unsafe { info.si_pid() } != 0)
    }
    fn reap(&mut self, until: u64) -> Result<()> {
        while !self.reaped {
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let rc = unsafe {
                libc::waitid(
                    libc::P_PIDFD,
                    self.pidfd.as_raw_fd() as u32,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG,
                )
            };
            if rc == 0 && unsafe { info.si_pid() } != 0 {
                self.reaped = true;
                ensure!(
                    info.si_code == libc::CLD_EXITED && unsafe { info.si_status() } == 0,
                    "orphan adopter probe failed"
                );
                break;
            }
            if rc < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                bail!("orphan adopter probe wait failed");
            }
            ensure!(monotonic_ns()? < until, "orphan adopter probe did not exit");
            poll_fd(self.pidfd.as_raw_fd(), libc::POLLIN, until)?;
        }
        Ok(())
    }
    fn settle(&mut self) -> Result<()> {
        if self.reaped {
            return Ok(());
        }
        let until = monotonic_ns()?.saturating_add(1_000_000_000);
        if self.reap(until).is_ok() {
            return Ok(());
        }
        // Only our own clone3 pidfd is ever signaled.
        unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.pidfd.as_raw_fd(),
                libc::SIGKILL,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            );
        }
        let until = monotonic_ns()?.saturating_add(1_000_000_000);
        let _ = self.reap(until);
        ensure!(self.reaped, "orphan adopter probe settlement unknown");
        Ok(())
    }
}

fn observe_orphan(
    me: i32,
    c: i32,
    probe: &mut ProbeChild,
    report: &File,
    hold: &File,
    release: &File,
    until: u64,
) -> Result<ProcessIdentity> {
    ensure!(
        poll_fd(report.as_raw_fd(), libc::POLLIN, until)? & libc::POLLIN != 0,
        "orphan adopter probe did not report"
    );
    let mut g = 0i32;
    ensure!(
        unsafe { libc::read(report.as_raw_fd(), (&mut g as *mut i32).cast(), 4) } == 4 && g > 0,
        "invalid orphan adopter probe report"
    );
    let orphan = pidfd_open(g)?;
    // C never waits, so while C is alive G's PID cannot be reused: the pidfd
    // above names C's actual child.
    ensure!(
        stat_ppid(g) == Some(c) && !probe.exited()?,
        "orphan adopter probe lost its child"
    );
    write_byte(hold)?;
    // Reaping C means exit_notify has already reparented G.
    probe.reap(until)?;
    let adopter_pid = stat_ppid(g).context("orphan adopter probe vanished")?;
    ensure!(
        adopter_pid > 0 && adopter_pid != me && adopter_pid != c,
        "orphan adopter is not visible in this PID namespace"
    );
    let adopter = ProcessIdentity::capture(adopter_pid)?;
    // G unreaped after both reads: the observed stat lines were G's own.
    ensure!(
        stat_ppid(g) == Some(adopter_pid) && pidfd_signal_zero(&orphan)?,
        "orphan adopter changed during qualification"
    );
    write_byte(release)?;
    while pidfd_signal_zero(&orphan)? {
        ensure!(
            monotonic_ns()? < until,
            "orphan adopter did not reap an orphan within the qualification bound"
        );
        if poll_fd(orphan.as_raw_fd(), libc::POLLIN, until)? & libc::POLLIN != 0 {
            // Exited; only the adopter's reaping is left to observe.
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    // Only G's parent can reap it; a replaced adopter would have done so.
    ensure!(
        adopter.is_current(),
        "orphan adopter changed during qualification"
    );
    Ok(adopter)
}

/// True while the pidfd's process is unreaped (live or zombie); false once its
/// parent reaped it. Signal 0 delivers nothing.
fn pidfd_signal_zero(fd: &File) -> Result<bool> {
    let rc = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            fd.as_raw_fd(),
            0,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    if rc == 0 {
        return Ok(true);
    }
    ensure!(
        std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH),
        "orphan observation failed"
    );
    Ok(false)
}

fn stat_ppid(pid: i32) -> Option<i32> {
    let stat = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
    let end = stat.iter().rposition(|b| *b == b')')?;
    let field = stat.get(end + 2..)?.split(|b| *b == b' ').nth(1)?;
    std::str::from_utf8(field).ok()?.parse().ok()
}

fn write_byte(f: &File) -> Result<()> {
    ensure!(
        unsafe { libc::write(f.as_raw_fd(), [1u8].as_ptr().cast(), 1) } == 1,
        "orphan adopter probe control failed"
    );
    Ok(())
}

// Post-fork probe wait: one bounded poll, no allocation.
unsafe fn linger(fd: RawFd, millis: i32) {
    let mut p = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    libc::poll(&mut p, 1, millis);
}

#[repr(C)]
struct CloneArgs {
    flags: u64,
    pidfd: u64,
    child_tid: u64,
    parent_tid: u64,
    exit_signal: u64,
    stack: u64,
    stack_size: u64,
    tls: u64,
    set_tid: u64,
    set_tid_size: u64,
    cgroup: u64,
}
/// Raw clone3: no libc atfork callbacks. A non-null `pidfd` receives the
/// child's pidfd atomically with its creation.
unsafe fn raw_clone(pidfd: *mut i32) -> i32 {
    let args = CloneArgs {
        flags: if pidfd.is_null() {
            0
        } else {
            libc::CLONE_PIDFD as u64
        },
        pidfd: pidfd as u64,
        child_tid: 0,
        parent_tid: 0,
        exit_signal: libc::SIGCHLD as u64,
        stack: 0,
        stack_size: 0,
        tls: 0,
        set_tid: 0,
        set_tid_size: 0,
        cgroup: 0,
    };
    libc::syscall(libc::SYS_clone3, &args, std::mem::size_of::<CloneArgs>()) as i32
}

/// Ephemeral exact arguments/environment; this is not an assignment or grant.
/// No inherited environment, hooks or implicit terminal descriptors.
#[derive(Debug)]
pub struct LaunchDescription {
    pub argv: Vec<String>,
    pub environment: Vec<(String, String)>,
    pub startup_timeout: Duration,
}
impl LaunchDescription {
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.argv.is_empty() && self.argv.len() <= 256 && self.environment.len() <= 256,
            "argument/environment count exceeds transport profile"
        );
        ensure!(
            !self.startup_timeout.is_zero() && self.startup_timeout <= MAX_STARTUP,
            "startup deadline outside supported bound"
        );
        for a in &self.argv {
            CString::new(a.as_bytes())?;
        }
        let mut keys = std::collections::BTreeSet::new();
        for (key, value) in &self.environment {
            ensure!(
                !key.is_empty() && !key.contains('=') && keys.insert(key),
                "invalid or duplicate environment key"
            );
            ensure!(
                !key.starts_with("LD_")
                    && !matches!(key.as_str(), "GLIBC_TUNABLES" | "GCONV_PATH" | "LOCPATH"),
                "loader environment injection requires an unsupported adapter"
            );
            CString::new(key.as_bytes())?;
            CString::new(value.as_bytes())?;
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Description {
    profile: String,
    parent: ProcessIdentity,
    thread: ProcessIdentity,
    adopter: ProcessIdentity,
    credentials: Credentials,
    helper: ImageIdentity,
    leaf: ImageIdentity,
    cwd: (u64, u64),
    stdio: [(u64, u64, u32); 3],
    argv: Vec<String>,
    environment: Vec<(String, String)>,
    nonce: [u8; 32],
    deadline_ns: u64,
    #[cfg(test)]
    fixture: FixtureBehavior,
}
#[cfg(test)]
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
enum FixtureBehavior {
    #[default]
    Normal,
    BeforePrctl,
    BeforeReady,
    BeforeExec,
    ExecFailure,
}

/// Explicit observation, never a claim of nonexecution based on death/EOF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancellation {
    pub reaped: bool,
    pub possibly_executed: bool,
    pub wait_status: Option<i32>,
}

/// !Send/!Sync keeps normal ownership on the creating thread. The OS still
/// kills the child if that thread exits unexpectedly. No public release API.
///
/// ```compile_fail
/// use aida_core::launch_transport::WaitingChild;
/// fn cannot_release(child: &mut WaitingChild) {
///     child.fixture_release(());
/// }
/// ```
#[derive(Debug)]
pub struct WaitingChild {
    pid: i32,
    pidfd: Option<File>,
    control: Option<File>,
    error: File,
    identity: Option<ProcessIdentity>,
    description: Description,
    digest: [u8; 32],
    reaped: Option<i32>,
    canceled: bool,
    possibly_executed: bool,
    _creating_thread: PhantomData<Rc<()>>,
}
impl WaitingChild {
    /// All inputs are already opened/sealed; no lifecycle mutation is possible.
    /// Caller must own waitpid for children it creates (no competing reaper).
    pub fn prepare(
        profile: HostProfile,
        helper: SealedExecImage,
        leaf: SealedExecImage,
        launch: LaunchDescription,
        cwd: &File,
        stdio: [&File; 3],
    ) -> Result<Self> {
        Self::prepare_inner(
            profile,
            helper,
            leaf,
            launch,
            cwd,
            stdio,
            #[cfg(test)]
            FixtureBehavior::Normal,
        )
    }
    fn prepare_inner(
        profile: HostProfile,
        helper: SealedExecImage,
        leaf: SealedExecImage,
        launch: LaunchDescription,
        cwd: &File,
        stdio: [&File; 3],
        #[cfg(test)] fixture: FixtureBehavior,
    ) -> Result<Self> {
        launch.validate()?;
        helper.verify()?;
        leaf.verify()?;
        ensure!(
            Credentials::capture()? == profile.credentials,
            "host profile changed"
        );
        // Re-observe rather than trust a stale qualification.
        ensure!(
            qualify_adopter()? == profile.adopter,
            "orphan adopter changed since host profile qualification"
        );
        ensure!(cwd.metadata()?.is_dir(), "cwd must be an opened directory");
        let parent = ProcessIdentity::capture(unsafe { libc::getpid() })?;
        let thread = ProcessIdentity::capture(unsafe { libc::syscall(libc::SYS_gettid) } as i32)?;
        let thread_stat = File::open(format!("/proc/{}/task/{}/stat", parent.pid, thread.pid))?;
        let mut nonce = [0u8; 32];
        ensure!(
            unsafe { libc::getrandom(nonce.as_mut_ptr().cast(), nonce.len(), 0) } == 32,
            "private channel randomness unavailable"
        );
        let mut stdio_id = [(0, 0, 0); 3];
        for (i, f) in stdio.iter().enumerate() {
            let m = f.metadata()?;
            stdio_id[i] = (m.dev(), m.ino(), m.mode());
        }
        let cm = cwd.metadata()?;
        let description = Description {
            profile: "linux-host-entry-v1".into(),
            parent,
            thread,
            adopter: profile.adopter,
            credentials: profile.credentials,
            helper: helper.identity().clone(),
            leaf: leaf.identity().clone(),
            cwd: (cm.dev(), cm.ino()),
            stdio: stdio_id,
            argv: launch.argv,
            environment: launch.environment,
            nonce,
            deadline_ns: monotonic_ns()?
                .checked_add(launch.startup_timeout.as_nanos().try_into()?)
                .context("deadline overflow")?,
            #[cfg(test)]
            fixture,
        };
        let bytes = serde_json::to_vec(&description)?;
        ensure!(
            bytes.len() as u64 <= MAX_DESCRIPTION,
            "transport description too large"
        );
        let digest = Sha256::digest(&bytes).into();
        let desc = sealed_bytes(&bytes, false)?;
        let (parent_socket, child_socket) = channel()?;
        let (error_read, error_write) = error_pipe()?;
        // Duplicate every mapping above fixed targets before fork, so remapping
        // cannot clobber another source. close_range closes *all* inherited locks.
        let sources = [
            stdio[0].as_raw_fd(),
            stdio[1].as_raw_fd(),
            stdio[2].as_raw_fd(),
            leaf.file.as_raw_fd(),
            desc.as_raw_fd(),
            child_socket.as_raw_fd(),
            error_write.as_raw_fd(),
            helper.file.as_raw_fd(),
            cwd.as_raw_fd(),
        ];
        let mapped: Vec<File> = sources
            .iter()
            .map(|fd| dup_high(*fd))
            .collect::<Result<_>>()?;
        let raw: Vec<RawFd> = mapped.iter().map(AsRawFd::as_raw_fd).collect();
        let helper_argv = [
            c"aida-waiting-child".as_ptr(),
            c"--aida-waiting-child".as_ptr(),
            std::ptr::null(),
        ];
        let env: Vec<CString> = description
            .environment
            .iter()
            .map(|(k, v)| CString::new(format!("{k}={v}")))
            .collect::<std::result::Result<_, _>>()?;
        let mut envp: Vec<_> = env.iter().map(|s| s.as_ptr()).collect();
        envp.push(std::ptr::null());
        // clone3 returns the pidfd atomically with child creation. No numeric
        // PID lookup window and no libc atfork callbacks in the raw child.
        let mut owned_pidfd = -1i32;
        let pid = unsafe { raw_clone(&mut owned_pidfd) };
        ensure!(
            pid >= 0,
            "waiting-child fork failed: {}",
            std::io::Error::last_os_error()
        );
        if pid == 0 {
            // Only async-signal-safe functions/syscalls and stack parsing here.
            unsafe {
                #[cfg(test)]
                if matches!(fixture, FixtureBehavior::BeforePrctl) {
                    libc::raise(libc::SIGSTOP);
                }
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) != 0
                    || libc::getppid() != description.parent.pid
                {
                    libc::_exit(126);
                }
                let mut stat = [0u8; 4096];
                let n = libc::pread(
                    thread_stat.as_raw_fd(),
                    stat.as_mut_ptr().cast(),
                    stat.len(),
                    0,
                );
                if n <= 0 || stat_start(&stat[..n as usize]) != Some(description.thread.start_ticks)
                {
                    libc::_exit(126);
                }
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                    libc::_exit(126);
                }
                for (target, source) in raw.iter().enumerate() {
                    let flags = if target >= 7 { libc::O_CLOEXEC } else { 0 };
                    if libc::dup3(*source, target as i32, flags) < 0 {
                        libc::_exit(126);
                    }
                }
                if libc::fchdir(CWD) != 0 || libc::close(CWD) != 0 {
                    libc::_exit(126);
                }
                if libc::syscall(libc::SYS_close_range, 8u32, u32::MAX, 0) != 0 {
                    libc::_exit(126);
                }
                // Signal masks/ignored dispositions can otherwise survive exec.
                let mut mask: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut mask);
                if libc::sigprocmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut()) != 0 {
                    libc::_exit(126);
                }
                for sig in 1..=64 {
                    if sig == libc::SIGKILL || sig == libc::SIGSTOP {
                        continue;
                    }
                    let mut action: libc::sigaction = std::mem::zeroed();
                    action.sa_sigaction = libc::SIG_DFL;
                    libc::sigemptyset(&mut action.sa_mask);
                    // libc reserves two internal signals on glibc.
                    if libc::sigaction(sig, &action, std::ptr::null_mut()) != 0
                        && sig != 32
                        && sig != 33
                    {
                        libc::_exit(126);
                    }
                }
                libc::syscall(
                    libc::SYS_execveat,
                    HELPER,
                    c"".as_ptr(),
                    helper_argv.as_ptr(),
                    envp.as_ptr(),
                    libc::AT_EMPTY_PATH,
                );
                let errno = *libc::__errno_location();
                libc::write(ERROR, (&errno as *const i32).cast(), 4);
                libc::_exit(126);
            }
        }
        drop(mapped);
        drop(child_socket);
        drop(error_write);
        let mut child = Self {
            pid,
            pidfd: Some(unsafe { File::from_raw_fd(owned_pidfd) }),
            control: Some(parent_socket),
            error: error_read,
            identity: None,
            description,
            digest,
            reaped: None,
            canceled: false,
            possibly_executed: false,
            _creating_thread: PhantomData,
        };
        // Until reaped, this PID belongs to our child. No observed PID ever
        // constructs WaitingChild and no failure path signals a bare PID.
        // The kernel returned this handle as part of the original clone3.
        #[cfg(test)]
        if matches!(fixture, FixtureBehavior::BeforePrctl) {
            return Ok(child);
        }
        let identity = ProcessIdentity::capture(pid)?;
        child.identity = Some(identity.clone());
        let frame = receive(
            child.socket()?,
            child.description.deadline_ns,
            pid,
            child.description.credentials.uid[0],
            child.description.credentials.gid[0],
        )?;
        ensure!(
            frame == child.frame(READY, &identity),
            "invalid waiting-child READY frame"
        );
        let exe = File::open(format!("/proc/{pid}/exe"))
            .context("open owned helper descriptor at READY")?;
        verify_image(&exe, &child.description.helper)?;
        ensure!(
            identity.is_current(),
            "waiting-child identity vanished at READY"
        );
        Ok(child)
    }
    pub fn identity(&self) -> &ProcessIdentity {
        self.identity
            .as_ref()
            .expect("only READY children are publicly returned")
    }
    pub fn deadline_remaining(&self) -> Result<Duration> {
        Ok(Duration::from_nanos(
            self.description.deadline_ns.saturating_sub(monotonic_ns()?),
        ))
    }
    fn socket(&self) -> Result<RawFd> {
        Ok(self.control.as_ref().context("channel closed")?.as_raw_fd())
    }
    fn frame(&self, kind: u8, identity: &ProcessIdentity) -> [u8; FRAME_LEN] {
        frame(kind, &self.description, &self.digest, identity)
    }
    /// Idempotent, bounded and identity-owned. False `reaped` retains uncertainty;
    /// it is never reported as successful cleanup. Call again to settle later.
    pub fn cancel(&mut self) -> Result<Cancellation> {
        self.canceled = true;
        if self.reaped.is_none() {
            if let Some(pidfd) = &self.pidfd {
                let rc = unsafe {
                    libc::syscall(
                        libc::SYS_pidfd_send_signal,
                        pidfd.as_raw_fd(),
                        libc::SIGKILL,
                        std::ptr::null::<libc::siginfo_t>(),
                        0,
                    )
                };
                if rc != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
                    // Do not discard the handle/channel on signaling failure.
                    return Err(std::io::Error::last_os_error())
                        .context("owned child cancellation failed");
                }
            }
            // Unreleased channel EOF is also a refusal; it never releases.
            self.control.take();
            self.settle(SETTLEMENT)?;
        }
        Ok(Cancellation {
            reaped: self.reaped.is_some(),
            possibly_executed: self.possibly_executed,
            wait_status: self.reaped,
        })
    }
    fn settle(&mut self, budget: Duration) -> Result<()> {
        let until = monotonic_ns()?.saturating_add(budget.as_nanos() as u64);
        loop {
            let fd = self
                .pidfd
                .as_ref()
                .context("owned pidfd missing; settlement unknown")?;
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let rc = unsafe {
                libc::waitid(
                    libc::P_PIDFD,
                    fd.as_raw_fd() as u32,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG,
                )
            };
            if rc == 0 && unsafe { info.si_pid() } != 0 {
                let code = unsafe { info.si_status() };
                self.reaped = Some(match info.si_code {
                    libc::CLD_EXITED => code << 8,
                    libc::CLD_KILLED => code,
                    libc::CLD_DUMPED => code | 0x80,
                    _ => bail!("unexpected owned child wait observation"),
                });
                return Ok(());
            }
            if rc < 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return Err(error).context("owned child wait failed; settlement unknown");
            }
            if monotonic_ns()? >= until {
                return Ok(());
            }
            poll_fd(fd.as_raw_fd(), libc::POLLIN, until)?;
        }
    }
    /// An error is a returned syscall observation. EOF alone also means death.
    pub fn exec_error(&self) -> Result<Option<i32>> {
        let mut errno = 0i32;
        let n = unsafe { libc::read(self.error.as_raw_fd(), (&mut errno as *mut i32).cast(), 4) };
        if n == 4 {
            return Ok(Some(errno));
        }
        if n == 0
            || (n == -1 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock)
        {
            return Ok(None);
        }
        bail!("invalid or unreadable exec-error observation")
    }
}
impl Drop for WaitingChild {
    fn drop(&mut self) {
        if self.reaped.is_none() {
            match self.cancel() {
                Ok(outcome) if outcome.reaped => {}
                _ => eprintln!("waiting-child cleanup did not confirm reaping; owned attempt remains unsettled"),
            }
        }
    }
}

fn monotonic_ns() -> Result<u64> {
    let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
    ensure!(
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } == 0,
        "monotonic clock unavailable"
    );
    ensure!(ts.tv_sec >= 0 && ts.tv_nsec >= 0, "invalid monotonic clock");
    (ts.tv_sec as u64)
        .checked_mul(1_000_000_000)
        .and_then(|v| v.checked_add(ts.tv_nsec as u64))
        .context("clock overflow")
}
fn pidfd_open(pid: i32) -> Result<File> {
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
    ensure!(
        fd >= 0,
        "owned pidfd unavailable: {}",
        std::io::Error::last_os_error()
    );
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn channel() -> Result<(File, File)> {
    let mut fds = [-1; 2];
    ensure!(
        unsafe {
            libc::socketpair(
                libc::AF_UNIX,
                libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
                0,
                fds.as_mut_ptr(),
            )
        } == 0,
        "private channel unavailable"
    );
    let files = unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) };
    for fd in fds {
        let yes: i32 = 1;
        ensure!(
            unsafe {
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_PASSCRED,
                    (&yes as *const i32).cast(),
                    4,
                )
            } == 0,
            "channel credentials unavailable"
        );
    }
    Ok(files)
}
fn error_pipe() -> Result<(File, File)> {
    let mut fds = [-1; 2];
    ensure!(
        unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } == 0,
        "exec-error pipe unavailable"
    );
    Ok(unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) })
}
fn poll_fd(fd: RawFd, events: i16, until: u64) -> Result<i16> {
    loop {
        let now = monotonic_ns()?;
        if now >= until {
            return Ok(0);
        }
        let millis = (until - now).div_ceil(1_000_000).min(30_000) as i32;
        let mut p = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        let rc = unsafe { libc::poll(&mut p, 1, millis) };
        if rc >= 0 {
            return Ok(p.revents);
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            bail!("private channel poll failed");
        }
    }
}
fn frame(
    kind: u8,
    d: &Description,
    digest: &[u8; 32],
    identity: &ProcessIdentity,
) -> [u8; FRAME_LEN] {
    let mut b = [0u8; FRAME_LEN];
    b[..4].copy_from_slice(b"AWC1");
    b[4] = kind;
    b[8..40].copy_from_slice(&d.nonce);
    b[40..72].copy_from_slice(digest);
    b[72..76].copy_from_slice(&identity.pid.to_le_bytes());
    b[80..88].copy_from_slice(&identity.start_ticks.to_le_bytes());
    b[88..96].copy_from_slice(&d.deadline_ns.to_le_bytes());
    b
}
fn send(fd: RawFd, b: &[u8]) -> Result<()> {
    // Exactly one nonblocking send; no partial-frame retransmission.
    let n = unsafe {
        libc::send(
            fd,
            b.as_ptr().cast(),
            b.len(),
            libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
        )
    };
    ensure!(
        n == b.len() as isize,
        "private frame delivery failed or uncertain"
    );
    Ok(())
}
fn receive(fd: RawFd, until: u64, pid: i32, uid: u32, gid: u32) -> Result<[u8; FRAME_LEN]> {
    let ready = poll_fd(fd, libc::POLLIN, until)?;
    ensure!(
        ready & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) == 0,
        "original private channel died"
    );
    ensure!(
        ready & libc::POLLIN != 0 && monotonic_ns()? < until,
        "waiting-child startup deadline expired"
    );
    let mut b = [0u8; FRAME_LEN];
    let mut iov = libc::iovec {
        iov_base: b.as_mut_ptr().cast(),
        iov_len: b.len(),
    };
    // Aligned storage for credentials. Reject ancillary rights instead of
    // accidentally accepting/leaking descriptors from an unexpected sender.
    let mut ancillary = [0usize; 32];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = ancillary.as_mut_ptr().cast();
    msg.msg_controllen = std::mem::size_of_val(&ancillary);
    let n = unsafe { libc::recvmsg(fd, &mut msg, libc::MSG_DONTWAIT | libc::MSG_CMSG_CLOEXEC) };
    let mut creds = None;
    let mut unexpected = false;
    unsafe {
        let mut h = libc::CMSG_FIRSTHDR(&msg);
        while !h.is_null() {
            if (*h).cmsg_level == libc::SOL_SOCKET
                && (*h).cmsg_type == libc::SCM_CREDENTIALS
                && (*h).cmsg_len
                    == libc::CMSG_LEN(std::mem::size_of::<libc::ucred>() as u32) as usize
            {
                if creds.is_some() {
                    unexpected = true;
                }
                creds = Some(std::ptr::read_unaligned(
                    libc::CMSG_DATA(h).cast::<libc::ucred>(),
                ));
            } else {
                unexpected = true;
                if (*h).cmsg_level == libc::SOL_SOCKET && (*h).cmsg_type == libc::SCM_RIGHTS {
                    let len = (*h).cmsg_len.saturating_sub(libc::CMSG_LEN(0) as usize) / 4;
                    for i in 0..len {
                        libc::close(std::ptr::read_unaligned(
                            libc::CMSG_DATA(h).cast::<i32>().add(i),
                        ));
                    }
                }
            }
            h = libc::CMSG_NXTHDR(&msg, h);
        }
    }
    ensure!(
        n == FRAME_LEN as isize
            && msg.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) == 0
            && !unexpected,
        "invalid or partial private frame"
    );
    let c = creds.context("missing kernel channel credentials")?;
    ensure!(
        c.pid == pid && c.uid == uid && c.gid == gid,
        "wrong private channel sender identity"
    );
    Ok(b)
}

pub(super) fn bootstrap_exit() -> ! {
    let result = bootstrap();
    if let Err(error) = result {
        eprintln!("waiting-child refused: {error:#}");
    }
    // No normal CLI destructors/telemetry/init, even on malformed invocation.
    unsafe { libc::_exit(126) }
}
fn bootstrap() -> Result<()> {
    super::require_supported_build()?;
    ensure!(
        std::env::args_os().count() == 2,
        "invalid bootstrap invocation"
    );
    // Restore CLOEXEC before any parsing, file access or diagnostic operation.
    for fd in LEAF..=ERROR {
        ensure!(
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == 0,
            "missing private bootstrap descriptor"
        );
    }
    let leaf = unsafe { File::from_raw_fd(LEAF) };
    let description = unsafe { File::from_raw_fd(DESCRIPTION) };
    let control = unsafe { File::from_raw_fd(CONTROL) };
    let error = unsafe { File::from_raw_fd(ERROR) };
    verify_sealed(&description)?;
    let bytes = read_bounded(&description, MAX_DESCRIPTION)?;
    let digest = Sha256::digest(&bytes).into();
    let d: Description = serde_json::from_slice(&bytes)?;
    ensure!(
        d.profile == "linux-host-entry-v1",
        "unknown transport profile"
    );
    let me = ProcessIdentity::capture(unsafe { libc::getpid() })?;
    validate_lifetime(&d)?;
    verify_image(&File::open("/proc/self/exe")?, &d.helper)?;
    verify_image(&leaf, &d.leaf)?;
    let cwd = std::fs::metadata(".")?;
    ensure!((cwd.dev(), cwd.ino()) == d.cwd, "cwd identity mismatch");
    for fd in 0..=2 {
        let mut m: libc::stat = unsafe { std::mem::zeroed() };
        ensure!(
            unsafe { libc::fstat(fd, &mut m) } == 0
                && (m.st_dev, m.st_ino, m.st_mode) == d.stdio[fd as usize],
            "stdio identity mismatch"
        );
    }
    let actual_env: std::collections::BTreeMap<_, _> = std::env::vars().collect();
    ensure!(
        actual_env == d.environment.iter().cloned().collect(),
        "bootstrap environment mismatch"
    );
    #[cfg(test)]
    if matches!(d.fixture, FixtureBehavior::BeforeReady) {
        unsafe {
            libc::raise(libc::SIGSTOP);
        }
    }
    send(control.as_raw_fd(), &frame(READY, &d, &digest, &me))?;
    let incoming = receive(
        control.as_raw_fd(),
        d.deadline_ns,
        d.parent.pid,
        d.credentials.uid[0],
        d.credentials.gid[0],
    )?;
    validate_lifetime(&d)?;
    if incoming == frame(CANCEL, &d, &digest, &me) {
        bail!("waiting child canceled");
    }
    #[cfg(test)]
    if incoming == frame(FIXTURE_RELEASE, &d, &digest, &me) {
        // Only cfg(test) can consume a fixture release. Normal binaries do not
        // contain this branch or the leaf exec implementation at all.
        return fixture_exec(&d, &leaf, &control, &error);
    }
    let _ = error;
    bail!("production release is unavailable")
}
fn validate_lifetime(d: &Description) -> Result<()> {
    ensure!(
        monotonic_ns()? < d.deadline_ns,
        "waiting-child startup deadline expired"
    );
    ensure!(
        unsafe { libc::getppid() } == d.parent.pid
            && d.parent.is_current()
            && d.thread.is_current(),
        "original supervisor or creating thread died"
    );
    ensure!(
        d.adopter.is_current(),
        "qualified orphan adopter identity changed"
    );
    let mut signal = 0;
    ensure!(
        unsafe { libc::prctl(libc::PR_GET_PDEATHSIG, &mut signal, 0, 0, 0) } == 0
            && signal == libc::SIGKILL,
        "parent-death protection missing"
    );
    ensure!(
        unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) } == 1,
        "no-new-privileges protection missing"
    );
    ensure!(
        Credentials::capture()? == d.credentials,
        "bootstrap credentials changed"
    );
    Ok(())
}

#[cfg(test)]
fn fixture_exec(d: &Description, leaf: &File, control: &File, error: &File) -> Result<()> {
    let argv: Vec<_> = d
        .argv
        .iter()
        .map(|v| CString::new(v.as_bytes()))
        .collect::<std::result::Result<_, _>>()?;
    let env: Vec<_> = d
        .environment
        .iter()
        .map(|(k, v)| CString::new(format!("{k}={v}")))
        .collect::<std::result::Result<_, _>>()?;
    let mut ap: Vec<_> = argv.iter().map(|s| s.as_ptr()).collect();
    ap.push(std::ptr::null());
    let mut ep: Vec<_> = env.iter().map(|s| s.as_ptr()).collect();
    ep.push(std::ptr::null());
    #[cfg(test)]
    if matches!(d.fixture, FixtureBehavior::BeforeExec) {
        unsafe {
            libc::raise(libc::SIGSTOP);
        }
    }
    for attempt in 0..=5 {
        validate_lifetime(d)?;
        verify_image(leaf, &d.leaf)?;
        // A queued second frame, dead endpoint or cancellation invalidates the
        // one-shot exchange. No current-authority/store lookup occurs here.
        let mut p = libc::pollfd {
            fd: control.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        ensure!(
            unsafe { libc::poll(&mut p, 1, 0) } == 0,
            "duplicate frame or original channel lost"
        );
        // Hash/loader inspection consumes the original budget too.
        validate_lifetime(d)?;
        let fd = if matches!(d.fixture, FixtureBehavior::ExecFailure) {
            -1
        } else {
            leaf.as_raw_fd()
        };
        unsafe {
            libc::syscall(
                libc::SYS_execveat,
                fd,
                c"".as_ptr(),
                ap.as_ptr(),
                ep.as_ptr(),
                libc::AT_EMPTY_PATH,
            );
        }
        let errno = std::io::Error::last_os_error()
            .raw_os_error()
            .context("missing exec error")?;
        unsafe {
            libc::write(error.as_raw_fd(), (&errno as *const i32).cast(), 4);
        }
        if errno == libc::ETXTBSY && attempt < 5 {
            let until = monotonic_ns()?
                .saturating_add(50_000_000)
                .min(d.deadline_ns);
            poll_fd(control.as_raw_fd(), libc::POLLIN, until)?;
            continue;
        }
        bail!(
            "native descriptor exec syscall failed: {}",
            std::io::Error::from_raw_os_error(errno)
        );
    }
    unreachable!()
}

#[cfg(all(test, target_arch = "x86_64", target_env = "gnu"))]
#[path = "tests.rs"]
mod tests;
