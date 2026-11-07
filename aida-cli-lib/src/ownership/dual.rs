//! Dual-write adapters (STORY-2325 PR-B, advisor amendment A6).
//!
//! PR-C cuts train/review/relaunch readers over to these records and stages
//! verified seat handoffs. The remaining adapters still mirror legacy writers;
//! failures are diagnostic until their own writer cutover.
//!
//! Each call site that writes one of today's ownership records (a session,
//! review or claim lease; the train's merge lease; the PR that `aida ship`
//! opens) also calls one function here. Legacy-writer failures are diagnostic
//! under `AIDA_OWNERSHIP_TRACE=1`; `aida own show --compare` reports gaps.
//! Post-open self-handoff failures are always reported without failing ship.
//! Ownership transfers to the PR, while the lease remains the retirement
//! handle for guarded session end after the seat exits.
//!
//! Where the writes happen (A1, A6): only in host-side processes. A contained
//! seat never writes `.aida/ownership/`, and its sandbox is never widened to
//! let it: its records are written by the host processes that already write
//! its other records, namely `aida queue work` / `aida session start` (the
//! lease, written before the seat's sandbox starts) and `aida ship
//! --from-seat` (the handoff, after the host has verified and pushed the
//! seat's bundle). A write from any pid namespace other than the host's
//! initial one records no process witness ([`super::process_witness`]); a
//! write that the sandbox denies is dropped.
//!
//! trace:STORY-2325 | ai:claude

use super::ops;
use super::store::{Expect, OwnershipStore, Step};
use super::types::{ChangeId, Holder, Phase, Role, SeatTree, TreeKind, Witness, WorkKey};
use crate::gate::canon::SpecKey;
use aida_core::process_probe::{ProbeSource, ProcessIdentity};
use chrono::{DateTime, Utc};
use std::path::Path;

/// The forge every change key names: the one behind `origin`.
// trace:STORY-2325 | ai:claude
pub(crate) const FORGE: &str = "origin";

/// A change on this repository's forge.
// trace:STORY-2325 | ai:claude
pub(crate) fn change(n: u64) -> ChangeId {
    ChangeId {
        forge: FORGE.to_string(),
        n,
    }
}

