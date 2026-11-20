use super::*;

/// Arguments of the internal `aida pr post-ship-cleanup` worker.
// trace:BUG-2384 | ai:claude
pub(crate) struct PostShipCleanupArgs<'a> {
    pub(crate) pr: Option<u64>,
    pub(crate) lease: Option<&'a str>,
    pub(crate) merge_lock: Option<&'a std::path::Path>,
    pub(crate) sweep: bool,
}

// trace:BUG-2384 | ai:codex
pub(super) fn post_ship_cleanup_worker(args: PostShipCleanupArgs<'_>) -> Result<()> {
    let main = main_worktree_root_from(&find_project_root()?);
    // Record, before any cleanup work, whether the ship's merge lease is
    // still on disk: cleanup must never run while it serializes merges.
    // trace:BUG-2384 | ai:claude
    if let Some(lock) = args.merge_lock {
        eprintln!(
            "  cleanup: merge lease {} held: {}",
            lock.display(),
            if lock.exists() { "yes" } else { "no" }
        );
    }
    // Ship cleanup's seat-clone scans are capped; other probe callers are not.
    // trace:BUG-2384 | ai:claude
    crate::seat_clone::scope_probe_budget_to_ship_cleanup();
    // Multiple ships may now finish their merge sections while a cleanup is
    // still active. Serialize their workers independently of the merge lease;
    // never let two post-ship sweeps retire/remove the same tree concurrently.
    // The permanent sidecar is not unlinked on release.
    use fs2::FileExt;
    let cleanup_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(main.join(".aida/post-ship-cleanup.lock"))?;
    eprintln!("  cleanup: waiting for the cleanup lock (merges remain unblocked)");
    cleanup_lock.lock_exclusive()?;
    eprintln!("  cleanup: acquired the cleanup lock");
    let mut end_failure = None;
    if let Some(lease) = args.lease {
        let status = std::process::Command::new(pr_ship_post_merge_aida_exe())
            .current_dir(&main)
            .env(
                crate::seat_clone::SHIP_CLEANUP_PROBE_ENV,
                crate::seat_clone::SHIP_CLEANUP_PROBE_BUDGET
                    .as_secs()
                    .to_string(),
            )
            .args(["session", "end", lease, "--yes", "--skip-ci"])
            .status_retrying_etxtbsy();
        // The worker owns the final EndLease ledger entry: it may finish long
        // after the ship invocation that launched it has exited.
        // trace:BUG-2384 | ai:claude
        use pr_ship::{ShipStep, StepOutcome};
        let outcome = match status {
            Ok(status) if status.success() => {
                eprintln!("  cleanup: ended lease {lease}");
                StepOutcome::Ok
            }
            Ok(status) => {
                let failure = format!(
                    "session end failed ({status}); run `aida session end {lease} --yes --skip-ci` to retry"
                );
                eprintln!("  {failure}");
                end_failure = Some(failure.clone());
                StepOutcome::Failed(failure)
            }
            Err(error) => {
                let failure = format!("could not invoke `aida session end {lease}`: {error:#}");
                eprintln!("  {failure}");
                end_failure = Some(failure.clone());
                StepOutcome::Failed(failure)
            }
        };
        log_ship_activity(&main, args.pr, &ShipStep::EndLease, &outcome);
    }
    // A process boundary covers the entire sweep, including synchronous
    // filesystem reads and probe setup outside the cooperative scan deadline.
    // Keep the cleanup lock in this parent until the sweep has stopped.
    // trace:BUG-2535 | ai:codex
    if args.sweep {
        let mut cmd = std::process::Command::new(pr_ship_post_merge_aida_exe());
        cmd.current_dir(&main).args(["pr", "post-ship-sweep"]);
        if let Err(error) =
            run_post_ship_sweep_bounded(cmd, doctor_cmd::POST_SHIP_GC_BUDGET, cleanup_lock)
        {
            eprintln!(
                "  post-merge cleanup deferred: {error:#}; run `aida doctor` to inspect and retry"
            );
        }
    }
    if let Some(failure) = end_failure {
        anyhow::bail!(failure);
    }
    Ok(())
}

