//! Fixed review-watch supervision by the operator-started script deputy.
//! Reservations remain in the ordinary review-watch state; this receipt owns
//! only the desired grant binding and its controller, under admission.lock.
// trace:STORY-2571 | ai:codex
use super::*;
use crate::advisor_cmd_root::AdvisorLoopsCommand;
use crate::gate::seat_authority::{self, SeatGrant};
use chrono::{DateTime, Utc};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

const SERVICE: &str = "service.json";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    grant: String,
    subject: String,
    principal: String,
    issued_at: DateTime<Utc>,
    delegations: Vec<String>,
}

impl Binding {
    fn from_grant(grant: &SeatGrant) -> Self {
        Self {
            grant: grant.id.clone(),
            subject: grant.subject.clone(),
            principal: grant.principal.clone(),
            issued_at: grant.issued_at,
            delegations: grant.delegable_seats.clone(),
        }
    }
    fn validate(&self, root: &Path) -> Result<SeatGrant> {
        let grant = seat_authority::validated_grant(root, &self.grant, &self.subject, Utc::now())
            .context(
            "review-watch is blocked-authority; rebind from the current advisor session",
        )?;
        anyhow::ensure!(
            grant.seat == "advisor"
                && grant.principal == self.principal
                && grant.issued_at == self.issued_at
                && grant.delegable_seats == self.delegations,
            "review-watch binding no longer matches its grant"
        );
        anyhow::ensure!(
            grant.delegable_seats.iter().any(|s| s == "advisor")
                && grant.delegable_seats.iter().any(|s| s == "reviewer"),
            "review-watch needs advisor and reviewer child delegation"
        );
        Ok(grant)
    }
    fn succeeds(&self, old: &Self) -> Result<()> {
        if self.grant == old.grant {
            return Ok(());
        }
        anyhow::ensure!(
            self.principal == old.principal && self.issued_at > old.issued_at,
            "stale review-watch successor refused"
        );
        anyhow::ensure!(
            self.delegations.iter().all(|s| old.delegations.contains(s)),
            "review-watch successor widens delegation; see the operator recovery procedure in docs/cli/06-roles-sessions.md#review-watch-service-recovery"
        );
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Service {
    version: u32,
    deputy: String,
    binding: Binding,
    /// None with child_grant present is an ambiguous spawn, never restartable.
    worker: Option<Identity>,
    child_grant: Option<String>,
    /// Confirmed exited controllers whose review descendants may still run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    retired_grants: Vec<String>,
    host_active: bool,
    host: Option<Identity>,
    checked_at: Option<DateTime<Utc>>,
    retry_after: DateTime<Utc>,
    last_exit: Option<i32>,
}

fn directory(root: &Path, create: bool) -> Result<Option<Directory>> {
    if create {
        let runtime = Directory::shared(&root.join(".aida"))?;
        return Directory::open(&runtime.path().join("review-watch")).map(Some);
    }
    let Some(runtime) = Directory::existing(&root.join(".aida"))? else {
        return Ok(None);
    };
    let dir = Directory::existing(&runtime.path().join("review-watch"))?;
    if let Some(dir) = &dir {
        anyhow::ensure!(
            dir.handle.metadata()?.mode() & 0o022 == 0,
            "unsafe review-watch service directory"
        );
    }
    Ok(dir)
}
fn read(dir: &Directory) -> Result<Option<Service>> {
    let Some(bytes) = dir.read(SERVICE)? else {
        anyhow::ensure!(dir.read("service-initialized")?.is_none(), "review-watch service receipt is missing after initialization; follow docs/cli/06-roles-sessions.md#review-watch-service-recovery");
        return Ok(None);
    };
    let state: Service =
        serde_json::from_slice(&bytes).context("invalid review-watch service receipt")?;
    anyhow::ensure!(
        state.version == 1 && !state.deputy.is_empty(),
        "unsupported review-watch service receipt"
    );
    anyhow::ensure!(
        state.worker.is_none() || state.child_grant.is_some(),
        "incomplete review-watch controller receipt"
    );
    Ok(Some(state))
}
fn write(dir: &Directory, state: &Service) -> Result<()> {
    // Validate the prior opened file before replacing it.
    read(dir)?;
    dir.write(SERVICE, &serde_json::to_vec_pretty(state)?)?;
    if dir.read("service-initialized")?.is_none() {
        dir.file("service-initialized", true)?.sync_all()?;
    }
    dir.handle.sync_all()?;
    Ok(())
}

pub(crate) fn rebind(root: &Path, deputy: &str, grant: &SeatGrant) -> Result<()> {
    let binding = Binding::from_grant(grant);
    binding.validate(root)?;
    let dir = directory(root, true)?.context("missing review-watch directory")?;
    let _lock = dir.lock("admission.lock")?;
    // Revalidate at the locked acceptance boundary, including the full parent chain.
    binding.validate(root)?;
    let state = match read(&dir)? {
        Some(mut state) => {
            anyhow::ensure!(
                state.deputy == deputy,
                "review-watch belongs to another deputy"
            );
            binding.succeeds(&state.binding)?;
            // Operator roots may hand off to their first advisor child. Once
            // bound to a session, its descendants are sub-advisors, not rotations.
            // trace:STORY-2571 | ai:codex
            if seat_authority::read_grant(&state.binding.grant)?
                .parent_grant_id
                .is_some()
            {
                let mut ancestor = grant.parent_grant_id.clone();
                while let Some(id) = ancestor {
                    anyhow::ensure!(
                        id != state.binding.grant,
                        "review-watch sub-advisor cannot replace its ancestor binding"
                    );
                    ancestor = seat_authority::read_grant(&id)?.parent_grant_id;
                }
            }
            state.binding = binding;
            state
        }
        None => Service {
            version: 1,
            deputy: deputy.into(),
            binding,
            worker: None,
            child_grant: None,
            retired_grants: Vec::new(),
            host_active: true,
            host: None,
            checked_at: None,
            retry_after: Utc::now(),
            last_exit: None,
        },
    };
    write(&dir, &state)
}

pub(crate) fn command(action: &AdvisorLoopsCommand) -> Result<()> {
    let root = forge::main_root(&std::env::current_dir()?)?;
    match action {
        AdvisorLoopsCommand::Status { json } => {
            let Some(dir) = directory(&root, false)? else {
                print_status("not-configured", None, &[], *json)?;
                return Ok(());
            };
            let _lock = dir.lock("admission.lock")?;
            let Some(state) = read(&dir)? else {
                print_status("not-configured", None, &[], *json)?;
                return Ok(());
            };
            let status = if !host_fresh(&state) {
                "blocked-deputy"
            } else if state.binding.validate(&root).is_err() {
                "blocked-authority"
            } else if state.child_grant.is_some() && state.worker.is_none() {
                "blocked-ambiguous-spawn"
            } else if state.worker.as_ref().is_some_and(Identity::alive) {
                "running"
            } else {
                "waiting-for-supervisor"
            };
            print_status(status, Some(&state), &review_blockers(&dir)?, *json)?;
            Ok(())
        }
        AdvisorLoopsCommand::Rebind { deputy } => {
            let grant = seat_authority::grant_check(&root)
                .into_valid()
                .context("rebind requires a live advisor seat grant")?;
            let queue = crate::deputy_shell::current_queue(deputy)?;
            let reply = crate::deputy_shell::socket::request(
                &queue,
                &crate::deputy_shell::socket::Request::ReviewWatchRebind {
                    project: root,
                    grant: grant.id,
                },
            )?;
            anyhow::ensure!(
                reply.ok,
                "review-watch rebind refused: {}",
                reply.why.unwrap_or_default()
            );
            println!("review-watch: binding accepted; the supervisor will reconcile the prior controller before restart");
            Ok(())
        }
        AdvisorLoopsCommand::Worker => worker(&root),
    }
}

// trace:STORY-2571 | ai:codex
fn print_status(
    status: &str,
    state: Option<&Service>,
    blockers: &[String],
    json: bool,
) -> Result<()> {
    let value = serde_json::json!({
        "service": "review-watch", "status": status,
        "deputy": state.map(|s| &s.deputy),
        "binding_issued_at": state.map(|s| s.binding.issued_at),
        "controller": state.and_then(|s| s.worker.as_ref().map(|w| w.pid)),
        "last_exit": state.and_then(|s| s.last_exit),
        "retired_controllers": state.map(|s| s.retired_grants.len()).unwrap_or(0),
        "review_blockers": blockers,
        "recovery": "docs/cli/06-roles-sessions.md#review-watch-service-recovery",
    });
    if json || crate::output_format_is_json() {
        println!("{}", crate::cache_output::json_pretty(&value)?);
    } else if matches!(
        crate::output_format_override(),
        Some(crate::OutputFormat::Toon)
    ) {
        for (key, value) in value.as_object().unwrap() {
            println!(
                "{}",
                crate::toon::scalar(
                    key,
                    &value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string())
                )
            );
        }
    } else {
        println!("review-watch: {status}; deputy {}; binding issued {}; controller {}; last exit {}; retired controllers {}; review blockers {}; recovery {}",
            value["deputy"], value["binding_issued_at"], value["controller"], value["last_exit"], value["retired_controllers"], value["review_blockers"], value["recovery"]);
    }
    Ok(())
}

thread_local! { static CONTROLLER: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) }; }