#[cfg(test)]
thread_local! {
    /// Tests opt in per thread, so no other test's lease write lands in a
    /// real checkout's `.aida/ownership/`.
    static TEST_ENABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Turn the dual-write on for this test thread.
#[cfg(test)]
pub(crate) fn enable_for_test() {
    TEST_ENABLED.with(|e| e.set(true));
}

fn enabled() -> bool {
    #[cfg(test)]
    {
        TEST_ENABLED.with(|e| e.get())
    }
    #[cfg(not(test))]
    {
        true
    }
}

fn report(what: &str, err: anyhow::Error) {
    if std::env::var_os("AIDA_OWNERSHIP_TRACE").is_some() {
        eprintln!("ownership: {what} was not recorded: {err:#}");
    }
}

/// Where and as whom a write happens: the host, its boot, and the pid
/// namespace this process runs in.
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WriterFacts {
    pub host: String,
    pub boot: Option<String>,
    pub pid_ns: Option<u64>,
}

impl WriterFacts {
    // trace:STORY-2325 | ai:claude
    pub(crate) fn here() -> Self {
        WriterFacts {
            host: crate::coordination::hostname(),
            boot: aida_core::local_boot_id(),
            pid_ns: ProbeSource::system().pid_namespace(std::process::id()).ok(),
        }
    }

    /// A process witness for `pid` whose start identity the old record
    /// stored as `start` (`linux-starttime:<ticks>`, today's lease and lock
    /// format). Only from the host's view (A1), and only with a start
    /// identity (A4): anything else records nothing.
    // trace:STORY-2325 | ai:claude
    pub(crate) fn witness(
        &self,
        pid: u32,
        start: Option<&str>,
        now: DateTime<Utc>,
    ) -> Option<Witness> {
        let id = identity_from_record(pid, start?, self.boot.as_deref()?)?;
        let record = aida_core::proc_identity::HolderRecord::capture(pid);
        let identity = record.validate().ok()?;
        if identity.legacy() != id {
            return None;
        }
        let mut witness = super::process_witness(self.pid_ns, &self.host, id, now)?;
        witness.admission = Some(record);
        Some(witness)
    }
}

/// Today's `linux-starttime:<ticks>` (read on this boot) as the probe's
/// `linux:<boot>:<ticks>` identity. Any other scheme (the legacy rfc3339
/// time, the non-Linux sysinfo time) cannot be converted and is `None`.
// trace:STORY-2325 | ai:claude
pub(crate) fn identity_from_record(pid: u32, start: &str, boot: &str) -> Option<ProcessIdentity> {
    let ticks: u64 = start
        .strip_prefix("linux-starttime:")?
        .trim()
        .parse()
        .ok()?;
    if pid == 0 || boot.is_empty() {
        return None;
    }
    Some(ProcessIdentity {
        pid,
        start: format!("linux:{boot}:{ticks}"),
    })
}

/// `PR-123` / `pr-123` / `MR-123` as a change number.
// trace:STORY-2325 | ai:claude
pub(crate) fn change_scope(scope: &str) -> Option<u64> {
    let s = scope.trim();
    let rest = s
        .get(..3)
        .filter(|p| p.eq_ignore_ascii_case("PR-") || p.eq_ignore_ascii_case("MR-"))
        .map(|_| &s[3..])?;
    rest.parse().ok().filter(|n| *n > 0)
}

/// The work key and phase a lease stands for, given a resolver for spec
/// labels. A review lease (`aida review`) is a `Review` of its PR's head
/// only when it names a `PR-N` and records that head; any other review lease
/// stands for no key and is not recorded, so a reviewer never takes the
/// change (overwriting its `InPr` handoff) or the spec (replacing its
/// implementer). Any other `PR-N` lease is the change itself; anything else
/// is a spec.
// trace:STORY-2325 | ai:claude
pub(crate) fn key_for_lease(
    lease: &crate::SessionLease,
    resolve: &dyn Fn(&str) -> Option<SpecKey>,
) -> Option<(WorkKey, Phase)> {
    if lease.review_verb {
        let n = change_scope(&lease.scope)?;
        let head = lease.pr_head_sha.as_deref().filter(|h| !h.is_empty())?;
        return Some((
            WorkKey::Review {
                change: change(n),
                head: head.to_string(),
            },
            Phase::Reviewing {
                head: head.to_string(),
            },
        ));
    }
    if let Some(n) = change_scope(&lease.scope) {
        return Some((WorkKey::Change { change: change(n) }, Phase::Leased));
    }
    let spec = resolve(&lease.scope)?;
    Some((WorkKey::Spec { spec }, Phase::Leased))
}

/// The holder a lease names, with the witnesses this writer can record.
// trace:STORY-2325 | ai:claude
pub(crate) fn holder_for_lease(
    lease: &crate::SessionLease,
    _writer: &WriterFacts,
    now: DateTime<Utc>,
) -> Holder {
    let role = if lease.review_verb {
        Role::Reviewer
    } else {
        match lease
            .role
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("reviewer") => Role::Reviewer,
            Some("advisor") => Role::Advisor,
            Some("operator") => Role::Operator,
            _ => Role::Implementer,
        }
    };
    let tree = (!lease.worktree_path.as_os_str().is_empty()).then(|| SeatTree {
        path: lease.worktree_path.clone(),
        kind: if lease.tree_kind == crate::seat_clone::TreeKind::Clone {
            TreeKind::Clone
        } else {
            TreeKind::Worktree
        },
        branch: Some(lease.branch.clone()).filter(|b| !b.is_empty()),
    });
    let seat = lease
        .worktree_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| lease.slug.clone());
    // trace:STORY-2537 trace:BUG-2458 | ai:codex
    let witnesses = crate::leases::lease_holder_record(lease)
        .map(|record| super::types::Witness {
            kind: super::types::WitnessKind::Process {
                host: lease.hostname.clone(),
                id: record
                    .identity()
                    .map(|id| id.legacy())
                    .unwrap_or(ProcessIdentity {
                        pid: lease.active_pid.unwrap_or(0),
                        start: String::new(),
                    }),
                pid_ns: record.identity().map(|id| id.pidns_inode),
            },
            admission: Some(record),
            recorded_at: now,
        })
        .into_iter()
        .collect();
    Holder {
        seat,
        role,
        session: lease.id.clone(),
        tree,
        witnesses,
    }
}

