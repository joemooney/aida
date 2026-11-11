// Shared fixed-service engine. Each inclusion has independent controller TLS.
// trace:STORY-2691 | ai:codex
use super::*;
use crate::advisor_cmd_root::AdvisorLoopsCommand;
use crate::gate::seat_authority::{self, SeatGrant};
use chrono::{DateTime, Utc};
use std::sync::atomic::Ordering;

const SERVICE: &str = "service.json";
const NAME: &str = if TRAIN { "auto-train" } else { "review-watch" };
// Bound retained ancestry even if startup repeatedly fails behind a review
// whose descendant evidence needs operator recovery.
// trace:STORY-2571 | ai:codex
const MAX_RETIRED_GRANTS: usize = 4;

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
            "advisor loop is blocked-authority; rebind from the current advisor session",
        )?;
        anyhow::ensure!(
            grant.seat == "advisor"
                && grant.principal == self.principal
                && grant.issued_at == self.issued_at
                && grant.delegable_seats == self.delegations,
            "advisor loop binding no longer matches its grant"
        );
        anyhow::ensure!(
            grant.delegable_seats.iter().any(|s| s == "advisor")
                && grant.delegable_seats.iter().any(|s| s == "reviewer"),
            "advisor loop needs advisor and reviewer child delegation"
        );
        Ok(grant)
    }
    fn succeeds(&self, old: &Self) -> Result<()> {
        if self.grant == old.grant {
            return Ok(());
        }
        anyhow::ensure!(
            self.principal == old.principal && self.issued_at > old.issued_at,
            "stale advisor loop successor refused"
        );
        anyhow::ensure!(
            self.delegations.iter().all(|s| old.delegations.contains(s)),
            "advisor loop successor widens delegation; see the operator recovery procedure in docs/cli/06-roles-sessions.md#review-watch-service-recovery"
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
        return Directory::open(&runtime.path().join(NAME)).map(Some);
    }
    let Some(runtime) = Directory::existing(&root.join(".aida"))? else {
        return Ok(None);
    };
    let dir = Directory::existing(&runtime.path().join(NAME))?;
    if let Some(dir) = &dir {
        anyhow::ensure!(
            dir.handle.metadata()?.mode() & 0o022 == 0,
            "unsafe advisor loop service directory"
        );
    }
    Ok(dir)
}
fn read(dir: &Directory) -> Result<Option<Service>> {
    let Some(bytes) = dir.read(SERVICE)? else {
        anyhow::ensure!(dir.read("service-initialized")?.is_none(), "advisor loop service receipt is missing after initialization; follow docs/cli/06-roles-sessions.md#review-watch-service-recovery");
        return Ok(None);
    };
    let state: Service =
        serde_json::from_slice(&bytes).context("invalid advisor loop service receipt")?;
    anyhow::ensure!(
        state.version == 1 && !state.deputy.is_empty(),
        "unsupported advisor loop service receipt"
    );
    anyhow::ensure!(
        state.worker.is_none() || state.child_grant.is_some(),
        "incomplete advisor loop controller receipt"
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
    let dir = directory(root, true)?.context("missing advisor loop directory")?;
    let _lock = dir.lock("admission.lock")?;
    // Revalidate at the locked acceptance boundary, including the full parent chain.
    binding.validate(root)?;
    let state = match read(&dir)? {
        Some(mut state) => {
            anyhow::ensure!(
                state.deputy == deputy,
                "advisor loop belongs to another deputy"
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
                        "advisor loop sub-advisor cannot replace its ancestor binding"
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
        AdvisorLoopsCommand::Status { json } | AdvisorLoopsCommand::TrainStatus { json } => {
            let Some(dir) = directory(&root, false)? else {
                print_status("not-configured", None, &[], *json)?;
                return Ok(());
            };
            let _lock = dir.lock("admission.lock")?;
            let Some(state) = read(&dir)? else {
                print_status("not-configured", None, &[], *json)?;
                return Ok(());
            };
            let status = if let Some(status) = host_refusal(&state) {
                status
            } else if state.binding.validate(&root).is_err() {
                "blocked-authority"
            } else if state.child_grant.is_some() && state.worker.is_none() {
                "blocked-ambiguous-spawn"
            } else if state.worker.as_ref().is_some_and(Identity::alive) {
                "running"
            } else if state.retired_grants.len() >= MAX_RETIRED_GRANTS {
                "blocked-retired-controllers"
            } else {
                "waiting-for-supervisor"
            };
            let mut blockers = review_blockers(&dir)?;
            blockers.extend(retry_blockers(&root, &dir)?);
            print_status(status, Some(&state), &blockers, *json)?;
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
            println!("advisor loops: bindings accepted; supervisors will reconcile prior controllers before restart");
            Ok(())
        }
        AdvisorLoopsCommand::Worker | AdvisorLoopsCommand::TrainWorker => worker(&root),
    }
}

// trace:STORY-2571 | ai:codex
fn print_status(
    status: &str,
    state: Option<&Service>,
    blockers: &[String],
    json: bool,
) -> Result<()> {
    let root = forge::main_root(&std::env::current_dir()?)?;
    let failure = failure_state(&root)?;
    let value = serde_json::json!({
        "service": NAME, "status": status,
        "consecutive_failures": failure.count, "alert": failure.alert,
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
        println!("{NAME}: {status}; deputy {}; binding issued {}; controller {}; last exit {}; retired controllers {}; review blockers {}; recovery {}; consecutive failures {}; alert {}",
            value["deputy"], value["binding_issued_at"], value["controller"], value["last_exit"], value["retired_controllers"], value["review_blockers"], value["recovery"], value["consecutive_failures"], value["alert"]);
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
    let dir = directory(root, false)?.context("advisor loop service state disappeared")?;
    let _lock = dir.lock("admission.lock")?;
    check_controller_locked(root, &dir)
}

// Service controllers validate delegation before per-PR work. Ordinary
// watchers may still finish superseding obsolete heads after authority narrows.
// trace:STORY-2571 | ai:codex
pub(super) fn validate_controller_delegation(root: &Path) -> Result<()> {
    if CONTROLLER.with(|id| id.borrow().is_some()) {
        seat_authority::validate_child_delegation(root, "reviewer")?;
    }
    Ok(())
}

// Called while ordinary review admission already holds admission.lock. Rebind
// and reservation creation thus compare the generation at the same boundary.
// trace:STORY-2571 | ai:codex
pub(super) fn check_controller_locked(root: &Path, dir: &Directory) -> Result<()> {
    CONTROLLER
        .with(|id| {
            let id = id.borrow();
            let Some(id) = id.as_ref() else { return Ok(()) };
            let state = read(dir)?.context("advisor loop service state disappeared")?;
            anyhow::ensure!(
                host_fresh(&state),
                "advisor loop deputy authority ended or its heartbeat is stale"
            );
            let child = seat_authority::grant_check(root)
                .into_valid()
                .context("advisor loop controller authority ended")?;
            anyhow::ensure!(child.id == *id, "advisor loop controller grant changed");
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
        "advisor loop controller binding superseded"
    );
    state.binding.validate(root)?;
    Ok(())
}

fn worker(root: &Path) -> Result<()> {
    let child = seat_authority::grant_check(root)
        .into_valid()
        .context("advisor loop worker needs its child grant")?;
    let own = Identity::current()?;
    // Parent writes the verified child receipt before any discovery/admission.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let dir = directory(root, false)?.context("advisor loop service state missing")?;
        let ready = {
            let _lock = dir.lock("admission.lock")?;
            let state = read(&dir)?.context("advisor loop service state missing")?;
            anyhow::ensure!(
                state.child_grant.as_ref() == Some(&child.id),
                "unexpected advisor loop worker grant"
            );
            state.worker.as_ref() == Some(&own)
        };
        if ready {
            break;
        }
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "advisor loop controller receipt not confirmed"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    CONTROLLER.with(|id| *id.borrow_mut() = Some(child.id));
    if TRAIN {
        return super::train::run(root);
    }
    run_with_program(
        &ReviewWatchArgs {
            once: false,
            interval: 10,
            max_concurrent: Some(2),
            vendor: None,
            release: None,
            launch_reservation: None,
        },
        root,
        None,
    )
    .map(|_| ())
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
                    if let Err(alert_error) = record_failure(&guard.project, Some(&reason)) {
                        eprintln!(
                            "{NAME}: could not persist/deliver service alert: {alert_error:#}"
                        );
                    }
                    if reported.as_ref() != Some(&reason) {
                        eprintln!("{NAME} supervisor refused: {reason}");
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
fn host_refusal(state: &Service) -> Option<&'static str> {
    if !state.host_active {
        return Some("blocked-deputy");
    }
    match state.host.as_ref().map(Identity::verdict) {
        Some(aida_core::proc_identity::Verdict::Alive) => {
            if state.checked_at.is_some_and(|at| {
                at <= Utc::now() && Utc::now() - at < chrono::Duration::seconds(5)
            }) {
                None
            } else {
                Some("blocked-deputy")
            }
        }
        Some(aida_core::proc_identity::Verdict::Dead(_)) => Some("blocked-deputy"),
        _ => Some("unknown"),
    }
}

fn host_fresh(state: &Service) -> bool {
    host_refusal(state).is_none()
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
    // individually block replacement, nor justify revoking possible reviewer
    // descendants. Repeated failures still obey the bounded retirement cap.
    // trace:STORY-2571 | ai:codex
    if exited {
        let rotated = state
            .child_grant
            .as_ref()
            .and_then(|id| seat_authority::read_grant(id).ok())
            .is_some_and(|child| child.parent_grant_id.as_ref() != Some(&state.binding.grant));
        if !rotated && state.last_exit != Some(0) {
            record_failure(root, Some("controller exited before successful dispatch"))?;
        }
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
    if !host_valid
        || state.child_grant.is_some()
        || state.retired_grants.len() >= MAX_RETIRED_GRANTS
        || Utc::now() < state.retry_after
    {
        return Ok(());
    }
    let parent = state.binding.validate(root)?;
    // Prepare fallible I/O before minting authority. Every failed attempt
    // backs off; a minted grant is revoked if reservation persistence fails.
    // trace:STORY-2571 | ai:codex
    let mut minted = None;
    let prepared = (|| {
        reconcile_reviews(root, &dir)?;
        let log = dir.file("service.log", true)?;
        let stdout = log.try_clone()?;
        let (grant, mut command) = launch(root, &parent)?;
        minted = Some(grant.id.clone());
        state.child_grant = Some(grant.id.clone());
        write(&dir, &state)?; // reserve before spawn; a crash retains ambiguity
        command
            .current_dir(root)
            .args([
                "advisor",
                "loops",
                if TRAIN { "train-worker" } else { "worker" },
            ])
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(log);
        Ok::<_, anyhow::Error>((grant, command))
    })();
    let (grant, mut command) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            state.retry_after = Utc::now() + chrono::Duration::seconds(10);
            let revoked = minted.as_ref().map(|id| seat_authority::revoke_by_id(id));
            if revoked.as_ref().is_none_or(|result| result.is_ok()) {
                state.child_grant = None;
            }
            let saved = write(&dir, &state);
            if let Some(result) = revoked {
                result?;
            }
            saved?;
            return Err(error);
        }
    };
    match command.spawn() {
        Ok(process) => {
            // Keep the handle even if recording fails: never reap first and
            // turn a fast-exiting worker into an ambiguous capture.
            *child = Some(process);
            state.worker = Some(Identity::child(child.as_ref().unwrap())?);
            write(&dir, &state)
        }
        Err(error) => {
            let revoked = seat_authority::revoke_by_id(&grant.id);
            if revoked.is_ok() {
                state.child_grant = None;
            }
            state.retry_after = Utc::now() + chrono::Duration::seconds(10);
            let saved = write(&dir, &state);
            revoked?;
            saved?;
            Err(error.into())
        }
    }
}

// Retired grants protect reviews; the cap bounds controller replacement.
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
    if TRAIN {
        return Ok(Vec::new());
    }
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

// Exhaustion is informational and must not retain retired grants.
// trace:BUG-2518 | ai:codex
pub(super) fn retry_blockers(root: &Path, dir: &Directory) -> Result<Vec<String>> {
    if TRAIN {
        return Ok(Vec::new());
    }
    let Some(bytes) = dir.read(STATE)? else {
        return Ok(Vec::new());
    };
    let reviews: State = serde_json::from_slice(&bytes)?;
    let store = Store::open(root, &reviews.repository)?;
    let mut blockers = Vec::new();
    for (key, entry) in &reviews.entries {
        for attempt in &entry.attempts {
            if let Some(reason) = no_verdict_blocker(attempt) {
                if !store.history(key)?.contains(&attempt.head) {
                    blockers.push(format!("{key}: {reason}"));
                }
            }
        }
    }
    Ok(blockers)
}

// Reuse the ordinary reservation recovery while holding admission.lock. A
// controller restart never clears an unrecorded or live review descendant.
// trace:STORY-2571 | ai:codex
fn reconcile_reviews(root: &Path, dir: &Directory) -> Result<()> {
    if TRAIN {
        return Ok(());
    }
    let Some(bytes) = dir.read(STATE)? else {
        anyhow::ensure!(
            dir.read("initialized")?.is_none(),
            "advisor loop state disappeared after initialization"
        );
        return Ok(());
    };
    let state: State = serde_json::from_slice(&bytes)
        .context("invalid advisor loop state before service restart")?;
    let store = Store {
        runtime: Directory::existing(&dir.path().join(".."))?
            .context("advisor loop runtime disappeared")?,
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

// Durable failure episodes: stable notice IDs make retries idempotent across
// restarts. Success starts a new episode; backoff ticks never clear failures.
// trace:STORY-2691 | ai:codex
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Failures {
    #[serde(default)]
    dispatch_pending: bool,
    count: u32,
    episode: String,
    alert: Option<Notice>,
}
fn failure_state(root: &Path) -> Result<Failures> {
    let Some(dir) = directory(root, false)? else {
        return Ok(Failures::default());
    };
    dir.read("failures.json")?
        .map(|b| serde_json::from_slice(&b).map_err(Into::into))
        .transpose()
        .map(|v| v.unwrap_or_default())
}
pub(super) fn record_failure(root: &Path, reason: Option<&str>) -> Result<()> {
    // Supervisor observations prove no dispatch success, even on healthy ticks.
    // Retain pending failures until the controller confirms a successful dispatch.
    record_outcome(root, reason, None)
}
fn record_outcome(root: &Path, reason: Option<&str>, dispatched: Option<bool>) -> Result<()> {
    let dir = directory(root, true)?.context("service directory missing")?;
    let _lock = dir.lock("failures.lock")?;
    let mut state = failure_state(root)?;
    match dispatched {
        Some(false) => state.dispatch_pending = true,
        Some(true) => state.dispatch_pending = false,
        None => {}
    }
    if let Some(reason) = reason {
        if state.episode.is_empty() {
            state.episode = uuid::Uuid::new_v4().to_string();
        }
        state.count = state.count.saturating_add(1);
        if state.count >= 3 && state.alert.is_none() {
            state.alert = Some(Notice {
                message: notice::message(format!("advisor-loop-{NAME}-{}", state.episode), "advisor",
                    format!("{NAME}: repeated service failure"),
                    format!("{NAME} failed {} consecutive times. Last failure: {}. Inspect aida advisor loops status and train-status, and .aida/{NAME}/service.log; renew/rebind the advisor grant if authority ended.", state.count, bounded_diagnostic(reason))),
                sent: false,
            });
        }
    } else if !state.dispatch_pending {
        // Success resets bookkeeping, but cannot discard an undelivered notice.
        let pending = state.alert.take().filter(|alert| !alert.sent);
        state = Failures {
            alert: pending,
            ..Failures::default()
        };
    }
    if let Some(alert) = state.alert.as_mut() {
        alert.message.from = NAME.into();
    }
    dir.write("failures.json", &serde_json::to_vec_pretty(&state)?)?;
    if let Some(alert) = state.alert.as_mut().filter(|a| !a.sent) {
        deliver(root, alert)?;
        alert.sent = true;
        dir.write("failures.json", &serde_json::to_vec_pretty(&state)?)?;
    }
    Ok(())
}

#[cfg(test)]
mod loop_tests {
    use super::*;

    // trace:STORY-2691 | ai:codex
    #[test]
    fn story2691_synchronous_dispatch_failures_survive_idle() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        CONTROLLER.with(|id| *id.borrow_mut() = Some("fixture".into()));
        for count in 1..=3 {
            controller_dispatch(root, Some("spawn refused"), false).unwrap();
            controller_pass(root, None).unwrap();
            assert_eq!(failure_state(root).unwrap().count, count);
        }
        assert!(failure_state(root).unwrap().alert.unwrap().sent);
        controller_dispatch(root, None, true).unwrap();
        assert_eq!(failure_state(root).unwrap().count, 0);
        CONTROLLER.with(|id| *id.borrow_mut() = None);
    }

    // trace:STORY-2691 | ai:codex
    #[test]
    fn story2691_supervisor_restart_failure_preserves_pending_dispatch() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        CONTROLLER.with(|id| *id.borrow_mut() = Some("old-controller".into()));
        controller_dispatch(root, Some("spawn refused"), false).unwrap();
        CONTROLLER.with(|id| *id.borrow_mut() = None);
        record_failure(root, Some("controller exited before successful dispatch")).unwrap();
        assert!(failure_state(root).unwrap().dispatch_pending);
        // A healthy supervisor tick is not evidence of a successful dispatch.
        record_failure(root, None).unwrap();
        assert_eq!(failure_state(root).unwrap().count, 2);
        assert!(failure_state(root).unwrap().dispatch_pending);
        CONTROLLER.with(|id| *id.borrow_mut() = Some("replacement-controller".into()));
        controller_pass(root, None).unwrap();
        assert_eq!(failure_state(root).unwrap().count, 2);
        controller_dispatch(root, None, true).unwrap();
        assert_eq!(failure_state(root).unwrap().count, 0);
        CONTROLLER.with(|id| *id.borrow_mut() = None);
    }

    // trace:STORY-2691 | ai:codex
    #[test]
    fn story2691_three_synchronous_failures_in_one_pass_alert() {
        if TRAIN {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        CONTROLLER.with(|id| *id.borrow_mut() = Some("fixture".into()));
        let store = Store::open(root, "github:github.com/owner/repo").unwrap();
        let changes = (1..=3)
            .map(|id| forge::Change {
                id,
                head: format!("{id:040x}"),
                base: "main".into(),
                author: "writer".into(),
                gitlab: false,
                overlap: Some(false),
                behind: Some(0),
                head_branch: None,
            })
            .collect();
        let mut observed = 0;
        super::super::watch_changes(&store, changes, |_| {
            observed += 1;
            let result = super::super::observe_dispatch_result(
                root,
                Err(anyhow::anyhow!("spawn {observed} refused")),
            );
            assert_eq!(failure_state(root).unwrap().count, observed);
            result
        })
        .unwrap();
        controller_pass(root, None).unwrap();
        let state = failure_state(root).unwrap();
        assert_eq!(state.count, 3);
        assert!(state.dispatch_pending);
        assert!(state.alert.unwrap().sent);
        CONTROLLER.with(|id| *id.borrow_mut() = None);
    }

    // Reap actual asynchronous children, rather than simulating record_failure calls.
    // trace:STORY-2691 | ai:codex
    #[cfg(target_os = "linux")]
    #[test]
    fn story2691_failed_dispatch_children_raise_a_durable_alert() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let store = Store::open(root, "github:github.com/owner/repo").unwrap();
        for batch in [2, 1] {
            let mut children = Vec::new();
            for _ in 0..batch {
                let mut child = std::process::Command::new("/bin/sh")
                    .args(["-c", "exit 23"])
                    .spawn()
                    .unwrap();
                child.wait().unwrap();
                children.push(super::super::Dispatcher {
                    child,
                    key: "PR-7".into(),
                    token: "missing".into(),
                });
            }
            let outcomes = super::super::reap_dispatchers(&store, &mut children);
            assert_eq!(outcomes.len(), batch);
            assert!(children.is_empty());
            for failure in outcomes {
                record_outcome(root, Some(&failure.unwrap()), Some(false)).unwrap();
            }
            record_outcome(root, None, None).unwrap();
        }
        assert!(failure_state(root).unwrap().alert.unwrap().sent);
        // Reloading the durable episode and an idle pass cannot erase it.
        record_outcome(root, None, None).unwrap();
        assert_eq!(failure_state(root).unwrap().count, 3);
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        child.wait().unwrap();
        let mut children = vec![super::super::Dispatcher {
            child,
            key: "PR-7".into(),
            token: "missing".into(),
        }];
        let outcomes = super::super::reap_dispatchers(&store, &mut children);
        assert_eq!(outcomes, vec![None]);
        record_outcome(root, None, Some(true)).unwrap();
        assert_eq!(failure_state(root).unwrap().count, 0);
    }

    // trace:STORY-2691 | ai:codex
    #[test]
    fn story2691_discovery_and_sync_failures_alert_then_recover() {
        if TRAIN {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        CONTROLLER.with(|id| *id.borrow_mut() = Some("fixture".into()));
        for label in ["discovery", "synchronization", "discovery"] {
            assert!(super::super::observe_pass_input::<()>(
                root,
                label,
                Err(anyhow::anyhow!("authentication unavailable"))
            )
            .is_err());
        }
        assert_eq!(failure_state(root).unwrap().count, 3);
        assert!(failure_state(root).unwrap().alert.unwrap().sent);
        super::super::observe_pass_input(root, "discovery", Ok(())).unwrap();
        controller_pass(root, None).unwrap();
        assert_eq!(failure_state(root).unwrap().count, 0);
        CONTROLLER.with(|id| *id.borrow_mut() = None);
    }

    // Both fixed services exercise the same durable alert state machine.
    // trace:STORY-2691 | ai:codex
    #[test]
    fn story2691_alert_once_per_failure_episode_survives_restart() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for n in 1..=5 {
            record_failure(root, Some("dispatch authority refused")).unwrap();
            assert_eq!(failure_state(root).unwrap().count, n);
        }
        let first = failure_state(root).unwrap().alert.unwrap();
        assert!(first.sent);
        record_failure(root, Some("another failure")).unwrap();
        assert_eq!(
            failure_state(root).unwrap().alert.unwrap().message.id,
            first.message.id
        );
        let count = || {
            std::fs::read_dir(root.join(".aida/mailbox"))
                .unwrap()
                .count()
        };
        assert_eq!(count(), 1);
        record_outcome(root, None, Some(true)).unwrap();
        assert_eq!(failure_state(root).unwrap().count, 0);
        for _ in 0..3 {
            record_failure(root, Some("new episode")).unwrap();
        }
        assert_ne!(
            failure_state(root).unwrap().alert.unwrap().message.id,
            first.message.id
        );
        assert_eq!(count(), 2);
    }

    // trace:STORY-2691 | ai:codex
    #[test]
    fn story2691_unsent_alert_retries_after_restart_before_idle_or_success() {
        for success in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            CONTROLLER.with(|id| *id.borrow_mut() = Some("old".into()));
            controller_dispatch(root, Some("failed"), false).unwrap();
            controller_dispatch(root, Some("failed"), false).unwrap();
            let mailbox = root.join(".aida/mailbox");
            std::fs::write(&mailbox, "temporarily unavailable").unwrap();
            assert!(controller_dispatch(root, Some("failed"), false).is_err());
            let original = failure_state(root).unwrap().alert.unwrap();
            assert!(!original.sent);
            CONTROLLER.with(|id| *id.borrow_mut() = Some("replacement".into()));
            assert!(controller_pass(root, None).is_err());
            if success {
                assert!(controller_dispatch(root, None, true).is_err());
                let saved = failure_state(root).unwrap();
                assert_eq!(saved.count, 0);
                assert!(!saved.dispatch_pending);
                assert_eq!(saved.alert.unwrap().message.id, original.message.id);
            }
            std::fs::remove_file(&mailbox).unwrap();
            if success {
                controller_dispatch(root, None, true).unwrap();
            } else {
                controller_pass(root, None).unwrap();
                assert!(failure_state(root).unwrap().alert.unwrap().sent);
                controller_dispatch(root, None, true).unwrap();
            }
            controller_pass(root, None).unwrap();
            let entries: Vec<_> = std::fs::read_dir(&mailbox).unwrap().collect();
            assert_eq!(entries.len(), 1);
            let message: serde_json::Value = serde_json::from_slice(
                &std::fs::read(entries[0].as_ref().unwrap().path()).unwrap(),
            )
            .unwrap();
            assert_eq!(message["id"], original.message.id);
            assert_eq!(failure_state(root).unwrap().count, 0);
            CONTROLLER.with(|id| *id.borrow_mut() = None);
        }
    }

    // Rotation changes only the binding, never a live worker or its grant.
    // It observes supersession at the next pass, after any current train.
    // trace:STORY-2691 | ai:codex
    #[test]
    fn story2691_rotation_preserves_live_worker_until_pass_boundary() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut env = crate::test_env::AmbientGuard::hermetic_with_seat(
            root,
            "advisor",
            &["advisor", "reviewer"],
        );
        let parent = seat_authority::grant_check(root).into_valid().unwrap();
        rebind(root, "default", &parent).unwrap();
        let child =
            seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        let mut state = read(&dir).unwrap().unwrap();
        state.child_grant = Some(child.id.clone());
        state.worker = Some(Identity::current().unwrap());
        state.host = state.worker.clone();
        state.checked_at = Some(Utc::now());
        write(&dir, &state).unwrap();
        let id = seat_authority::test_support::mint_grant_for(
            root,
            &parent.subject,
            "advisor",
            &["advisor", "reviewer"],
        );
        let next = seat_authority::read_grant(&id).unwrap();
        rebind(root, "default", &next).unwrap();
        let after = read(&dir).unwrap().unwrap();
        assert_eq!(after.child_grant, state.child_grant);
        assert_eq!(after.worker, state.worker);
        assert_eq!(after.binding.grant, next.id);
        env.set_key(seat_authority::GRANT_ENV, &child.id);
        env.set_key("AIDA_USER", &child.subject);
        env.set_key("AIDA_AGENT_NAME", &child.subject);
        CONTROLLER.with(|id| *id.borrow_mut() = Some(child.id.clone()));
        let refused = check_controller(root).unwrap_err();
        CONTROLLER.with(|id| *id.borrow_mut() = None);
        assert!(format!("{refused:#}").contains("superseded"));
        tick_with_launcher(root, "default", true, &mut None, |_, _| {
            panic!("must not spawn over live worker")
        })
        .unwrap();
        assert!(seat_authority::read_grant(&child.id).is_ok());
    }
}

// trace:STORY-2691 | ai:codex
pub(super) fn controller_pass(root: &Path, failure: Option<&str>) -> Result<()> {
    if CONTROLLER.with(|id| id.borrow().is_some()) {
        record_outcome(root, failure, None)?;
    }
    Ok(())
}

// A failed asynchronous dispatch remains an episode across controller restarts.
// Only a confirmed successful dispatch clears that part of the episode.
// trace:STORY-2691 | ai:codex
pub(super) fn controller_dispatch(root: &Path, failure: Option<&str>, success: bool) -> Result<()> {
    if CONTROLLER.with(|id| id.borrow().is_some()) {
        if failure.is_some() {
            record_outcome(root, failure, Some(false))?;
        } else if success {
            record_outcome(root, None, Some(true))?;
        }
    }
    Ok(())
}
