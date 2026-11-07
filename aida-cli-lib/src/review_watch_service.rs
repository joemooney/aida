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
            "review-watch successor widens delegation; operator intervention required"
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
        anyhow::ensure!(dir.read("service-initialized")?.is_none(), "review-watch service receipt is missing after initialization; reconcile the controller before recovery");
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
    dir.file("service-initialized", true)?.sync_all()?;
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
            state.binding = binding;
            state
        }
        None => Service {
            version: 1,
            deputy: deputy.into(),
            binding,
            worker: None,
            child_grant: None,
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
        AdvisorLoopsCommand::Status => {
            let Some(dir) = directory(&root, false)? else {
                println!("review-watch: not configured");
                return Ok(());
            };
            let _lock = dir.lock("admission.lock")?;
            let Some(state) = read(&dir)? else {
                println!("review-watch: not configured");
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
            println!("review-watch: {status}; deputy {}; binding issued {}; controller {}; last exit {:?}",
                state.deputy, state.binding.issued_at,
                state.worker.as_ref().map(|w| w.pid.to_string()).unwrap_or_else(|| "none".into()), state.last_exit);
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
    state.host_active = host_valid;
    if host_valid {
        state.host = Some(Identity::current()?);
        state.checked_at = Some(Utc::now());
    }
    if let Some(process) = child.as_mut() {
        // The identity was captured before any try_wait/reap.
        if let Some(status) = process.try_wait()? {
            state.last_exit = status.code();
            state.worker = None;
            state.child_grant = None;
            state.retry_after = Utc::now() + chrono::Duration::seconds(10);
            *child = None;
        }
    } else if let Some(worker) = &state.worker {
        match worker.verdict() {
            aida_core::proc_identity::Verdict::Dead(_) => {
                state.worker = None;
                state.child_grant = None;
                state.retry_after = Utc::now() + chrono::Duration::seconds(10);
            }
            // Alive and unknown both retain the controller reservation.
            _ => {}
        }
    }
    write(&dir, &state)?;
    if !host_valid || state.child_grant.is_some() || Utc::now() < state.retry_after {
        return Ok(());
    }
    let parent = match state.binding.validate(root) {
        Ok(parent) => parent,
        Err(_) => return Ok(()),
    };
    reconcile_reviews(root, &dir)?;
    let (grant, mut command) =
        crate::launch::review_watch_controller(root, &parent.id, &parent.subject)?;
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
    let Some(dir) = directory(&root, false)? else {
        return Ok(());
    };
    let state = {
        let _lock = dir.lock("admission.lock")?;
        read(&dir)?
    };
    let Some(state) = state else { return Ok(()) };
    anyhow::ensure!(state.binding.subject == grant.subject, "review-watch binding belongs to another advisor handle; explicitly rebind from the current advisor");
    let queue = crate::deputy_shell::current_queue(&state.deputy)?;
    let reply = crate::deputy_shell::socket::request(
        &queue,
        &crate::deputy_shell::socket::Request::ReviewWatchRebind {
            project: root,
            grant: grant.id.clone(),
        },
    )?;
    anyhow::ensure!(
        reply.ok,
        "review-watch rotation rebind refused: {}",
        reply.why.unwrap_or_default()
    );
    Ok(())
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
        let (temp, _guard, first) = fixture();
        rebind(temp.path(), "default", &first).unwrap();
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
                if c.id == 1 {
                    anyhow::bail!("PR pickup refused");
                }
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