/// A spec label (display id, spec id or uuid) as its canonical key, read
/// from the requirement store.
// trace:STORY-2325 | ai:claude
pub(crate) fn spec_key_in_store(
    store: &aida_core::RequirementsStore,
    label: &str,
) -> Option<SpecKey> {
    let want = label.trim();
    store
        .requirements
        .iter()
        .find(|r| {
            r.id.to_string().eq_ignore_ascii_case(want)
                || [r.agreed_id.as_deref(), r.spec_id.as_deref()]
                    .into_iter()
                    .flatten()
                    .any(|s| s.eq_ignore_ascii_case(want))
        })
        .map(|r| SpecKey(r.id))
}

fn store_for(project_root: &Path) -> Option<OwnershipStore> {
    if !enabled() {
        return None;
    }
    // A1/A6: refuse explicitly instead of relying on the sandbox to deny the
    // write. From a seat clone the main worktree resolves to the clone, so
    // a write that got through would land in the clone's own directory.
    if crate::session::process_contained() {
        report(
            "the ownership store",
            anyhow::anyhow!("a contained seat never writes the ownership records"),
        );
        return None;
    }
    match OwnershipStore::for_project(project_root) {
        Ok(store) => store,
        Err(e) => {
            report("the ownership store", e);
            None
        }
    }
}

/// A lease was written: its holder acquires the work it names.
// trace:STORY-2325 | ai:claude
pub(crate) fn lease_written(project_root: &Path, lease: &crate::SessionLease) {
    let Some(store) = store_for(project_root) else {
        return;
    };
    let resolve = |label: &str| {
        // A uuid scope resolves without loading the store.
        if let Ok(id) = uuid::Uuid::parse_str(label.trim()) {
            return Some(SpecKey(id));
        }
        crate::load_store_for_lookup(project_root)
            .as_ref()
            .and_then(|s| spec_key_in_store(s, label))
    };
    let Some((key, phase)) = key_for_lease(lease, &resolve) else {
        if lease.review_verb {
            // A review with no recorded PR head is not recorded (see
            // `key_for_lease`); that is expected, not a gap.
            return;
        }
        report(
            &format!("lease {}", lease.id),
            anyhow::anyhow!("its scope `{}` names no requirement", lease.scope),
        );
        return;
    };
    let holder = holder_for_lease(lease, &WriterFacts::here(), Utc::now());
    if let Err(e) = store.update(&key, "acquire", Expect::Any, |cur| {
        ops::acquire(cur, &key, holder, phase)
    }) {
        report(&format!("lease {}", lease.id), e);
    }
}

/// A lease was removed: every record its session holds is released.
// trace:STORY-2325 | ai:claude
pub(crate) fn lease_released(project_root: &Path, lease_id: &str, why: &str) {
    let Some(store) = store_for(project_root) else {
        return;
    };
    let (records, _) = store.list();
    let steps = release_steps(&records, lease_id, why);
    if steps.is_empty() {
        return;
    }
    if let Err(e) = store.update_all(steps) {
        report(&format!("the release of lease {lease_id}"), e);
    }
}

/// One `release` step per record `session` holds.
// trace:STORY-2325 | ai:claude
pub(crate) fn release_steps(
    records: &[super::types::Ownership],
    session: &str,
    why: &str,
) -> Vec<Step<'static>> {
    records
        .iter()
        .filter(|r| r.holder.as_ref().is_some_and(|h| h.session == session))
        .map(|r| {
            let session = session.to_string();
            let why = why.to_string();
            Step {
                key: r.key.clone(),
                op: "release".to_string(),
                expect: Expect::Any,
                transition: Box::new(move |cur| ops::release(cur, &session, &why)),
            }
        })
        .collect()
}

