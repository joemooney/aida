// Isolated kernel witnesses, not source-string assertions.
// trace:TASK-1612 | ai:codex
// trace:BUG-1808 | ai:codex
use super::*;
use std::io::Write;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

// The test executable doubles as the compiled sealed helper. glibc runs
// .init_array after Rust's argv initializer (.init_array.00099), before libtest
// starts threads. This fixture-only entry calls the SAME bootstrap as main_entry.
// No environment release switch exists, including in the test binary.
#[used]
#[link_section = ".init_array.00100"]
static FIXTURE_ENTRY: unsafe extern "C" fn(
    i32,
    *const *const libc::c_char,
    *const *const libc::c_char,
) = early_fixture;
unsafe extern "C" fn early_fixture(
    argc: i32,
    argv: *const *const libc::c_char,
    _env: *const *const libc::c_char,
) {
    if argc == 2 && std::ffi::CStr::from_ptr(*argv.add(1)).to_bytes() == b"--aida-waiting-child" {
        bootstrap_exit();
    }
}

struct FixturePermit(());
impl WaitingChild {
    fn fixture_release(&mut self, _permit: FixturePermit) -> Result<()> {
        ensure!(
            !self.canceled && !self.possibly_executed && self.reaped.is_none(),
            "fixture release already consumed/canceled"
        );
        ensure!(
            ProcessIdentity::capture(unsafe { libc::syscall(libc::SYS_gettid) } as i32)?
                == self.description.thread,
            "fixture must release on the creating thread"
        );
        ensure!(
            monotonic_ns()? < self.description.deadline_ns,
            "fixture deadline expired"
        );
        let b = self.frame(FIXTURE_RELEASE, self.identity());
        // Intent conservatively precedes delivery. ACK/death never clears it.
        self.possibly_executed = true;
        if let Err(e) = send(self.socket()?, &b) {
            let _ = self.cancel();
            return Err(e);
        }
        Ok(())
    }
}