// The inherited open file description retains the flock even if the supervisor
// exits while SIGKILL is pending in uninterruptible I/O.
// trace:BUG-2535 | ai:codex
const SWEEP_LOCK_FD: std::os::fd::RawFd = 198;

// trace:BUG-2535 | ai:codex
pub(super) fn verify_post_ship_sweep_handoff(
    inputs: &aida_core::exec_context::Environment,
) -> Result<std::fs::File> {
    let main = main_worktree_root_from(&find_project_root()?);
    verify_sweep_lock(&main.join(".aida/post-ship-cleanup.lock"), inputs)
}

// trace:BUG-2535 | ai:codex
pub(super) fn verify_sweep_lock(
    path: &std::path::Path,
    inputs: &aida_core::exec_context::Environment,
) -> Result<std::fs::File> {
    use fs2::FileExt;
    use std::os::{fd::FromRawFd, unix::fs::MetadataExt};
    anyhow::ensure!(
        inputs.var("AIDA_SWEEP_PARENT").ok()
            == Some(unsafe { libc::getppid() }.to_string().as_str()),
        "post-ship-sweep requires a supervisor lock handoff"
    );
    let fd = unsafe { libc::fcntl(SWEEP_LOCK_FD, libc::F_DUPFD_CLOEXEC, 3) };
    anyhow::ensure!(fd >= 0, "missing inherited cleanup lock");
    let inherited = unsafe { std::fs::File::from_raw_fd(fd) };
    let independent = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    let actual = inherited.metadata()?;
    let expected = independent.metadata()?;
    anyhow::ensure!(
        actual.is_file() && actual.dev() == expected.dev() && actual.ino() == expected.ino(),
        "cleanup lock identity mismatch"
    );
    // A new description must conflict while the inherited description must
    // already own the exclusive lock. This also refuses an unlocked fd.
    anyhow::ensure!(
        matches!(independent.try_lock_exclusive(), Err(e)
        if e.kind() == std::io::ErrorKind::WouldBlock),
        "cleanup lock is not held"
    );
    inherited
        .try_lock_exclusive()
        .context("cleanup lock handoff does not own the lock")?;
    // Only the sweep itself needs the descriptor, not its exec descendants.
    unsafe {
        libc::close(SWEEP_LOCK_FD);
    }
    Ok(inherited)
}