/// The steps that end a train's merge lease held by `session`: release
/// every record it holds, and move every rider change still riding one of
/// its trains out of `Training`.
// trace:STORY-2325 | ai:claude
pub(crate) fn train_release_steps(
    records: &[super::types::Ownership],
    session: &str,
    why: &str,
) -> Vec<Step<'static>> {
    let trains: Vec<ChangeId> = records
        .iter()
        .filter(|r| r.holder.as_ref().is_some_and(|h| h.session == session))
        .filter_map(|r| match &r.key {
            WorkKey::Train { train } => Some(train.clone()),
            _ => None,
        })
        .collect();
    let mut steps = release_steps(records, session, why);
    for r in records {
        let (WorkKey::Change { .. }, Phase::Training { train }) = (&r.key, &r.phase) else {
            continue;
        };
        if !trains.contains(train) {
            continue;
        }
        let key = r.key.clone();
        let train = train.clone();
        let why = why.to_string();
        steps.push(Step {
            key: key.clone(),
            op: "leave-train".to_string(),
            expect: Expect::Any,
            transition: Box::new(move |cur| ops::leave_train(cur, &key, &train, &why)),
        });
    }
    steps
}

/// What a handoff was verified against.
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HandoffFacts {
    /// The spec the PR ships, as its canonical key, when it resolves.
    pub spec: Option<SpecKey>,
    pub change: ChangeId,
    /// The head now on origin for the PR's branch.
    pub head: String,
}