struct Fixture {
    root: tempfile::TempDir,
    home: PathBuf,
    helper: PathBuf,
    leaf: PathBuf,
    helper_digest: [u8; 32],
    leaf_digest: [u8; 32],
    output: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("aida-waiting-fixture-")
            .tempdir_in(std::env::var_os("TMPDIR").unwrap_or_else(|| "/tmp".into()))
            .unwrap();
        let home = root.path().join("home");
        std::fs::create_dir(&home).unwrap();
        std::fs::create_dir(home.join(".aida")).unwrap();
        std::fs::write(home.join(".aida/untouched"), b"unchanged").unwrap();
        let helper = root.path().join("helper");
        std::fs::copy(std::env::current_exe().unwrap(), &helper).unwrap();
        // Debug DWARF is irrelevant to the helper contract and dominates hash
        // time; retain real compiled code/ELF/runtime and seal the stripped bytes.
        assert!(Command::new("strip")
            .arg("--strip-all")
            .arg(&helper)
            .env("HOME", &home)
            .status()
            .unwrap()
            .success());
        let leaf = root.path().join("leaf");
        let source = root.path().join("leaf.c");
        std::fs::write(&source, r#"
#include <unistd.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/prctl.h>
#include <dirent.h>
#include <string.h>
int main(int argc, char **argv) {
  if (argc != 2) return 40;
  int bad = 0;
  for (int i=3; i<4096; i++) if (fcntl(i,F_GETFD) != -1) bad++;
  int sig=0; prctl(PR_GET_PDEATHSIG,&sig);
  FILE *out = fopen(argv[1],"wx"); if(!out) return 41;
  fprintf(out,"native pid=%d uid=%d nnp=%d death=%d fds=%d home=%s\n",getpid(),geteuid(),prctl(PR_GET_NO_NEW_PRIVS,0,0,0,0),sig,bad,getenv("HOME"));
  FILE *status=fopen("/proc/self/status","r"); char line[1024];
  while(fgets(line,sizeof(line),status)) if(!strncmp(line,"Uid:",4)||!strncmp(line,"Gid:",4)||!strncmp(line,"Groups:",7)||!strncmp(line,"Cap",3)) fputs(line,out);
  fclose(status); fclose(out); return bad ? 42 : 0;
}
"#).unwrap();
        let output = Command::new("cc")
            .args(["-O0", "-o"])
            .arg(&leaf)
            .arg(&source)
            .env("HOME", &home)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        static HELPER_DIGEST: OnceLock<[u8; 32]> = OnceLock::new();
        let helper_digest = *HELPER_DIGEST.get_or_init(|| hash(&helper));
        let leaf_digest = hash(&leaf);
        let output = root.path().join("sentinel");
        Self {
            root,
            home,
            helper,
            leaf,
            helper_digest,
            leaf_digest,
            output,
        }
    }
    fn images(&self) -> (SealedExecImage, SealedExecImage) {
        (
            SealedExecImage::prepare(&self.helper, self.helper_digest).unwrap(),
            SealedExecImage::prepare(&self.leaf, self.leaf_digest).unwrap(),
        )
    }
    fn launch(&self, timeout: Duration) -> LaunchDescription {
        LaunchDescription {
            argv: vec![
                "fake-native-leaf".into(),
                self.output.to_str().unwrap().into(),
            ],
            environment: vec![("HOME".into(), self.home.to_str().unwrap().into())],
            startup_timeout: timeout,
        }
    }
    fn start(&self, behavior: FixtureBehavior, timeout: Duration) -> Result<WaitingChild> {
        let (helper, leaf) = self.images();
        self.start_images(helper, leaf, behavior, timeout)
    }
    fn start_images(
        &self,
        helper: SealedExecImage,
        leaf: SealedExecImage,
        behavior: FixtureBehavior,
        timeout: Duration,
    ) -> Result<WaitingChild> {
        let cwd = File::open(self.root.path())?;
        let null = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/null")?;
        WaitingChild::prepare_inner(
            HostProfile::probe()?,
            helper,
            leaf,
            self.launch(timeout),
            &cwd,
            [&null, &null, &null],
            behavior,
        )
    }
    fn unchanged_home(&self) {
        // Complete root inventory: allow only this fixture's declared assets
        // and native sentinel, not merely absence of one known store filename.
        let allowed = [
            "home",
            "helper",
            "leaf",
            "leaf.c",
            "sentinel",
            "helper-link",
            "leaf-link",
            "replacement",
            "replacement.c",
            "owned-lock",
            "script",
            "rpath-leaf",
            "noexec-home",
        ];
        for entry in std::fs::read_dir(self.root.path()).unwrap() {
            let entry = entry.unwrap();
            assert!(
                allowed.contains(&entry.file_name().to_str().unwrap()),
                "unexpected fixture residue: {}",
                entry.path().display()
            );
        }
        assert_eq!(
            std::fs::read(self.home.join(".aida/untouched")).unwrap(),
            b"unchanged"
        );
        assert_eq!(std::fs::read_dir(&self.home).unwrap().count(), 1);
        assert_eq!(
            std::fs::read_dir(self.home.join(".aida")).unwrap().count(),
            1
        );
    }
    fn positive(&self, child: &mut WaitingChild) {
        let pid = child.identity().pid;
        child.fixture_release(FixturePermit(())).unwrap();
        child.settle(Duration::from_secs(10)).unwrap();
        assert_eq!(child.reaped, Some(0));
        let output = std::fs::read_to_string(&self.output).unwrap();
        assert!(
            output.starts_with(&format!("native pid={pid} ")),
            "{output}"
        );
        assert!(output.contains("nnp=1 death=9 fds=0"), "{output}");
        assert!(output.contains(self.home.to_str().unwrap()), "{output}");
        for (label, expected) in [
            ("Uid:", &child.description.credentials.uid),
            ("Gid:", &child.description.credentials.gid),
            ("Groups:", &child.description.credentials.groups),
        ] {
            let line = output.lines().find_map(|l| l.strip_prefix(label)).unwrap();
            let actual: Vec<u32> = line
                .split_whitespace()
                .map(|v| v.parse().unwrap())
                .collect();
            assert_eq!(&actual, expected);
        }
        for (i, label) in ["CapInh:", "CapPrm:", "CapEff:", "CapBnd:", "CapAmb:"]
            .iter()
            .enumerate()
        {
            assert_eq!(
                output
                    .lines()
                    .find_map(|l| l.strip_prefix(label))
                    .unwrap()
                    .trim(),
                child.description.credentials.capabilities[i]
            );
        }
        self.unchanged_home();
    }
}
fn hash(path: &Path) -> [u8; 32] {
    Sha256::digest(std::fs::read(path).unwrap()).into()
}