// Bound the caller's termination wait. A separate reaper owns both resources
// until wait confirms exit; the sweep's inherited descriptor additionally keeps
// serialization intact if this supervisor process exits first.
// trace:BUG-2535 | ai:codex
pub(super) fn reap_sweep_bounded(
    mut child: std::process::Child,
    lock: std::fs::File,
    budget: std::time::Duration,
    mut poll: impl FnMut(&mut std::process::Child) -> std::io::Result<Option<std::process::ExitStatus>>,
) {
    let started = std::time::Instant::now();
    loop {
        if matches!(poll(&mut child), Ok(Some(_))) {
            return;
        }
        if started.elapsed() >= budget {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    std::thread::spawn(move || {
        let _lock = lock;
        // Never release ownership on an observation error.
        loop {
            if child.wait().is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    });
}

// trace:BUG-2535 | ai:codex
pub(super) fn run_post_ship_sweep() {
    crate::seat_clone::scope_probe_budget_to_ship_cleanup();
    run_post_merge_worktree_gc(
        true,
        false,
        || {
            crate::session_reap::run_session_reap(crate::session_reap::ReapOptions {
                dry_run: false,
                yes: true,
                json: false,
                quiet_when_empty: true,
                release_unknown_owner: false,
                release_protected: false,
            })
        },
        worktree_gc::run_merged_agent_worktree_gc_quiet,
        &mut std::io::stderr(),
    );
}

/// Inherit the terminal and progress streams while enforcing one wall-clock
/// ceiling over reap, discovery, clone probes and pruning together.
// trace:BUG-2535 | ai:codex
pub(super) fn run_post_ship_sweep_bounded(
    mut cmd: std::process::Command,
    budget: std::time::Duration,
    cleanup_lock: std::fs::File,
) -> Result<()> {
    use std::os::{fd::AsRawFd, unix::process::CommandExt};
    let lock_fd = cleanup_lock.as_raw_fd();
    cmd.env("AIDA_SWEEP_PARENT", std::process::id().to_string());
    unsafe {
        cmd.pre_exec(move || {
            if libc::dup2(lock_fd, SWEEP_LOCK_FD) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::fcntl(SWEEP_LOCK_FD, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.process_group(0);
    let mut forwarding = crate::forwarded_signals::Forwarding::install()?;
    let started = std::time::Instant::now();
    let mut child = cmd.spawn_retrying_etxtbsy()?;
    loop {
        let signal = forwarding.take();
        let result = if signal.is_some() {
            Ok(None)
        } else {
            child.try_wait()
        };
        match result {
            Ok(Some(status)) => {
                anyhow::ensure!(status.success(), "cleanup sweep exited with {status}");
                return Ok(());
            }
            Ok(None) if signal.is_none() && started.elapsed() < budget => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            result => {
                // Probe subprocesses may create their own groups. Stop the
                // whole observed tree as well as the sweep's original group.
                let tree = crate::exit_signal::collect_process_tree(child.id());
                crate::forwarded_signals::signal_child_tree(child.id(), &tree, libc::SIGKILL);
                crate::kill_process_group(child.id());
                let _ = child.kill();
                reap_sweep_bounded(
                    child,
                    cleanup_lock,
                    std::time::Duration::from_millis(200),
                    |child| child.try_wait(),
                );
                if let Some(signal) = signal {
                    drop(forwarding);
                    crate::forwarded_signals::reraise(signal);
                    anyhow::bail!("cleanup sweep interrupted by signal {signal}");
                }
                if let Err(error) = result {
                    return Err(error.into());
                }
                anyhow::bail!(
                    "cleanup sweep exceeded its {}s budget; termination requested; cleanup lock retained until exit; inspect remaining work",
                    budget.as_secs()
                );
            }
        }
    }
}

/// Why a post-ship cleanup run did not succeed: the worker never started
/// (ship must record the EndLease failure), or it ran and exited non-zero
/// (it already recorded its own outcome).
// trace:BUG-2384 | ai:claude
#[derive(Debug)]
pub(super) enum ShipCleanupError {
    NotStarted(anyhow::Error),
    Exited(anyhow::Error),
}

impl std::fmt::Display for ShipCleanupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotStarted(e) | Self::Exited(e) => write!(f, "{e:#}"),
        }
    }
}

/// Cleanup logs kept under `.aida/cleanup-logs/`; older ones are pruned
/// whenever a new worker log is opened.
// trace:BUG-2384 | ai:claude
pub(super) const CLEANUP_LOGS_KEPT: usize = 20;

/// Remove all but the newest `keep - 1` cleanup logs, making room for the
/// one about to be created. Best-effort: retention never blocks cleanup.
// trace:BUG-2384 | ai:claude
fn prune_cleanup_logs(logs: &std::path::Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(logs) else {
        return;
    };
    let mut found: Vec<(std::time::SystemTime, std::path::PathBuf)> = entries
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "log"))
        .filter_map(|path| {
            let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
            Some((modified, path))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in found.into_iter().skip(keep.saturating_sub(1)) {
        let _ = std::fs::remove_file(path);
    }
}

/// An operator's terminal ship: run the worker in the foreground with
/// inherited stdio, so it sees the same attended TTY the ship did.
// trace:BUG-2384 trace:TASK-1145 | ai:claude
pub(super) fn run_ship_cleanup_attended(
    mut cmd: std::process::Command,
) -> std::result::Result<bool, ShipCleanupError> {
    let status = cmd
        .status_retrying_etxtbsy()
        .map_err(|e| ShipCleanupError::NotStarted(e.into()))?;
    if !status.success() {
        return Err(ShipCleanupError::Exited(anyhow::anyhow!(
            "cleanup exited with {status}"
        )));
    }
    Ok(true)
}

// The foreground wait is bounded. Lease end can continue in the background;
// the step-6 sweep has its own supervised budget and keeps its safety gates.
// `true` means completed, `false` means continuing in the background.
// trace:BUG-2384 | ai:codex
pub(super) fn run_ship_cleanup_command(
    main: &std::path::Path,
    mut cmd: std::process::Command,
    wait: std::time::Duration,
) -> std::result::Result<bool, ShipCleanupError> {
    use std::os::unix::{fs::OpenOptionsExt, process::CommandExt};
    use std::process::Stdio;
    let not_started = |e: std::io::Error| ShipCleanupError::NotStarted(e.into());
    let logs = main.join(".aida/cleanup-logs");
    std::fs::create_dir_all(&logs).map_err(not_started)?;
    prune_cleanup_logs(&logs, CLEANUP_LOGS_KEPT);
    let path = logs.join(format!("{}.log", uuid::Uuid::new_v4()));
    let log = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(not_started)?;
    cmd.stdin(Stdio::null())
        .stdout(log.try_clone().map_err(not_started)?)
        .stderr(log);
    // Detach from terminal lifetime, stdin and the launcher's output pipes.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn_retrying_etxtbsy().map_err(not_started)?;
    let end = std::time::Instant::now() + wait;
    eprintln!(
        "  cleanup: pid {}, progress log {}",
        child.id(),
        path.display()
    );
    loop {
        match child.try_wait() {
            Ok(Some(status)) if !status.success() => {
                return Err(ShipCleanupError::Exited(anyhow::anyhow!(
                    "cleanup exited with {status}; details: {}",
                    path.display()
                )));
            }
            Ok(Some(_)) => {
                eprintln!("  cleanup completed; details: {}", path.display());
                return Ok(true);
            }
            Ok(None) => {}
            Err(error) => return Err(ShipCleanupError::Exited(error.into())),
        }
        if std::time::Instant::now() >= end {
            eprintln!(
                "  cleanup continuing in background (pid {}); progress: {}",
                child.id(),
                path.display()
            );
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Ok(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Step 6 of `aida pr ship`: after a merge this run performed, reap exited
/// sessions (`reap`) and prune merged agent worktrees (`prune`), in that
/// order. `--no-cleanup` skips both and says so. The worker supervisor bounds
/// the entire phase; prune also keeps its cooperative deadline. A failure is
/// printed as a warning and never fails the ship.
// trace:BUG-B6B1278890774EF993D2EBC327862470-033 | ai:claude
pub(crate) fn run_post_merge_worktree_gc(
    merged_this_run: bool,
    no_cleanup: bool,
    reap: impl FnOnce() -> Result<()>,
    prune: impl FnOnce() -> Result<()>,
    err: &mut impl std::io::Write,
) {
    use std::io::Write;
    if !merged_this_run {
        return;
    }
    if no_cleanup {
        let _ = writeln!(
            err,
            "  step 6: skipping the session reap and merged agent worktree pruning (--no-cleanup)"
        );
        return;
    }
    if let Err(e) = reap() {
        let _ = writeln!(
            err,
            "  {} post-merge session reap failed or was partially applied: {e:#}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        );
    }
    let _ = writeln!(
        err,
        "  step 6: pruning verified merged agent worktrees (merged branches only, \
         up to {}s; skip with --no-cleanup)",
        doctor_cmd::POST_SHIP_GC_BUDGET.as_secs()
    );
    if let Err(e) = prune() {
        let _ = writeln!(
            err,
            "  {} post-merge worktree gc failed or was partially applied: {e:#}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        );
    }
}
