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
        AdvisorLoopsCommand::Status { json } => {
            let Some(dir) = directory(&root, false)? else {
                print_status("not-configured", None, *json)?;
                return Ok(());
            };
            let _lock = dir.lock("admission.lock")?;
            let Some(state) = read(&dir)? else {
                print_status("not-configured", None, *json)?;
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
            print_status(status, Some(&state), *json)?;
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
fn print_status(status: &str, state: Option<&Service>, json: bool) -> Result<()> {
    let value = serde_json::json!({
        "service": "review-watch", "status": status,
        "deputy": state.map(|s| &s.deputy),
        "binding_issued_at": state.map(|s| s.binding.issued_at),
        "controller": state.and_then(|s| s.worker.as_ref().map(|w| w.pid)),
        "last_exit": state.and_then(|s| s.last_exit),
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
        println!("review-watch: {status}; deputy {}; binding issued {}; controller {}; last exit {}; recovery {}",
            value["deputy"], value["binding_issued_at"], value["controller"], value["last_exit"], value["recovery"]);
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
    if let Some(process) = child.as_mut() {
        // The identity was captured before any try_wait/reap.
        if let Some(status) = process.try_wait()? {
            revoke_controller(&state)?;
            state.last_exit = status.code();
            state.worker = None;
            state.child_grant = None;
            state.retry_after = Utc::now() + chrono::Duration::seconds(10);
            *child = None;
        }
    } else if let Some(worker) = &state.worker {
        match worker.verdict() {
            aida_core::proc_identity::Verdict::Dead(_) => {
                revoke_controller(&state)?;
                state.worker = None;
                state.child_grant = None;
                state.retry_after = Utc::now() + chrono::Duration::seconds(10);
            }
            // Alive and unknown both retain the controller reservation.
            _ => {}
        }
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

// Keep the receipt until revocation succeeds, so a failed revoke is retried.
// trace:STORY-2571 | ai:codex
fn revoke_controller(state: &Service) -> Result<()> {
    if let Some(id) = &state.child_grant {
        seat_authority::revoke_by_id(id)?;
    }
    Ok(())
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
    rebind_after_launch_with(&root, grant, |deputy, root, grant| {
        let queue = crate::deputy_shell::current_queue(deputy)?;
        let reply = crate::deputy_shell::socket::request(
            &queue,
            &crate::deputy_shell::socket::Request::ReviewWatchRebind {
                project: root.into(),
                grant: grant.id.clone(),
            },
        )?;
        anyhow::ensure!(
            reply.ok,
            "review-watch rotation rebind refused: {}",
            reply.why.unwrap_or_default()
        );
        Ok(())
    })
}

// Trusted launchers may bootstrap the default deputy or rotate an operator's
// binding to the advisor handle they just minted. The socket still attributes
// the launcher to that grant and rebind applies all successor checks.
// trace:STORY-2571 | ai:codex
fn rebind_after_launch_with(
    root: &Path,
    grant: &SeatGrant,
    request: impl FnOnce(&str, &Path, &SeatGrant) -> Result<()>,
) -> Result<()> {
    let state = if let Some(dir) = directory(root, false)? {
        let _lock = dir.lock("admission.lock")?;
        read(&dir)?
    } else {
        None
    };
    let deputy = state
        .as_ref()
        .map(|s| s.deputy.as_str())
        .unwrap_or("default");
    request(deputy, root, grant)
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
        // The public hook must attempt bootstrap even without a service receipt.
        // This hermetic home has no deputy, so transport must refuse, not skip.
        assert!(rebind_after_launch(root, &operator).is_err());
        let send = |deputy: &str, root: &Path, grant: &SeatGrant| rebind(root, deputy, grant);
        rebind_after_launch_with(root, &operator, send).unwrap();
        let next = seat_authority::issue_launch_child_with_delegation(
            root,
            "advisor",
            "new-advisor-handle",
            false,
            &[],
        )
        .unwrap();
        rebind_after_launch_with(root, &next, send).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        assert_eq!(
            read(&dir).unwrap().unwrap().binding.subject,
            "new-advisor-handle"
        );
        assert!(rebind_after_launch_with(root, &operator, send).is_err());
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