#[test]
fn sealed_helper_and_leaf_survive_symlink_and_in_place_replacement() {
    let f = Fixture::new();
    let replacement = f.root.path().join("replacement");
    let replacement_source = f.root.path().join("replacement.c");
    std::fs::write(&replacement_source, "#include <stdio.h>\nint main(int n,char **v){if(n>1){FILE *f=fopen(v[1],\"w\");if(f){fputs(\"replacement\",f);fclose(f);}}return 77;}\n").unwrap();
    assert!(Command::new("cc")
        .arg("-o")
        .arg(&replacement)
        .arg(&replacement_source)
        .env("HOME", &f.home)
        .status()
        .unwrap()
        .success());
    let replacement_bytes = std::fs::read(&replacement).unwrap();
    let helper_link = f.root.path().join("helper-link");
    let leaf_link = f.root.path().join("leaf-link");
    symlink(&f.helper, &helper_link).unwrap();
    symlink(&f.leaf, &leaf_link).unwrap();
    let helper = SealedExecImage::prepare(&helper_link, f.helper_digest).unwrap();
    let leaf = SealedExecImage::prepare(&leaf_link, f.leaf_digest).unwrap();
    for link in [&helper_link, &leaf_link] {
        std::fs::remove_file(link).unwrap();
        symlink(&replacement, link).unwrap();
    }
    // Same original inode changed after sealing, before helper exec.
    std::fs::write(&f.helper, &replacement_bytes).unwrap();
    let mut child = f
        .start_images(
            helper,
            leaf,
            FixtureBehavior::Normal,
            Duration::from_secs(10),
        )
        .unwrap();
    // Leaf original inode changed after actual READY/binding.
    std::fs::write(&f.leaf, &replacement_bytes).unwrap();
    f.positive(&mut child);
    assert!(SealedExecImage::prepare(&f.leaf, f.leaf_digest).is_err());
    assert!(SealedExecImage::prepare(&f.helper, f.helper_digest).is_err());
}