/// Q2's pre-check for a seat this process can read: no tracked change, and
/// HEAD is exactly the head just pushed to origin. `Err` names the refusal.
// trace:STORY-2325 | ai:claude
pub(crate) fn verify_seat_at_pr_head(seat: &Path, branch: &str) -> Result<String, String> {
    let git = |args: &[&str]| -> Result<String, String> {
        // BUG-1985/BUG-1929: tree-walking git goes through the hardened host builder.
        let out = aida_core::git_ops::host_git_command(seat)
            .args(args)
            .env("LC_ALL", "C")
            .output()
            .map_err(|e| format!("git could not run: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let dirty = git(&["status", "--porcelain", "--untracked-files=normal"])?;
    if !dirty.is_empty() {
        return Err(format!(
            "the seat has uncommitted changes ({} path(s))",
            dirty.lines().count()
        ));
    }
    let head = git(&["rev-parse", "--verify", "HEAD^{commit}"])?;
    let origin = git(&[
        "rev-parse",
        "--verify",
        &format!("refs/remotes/origin/{branch}^{{commit}}"),
    ])?;
    if head != origin {
        return Err(format!(
            "the seat HEAD {head} is not the head {origin} pushed to origin/{branch}"
        ));
    }
    Ok(head)
}

/// Staged handoff transitions. The caller binds every key to the owner and
/// expected generation, then verifies and detaches under the ownership lock.
// trace:STORY-2325 | ai:claude
pub(crate) fn handoff_steps(facts: &HandoffFacts, now: DateTime<Utc>) -> Vec<Step<'static>> {
    let mut keys = vec![WorkKey::Change {
        change: facts.change.clone(),
    }];
    if let Some(spec) = facts.spec {
        keys.push(WorkKey::Spec { spec });
    }
    keys.into_iter()
        .map(|key| {
            let change = facts.change.clone();
            let head = facts.head.clone();
            let k = key.clone();
            Step {
                key,
                op: "handoff".to_string(),
                expect: Expect::Any,
                transition: Box::new(move |cur| ops::handoff(cur, &k, &change, &head, now)),
            }
        })
        .collect()
}

fn resolve_spec(project_root: &Path, spec: &str) -> Option<SpecKey> {
    if let Ok(id) = uuid::Uuid::parse_str(spec.trim()) {
        return Some(SpecKey(id));
    }
    crate::load_store_for_lookup(project_root)
        .as_ref()
        .and_then(|s| spec_key_in_store(s, spec))
}

fn commit_handoff(project_root: &Path, facts: HandoffFacts) {
    let Some(store) = store_for(project_root) else {
        return;
    };
    if let Err(e) = store.update_all(handoff_steps(&facts, Utc::now())) {
        report(&format!("the handoff to PR-{}", facts.change.n), e);
    }
}

/// `aida ship` opened (or found) PR `pr` for the seat `seat` on `branch`.
/// Only the owning branch lease may hand off. A refusal is reported, but
/// never fails a ship whose PR is already open. The lease remains for cleanup.
// trace:STORY-2325 | ai:claude
pub(crate) fn handed_off(
    project_root: &Path,
    seat: &Path,
    branch: &str,
    spec: &str,
    pr: u64,
) -> anyhow::Result<()> {
    if !enabled() || pr == 0 {
        return Ok(());
    }
    let attempt = || -> anyhow::Result<()> {
        let input = aida_core::exec_context::Environment::capture();
        let session = input
            .var("AIDA_SESSION_ID")
            .map_err(|_| anyhow::anyhow!("no owning session identity (operator ship)"))?;
        let owns_branch = crate::list_leases(project_root)
            .into_iter()
            .any(|lease| lease.id == session && lease.branch == branch);
        anyhow::ensure!(owns_branch, "this session does not own the branch lease");
        let store = store_for(project_root)
            .ok_or_else(|| anyhow::anyhow!("ownership store unavailable for handoff"))?;
        handoff_seat(
            &store,
            seat,
            branch,
            resolve_spec(project_root, spec),
            pr,
            session,
        )
    };
    if let Err(error) = attempt() {
        eprintln!("ownership: PR-{pr} is open; seat handoff requires attention: {error:#}");
    }
    // Keep the lease as the retirement handle for session_end after merge.
    // The ownership record alone transfers to InPr; the running seat keeps
    // its lifecycle/cleanup identity until guarded session teardown.
    Ok(())
}

/// Testable self-handoff. CAS binds both records; all ownership checks and git
/// verification precede detachment under the same lock as batch publication.
// trace:STORY-2325 | ai:codex
pub(crate) fn handoff_seat(
    store: &OwnershipStore,
    seat: &Path,
    branch: &str,
    spec: Option<SpecKey>,
    pr: u64,
    session: &str,
) -> anyhow::Result<()> {
    use anyhow::{bail, Context};
    let spec = spec.ok_or_else(|| anyhow::anyhow!("handoff needs a canonical spec"))?;
    let spec_key = WorkKey::Spec { spec };
    let held = store
        .load(&spec_key)?
        .ok_or_else(|| anyhow::anyhow!("spec ownership record missing"))?;
    let holder = held
        .holder
        .as_ref()
        .filter(|h| h.session == session)
        .ok_or_else(|| anyhow::anyhow!("handoff refused: this session does not own the spec"))?;
    let tree = holder
        .tree
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("handoff refused: owner has no seat"))?;
    let owner_path = tree.path.canonicalize().context("resolving owner's seat")?;
    let seat_path = seat.canonicalize().context("resolving handoff seat")?;
    if owner_path != seat_path || tree.branch.as_deref() != Some(branch) {
        bail!("handoff refused: the owner's seat and branch do not match");
    }
    let head = verify_seat_at_pr_head(seat, branch).map_err(anyhow::Error::msg)?;
    let facts = HandoffFacts {
        spec: Some(spec),
        change: change(pr),
        head: head.clone(),
    };
    let mut steps = handoff_steps(&facts, Utc::now());
    for step in &mut steps {
        let current = store.load(&step.key)?;
        if current
            .as_ref()
            .and_then(|r| r.holder.as_ref())
            .is_some_and(|h| h.session != session)
        {
            bail!("handoff refused: another session owns a handoff key");
        }
        if current
            .as_ref()
            .is_some_and(|r| matches!(r.phase, Phase::Training { .. }))
        {
            bail!("handoff refused: a handoff key is riding a train");
        }
        step.expect = if step.key == spec_key {
            Expect::Generation(held.generation)
        } else {
            current
                .as_ref()
                .map_or(Expect::Absent, |r| Expect::Generation(r.generation))
        };
    }
    let mut detached = false;
    let result = store.update_all_checked(steps, || {
        let current = aida_core::git_ops::host_git_command(seat)
            .args(["symbolic-ref", "--quiet", "HEAD"])
            .output()?;
        if !current.status.success()
            || String::from_utf8_lossy(&current.stdout).trim() != format!("refs/heads/{branch}")
        {
            bail!("handoff refused: seat no longer has the expected branch checked out");
        }
        let verified = verify_seat_at_pr_head(seat, branch).map_err(anyhow::Error::msg)?;
        if verified != head {
            bail!("handoff refused: seat HEAD moved");
        }
        // update-ref changes only HEAD, with an old-value check; it does not
        // run checkout hooks or alter the index/worktree.
        let status = aida_core::git_ops::host_git_command(seat)
            .args(["update-ref", "--no-deref", "HEAD", &head, &head])
            .status()?;
        if !status.success() {
            bail!("could not detach seat HEAD");
        }
        detached = true;
        Ok(())
    });
    // Before publication an error restores the branch. After publication the
    // durable batch owns recovery, so HEAD stays detached to match InPr.
    if result.is_err() && detached && !store.dir().join(".pending.json").exists() {
        let status = aida_core::git_ops::host_git_command(seat)
            .args(["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")])
            .status()
            .context("restoring seat HEAD after handoff staging failure")?;
        if !status.success() {
            bail!("handoff failed and branch restoration failed; inspect detached seat");
        }
    }
    result.map(|_| ())
}