/// Ordinary review-watch invocations keep their existing behavior. The fixed
/// service worker checks binding and authority at every pass and PR admission.
pub(super) fn check_controller(root: &Path) -> Result<()> {
    if !CONTROLLER.with(|id| id.borrow().is_some()) {
        return Ok(());
    }
    let dir = directory(root, false)?.context("review-watch service state disappeared")?;
    let _lock = dir.lock("admission.lock")?;
    check_controller_locked(root, &dir)
}

// Called while ordinary review admission already holds admission.lock. Rebind
// and reservation creation thus compare the generation at the same boundary.
// trace:STORY-2571 | ai:codex
pub(super) fn check_controller_locked(root: &Path, dir: &Directory) -> Result<()> {
    CONTROLLER
        .with(|id| {
            let id = id.borrow();
            let Some(id) = id.as_ref() else { return Ok(()) };
            let state = read(dir)?.context("review-watch service state disappeared")?;
            anyhow::ensure!(
                host_fresh(&state),
                "review-watch deputy authority ended or its heartbeat is stale"
            );
            let child = seat_authority::grant_check(root)
                .into_valid()
                .context("review-watch controller authority ended")?;
            anyhow::ensure!(child.id == *id, "review-watch controller grant changed");
            check_binding(root, &state, &child)?;
            Ok(())
        })
        .context(GlobalState)
}

// trace:STORY-2571 | ai:codex
fn check_binding(root: &Path, state: &Service, child: &SeatGrant) -> Result<()> {
    anyhow::ensure!(
        state.child_grant.as_ref() == Some(&child.id)
            && child.parent_grant_id.as_ref() == Some(&state.binding.grant),
        "review-watch controller binding superseded"
    );
    state.binding.validate(root)?;
    Ok(())
}

fn worker(root: &Path) -> Result<()> {
    let child = seat_authority::grant_check(root)
        .into_valid()
        .context("review-watch worker needs its child grant")?;
    let own = Identity::current()?;
    // Parent writes the verified child receipt before any discovery/admission.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let dir = directory(root, false)?.context("review-watch service state missing")?;
        let ready = {
            let _lock = dir.lock("admission.lock")?;
            let state = read(&dir)?.context("review-watch service state missing")?;
            anyhow::ensure!(
                state.child_grant.as_ref() == Some(&child.id),
                "unexpected review-watch worker grant"
            );
            state.worker.as_ref() == Some(&own)
        };
        if ready {
            break;
        }
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "review-watch controller receipt not confirmed"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    CONTROLLER.with(|id| *id.borrow_mut() = Some(child.id));
    run_with_program(
        &ReviewWatchArgs {
            once: false,
            interval: 10,
            max_concurrent: 2,
            vendor: None,
            release: None,
            launch_reservation: None,
        },
        root,
        None,
    )
    .map(|_| ())
}

/// Trusted context supplied only by the operator-started deputy, shared with
/// the socket and supervisor so runner refresh never leaves a stale selector.
// trace:STORY-2571 | ai:codex
#[derive(Clone)]
pub(crate) struct HostAuthority {
    pub(crate) project: PathBuf,
    grant: Arc<std::sync::Mutex<String>>,
    cfg: crate::deputy_shell::runner::RunnerConfig,
    active: Arc<AtomicBool>,
}
impl HostAuthority {
    pub(crate) fn new(cfg: &crate::deputy_shell::runner::RunnerConfig) -> Self {
        Self {
            project: cfg.project_root.clone(),
            grant: Arc::new(std::sync::Mutex::new(cfg.grant_id.clone())),
            cfg: cfg.clone(),
            active: Arc::new(AtomicBool::new(true)),
        }
    }
    pub(crate) fn refreshed(&self, id: &str) {
        if let Ok(mut grant) = self.grant.lock() {
            *grant = id.into();
        }
    }
    pub(crate) fn validate(&self) -> Result<()> {
        anyhow::ensure!(self.active.load(Ordering::SeqCst), "deputy stopped");
        let id = self
            .grant
            .lock()
            .map_err(|_| anyhow::anyhow!("deputy authority unavailable"))?;
        match (self.cfg.grant)(&id) {
            crate::deputy::worker::GrantState::Valid { expires_at } => {
                anyhow::ensure!(
                    expires_at.is_none_or(|at| at > (self.cfg.clock)()),
                    "deputy authority expired"
                );
            }
            _ => anyhow::bail!("deputy authority ended"),
        }
        anyhow::ensure!((self.cfg.mandate)().is_none(), "deputy mandate refused");
        Ok(())
    }
}