#[test]
fn image_seals_descriptor_policy_and_native_positive() {
    let f = Fixture::new();
    let (helper, leaf) = f.images();
    let writer = std::fs::OpenOptions::new()
        .write(true)
        .open(format!("/proc/self/fd/{}", leaf.file.as_raw_fd()))
        .unwrap();
    assert_eq!(
        unsafe { libc::pwrite(writer.as_raw_fd(), b"X".as_ptr().cast(), 1, 0) },
        -1
    );
    assert_eq!(unsafe { libc::ftruncate(writer.as_raw_fd(), 0) }, -1);
    assert_eq!(
        unsafe {
            libc::fcntl(
                writer.as_raw_fd(),
                libc::F_ADD_SEALS,
                libc::F_SEAL_FUTURE_WRITE,
            )
        },
        -1
    );
    let mapping = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            4096,
            libc::PROT_WRITE,
            libc::MAP_SHARED,
            writer.as_raw_fd(),
            0,
        )
    };
    assert_eq!(mapping, libc::MAP_FAILED);
    // Exec bits themselves are immutable too; credential metadata is checked
    // independently of byte seals. A set-id change with unchanged execute bits
    // is rejected on the opened image before bootstrap creation.
    assert_eq!(unsafe { libc::fchmod(leaf.file.as_raw_fd(), 0o600) }, -1);
    assert_eq!(unsafe { libc::fchmod(leaf.file.as_raw_fd(), 0o4700) }, 0);
    assert!(leaf.verify().is_err());
    assert_eq!(unsafe { libc::fchmod(leaf.file.as_raw_fd(), 0o700) }, 0);
    drop(writer);
    let lock = File::create(f.root.path().join("owned-lock")).unwrap();
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    // Deliberately non-CLOEXEC inherited FD: helper must not keep the lock.
    unsafe {
        libc::fcntl(lock.as_raw_fd(), libc::F_SETFD, 0);
    }
    let mut child = f
        .start_images(
            helper,
            leaf,
            FixtureBehavior::Normal,
            Duration::from_secs(10),
        )
        .unwrap();
    let inventory: Vec<i32> = std::fs::read_dir(format!("/proc/{}/fd", child.pid))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_str().unwrap().parse().unwrap())
        .collect();
    assert_eq!(
        inventory
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        (0..=6).collect()
    );
    for fd in 3..=6 {
        let info = std::fs::read_to_string(format!("/proc/{}/fdinfo/{fd}", child.pid)).unwrap();
        let flags = info
            .lines()
            .find_map(|l| l.strip_prefix("flags:\t"))
            .unwrap();
        assert_ne!(
            u32::from_str_radix(flags, 8).unwrap() & libc::O_CLOEXEC as u32,
            0
        );
    }
    drop(lock);
    let second = File::open(f.root.path().join("owned-lock")).unwrap();
    assert_eq!(
        unsafe { libc::flock(second.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0,
        "helper inherited lock"
    );
    f.positive(&mut child);
}

#[test]
fn unsupported_images_and_credentials_refuse_without_lifecycle_effects() {
    let f = Fixture::new();
    let script = f.root.path().join("script");
    std::fs::write(&script, b"#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(SealedExecImage::prepare(&script, hash(&script))
        .unwrap_err()
        .to_string()
        .contains("native 64-bit ELF"));
    std::fs::set_permissions(&f.leaf, std::fs::Permissions::from_mode(0o4755)).unwrap();
    assert!(format!(
        "{:#}",
        SealedExecImage::prepare(&f.leaf, f.leaf_digest).unwrap_err()
    )
    .contains("set-id"));
    std::fs::set_permissions(&f.leaf, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(SealedExecImage::prepare(&f.leaf, f.leaf_digest).is_err());
    std::fs::set_permissions(&f.leaf, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut unknown = std::fs::read(&f.leaf).unwrap();
    let loader = b"/lib64/ld-linux-x86-64.so.2";
    if let Some(i) = unknown.windows(loader.len()).position(|s| s == loader) {
        unknown[i + 1] = b'X';
        std::fs::write(&f.leaf, unknown).unwrap();
        assert!(SealedExecImage::prepare(&f.leaf, hash(&f.leaf))
            .unwrap_err()
            .to_string()
            .contains("loader"));
    } else {
        panic!("fixture must have the expected system loader for this witness");
    }
    let mut launch = f.launch(Duration::from_secs(1));
    launch
        .environment
        .push(("LD_PRELOAD".into(), "/tmp/unbound.so".into()));
    assert!(launch.validate().is_err());
    launch.environment.clear();
    launch.argv.push("bad\0arg".into());
    assert!(launch.validate().is_err());
    assert!(ProcessIdentity::capture(i32::MAX).is_err());
    assert!(!f.output.exists());
    f.unchanged_home();
}

#[test]
fn wrong_partial_stale_duplicate_frames_and_deadline_never_release() {
    for case in 0..5 {
        let f = Fixture::new();
        let mut child = f
            .start(FixtureBehavior::Normal, Duration::from_secs(10))
            .unwrap();
        let mut b = child.frame(FIXTURE_RELEASE, child.identity());
        match case {
            0 => b[8] ^= 1,
            1 => b[80] ^= 1,
            2 => b[72..76].copy_from_slice(&unsafe { libc::getpid() }.to_le_bytes()),
            3 => b[88..96].copy_from_slice(&0u64.to_le_bytes()),
            _ => {}
        }
        send(
            child.socket().unwrap(),
            if case == 4 { &b[..20] } else { &b },
        )
        .unwrap();
        child.settle(Duration::from_secs(2)).unwrap();
        assert!(child.reaped.is_some());
        assert!(!f.output.exists());
        f.unchanged_home();
    }
    let f = Fixture::new();
    let mut child = f
        .start(FixtureBehavior::BeforeExec, Duration::from_secs(10))
        .unwrap();
    child.fixture_release(FixturePermit(())).unwrap();
    wait_stopped(child.pid);
    let b = child.frame(FIXTURE_RELEASE, child.identity());
    send(child.socket().unwrap(), &b).unwrap();
    signal(&child, libc::SIGCONT);
    child.settle(Duration::from_secs(2)).unwrap();
    assert!(!f.output.exists());
    assert!(child.possibly_executed); // conservative even with controlled rejection
    let f = Fixture::new();
    let before = std::time::Instant::now();
    assert!(f
        .start(FixtureBehavior::BeforeReady, Duration::from_millis(100))
        .is_err());
    assert!(before.elapsed() < Duration::from_secs(10));
    assert!(!f.output.exists());
    f.unchanged_home();
}

#[test]
fn cancel_drop_and_exec_failure_keep_owned_identity_and_uncertainty() {
    let f = Fixture::new();
    let mut child = f
        .start(FixtureBehavior::Normal, Duration::from_secs(10))
        .unwrap();
    let pid = child.pid;
    let outcome = child.cancel().unwrap();
    assert!(outcome.reaped && !outcome.possibly_executed);
    assert_eq!(outcome, child.cancel().unwrap());
    assert!(child.fixture_release(FixturePermit(())).is_err());
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
    let f = Fixture::new();
    let child = f
        .start(FixtureBehavior::Normal, Duration::from_secs(10))
        .unwrap();
    let pid = child.pid;
    drop(child);
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
    assert!(!f.output.exists());
    let f = Fixture::new();
    let mut child = f
        .start(FixtureBehavior::ExecFailure, Duration::from_secs(10))
        .unwrap();
    child.fixture_release(FixturePermit(())).unwrap();
    child.settle(Duration::from_secs(2)).unwrap();
    assert_eq!(child.exec_error().unwrap(), Some(libc::EBADF));
    assert!(child.possibly_executed);
    assert!(child.fixture_release(FixturePermit(())).is_err());
    let outcome = child.cancel().unwrap();
    assert!(outcome.reaped && outcome.possibly_executed);
    assert!(!f.output.exists());
    f.unchanged_home();
}

fn signal(child: &WaitingChild, sig: i32) {
    assert_eq!(
        unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                child.pidfd.as_ref().unwrap().as_raw_fd(),
                sig,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        },
        0
    );
}
fn wait_stopped(pid: i32) {
    // Bounded foreground test wait, exclusively owned child. waitid WNOWAIT
    // preserves wait ownership; never model-side process discovery/polling.
    let until = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as u32,
                &mut info,
                libc::WSTOPPED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        assert_eq!(rc, 0);
        if unsafe { info.si_pid() } == pid {
            return;
        }
        assert!(
            std::time::Instant::now() < until,
            "owned fixture did not reach barrier"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

// Each lifetime case runs under a dedicated subprocess subreaper. No global
// test-runner reaper/credentials/environment are changed, and no unrelated
// process can be adopted or signaled by these fixtures.
#[test]
fn parent_creating_thread_pre_prctl_and_orphan_reaping() {
    for case in [
        "parent-ready",
        "parent-pre-prctl",
        "parent-before-exec",
        "thread-ready",
        "thread-pre-prctl",
    ] {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("case"), case).unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "launch_transport::linux::tests::lifetime_worker",
                "--ignored",
                "--nocapture",
            ])
            .env_clear()
            .env("HOME", root.path())
            .env("TMPDIR", root.path())
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{case}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(std::fs::read(root.path().join("reaped")).unwrap(), b"yes");
    }
}

#[test]
#[ignore = "subprocess-only lifetime fixture"]
fn lifetime_worker() {
    let home = crate::home::home_dir().expect("isolated fixture HOME");
    let case = std::fs::read_to_string(home.join("case")).unwrap();
    assert_eq!(
        unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) },
        0
    );
    let mut supervisor = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "launch_transport::linux::tests::supervisor_worker",
            "--ignored",
            "--nocapture",
        ])
        .env_clear()
        .env("HOME", &home)
        .env("TMPDIR", &home)
        .env("PATH", "/usr/bin:/bin")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .unwrap();
    let until = std::time::Instant::now() + Duration::from_secs(15);
    let identity: ProcessIdentity = loop {
        if let Ok(b) = std::fs::read(home.join("owned-child")) {
            if let Ok(id) = serde_json::from_slice(&b) {
                break id;
            }
        }
        assert!(
            std::time::Instant::now() < until,
            "supervisor did not publish fixture child"
        );
        assert!(
            supervisor.try_wait().unwrap().is_none(),
            "fixture supervisor exited early"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(identity.is_current());
    let pidfd = pidfd_open(identity.pid).unwrap();
    assert!(identity.is_current());
    if case.starts_with("parent") {
        supervisor.kill().unwrap();
        supervisor.wait().unwrap();
    } else {
        // Returning the creating thread, while its process remains alive, must
        // kill the bootstrap too. The main supervisor stays on stdin afterward.
        supervisor
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"thread-exit\n")
            .unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(10);
        while !home.join("thread-exited").exists() {
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(supervisor.try_wait().unwrap().is_none());
    }
    if case.ends_with("pre-prctl") {
        // No death signal was armed before the barrier. Continuing the same
        // owned pidfd exercises the post-prctl thread/start/reparent check.
        assert_eq!(
            unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    pidfd.as_raw_fd(),
                    libc::SIGCONT,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                )
            },
            0
        );
    }
    let until = monotonic_ns().unwrap() + 3_000_000_000;
    assert_ne!(
        poll_fd(pidfd.as_raw_fd(), libc::POLLIN, until).unwrap() & libc::POLLIN,
        0,
        "child survived creator death"
    );
    if case.starts_with("thread") {
        // Only after proving death with the process still alive do we terminate
        // the supervisor; then this already-established subreaper adopts it.
        supervisor.kill().unwrap();
        supervisor.wait().unwrap();
    }
    let mut status = 0;
    assert_eq!(
        unsafe { libc::waitpid(identity.pid, &mut status, libc::WNOHANG) },
        identity.pid,
        "declared fixture adopter did not reap"
    );
    assert!(libc::WIFSIGNALED(status) || libc::WEXITSTATUS(status) == 126);
    assert!(!Path::new(&format!("/proc/{}", identity.pid)).exists());
    let sentinel = std::fs::read_to_string(home.join("sentinel-path")).unwrap();
    assert!(
        !Path::new(&sentinel).exists(),
        "controlled pre-exec death ran the native leaf"
    );
    std::fs::write(home.join("reaped"), b"yes").unwrap();
}