/// `aida ship --from-seat` pushed the contained seat's verified bundle tip
/// `tip` and opened PR `pr`. The host never runs git in the seat's clone
/// (TASK-2053), so the head checked is the bundle tip the host verified and
/// pushed; this is the host-side write that stands in for the seat.
// trace:STORY-2325 | ai:claude
pub(crate) fn handed_off_from_seat(main: &Path, spec: &str, tip: &str, pr: u64) {
    if !enabled() || pr == 0 || tip.is_empty() {
        return;
    }
    commit_handoff(
        main,
        HandoffFacts {
            spec: resolve_spec(main, spec),
            change: change(pr),
            head: tip.to_string(),
        },
    );
}

/// The session name a merge lease holder records under.
// trace:STORY-2325 | ai:claude
pub(crate) fn merge_lease_session(pid: u32) -> String {
    format!("merge-lease:{pid}")
}

/// The train for PR `train` took its merge lease in this process.
// trace:STORY-2325 | ai:claude
pub(crate) fn train_lease_taken(project_root: &Path, train: u64) {
    let Some(store) = store_for(project_root) else {
        return;
    };
    let writer = WriterFacts::here();
    let now = Utc::now();
    let pid = std::process::id();
    let start = aida_core::liveness::process_start_identity(pid);
    let holder = Holder {
        seat: "train".to_string(),
        role: Role::Train,
        session: merge_lease_session(pid),
        tree: None,
        witnesses: writer
            .witness(pid, start.as_deref(), now)
            .into_iter()
            .collect(),
    };
    let key = WorkKey::Train {
        train: change(train),
    };
    let phase = Phase::Training {
        train: change(train),
    };
    if let Err(e) = store.update(&key, "begin-train", Expect::Any, |cur| {
        ops::acquire(cur, &key, holder, phase)
    }) {
        report(&format!("train PR-{train}"), e);
    }
}

/// The train's merge lease in this process was released: the train record
/// closes and its riders leave it.
// trace:STORY-2325 | ai:claude
pub(crate) fn train_lease_released(project_root: &Path) {
    train_released_by(project_root, &merge_lease_session(std::process::id()));
}

/// [`train_lease_released`] for the merge lease held under `session`.
// trace:STORY-2325 | ai:claude
pub(crate) fn train_released_by(project_root: &Path, session: &str) {
    let Some(store) = store_for(project_root) else {
        return;
    };
    let (records, _) = store.list();
    let steps = train_release_steps(&records, session, "the train's merge lease was released");
    if steps.is_empty() {
        return;
    }
    if let Err(e) = store.update_all(steps) {
        report(&format!("the release of {session}"), e);
    }
}

/// Train PR `train` opened carrying `riders`: each rider's change record
/// names the train.
// trace:STORY-2325 | ai:claude
pub(crate) fn train_opened(project_root: &Path, train: u64, riders: &[u64]) {
    let Some(store) = store_for(project_root) else {
        return;
    };
    let steps = riders
        .iter()
        .map(|n| {
            let key = WorkKey::Change { change: change(*n) };
            let k = key.clone();
            let t = change(train);
            Step {
                key,
                op: "ride".to_string(),
                expect: Expect::Any,
                transition: Box::new(move |cur| ops::ride(cur, &k, &t)),
            }
        })
        .collect();
    if let Err(e) = store.update_all(steps) {
        report(&format!("the riders of train PR-{train}"), e);
    }
}
