//! AIDA owns opted-in Claude background workers; the vendor daemon is never used.
//! The private socket carries launch data, never argv or a persisted capability.
// trace:TASK-3219 | ai:codex
use super::codex_bg_receipt::ProcId;
use super::*;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::{
    fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    net::UnixStream,
};
use std::path::{Path, PathBuf};
use std::time::Duration;

const FD: i32 = 3;
const LIMIT: usize = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(45);

#[derive(serde::Serialize, serde::Deserialize)]
struct Handoff {
    parent: ProcId,
    binary: PathBuf,
    args: Vec<String>,
    delegate_seats: Vec<String>,
    plan: AgentLaunchPlan,
    context: Option<AgentLaunchContext>,
    description: Option<String>,
    attempt: String,
    #[cfg(test)]
    aida: Option<PathBuf>,
}

fn send<T: serde::Serialize>(socket: &mut UnixStream, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    anyhow::ensure!(bytes.len() <= LIMIT, "background handoff exceeds limit");
    socket.write_all(&(bytes.len() as u32).to_be_bytes())?;
    socket.write_all(&bytes)?;
    Ok(())
}

fn receive<T: serde::de::DeserializeOwned>(socket: &mut UnixStream) -> Result<T> {
    let mut len = [0; 4];
    socket.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    anyhow::ensure!(len <= LIMIT, "background handoff exceeds limit");
    let mut bytes = vec![0; len];
    socket.read_exact(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn attempt_dir(root: &Path, attempt: &str) -> Result<PathBuf> {
    let parsed = uuid::Uuid::parse_str(attempt).context("expected the background attempt UUID")?;
    anyhow::ensure!(
        parsed.to_string() == attempt,
        "expected canonical background attempt UUID"
    );
    Ok(root.join(".aida/claude-background").join(attempt))
}

/// Only private, host-owned, singly linked regular files are readable.
fn open_log(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let m = file.metadata()?;
    anyhow::ensure!(
        m.is_file()
            && m.nlink() == 1
            && m.uid() == unsafe { libc::getuid() }
            && m.mode() & 0o077 == 0,
        "refused unsafe background log"
    );
    Ok(file)
}

fn private_file(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?)
}

pub(crate) fn dispatch(
    binary: &Path,
    config: &AgentLaunchConfig,
    plan: &AgentLaunchPlan,
    context: Option<&AgentLaunchContext>,
    prompt: &[String],
    description: Option<String>,
    take_ownership: impl FnOnce(),
) -> Result<()> {
    let attempt = uuid::Uuid::new_v4().to_string();
    let dir = attempt_dir(&plan.project_root, &attempt)?;
    // This host directory is outside the wrapper's writable profile.
    let base = dir.parent().context("missing background directory")?;
    match std::fs::DirBuilder::new().mode(0o700).create(base) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    let m = std::fs::symlink_metadata(base)?;
    anyhow::ensure!(
        m.is_dir() && m.uid() == unsafe { libc::getuid() } && m.mode() & 0o077 == 0,
        "refused unsafe background directory"
    );
    std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    let output = private_file(&dir.join("output.log"))?;
    let mut args = config.default_args.clone();
    args.extend_from_slice(prompt);
    let init = Handoff {
        parent: ProcId::current().context("launcher process identity unavailable")?,
        binary: binary.into(),
        args,
        delegate_seats: config.delegate_seats.clone(),
        plan: plan.clone(),
        context: context.cloned(),
        description,
        attempt: attempt.clone(),
        #[cfg(test)]
        aida: crate::kernel::test_aida_exe::current(),
    };
    let (mut parent, child) = UnixStream::pair()?;
    parent.set_read_timeout(Some(TIMEOUT))?;
    parent.set_write_timeout(Some(TIMEOUT))?;
    let mut helper = crate::launch::control::control_child()?;
    helper.current_dir(&plan.project_root);
    #[cfg(not(test))]
    helper.args(["internal", "claude-bg-supervise"]);
    #[cfg(test)]
    helper.args([
        "--exact",
        "agent_launch::agent_claude_background::tests::supervisor_child_entry",
        "--ignored",
        "--nocapture",
    ]);
    dispatch_with_helper(
        &mut helper,
        child,
        output,
        &mut parent,
        &init,
        take_ownership,
    )
    .with_context(|| {
        format!(
            "background dispatch refused; inspect {}",
            dir.join("output.log").display()
        )
    })?;
    eprintln!("{}", crate::vendors::supervised_background_hint(&attempt));
    Ok(())
}

fn dispatch_with_helper(
    helper: &mut std::process::Command,
    child: UnixStream,
    output: File,
    parent: &mut UnixStream,
    init: &Handoff,
    take_ownership: impl FnOnce(),
) -> Result<()> {
    crate::launch::control::detach_with_socket(helper, child.as_raw_fd(), FD);
    helper
        .stdin(std::process::Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(output);
    let mut supervisor = helper.spawn_retrying_etxtbsy()?;
    drop(child);
    let mut take_ownership = Some(take_ownership);
    let mut transferred = false;
    let result = (|| {
        send(parent, init)?;
        let ready: String = receive(parent)?;
        anyhow::ensure!(
            ready == "ready",
            "background supervisor refused before release: {ready}"
        );
        // Ownership moves before the supervisor can release the vendor. If the
        // final acknowledgement is lost, retain the admitted worktree and lease.
        take_ownership.take().expect("ownership callback")();
        transferred = true;
        send(parent, &"release")?;
        let accepted: String = receive(parent)?;
        anyhow::ensure!(
            accepted == "accepted",
            "background supervisor did not accept: {accepted}"
        );
        Ok(())
    })();
    if result.is_err() {
        cancel_supervisor(
            &mut supervisor,
            parent,
            transferred,
            take_ownership,
            if transferred {
                Duration::from_secs(2)
            } else {
                TIMEOUT
            },
        )?;
    }
    result
}

// trace:TASK-3219 | ai:codex
fn cancel_supervisor(
    supervisor: &mut std::process::Child,
    parent: &UnixStream,
    transferred: bool,
    mut retain: Option<impl FnOnce()>,
    grace: Duration,
) -> Result<()> {
    let result = (|| {
        // Closing the release socket cancels slow prepare before WorkerSignals
        // exists. The owner can unwind and revoke its grant when prepare returns.
        let _ = parent.shutdown(std::net::Shutdown::Both);
        if transferred {
            // Ready is sent only after WorkerSignals was installed.
            unsafe {
                libc::kill(supervisor.id() as i32, libc::SIGTERM);
            }
        }
        let deadline = std::time::Instant::now() + grace;
        while supervisor.try_wait()?.is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if supervisor.try_wait()?.is_none() {
            // Forced exit cannot prove RAII teardown. Retain the seat for recovery.
            if let Some(retain) = retain.take() {
                retain();
            }
            eprintln!("background supervisor required forced termination; launch resources retained for manual recovery");
            supervisor.kill()?;
        }
        supervisor.wait()?; // Caller cleanup must never race a live owner.
        Ok(())
    })();
    if result.is_err() {
        // A failed wait/kill also leaves ownership uncertain. Do not let the
        // caller remove the worktree underneath a possibly live supervisor.
        if let Some(retain) = retain.take() {
            retain();
        }
    }
    result
}

/// Called with the worker held behind the wrapper's release barrier.
pub(crate) fn before_release(socket: &mut UnixStream) -> Result<()> {
    send(socket, &"ready")?;
    let response: String = receive(socket)?;
    anyhow::ensure!(
        response == "release",
        "dispatcher did not transfer background ownership"
    );
    Ok(())
}

pub(crate) fn supervise_main() -> Result<()> {
    // The inherited socket is the sole launch input; no arbitrary state file,
    // token-bearing argv, full parent environment snapshot or shell command.
    let mut socket = unsafe { UnixStream::from_raw_fd(FD) };
    unsafe {
        libc::fcntl(FD, libc::F_SETFD, libc::FD_CLOEXEC);
    }
    socket.set_read_timeout(Some(TIMEOUT))?;
    socket.set_write_timeout(Some(TIMEOUT))?;
    let init: Handoff = receive(&mut socket)?;
    anyhow::ensure!(
        init.parent.pid == unsafe { libc::getppid() } as u32 && init.parent.is_live(),
        "background launch parent identity changed"
    );
    anyhow::ensure!(
        session::claude_uses_contained_wrapper(&init.plan.launch_cwd)?,
        "background supervisor requires trusted Claude wrapper opt-in"
    );
    #[cfg(test)]
    let _aida = init.aida.clone().map(crate::kernel::test_aida_exe::set);
    let dir = attempt_dir(&init.plan.project_root, &init.attempt)?;
    let me = ProcId::current().context("supervisor identity unavailable")?;
    serde_json::to_writer(
        private_file(&dir.join("supervisor.json"))?,
        &(me, init.parent),
    )?;
    let config = AgentLaunchConfig {
        agent_type: crate::vendors::supervised_background_name(),
        binary: crate::vendors::supervised_background_name(),
        default_args: init.args,
        prompt_style: AgentPromptStyle::Positional,
        contained: true,
        delegate_seats: init.delegate_seats,
    };
    let mut accepted = socket.try_clone()?;
    let result = run_agent_generation_with_io(
        &init.binary,
        &config,
        &init.plan,
        init.context.as_ref(),
        &[],
        init.description,
        false,
        false,
        None,
        None,
        || send(&mut accepted, &"accepted"),
        Some(&mut socket),
    );
    if let Err(e) = &result {
        eprintln!("AIDA background supervisor failed: {e:#}");
    }
    // An explicit terminal result also makes an attach after exit finite.
    writeln!(
        private_file(&dir.join("ended"))?,
        "{:?}",
        result.as_ref().map(|r| r.status.code())
    )?;
    result.map(|_| ())
}

// Mark a background row ended on handshake, release and wait errors too.
pub(crate) struct RegistryEnd {
    pub(crate) root: PathBuf,
    pub(crate) pid: u32,
}
impl Drop for RegistryEnd {
    fn drop(&mut self) {
        if let Err(e) = agent_registry::mark_agent_ended(
            &self.root,
            crate::vendors::supervised_background_name(),
            self.pid,
        ) {
            eprintln!("failed to mark background worker ended: {e:#}");
        }
    }
}

pub(crate) fn logs(attempt: &str, follow: bool) -> Result<()> {
    let root = find_aida_project_root_from(&std::env::current_dir()?)?;
    let root = agent_launch_project_root_from(&root);
    let dir = attempt_dir(&root, attempt)?;
    // Reject linked directory components as well as a linked log leaf.
    for path in [
        dir.parent().context("missing background directory")?,
        dir.as_path(),
    ] {
        let m = std::fs::symlink_metadata(path)?;
        anyhow::ensure!(
            m.is_dir() && m.uid() == unsafe { libc::getuid() } && m.mode() & 0o077 == 0,
            "refused unsafe background directory"
        );
    }
    let mut log = open_log(&dir.join("output.log"))?;
    let mut output = std::io::stdout().lock();
    let mut buf = [0; 16384];
    loop {
        let n = log.read(&mut buf)?;
        if n != 0 {
            output.write_all(&buf[..n])?;
            output.flush()?;
            continue;
        }
        if !follow {
            break;
        }
        if dir.join("ended").exists() {
            // The terminal marker follows worker exit. The writer may have
            // appended between our EOF read and this observation.
            std::io::copy(&mut log, &mut output)?;
            output.flush()?;
            break;
        }
        let (me, _): (ProcId, ProcId) =
            serde_json::from_reader(open_log(&dir.join("supervisor.json"))?)?;
        anyhow::ensure!(
            me.is_live(),
            "background supervisor exited without a terminal result; inspect the log"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    #[test]
    fn supervisor_protocol_refuses_unowned_release_and_oversized_frames() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        send(&mut a, &"wrong").unwrap();
        assert!(before_release(&mut b).is_err());
        assert_eq!(receive::<String>(&mut a).unwrap(), "ready");
        a.write_all(&((LIMIT + 1) as u32).to_be_bytes()).unwrap();
        assert!(receive::<String>(&mut b).is_err());
        let root = tempfile::tempdir().unwrap();
        assert!(attempt_dir(root.path(), "../escape").is_err());
        let leaf = root.path().join("log");
        private_file(&leaf).unwrap().write_all(b"evidence").unwrap();
        assert!(open_log(&leaf).is_ok());
        std::os::unix::fs::symlink(&leaf, root.path().join("link")).unwrap();
        assert!(open_log(&root.path().join("link")).is_err());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn supervisor_cancellation_closes_socket_and_reaps_before_cleanup() {
        let (parent, child) = UnixStream::pair().unwrap();
        let mut helper = std::process::Command::new("/bin/sh");
        helper.args(["-c", "sleep 0.1; read x <&3 || exit 0; exit 1"]);
        crate::launch::control::detach_with_socket(&mut helper, child.as_raw_fd(), FD);
        let mut process = helper.spawn().unwrap();
        drop(child);
        let retained = std::cell::Cell::new(false);
        cancel_supervisor(
            &mut process,
            &parent,
            false,
            Some(|| retained.set(true)),
            Duration::from_secs(2),
        )
        .unwrap();
        assert!(process.wait().unwrap().success());
        assert!(!retained.get(), "graceful unwind permits ordinary cleanup");

        let (parent, child) = UnixStream::pair().unwrap();
        let mut helper = std::process::Command::new("/bin/sleep");
        helper.arg("60");
        crate::launch::control::detach_with_socket(&mut helper, child.as_raw_fd(), FD);
        let mut process = helper.spawn().unwrap();
        drop(child);
        cancel_supervisor(
            &mut process,
            &parent,
            false,
            Some(|| retained.set(true)),
            Duration::from_millis(10),
        )
        .unwrap();
        assert!(
            retained.get(),
            "forced termination retains uncertain resources"
        );
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(process.wait().unwrap().signal(), Some(libc::SIGKILL));
    }

    // A dispatcher process exits after dispatch; the live test remains alive
    // to observe the independently owned wrapper and its grant teardown.
    pub(crate) fn dispatch_via_exiting_process(
        binary: &Path,
        config: &AgentLaunchConfig,
        plan: &AgentLaunchPlan,
    ) -> Result<()> {
        let init = Handoff {
            parent: ProcId::current().unwrap(),
            binary: binary.into(),
            args: config.default_args.clone(),
            delegate_seats: config.delegate_seats.clone(),
            plan: plan.clone(),
            context: None,
            description: None,
            attempt: uuid::Uuid::new_v4().to_string(),
            aida: crate::kernel::test_aida_exe::current(),
        };
        let (mut parent, child) = UnixStream::pair()?;
        parent.set_read_timeout(Some(TIMEOUT))?;
        parent.set_write_timeout(Some(TIMEOUT))?;
        let mut helper = crate::launch::control::control_child()?;
        helper.current_dir(&plan.project_root);
        helper.args([
            "--exact",
            "agent_launch::agent_claude_background::tests::supervisor_dispatcher_entry",
            "--ignored",
            "--nocapture",
        ]);
        let log = tempfile::tempfile()?;
        let mut diagnostics = log.try_clone()?;
        let result = dispatch_with_helper(&mut helper, child, log, &mut parent, &init, || {});
        if let Err(e) = result {
            use std::io::{Seek, SeekFrom};
            diagnostics.seek(SeekFrom::Start(0))?;
            let mut tail = String::new();
            diagnostics.read_to_string(&mut tail)?;
            if let Ok(attempts) =
                std::fs::read_dir(init.plan.project_root.join(".aida/claude-background"))
            {
                for entry in attempts.flatten() {
                    if let Ok(log) = std::fs::read_to_string(entry.path().join("output.log")) {
                        tail.push_str("\nSupervisor output:\n");
                        tail.push_str(&log);
                    }
                }
            }
            anyhow::bail!("{e:#}: dispatcher output:\n{tail}");
        }
        Ok(())
    }

    #[test]
    #[ignore]
    fn supervisor_dispatcher_entry() {
        let mut socket = unsafe { UnixStream::from_raw_fd(FD) };
        unsafe {
            libc::fcntl(FD, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        socket.set_read_timeout(Some(TIMEOUT)).unwrap();
        socket.set_write_timeout(Some(TIMEOUT)).unwrap();
        let init: Handoff = receive(&mut socket).unwrap();
        let _aida = init.aida.clone().map(crate::kernel::test_aida_exe::set);
        let config = AgentLaunchConfig {
            agent_type: crate::vendors::supervised_background_name(),
            binary: crate::vendors::supervised_background_name(),
            default_args: init.args,
            prompt_style: AgentPromptStyle::Positional,
            contained: true,
            delegate_seats: init.delegate_seats,
        };
        dispatch(&init.binary, &config, &init.plan, None, &[], None, || {
            before_release(&mut socket).unwrap()
        })
        .unwrap();
        send(&mut socket, &"accepted").unwrap();
    }

    // Re-exec entrypoint for the actual detached supervisor in the live test.
    #[test]
    #[ignore]
    fn supervisor_child_entry() {
        supervise_main().unwrap();
    }
}