#[test]
#[ignore = "subprocess-only creating supervisor"]
fn supervisor_worker() {
    let home = crate::home::home_dir().expect("isolated fixture HOME");
    let case = std::fs::read_to_string(home.join("case")).unwrap();
    let thread_home = home.clone();
    let is_thread = case.starts_with("thread");
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let f = Fixture::new();
        let behavior = if case.ends_with("pre-prctl") {
            FixtureBehavior::BeforePrctl
        } else if case.ends_with("before-exec") {
            FixtureBehavior::BeforeExec
        } else {
            FixtureBehavior::Normal
        };
        let (helper, leaf) = f.images();
        let mut profile = HostProfile::probe().unwrap();
        // This is the preexisting outer subreaper, not the doomed supervisor.
        profile.adopter = ProcessIdentity::capture(unsafe { libc::getppid() }).unwrap();
        let null = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/null")
            .unwrap();
        let cwd = File::open(f.root.path()).unwrap();
        let mut child = WaitingChild::prepare_inner(
            profile,
            helper,
            leaf,
            f.launch(Duration::from_secs(10)),
            &cwd,
            [&null, &null, &null],
            behavior,
        )
        .unwrap();
        if matches!(behavior, FixtureBehavior::BeforeExec) {
            child.fixture_release(FixturePermit(())).unwrap();
            wait_stopped(child.pid);
        }
        if matches!(behavior, FixtureBehavior::BeforePrctl) {
            wait_stopped(child.pid);
        }
        let id = ProcessIdentity::capture(child.pid).unwrap();
        std::fs::write(
            thread_home.join("sentinel-path"),
            f.output.to_str().unwrap(),
        )
        .unwrap();
        std::fs::write(
            thread_home.join("owned-child"),
            serde_json::to_vec(&id).unwrap(),
        )
        .unwrap();
        rx.recv_timeout(Duration::from_secs(12)).unwrap();
        // Deliberately model abrupt creating-thread loss, bypassing RAII. The
        // test supervisor is isolated and all leaked FDs die with it afterward.
        std::mem::forget(child);
        std::mem::forget(f);
    });
    let mut byte = [0u8; 1];
    use std::io::Read;
    std::io::stdin().read_exact(&mut byte).unwrap();
    assert!(is_thread);
    tx.send(()).unwrap();
    worker.join().unwrap();
    std::fs::write(home.join("thread-exited"), b"yes").unwrap();
    // Stay alive so the outer witness distinguishes thread death from TGID death.
    let mut all = Vec::new();
    std::io::stdin().read_to_end(&mut all).unwrap();
}