/// The operator-started deputy owns this thread. Controllers always execute
/// the running AIDA image's fixed worker verb.
// trace:STORY-2571 | ai:codex
pub(crate) struct Supervisor {
    host: HostAuthority,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Supervisor {
    pub(crate) fn start(queue: crate::deputy_shell::Queue, host: HostAuthority) -> Self {
        let guard = host.clone();
        let deputy = queue.name.clone();
        let thread = std::thread::spawn(move || {
            let mut child: Option<Child> = None;
            let mut reported = None;
            while guard.active.load(Ordering::SeqCst) {
                if let Err(error) = tick(
                    &guard.project,
                    &deputy,
                    guard.validate().is_ok() && !queue.paused(),
                    &mut child,
                ) {
                    let reason = format!("{error:#}");
                    if reported.as_ref() != Some(&reason) {
                        eprintln!("review-watch supervisor refused: {reason}");
                        reported = Some(reason);
                    }
                } else {
                    reported = None;
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            let _ = tick(&guard.project, &deputy, false, &mut child);
            // Dropping a child handle never proves exit or clears its receipt.
        });
        Self {
            host,
            thread: Some(thread),
        }
    }
}
impl Drop for Supervisor {
    fn drop(&mut self) {
        self.host.active.store(false, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

// A dead, unknown, paused or stalled deputy cannot lend remembered authority
// to a surviving controller. The heartbeat carries no independent authority.
// trace:STORY-2571 | ai:codex
fn host_fresh(state: &Service) -> bool {
    state.host_active
        && state
            .checked_at
            .is_some_and(|at| at <= Utc::now() && Utc::now() - at < chrono::Duration::seconds(5))
        && state
            .host
            .as_ref()
            .is_some_and(|host| matches!(host.verdict(), aida_core::proc_identity::Verdict::Alive))
}

fn tick(root: &Path, deputy: &str, host_valid: bool, child: &mut Option<Child>) -> Result<()> {
    tick_with_launcher(root, deputy, host_valid, child, |root, parent| {
        crate::launch::review_watch_controller(root, &parent.id, &parent.subject)
    })
}

// Keep the supervision/spawn boundary testable without running a CLI worker
// inside the libtest image. Production always uses the fixed launcher above.
// trace:STORY-2571 | ai:codex
fn tick_with_launcher(
    root: &Path,
    deputy: &str,
    host_valid: bool,
    child: &mut Option<Child>,
    launch: impl FnOnce(&Path, &SeatGrant) -> Result<(SeatGrant, Command)>,
) -> Result<()> {
    let Some(dir) = directory(root, false)? else {
        return Ok(());
    };
    let _lock = dir.lock("admission.lock")?;
    let Some(mut state) = read(&dir)? else {
        return Ok(());
    };
    if state.deputy != deputy {
        return Ok(());
    }
    let before = serde_json::to_vec(&state)?;
    state.host_active = host_valid;
    if host_valid
        && state
            .checked_at
            .is_none_or(|at| Utc::now() - at >= chrono::Duration::seconds(2))
    {
        state.host = Some(Identity::current()?);
        state.checked_at = Some(Utc::now());
    }
    let exited = if let Some(process) = child.as_mut() {
        // Capture the exit before dropping the reaped handle. Retain the
        // persisted identity and grant while reviews may still descend from it.
        if let Some(status) = process.try_wait()? {
            state.last_exit = status.code();
            *child = None;
            true
        } else {
            false
        }
    } else {
        state.worker.as_ref().is_some_and(|worker| {
            matches!(worker.verdict(), aida_core::proc_identity::Verdict::Dead(_))
        })
    };
    // Restart and grant revocation are independent: supersede progress may
    // require a live controller. Unclaimed reservations can also hide
    // a dispatcher spawned before its identity was persisted. Neither may
    // block replacement, nor justify revoking possible reviewer descendants.
    // trace:STORY-2571 | ai:codex
    if exited {
        if let Some(grant) = state.child_grant.take() {
            state.retired_grants.push(grant);
        }
        state.worker = None;
        state.retry_after = Utc::now() + chrono::Duration::seconds(10);
    }
    // Persist retirement before cleanup or spawn, including across failures.
    if before != serde_json::to_vec(&state)? {
        write(&dir, &state)?;
    }
    let before = serde_json::to_vec(&state)?;
    if !state.retired_grants.is_empty() && reviews_finished(root, &dir)? {
        for id in &state.retired_grants {
            seat_authority::revoke_by_id(id)?;
        }
        state.retired_grants.clear();
    }
    if before != serde_json::to_vec(&state)? {
        write(&dir, &state)?;
    }
    if !host_valid || state.child_grant.is_some() || Utc::now() < state.retry_after {
        return Ok(());
    }
    let parent = match state.binding.validate(root) {
        Ok(parent) => parent,
        Err(_) => return Ok(()),
    };
    reconcile_reviews(root, &dir)?;
    let (grant, mut command) = launch(root, &parent)?;
    state.child_grant = Some(grant.id.clone());
    write(&dir, &state)?; // reserve before spawn; a crash retains ambiguity
    let log = dir.file("service.log", true)?;
    command
        .current_dir(root)
        .args(["advisor", "loops", "worker"])
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    match command.spawn() {
        Ok(process) => {
            // Keep the handle even if recording fails: never reap first and
            // turn a fast-exiting worker into an ambiguous capture.
            *child = Some(process);
            state.worker = Some(Identity::child(child.as_ref().unwrap())?);
            write(&dir, &state)
        }
        Err(error) => {
            seat_authority::revoke_by_id(&grant.id)?;
            state.child_grant = None;
            state.retry_after = Utc::now() + chrono::Duration::seconds(10);
            write(&dir, &state)?;
            Err(error.into())
        }
    }
}

// Retired grants protect reviews but never gate controller replacement.
// Reservations do not record grant ancestry, so conservatively protect every
// unresolved attempt, including unrecorded and unknown descendants.
// trace:STORY-2571 | ai:codex
fn reviews_finished(root: &Path, dir: &Directory) -> Result<bool> {
    reconcile_reviews(root, dir)?;
    Ok(review_blockers(dir)?.is_empty())
}

// Expose the exact reservations retaining retired grants without disclosing
// grant selectors. Unknown descendants remain blockers even after termination.
// trace:STORY-2571 | ai:codex
fn review_blockers(dir: &Directory) -> Result<Vec<String>> {
    let Some(bytes) = dir.read(STATE)? else {
        return Ok(Vec::new());
    };
    let reviews: State = serde_json::from_slice(&bytes)?;
    Ok(reviews
        .entries
        .iter()
        .flat_map(|(key, entry)| {
            entry
                .attempts
                .iter()
                .filter(|a| {
                    active(a) || a.unrecorded_descendant() || a.recorded().iter().any(|p| p.alive())
                })
                .map(move |a| format!("{key}:{} ({:?})", a.token, a.phase))
        })
        .collect())
}

// Reuse the ordinary reservation recovery while holding admission.lock. A
// controller restart never clears an unrecorded or live review descendant.
// trace:STORY-2571 | ai:codex
fn reconcile_reviews(root: &Path, dir: &Directory) -> Result<()> {
    let Some(bytes) = dir.read(STATE)? else {
        anyhow::ensure!(
            dir.read("initialized")?.is_none(),
            "review-watch state disappeared after initialization"
        );
        return Ok(());
    };
    let state: State = serde_json::from_slice(&bytes)
        .context("invalid review-watch state before service restart")?;
    let store = Store {
        runtime: Directory::existing(&dir.path().join(".."))?
            .context("review-watch runtime disappeared")?,
        dir: Directory {
            handle: dir.handle.try_clone()?,
        },
        repository: state.repository.clone(),
        root: root.into(),
    };
    let mut state = store.load()?;
    let before = serde_json::to_vec(&state)?;
    store.reconcile_lost_dispatchers(&mut state);
    // A crash after persisting supersede intent must not leave the service
    // dependent on an old watcher pass. Finish only with the ordinary complete
    // process/group proof; a replacement handles signals and ambiguous cases.
    // trace:STORY-2571 | ai:codex
    for attempt in state.entries.values_mut().flat_map(|e| &mut e.attempts) {
        if attempt.phase == Phase::Superseding && confirm(attempt).is_ok() {
            attempt.phase = Phase::Superseded;
        }
    }
    validate_state(&state, &store.repository)?;
    if before != serde_json::to_vec(&state)? {
        dir.write(STATE, &serde_json::to_vec_pretty(&state)?)?;
    }
    Ok(())
}

/// The trusted launcher calls this only after issuing the new advisor grant.
/// Socket attribution still checks the launcher's process binding; a remembered
/// advisor registration never authorizes this operation.
// trace:STORY-2571 | ai:codex
pub(crate) fn rebind_after_launch(start: &Path, grant: &SeatGrant) -> Result<()> {
    let root = forge::main_root(start)?;
    let default_running = matches!(
        crate::deputy_shell::current_queue("default")?.runner_status(),
        crate::deputy_shell::RunnerStatus::Live(_) | crate::deputy_shell::RunnerStatus::Legacy(_)
    );
    rebind_after_launch_with(
        &root,
        grant,
        default_running,
        |deputy, root, grant, configured| {
            let queue = crate::deputy_shell::current_queue(deputy)?;
            let reply = match crate::deputy_shell::socket::request(
                &queue,
                &crate::deputy_shell::socket::Request::ReviewWatchRebind {
                    project: root.into(),
                    grant: grant.id.clone(),
                },
            ) {
                Ok(reply) => reply,
                // A deputy can disappear between the liveness probe and connect.
                // An unused service is silent unless a deputy actually refuses it.
                Err(_) if !configured => return Ok(()),
                Err(error) => return Err(error),
            };
            anyhow::ensure!(
                reply.ok,
                "review-watch rotation rebind refused: {}",
                reply.why.unwrap_or_default()
            );
            Ok(())
        },
    )
}

// Trusted launchers may bootstrap the default deputy or rotate an operator's
// binding to the advisor handle they just minted. The socket still attributes
// the launcher to that grant and rebind applies all successor checks.
// trace:STORY-2571 | ai:codex
fn rebind_after_launch_with(
    root: &Path,
    grant: &SeatGrant,
    default_running: bool,
    request: impl FnOnce(&str, &Path, &SeatGrant, bool) -> Result<()>,
) -> Result<()> {
    let state = if let Some(dir) = directory(root, false)? {
        let _lock = dir.lock("admission.lock")?;
        read(&dir)?
    } else {
        None
    };
    if state.is_none() && !default_running {
        return Ok(());
    }
    let deputy = state
        .as_ref()
        .map(|s| s.deputy.as_str())
        .unwrap_or("default");
    request(deputy, root, grant, state.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, crate::test_env::AmbientGuard, SeatGrant) {
        let temp = tempfile::tempdir().unwrap();
        let guard = crate::test_env::AmbientGuard::hermetic_with_seat(
            temp.path(),
            "advisor",
            &["advisor", "reviewer"],
        );
        let grant = seat_authority::grant_check(temp.path())
            .into_valid()
            .unwrap();
        (temp, guard, grant)
    }
    fn successor(root: &Path, old: &SeatGrant) -> SeatGrant {
        let id = seat_authority::test_support::mint_grant_for(
            root,
            &old.subject,
            "advisor",
            &["advisor", "reviewer"],
        );
        seat_authority::read_grant(&id).unwrap()
    }

    struct ControllerScope;
    impl Drop for ControllerScope {
        fn drop(&mut self) {
            CONTROLLER.with(|id| *id.borrow_mut() = None);
        }
    }
    fn controller(
        root: &Path,
        guard: &mut crate::test_env::AmbientGuard,
        parent: &SeatGrant,
    ) -> ControllerScope {
        rebind(root, "default", parent).unwrap();
        let child =
            seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        let mut state = read(&dir).unwrap().unwrap();
        state.child_grant = Some(child.id.clone());
        state.worker = Some(Identity::current().unwrap());
        state.host = Some(Identity::current().unwrap());
        state.checked_at = Some(Utc::now());
        write(&dir, &state).unwrap();
        guard.set_key(seat_authority::GRANT_ENV, &child.id);
        guard.set_key("AIDA_USER", &child.subject);
        guard.set_key("AIDA_AGENT_NAME", &child.subject);
        CONTROLLER.with(|id| *id.borrow_mut() = Some(child.id));
        ControllerScope
    }
    fn change(id: u64) -> forge::Change {
        forge::Change {
            id,
            head: format!("{id:040x}"),
            base: "main".into(),
            author: "writer".into(),
            gitlab: false,
            overlap: Some(false),
            behind: Some(0),
            head_branch: None,
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn story2571_reaped_exit_and_confirmed_dead_revoke_controller_grant() {
        for reap_in_tick in [true, false] {
            let (temp, _guard, parent) = fixture();
            let root = temp.path();
            rebind(root, "default", &parent).unwrap();
            let dir = directory(root, false).unwrap().unwrap();
            let grant = seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject)
                .unwrap();
            let mut process = Command::new("sh").args(["-c", "exit 7"]).spawn().unwrap();
            let mut state = read(&dir).unwrap().unwrap();
            state.worker = Some(Identity::child(&process).unwrap());
            state.child_grant = Some(grant.id.clone());
            write(&dir, &state).unwrap();
            if !reap_in_tick {
                process.wait().unwrap();
            }
            let mut child = reap_in_tick.then_some(process);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                tick(root, "default", false, &mut child).unwrap();
                if read(&dir).unwrap().unwrap().child_grant.is_none() {
                    break;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(seat_authority::read_grant(&grant.id)
                .unwrap()
                .revoked_at
                .is_some());
            assert!(child.is_none());
            let stopped = read(&dir).unwrap().unwrap();
            assert!(stopped.worker.is_none());
            if reap_in_tick {
                assert_eq!(stopped.last_exit, Some(7));
            }
            assert!(stopped.retry_after > Utc::now());
        }
    }

    // Exercise the actual reviewer ancestry, not just the controller record.
    // trace:STORY-2571 | ai:codex
    #[test]
    fn story2571_live_reviewer_survives_controller_reap_and_rotation() {
        for reap_in_tick in [true, false] {
            for rotate in [true, false] {
                let (temp, _guard, parent) = fixture();
                let root = temp.path();
                rebind(root, "default", &parent).unwrap();
                let dir = directory(root, false).unwrap().unwrap();
                let grant =
                    seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject)
                        .unwrap();
                let reviewer = seat_authority::issue_child_from(
                    grant.clone(),
                    "reviewer",
                    "live-reviewer",
                    false,
                )
                .unwrap();
                assert_eq!(reviewer.parent_grant_id.as_ref(), Some(&grant.id));
                let mut process = Command::new("sh").args(["-c", "exit 7"]).spawn().unwrap();
                let mut state = read(&dir).unwrap().unwrap();
                state.worker = Some(Identity::child(&process).unwrap());
                state.child_grant = Some(grant.id.clone());
                write(&dir, &state).unwrap();
                let store = Store::open(root, "github:github.com/owner/repo").unwrap();
                assert!(matches!(
                    select(&store, &change(1), None, 2).unwrap(),
                    Action::Dispatch(_)
                ));
                let mut reviews = store.load().unwrap();
                let attempt = &mut reviews.entries.get_mut("PR-1").unwrap().attempts[0];
                attempt.phase = Phase::Running;
                attempt.identity = Some(Identity::current().unwrap());
                attempt.reviewer_spawning = true;
                attempt.reviewer = Some(Identity::current().unwrap());
                dir.write(STATE, &serde_json::to_vec_pretty(&reviews).unwrap())
                    .unwrap();
                if rotate {
                    rebind(root, "default", &successor(root, &parent)).unwrap();
                }
                if !reap_in_tick {
                    process.wait().unwrap();
                }
                let mut child = reap_in_tick.then_some(process);
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
                loop {
                    tick(root, "default", false, &mut child).unwrap();
                    if child.is_none() {
                        break;
                    }
                    assert!(std::time::Instant::now() < deadline);
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                assert!(
                    seat_authority::validated_grant(
                        root,
                        &reviewer.id,
                        &reviewer.subject,
                        Utc::now()
                    )
                    .is_some(),
                    "reviewer must retain full-chain authority after controller exit"
                );
                assert_eq!(
                    read(&dir).unwrap().unwrap().retired_grants,
                    vec![grant.id.clone()]
                );
                // An unrecorded spawn is also protected across supervisor ticks.
                let attempt = &mut reviews.entries.get_mut("PR-1").unwrap().attempts[0];
                attempt.reviewer = None;
                dir.write(STATE, &serde_json::to_vec_pretty(&reviews).unwrap())
                    .unwrap();
                tick(root, "default", false, &mut child).unwrap();
                assert!(seat_authority::validated_grant(
                    root,
                    &reviewer.id,
                    &reviewer.subject,
                    Utc::now()
                )
                .is_some());
                // Once the review is terminal and all identities are gone,
                // revocation and ordinary restart backoff may proceed.
                let attempt = &mut reviews.entries.get_mut("PR-1").unwrap().attempts[0];
                attempt.phase = Phase::Exited;
                attempt.identity = None;
                attempt.reviewer_spawning = false;
                dir.write(STATE, &serde_json::to_vec_pretty(&reviews).unwrap())
                    .unwrap();
                tick(root, "default", false, &mut child).unwrap();
                assert!(read(&dir).unwrap().unwrap().child_grant.is_none());
                assert!(seat_authority::read_grant(&grant.id)
                    .unwrap()
                    .revoked_at
                    .is_some());
            }
        }
    }

    // Both crash windows previously prevented all replacement controllers.
    // Exercise the real supervisor spawn boundary, retaining the old grant
    // until the same descendant evidence used in production proves it safe.
    // trace:STORY-2571 | ai:codex
    #[test]
    fn story2571_crash_mid_superseding_or_unpinned_reservation_restarts() {
        for (superseding, retained_child) in
            [(true, false), (false, false), (true, true), (false, true)]
        {
            let (temp, _guard, parent) = fixture();
            let root = temp.path();
            rebind(root, "default", &parent).unwrap();
            let dir = directory(root, false).unwrap().unwrap();
            let grant = seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject)
                .unwrap();
            let reviewer = seat_authority::issue_child_from(
                grant.clone(),
                "reviewer",
                "surviving-reviewer",
                false,
            )
            .unwrap();
            let mut process = Command::new("sh").args(["-c", "exit 7"]).spawn().unwrap();
            let exited_identity = Identity::child(&process).unwrap();
            process.wait().unwrap();
            let mut state = read(&dir).unwrap().unwrap();
            state.worker = Some(exited_identity.clone());
            state.child_grant = Some(grant.id.clone());
            write(&dir, &state).unwrap();

            let store = Store::open(root, "github:github.com/owner/repo").unwrap();
            let Action::Dispatch(reservation) = select(&store, &change(1), None, 2).unwrap() else {
                panic!("expected reservation")
            };
            let mut reviews = store.load().unwrap();
            if superseding {
                let attempt = &mut reviews.entries.get_mut("PR-1").unwrap().attempts[0];
                attempt.phase = Phase::Superseding;
                attempt.identity = Some(exited_identity.clone());
                attempt.reviewer_spawning = true;
                attempt.reviewer = Some(Identity::current().unwrap());
                attempt.supersede = Some(Supersede {
                    by: change(2).head,
                    requested_at: Utc::now(),
                    term_at: None,
                    kill_at: None,
                    diagnostic: None,
                });
                dir.write(STATE, &serde_json::to_vec_pretty(&reviews).unwrap())
                    .unwrap();
            }
            // Cover both recovery from the persisted dead identity and the
            // live supervisor observing its retained Child's exit status.
            let mut controller = if retained_child { Some(process) } else { None };
            tick(root, "default", true, &mut controller).unwrap();
            let mut retired = read(&dir).unwrap().unwrap();
            assert!(retired.worker.is_none());
            assert!(retired.child_grant.is_none());
            assert_eq!(retired.retired_grants, vec![grant.id.clone()]);
            assert!(retired.retry_after > Utc::now());
            assert!(controller.is_none());
            assert!(review_blockers(&dir)
                .unwrap()
                .iter()
                .any(|b| b.contains("PR-1") && b.contains(&reservation.token)));

            // Advance the clock past normal backoff without sleeping. Exercise
            // production supervision and spawn with a harmless controller;
            // drive restart reconciliation explicitly below.
            retired.retry_after = Utc::now() - chrono::Duration::seconds(1);
            write(&dir, &retired).unwrap();
            tick_with_launcher(root, "default", true, &mut controller, |root, parent| {
                let grant =
                    seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject)?;
                let mut command = Command::new("sh");
                command.args(["-c", "exit 0"]);
                Ok((grant, command))
            })
            .unwrap();
            let running = read(&dir).unwrap().unwrap();
            assert!(
                controller.is_some(),
                "unresolved attempts must permit replacement"
            );
            assert!(running.worker.is_some());
            assert_ne!(running.child_grant.as_ref(), Some(&grant.id));
            assert_eq!(running.retired_grants, vec![grant.id.clone()]);
            assert!(
                seat_authority::validated_grant(root, &reviewer.id, &reviewer.subject, Utc::now(),)
                    .is_some(),
                "replacement must preserve reviewer ancestry"
            );
            controller.as_mut().unwrap().wait().unwrap();

            if superseding {
                // The final reviewer has now exited. Restart reconciliation
                // finishes persisted intent without needing the old watcher.
                let attempt = &mut reviews.entries.get_mut("PR-1").unwrap().attempts[0];
                attempt.reviewer = Some(exited_identity);
                dir.write(STATE, &serde_json::to_vec_pretty(&reviews).unwrap())
                    .unwrap();
                tick(root, "default", false, &mut controller).unwrap();
                assert_eq!(
                    store.load().unwrap().entries["PR-1"].attempts[0].phase,
                    Phase::Superseded
                );
                assert!(seat_authority::read_grant(&grant.id)
                    .unwrap()
                    .revoked_at
                    .is_some());
                assert!(read(&dir).unwrap().unwrap().retired_grants.is_empty());
            } else {
                // No dispatcher receipt could mean a spawn/persist crash.
                // Keep that capacity and old grant, but never block restart.
                tick(root, "default", false, &mut controller).unwrap();
                assert_eq!(
                    store.load().unwrap().entries["PR-1"].attempts[0].phase,
                    Phase::Reserved
                );
                assert!(seat_authority::read_grant(&grant.id)
                    .unwrap()
                    .revoked_at
                    .is_none());
            }
        }
    }

    // Restart itself completes persisted supersede intent when no descendant
    // remains; the replacement must not be needed to release the old grant.
    // trace:STORY-2571 | ai:codex
    #[test]
    fn story2571_restart_finishes_dead_superseding_before_launch() {
        let (temp, _guard, parent) = fixture();
        let root = temp.path();
        rebind(root, "default", &parent).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        let grant =
            seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject).unwrap();
        let mut process = Command::new("sh").args(["-c", "exit 7"]).spawn().unwrap();
        let identity = Identity::child(&process).unwrap();
        process.wait().unwrap();
        let mut service = read(&dir).unwrap().unwrap();
        service.worker = Some(identity.clone());
        service.child_grant = Some(grant.id.clone());
        write(&dir, &service).unwrap();

        let store = Store::open(root, "github:github.com/owner/repo").unwrap();
        assert!(matches!(
            select(&store, &change(1), None, 2).unwrap(),
            Action::Dispatch(_)
        ));
        let mut reviews = store.load().unwrap();
        let attempt = &mut reviews.entries.get_mut("PR-1").unwrap().attempts[0];
        attempt.phase = Phase::Superseding;
        attempt.identity = Some(identity);
        attempt.supersede = Some(Supersede {
            by: change(2).head,
            requested_at: Utc::now(),
            term_at: None,
            kill_at: None,
            diagnostic: None,
        });
        dir.write(STATE, &serde_json::to_vec_pretty(&reviews).unwrap())
            .unwrap();

        let mut controller = None;
        tick(root, "default", true, &mut controller).unwrap();
        assert_eq!(
            store.load().unwrap().entries["PR-1"].attempts[0].phase,
            Phase::Superseded
        );
        assert!(seat_authority::read_grant(&grant.id)
            .unwrap()
            .revoked_at
            .is_some());
        let mut retired = read(&dir).unwrap().unwrap();
        assert!(retired.retired_grants.is_empty());
        assert!(controller.is_none());
        retired.retry_after = Utc::now() - chrono::Duration::seconds(1);
        write(&dir, &retired).unwrap();
        tick_with_launcher(root, "default", true, &mut controller, |root, parent| {
            let grant =
                seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject)?;
            let mut command = Command::new("sh");
            command.args(["-c", "exit 0"]);
            Ok((grant, command))
        })
        .unwrap();
        controller
            .as_mut()
            .expect("replacement started")
            .wait()
            .unwrap();
    }

    #[test]
    fn story2571_sub_advisor_does_not_narrow_binding_before_rotation() {
        let temp = tempfile::tempdir().unwrap();
        let mut guard = crate::test_env::AmbientGuard::hermetic_with_seat(
            temp.path(),
            "advisor",
            &["advisor", "reviewer", "implementer"],
        );
        let root = temp.path();
        let operator = seat_authority::grant_check(root).into_valid().unwrap();
        rebind(root, "default", &operator).unwrap();
        let advisor =
            seat_authority::issue_child_from(operator.clone(), "advisor", "first-advisor", false)
                .unwrap();
        rebind_after_launch_with(root, &advisor, false, |d, r, g, _| rebind(r, d, g)).unwrap();
        let sub =
            seat_authority::issue_child_from(advisor.clone(), "advisor", "sub-advisor", false)
                .unwrap();
        // Explicitly narrow the sub-advisor's delegation as a real --delegate-seat launch does.
        guard.set_key(seat_authority::GRANT_ENV, &advisor.id);
        guard.set_key("AIDA_USER", &advisor.subject);
        guard.set_key("AIDA_AGENT_NAME", &advisor.subject);
        let narrow = seat_authority::issue_launch_child_with_delegation(
            root,
            "advisor",
            "narrow-advisor",
            false,
            &["advisor".into(), "reviewer".into()],
        )
        .unwrap();
        for sub in [sub, narrow] {
            assert!(
                rebind_after_launch_with(root, &sub, false, |d, r, g, _| rebind(r, d, g))
                    .unwrap_err()
                    .to_string()
                    .contains("sub-advisor")
            );
        }
        let next = seat_authority::issue_child_from(operator, "advisor", "rotated-advisor", false)
            .unwrap();
        rebind_after_launch_with(root, &next, false, |d, r, g, _| rebind(r, d, g)).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        assert_eq!(read(&dir).unwrap().unwrap().binding.grant, next.id);
        assert_eq!(
            read(&dir).unwrap().unwrap().binding.delegations,
            advisor.delegable_seats
        );
    }

    #[test]
    fn story2571_controller_select_after_rotation_stops_pass() {
        let (temp, mut guard, parent) = fixture();
        let root = temp.path();
        let _scope = controller(root, &mut guard, &parent);
        check_controller(root).unwrap();
        seat_authority::validate_child_delegation(root, "reviewer").unwrap();
        let store = Store::open(root, "github:github.com/owner/repo").unwrap();
        assert!(matches!(
            select(&store, &change(1), None, 2).unwrap(),
            Action::Dispatch(_)
        ));
        let next = successor(root, &parent);
        rebind(root, "default", &next).unwrap();
        let mut visited = Vec::new();
        let error = watch_changes(&store, vec![change(2), change(3)], |c| {
            visited.push(c.id);
            select(&store, &c, None, 2).map(|_| ())
        })
        .unwrap_err();
        assert!(stops_watcher(&error), "{error:#}");
        assert!(format!("{error:#}").contains("superseded"));
        assert_eq!(visited, [2]);
        assert!(stops_watcher(&check_controller(root).unwrap_err()));
    }

    #[test]
    fn story2571_watch_change_checks_controller_reviewer_delegation_before_forge() {
        let (temp, mut guard, parent) = fixture();
        let root = temp.path();
        let narrow = seat_authority::issue_launch_child_with_delegation(
            root,
            "advisor",
            "narrow-controller",
            false,
            &["advisor".into()],
        )
        .unwrap();
        let _scope = controller(root, &mut guard, &parent);
        let dir = directory(root, false).unwrap().unwrap();
        let mut state = read(&dir).unwrap().unwrap();
        state.child_grant = Some(narrow.id.clone());
        write(&dir, &state).unwrap();
        guard.set_key(seat_authority::GRANT_ENV, &narrow.id);
        guard.set_key("AIDA_USER", &narrow.subject);
        guard.set_key("AIDA_AGENT_NAME", &narrow.subject);
        CONTROLLER.with(|id| *id.borrow_mut() = Some(narrow.id.clone()));
        check_controller(root).unwrap();
        for args in [
            vec!["init", "-q"],
            vec![
                "remote",
                "add",
                "origin",
                "https://github.com/owner/repo.git",
            ],
        ] {
            assert!(Command::new("git")
                .current_dir(root)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        let repo = forge::Repository::load(root).unwrap();
        let store = Store::open(root, &repo.identity).unwrap();
        let args = ReviewWatchArgs {
            once: true,
            interval: 10,
            max_concurrent: 2,
            vendor: None,
            release: None,
            launch_reservation: None,
        };
        let error = watch_change(
            root,
            &repo,
            &store,
            &args,
            &mut Vec::new(),
            &update::AuthorLiveness::default(),
            change(1),
            None,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("reviewer"), "{error:#}");
        assert!(!stops_watcher(&error), "delegation refusal remains per-PR");
        assert!(
            store.load().unwrap().entries.is_empty(),
            "no PR admitted before delegation validation"
        );
    }

    #[test]
    fn story2571_launcher_bootstraps_and_takes_over_operator_binding() {
        let (temp, _guard, operator) = fixture();
        let root = temp.path();
        assert!(Command::new("git")
            .current_dir(root)
            .args(["init", "-q"])
            .status()
            .unwrap()
            .success());
        // An unused service with no deputy must not warn on advisor launch.
        rebind_after_launch(root, &operator).unwrap();
        rebind_after_launch_with(root, &operator, false, |_, _, _, _| {
            panic!("no deputy or service: do not send a request")
        })
        .unwrap();
        assert!(
            rebind_after_launch_with(root, &operator, true, |_, _, _, configured| {
                assert!(!configured);
                anyhow::bail!("deputy answered with a refusal")
            })
            .is_err()
        );
        let send =
            |deputy: &str, root: &Path, grant: &SeatGrant, _: bool| rebind(root, deputy, grant);
        rebind_after_launch_with(root, &operator, true, send).unwrap();
        assert!(
            rebind_after_launch_with(root, &operator, false, |_, _, _, configured| {
                assert!(configured);
                anyhow::bail!("bound deputy unavailable")
            })
            .is_err()
        );
        let next = seat_authority::issue_launch_child_with_delegation(
            root,
            "advisor",
            "new-advisor-handle",
            false,
            &[],
        )
        .unwrap();
        rebind_after_launch_with(root, &next, false, send).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        assert_eq!(
            read(&dir).unwrap().unwrap().binding.subject,
            "new-advisor-handle"
        );
        assert!(rebind_after_launch_with(root, &operator, true, send).is_err());
        assert_eq!(read(&dir).unwrap().unwrap().binding.grant, next.id);
        let source = include_str!("agent_launch.rs");
        assert!(source.contains("service::rebind_after_launch(&plan.project_root, &grant)"));
    }

    #[test]
    fn story2571_unchanged_tick_does_not_replace_service_or_marker() {
        use std::os::unix::fs::MetadataExt;
        let (temp, _guard, parent) = fixture();
        rebind(temp.path(), "default", &parent).unwrap();
        let dir = directory(temp.path(), false).unwrap().unwrap();
        let mut child = None;
        tick(temp.path(), "default", false, &mut child).unwrap();
        let receipt = std::fs::metadata(dir.path().join(SERVICE)).unwrap();
        let marker = std::fs::metadata(dir.path().join("service-initialized")).unwrap();
        tick(temp.path(), "default", false, &mut child).unwrap();
        assert_eq!(
            std::fs::metadata(dir.path().join(SERVICE)).unwrap().ino(),
            receipt.ino()
        );
        assert_eq!(
            std::fs::metadata(dir.path().join("service-initialized"))
                .unwrap()
                .modified()
                .unwrap(),
            marker.modified().unwrap()
        );
    }

    // The real persisted binding moves forward while an active/ambiguous
    // controller receipt and ordinary PR reservations survive untouched.
    #[test]
    fn story2571_rotation_rebind_preserves_receipts_and_refuses_stale_successor() {
        let (temp, _guard, first) = fixture();
        let root = temp.path();
        rebind(root, "default", &first).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        let mut state = read(&dir).unwrap().unwrap();
        state.worker = Some(Identity::current().unwrap());
        let child =
            seat_authority::issue_review_watch_child(root, &first.id, &first.subject).unwrap();
        state.child_grant = Some(child.id.clone());
        check_binding(root, &state, &child).unwrap();
        write(&dir, &state).unwrap();
        let store = Store::open(root, "github:github.com/owner/repo").unwrap();
        let change = forge::Change {
            id: 1,
            head: "1".repeat(40),
            base: "main".into(),
            author: "writer".into(),
            gitlab: false,
            overlap: Some(false),
            behind: Some(0),
            head_branch: None,
        };
        assert!(matches!(
            select(&store, &change, None, 2).unwrap(),
            Action::Dispatch(_)
        ));
        let before = std::fs::read(root.join(".aida/review-watch/state.json")).unwrap();
        let next = successor(root, &first);
        rebind(root, "default", &next).unwrap();
        let rebound = read(&dir).unwrap().unwrap();
        assert_eq!(rebound.binding.grant, next.id);
        assert_eq!(rebound.worker, state.worker);
        assert_eq!(rebound.child_grant, state.child_grant);
        assert!(check_binding(root, &rebound, &child)
            .unwrap_err()
            .to_string()
            .contains("superseded"));
        let mut replacement = None;
        tick(root, "default", true, &mut replacement).unwrap();
        assert!(
            replacement.is_none(),
            "a live prior controller must block replacement"
        );
        reconcile_reviews(root, &dir).unwrap();
        assert!(rebind(root, "default", &first)
            .unwrap_err()
            .to_string()
            .contains("stale"));
        assert_eq!(read(&dir).unwrap().unwrap().binding.grant, next.id);
        assert_eq!(
            std::fs::read(root.join(".aida/review-watch/state.json")).unwrap(),
            before
        );
    }

    #[test]
    // Rotation must recover from an invalid old parent without borrowing its
    // authority or discarding unresolved reviews. trace:STORY-2571 | ai:codex
    fn story2571_rotation_recovers_revoked_parent_after_controller_exit() {
        let (temp, _guard, first) = fixture();
        let root = temp.path();
        rebind(root, "default", &first).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        let child =
            seat_authority::issue_review_watch_child(root, &first.id, &first.subject).unwrap();
        let mut process = Command::new("sh").args(["-c", "exit 0"]).spawn().unwrap();
        let mut state = read(&dir).unwrap().unwrap();
        state.worker = Some(Identity::child(&process).unwrap());
        state.child_grant = Some(child.id.clone());
        write(&dir, &state).unwrap();
        process.wait().unwrap();

        let store = Store::open(root, "github:github.com/owner/repo").unwrap();
        assert!(matches!(
            select(&store, &change(1), None, 2).unwrap(),
            Action::Dispatch(_)
        ));
        let reservations = std::fs::read(dir.path().join(STATE)).unwrap();
        let next = successor(root, &first);
        seat_authority::revoke_by_id(&first.id).unwrap();
        assert!(state.binding.validate(root).is_err());
        rebind(root, "default", &next).unwrap();

        let mut controller = None;
        tick(root, "default", true, &mut controller).unwrap();
        let rebound = read(&dir).unwrap().unwrap();
        assert_eq!(rebound.binding.validate(root).unwrap().id, next.id);
        assert!(rebound.worker.is_none());
        assert!(rebound.child_grant.is_none());
        assert_eq!(rebound.retired_grants, vec![child.id.clone()]);
        assert!(controller.is_none(), "restart observes ordinary backoff");
        reconcile_reviews(root, &dir).unwrap();
        assert_eq!(std::fs::read(dir.path().join(STATE)).unwrap(), reservations);
        let mut reviews = store.load().unwrap();
        reviews.entries.get_mut("PR-1").unwrap().attempts[0].phase = Phase::Exited;
        dir.write(STATE, &serde_json::to_vec_pretty(&reviews).unwrap())
            .unwrap();
        tick(root, "default", true, &mut controller).unwrap();
        let stopped = read(&dir).unwrap().unwrap();
        assert!(stopped.child_grant.is_none());
        assert!(stopped.retry_after > Utc::now());
        assert!(seat_authority::read_grant(&child.id)
            .unwrap()
            .revoked_at
            .is_some());
        assert!(rebind(root, "default", &first).is_err());
        assert_eq!(read(&dir).unwrap().unwrap().binding.grant, next.id);
    }

    #[test]
    fn story2571_revoked_successor_and_widening_refuse_without_mutation() {
        let (temp, _guard, first) = fixture();
        rebind(temp.path(), "default", &first).unwrap();
        let next = successor(temp.path(), &first);
        seat_authority::revoke_by_id(&next.id).unwrap();
        assert!(rebind(temp.path(), "default", &next).is_err());
        let mut wide = Binding::from_grant(&first);
        wide.grant = "another".into();
        wide.issued_at += chrono::Duration::seconds(1);
        wide.delegations.push("implementer".into());
        assert!(wide.succeeds(&Binding::from_grant(&first)).is_err());
        let dir = directory(temp.path(), false).unwrap().unwrap();
        assert_eq!(read(&dir).unwrap().unwrap().binding.grant, first.id);
    }

    #[test]
    fn story2571_ambiguous_spawn_is_not_replaced_and_unsafe_state_refuses() {
        use std::os::unix::fs::symlink;
        let (temp, _guard, first) = fixture();
        rebind(temp.path(), "default", &first).unwrap();
        let dir = directory(temp.path(), false).unwrap().unwrap();
        let mut state = read(&dir).unwrap().unwrap();
        state.child_grant = Some("ambiguous".into());
        write(&dir, &state).unwrap();
        let mut child = None;
        tick(temp.path(), "default", true, &mut child).unwrap();
        assert!(child.is_none());
        assert_eq!(
            read(&dir).unwrap().unwrap().child_grant.as_deref(),
            Some("ambiguous")
        );
        std::fs::remove_file(dir.path().join(SERVICE)).unwrap();
        symlink("missing", dir.path().join(SERVICE)).unwrap();
        assert!(rebind(temp.path(), "default", &first).is_err());
    }

    #[test]
    fn story2571_pr_refusal_keeps_next_pr_and_next_pass_available() {
        let (temp, mut guard, first) = fixture();
        let _scope = controller(temp.path(), &mut guard, &first);
        let store = Store::open(temp.path(), "github:github.com/owner/repo").unwrap();
        let change = |id| forge::Change {
            id,
            head: format!("{id:040x}"),
            base: "main".into(),
            author: "writer".into(),
            gitlab: false,
            overlap: Some(false),
            behind: Some(0),
            head_branch: None,
        };
        let mut handled = Vec::new();
        for _ in 0..2 {
            watch_changes(&store, vec![change(1), change(2)], |c| {
                check_controller(temp.path())?;
                seat_authority::validate_child_delegation(temp.path(), "reviewer")?;
                let mut c = c;
                if c.id == 1 {
                    c.overlap = None;
                }
                select(&store, &c, None, 2)?;
                handled.push(c.id);
                Ok(())
            })
            .unwrap();
        }
        assert_eq!(handled, [2, 2]);
        let dir = directory(temp.path(), false).unwrap().unwrap();
        assert_eq!(read(&dir).unwrap().unwrap().binding.grant, first.id);
    }
}