#[test]
fn kernel_sender_credentials_reject_peer_and_cancel_never_targets_peer() {
    let f = Fixture::new();
    let mut child = f
        .start(FixtureBehavior::Normal, Duration::from_secs(10))
        .unwrap();
    let socket = child.socket().unwrap();
    let b = child.frame(FIXTURE_RELEASE, child.identity());
    let (read, write) = error_pipe().unwrap();
    let peer = unsafe { libc::fork() };
    assert!(peer >= 0);
    if peer == 0 {
        // Fork child uses raw syscalls only. It has the original endpoint and
        // exact bytes, but SCM_CREDENTIALS exposes its wrong actual PID.
        unsafe {
            libc::close(write.as_raw_fd());
            libc::send(socket, b.as_ptr().cast(), b.len(), libc::MSG_NOSIGNAL);
            let mut p = libc::pollfd {
                fd: read.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            libc::poll(&mut p, 1, 3000);
            libc::_exit(0);
        }
    }
    drop(read);
    let peer_identity = ProcessIdentity::capture(peer).unwrap();
    let peer_pidfd = pidfd_open(peer).unwrap();
    child.settle(Duration::from_secs(2)).unwrap();
    assert!(child.reaped.is_some());
    assert!(!f.output.exists());
    // A corrupted observation cannot retarget the owned kernel handle.
    child.identity = Some(peer_identity.clone());
    child.cancel().unwrap();
    assert!(peer_identity.is_current());
    assert_eq!(
        unsafe { libc::write(write.as_raw_fd(), b"x".as_ptr().cast(), 1) },
        1
    );
    assert_ne!(
        poll_fd(
            peer_pidfd.as_raw_fd(),
            libc::POLLIN,
            monotonic_ns().unwrap() + 3_000_000_000
        )
        .unwrap()
            & libc::POLLIN,
        0
    );
    let mut status = 0;
    assert_eq!(unsafe { libc::waitpid(peer, &mut status, 0) }, peer);
    assert_eq!(status, 0);
    f.unchanged_home();
}

#[test]
fn startup_refusal_has_no_cli_or_home_initialization() {
    let root = tempfile::tempdir().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .arg("--aida-waiting-child")
        .env_clear()
        .env("HOME", root.path())
        .current_dir(root.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(126));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("missing private bootstrap descriptor")
    );
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn returned_etxtbsy_retries_only_same_sealed_image_pid_and_deadline() {
    let f = Fixture::new();
    let (helper, leaf) = f.images();
    // F_SEAL_WRITE forbids writes, but a separately opened writer still holds
    // inode write access and makes the real execveat syscall return ETXTBSY.
    let writer = std::fs::OpenOptions::new()
        .write(true)
        .open(format!("/proc/self/fd/{}", leaf.file.as_raw_fd()))
        .unwrap();
    let mut child = f
        .start_images(
            helper,
            leaf,
            FixtureBehavior::BeforeExec,
            Duration::from_secs(10),
        )
        .unwrap();
    let pid = child.pid;
    let deadline = child.description.deadline_ns;
    child.fixture_release(FixturePermit(())).unwrap();
    wait_stopped(pid);
    signal(&child, libc::SIGCONT);
    // First returned error is real-kernel evidence, not timing alone.
    assert_ne!(
        poll_fd(
            child.error.as_raw_fd(),
            libc::POLLIN,
            monotonic_ns().unwrap() + 2_000_000_000
        )
        .unwrap()
            & libc::POLLIN,
        0
    );
    assert_eq!(child.exec_error().unwrap(), Some(libc::ETXTBSY));
    drop(writer);
    child.settle(Duration::from_secs(2)).unwrap();
    assert_eq!(child.reaped, Some(0));
    assert!(std::fs::read_to_string(&f.output)
        .unwrap()
        .starts_with(&format!("native pid={pid} ")));
    assert_eq!(child.description.deadline_ns, deadline);
    assert!(child.fixture_release(FixturePermit(())).is_err());
    f.unchanged_home();
}

#[test]
fn opened_noexec_mount_and_runtime_search_path_are_rejected() {
    let f = Fixture::new();
    let native = f.root.path().join("rpath-leaf");
    assert!(Command::new("cc")
        .args(["-Wl,-rpath,/tmp/unbound-runtime", "-o"])
        .arg(&native)
        .arg(f.root.path().join("leaf.c"))
        .env("HOME", &f.home)
        .status()
        .unwrap()
        .success());
    let error = SealedExecImage::prepare(&native, hash(&native)).unwrap_err();
    assert!(error.to_string().contains("runtime path"), "{error:#}");
    // Mount only in a fresh user+mount namespace. This host's /dev/shm is
    // executable, so assuming a default mount flag would be a vacuous witness.
    let noexec_home = f.root.path().join("noexec-home");
    std::fs::create_dir(&noexec_home).unwrap();
    std::fs::copy(&f.leaf, noexec_home.join("source")).unwrap();
    let output = Command::new("unshare")
        .args([
            "--user",
            "--map-root-user",
            "--mount",
            "--propagation",
            "private",
            "--fork",
        ])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "launch_transport::linux::tests::noexec_worker",
            "--ignored",
            "--nocapture",
        ])
        .env_clear()
        .env("HOME", &noexec_home)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "isolated noexec witness failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!f.output.exists());
    f.unchanged_home();
}

#[test]
#[ignore = "subprocess-only private noexec mount fixture"]
fn noexec_worker() {
    let home = crate::home::home_dir().expect("isolated fixture HOME");
    let mount = home.join("mount");
    std::fs::create_dir(&mount).unwrap();
    let path = CString::new(mount.to_str().unwrap()).unwrap();
    assert_eq!(
        unsafe {
            libc::mount(
                c"tmpfs".as_ptr(),
                path.as_ptr(),
                c"tmpfs".as_ptr(),
                libc::MS_NOEXEC | libc::MS_NODEV | libc::MS_NOSUID,
                std::ptr::null(),
            )
        },
        0
    );
    let selected = mount.join("native");
    std::fs::copy(home.join("source"), &selected).unwrap();
    let error = SealedExecImage::prepare(&selected, hash(&selected)).unwrap_err();
    assert!(format!("{error:#}").contains("noexec mount"), "{error:#}");
    assert_eq!(unsafe { libc::umount2(path.as_ptr(), 0) }, 0);
}
