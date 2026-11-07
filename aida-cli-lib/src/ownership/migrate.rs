//! `aida own migrate`: the one-time rewrite of today's ownership records into
//! `.aida/ownership/` (STORY-2325 PR-B, sketch section 3, amendment A4).
//!
//! The classification is pure ([`classify`]): every fact is read first by
//! [`collect`]. The rewrite only ADDS records. It never edits, archives or
//! deletes a lease, lane file, review marker or merge lock (they stay
//! authoritative until PR-C, A6), never detaches a seat HEAD (deferred to
//! PR-C by the advisor ruling of 2026-11-03), and never touches a tree. Rolling it back is removing
//! `.aida/ownership/`.
//!
//! A4: a process is recorded alive only when its recorded start identity
//! matches the running process now; a pid with no start identity, or one
//! that cannot be probed from here, records no witness (its holder is
//! Unknown). `--execute` refuses from any pid namespace but the host's.
//!
//! trace:STORY-2325 | ai:claude

use super::dual::{self, WriterFacts};
use super::store::{Expect, OwnershipStore, Step};
use super::types::{
    Holder, HostFacts, Ownership, Phase, Role, TreeReading, Witness, WitnessReading, WorkKey,
};
use crate::gate::canon::SpecKey;
use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// An open PR on origin: its number and current head.
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenPr {
    pub n: u64,
    pub head: String,
}

/// Everything the classifier reads about one lease.
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct LeaseFacts {
    pub lease: crate::SessionLease,
    /// `key_for_lease` over the store snapshot; `None` = the scope names no
    /// requirement.
    pub key: Option<(WorkKey, Phase)>,
    /// The spec is finished (completed, rejected, done).
    pub spec_closed: bool,
    /// The open PR for the lease's branch (or the `PR-N` it names), when the
    /// forge answered.
    pub pr: Option<OpenPr>,
    pub tree: TreeReading,
    /// A4: only witnesses that probe Alive now, with a matching start
    /// identity.
    pub live_witnesses: Vec<Witness>,
}

/// How one input was classified.
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Class {
    /// The work is in its open PR; the seat holds nothing the PR lacks.
    InPr,
    /// The spec is finished and the seat holds nothing unpushed.
    Closed,
    /// A holder probes alive now.
    Live,
    /// Dirty, unpushed or unreadable: kept leased, listed as salvage.
    Salvage,
    /// The tree is not a git checkout: listed for the operator.
    NotAGitTree,
    /// Clean, no open PR, no live witness: leased, holder Unknown.
    Unknown,
    /// The scope names no requirement: nothing is written.
    Unresolved,
}

/// One lease's plan: the records to write (none for `Unresolved`).
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LeasePlan {
    pub lease_id: String,
    pub scope: String,
    pub class: Class,
    pub detail: String,
    pub records: Vec<Ownership>,
}

fn record(key: WorkKey, holder: Option<Holder>, phase: Phase) -> Ownership {
    Ownership {
        key,
        generation: 0,
        holder,
        phase,
    }
}

/// Is the tree clean and fully contained in `pr_head` (or gone)?
fn tree_in(
    tree: &TreeReading,
    pr_head: Option<&str>,
    is_ancestor: &dyn Fn(&str, &str) -> bool,
) -> bool {
    match tree {
        TreeReading::Missing => true,
        TreeReading::Read {
            dirty,
            unpushed,
            head,
        } if dirty.is_empty() && unpushed.is_empty() => match (head.as_deref(), pr_head) {
            (None, _) => true,
            (Some(h), Some(p)) => h == p || is_ancestor(h, p),
            (Some(_), None) => false,
        },
        _ => false,
    }
}

/// Is the tree clean with nothing origin lacks (or gone)?
fn tree_clean(tree: &TreeReading) -> bool {
    match tree {
        TreeReading::Missing => true,
        TreeReading::Read {
            dirty, unpushed, ..
        } => dirty.is_empty() && unpushed.is_empty(),
        _ => false,
    }
}

/// Sketch section 3, step 2, for one lease. Pure.
// trace:STORY-2325 | ai:claude
pub(crate) fn classify(
    f: &LeaseFacts,
    writer: &WriterFacts,
    is_ancestor: &dyn Fn(&str, &str) -> bool,
    now: DateTime<Utc>,
) -> LeasePlan {
    let plan = |class, detail: String, records| LeasePlan {
        lease_id: f.lease.id.clone(),
        scope: f.lease.scope.clone(),
        class,
        detail,
        records,
    };
    let Some((key, leased_phase)) = f.key.clone() else {
        if f.lease.review_verb {
            return plan(
                Class::Unresolved,
                "a review lease with no recorded PR head: reviews are not recorded".to_string(),
                vec![],
            );
        }
        return plan(
            Class::Unresolved,
            format!("its scope `{}` names no requirement", f.lease.scope),
            vec![],
        );
    };
    let mut holder = dual::holder_for_lease(&f.lease, writer, now);
    // A4: the witnesses are exactly the ones that probe alive now.
    holder.witnesses = f.live_witnesses.clone();
    let pr_head = f.pr.as_ref().map(|p| p.head.as_str());

    if matches!(key, WorkKey::Review { .. }) {
        let class = if holder.witnesses.is_empty() {
            Class::Unknown
        } else {
            Class::Live
        };
        return plan(
            class,
            "a review lease".to_string(),
            vec![record(key, Some(holder), leased_phase)],
        );
    }
    match &f.tree {
        TreeReading::NotAGitTree => {
            return plan(
                Class::NotAGitTree,
                format!(
                    "{} is not a git checkout; archive or remove it",
                    f.lease.worktree_path.display()
                ),
                vec![record(
                    key,
                    Some(Holder {
                        witnesses: vec![],
                        ..holder
                    }),
                    Phase::Leased,
                )],
            )
        }
        TreeReading::Unreadable(why) => {
            return plan(
                Class::Salvage,
                format!("its tree could not be read ({why})"),
                vec![record(key, Some(holder), Phase::Leased)],
            )
        }
        TreeReading::Read {
            dirty, unpushed, ..
        } if !dirty.is_empty() || !unpushed.is_empty() => {
            return plan(
                Class::Salvage,
                format!(
                    "{} uncommitted change(s), {} commit(s) origin lacks",
                    dirty.len(),
                    unpushed.len()
                ),
                vec![record(key, Some(holder), Phase::Leased)],
            )
        }
        _ => {}
    }
    if let Some(pr) = &f.pr {
        if tree_in(&f.tree, pr_head, is_ancestor) {
            let change = dual::change(pr.n);
            let in_pr = Phase::InPr {
                change: change.clone(),
                handed_off_at: now,
                head: pr.head.clone(),
            };
            let mut records = vec![record(key.clone(), None, in_pr.clone())];
            let change_key = WorkKey::Change { change };
            if change_key != key {
                records.push(record(change_key, None, in_pr));
            }
            return plan(
                Class::InPr,
                format!("its work is in open PR-{}", pr.n),
                records,
            );
        }
    }
    if f.spec_closed && tree_clean(&f.tree) {
        return plan(
            Class::Closed,
            "its spec is finished".to_string(),
            vec![record(
                key,
                None,
                Phase::Closed {
                    why: "finished before the ownership migration".to_string(),
                },
            )],
        );
    }
    if !holder.witnesses.is_empty() {
        return plan(
            Class::Live,
            "its holder is running".to_string(),
            vec![record(key, Some(holder), leased_phase)],
        );
    }
    let detail = match (&f.pr, &f.tree) {
        (Some(pr), _) => format!("its HEAD is not in open PR-{}", pr.n),
        (None, _) => "no open PR and no running holder".to_string(),
    };
    plan(
        Class::Unknown,
        detail,
        vec![record(key, Some(holder), leased_phase)],
    )
}

/// A merge lease the migration found.
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct TrainLeaseFacts {
    pub train: u64,
    pub pid: u32,
    /// A4: the witness, only when it probes alive now.
    pub live_witness: Option<Witness>,
}

/// A train's merge lease as a `Training` record; its holder has a witness
/// only when one probes alive now.
// trace:STORY-2325 | ai:claude
pub(crate) fn classify_train(t: &TrainLeaseFacts) -> Ownership {
    let train = dual::change(t.train);
    record(
        WorkKey::Train {
            train: train.clone(),
        },
        Some(Holder {
            seat: "train".to_string(),
            role: Role::Train,
            session: dual::merge_lease_session(t.pid),
            tree: None,
            witnesses: t.live_witness.clone().into_iter().collect(),
        }),
        Phase::Training { train },
    )
}

/// Inputs accounted for without a record of their own.
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Accounted {
    /// Lane files whose lease is in this migration (derived from it).
    pub lanes_derived: usize,
    /// Lane files whose lease is gone.
    pub lanes_orphaned: Vec<String>,
    /// Review-in-progress markers (the review lease is the record).
    pub review_markers: Vec<String>,
    /// Merge leases that are not a train's.
    pub other_merge_leases: Vec<String>,
}

/// The whole migration plan.
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct MigrationPlan {
    /// Pinned before collection; repair never releases work or changes tree/phase.
    pub existing: Vec<Ownership>,
    pub leases: Vec<LeasePlan>,
    pub trains: Vec<Ownership>,
    pub accounted: Accounted,
    /// Records two inputs both planned for one key: a Live plan wins over
    /// any other, then the first in list order.
    pub duplicates: Vec<(WorkKey, String)>,
    /// What could not be read (the forge, the store); classification ran
    /// without it.
    pub gaps: Vec<String>,
}

impl MigrationPlan {
    /// Every record to write, one per key: a Live plan wins over any other,
    /// then the first in list order.
    // trace:STORY-2325 | ai:claude
    pub(crate) fn records(&self) -> Vec<Ownership> {
        let mut seen = BTreeMap::new();
        for rec in by_preference(&self.leases)
            .into_iter()
            .flat_map(|p| p.records.iter())
            .chain(self.trains.iter())
        {
            seen.entry(rec.key.clone()).or_insert_with(|| rec.clone());
        }
        seen.into_values().collect()
    }

    pub(crate) fn counts(&self) -> BTreeMap<Class, usize> {
        let mut out = BTreeMap::new();
        for p in &self.leases {
            *out.entry(p.class.clone()).or_insert(0) += 1;
        }
        out
    }
}

/// The lease plans in the order their records claim a key: Live plans
/// first (a running holder beats a dead or unproven one for the same key),
/// otherwise list order.
// trace:STORY-2325 | ai:claude
fn by_preference(plans: &[LeasePlan]) -> Vec<&LeasePlan> {
    let mut out: Vec<&LeasePlan> = plans.iter().collect();
    out.sort_by_key(|p| p.class != Class::Live);
    out
}

/// Plan the records for every lease and train lease. Pure.
// trace:STORY-2325 | ai:claude
pub(crate) fn plan(
    leases: &[LeaseFacts],
    trains: &[TrainLeaseFacts],
    accounted: Accounted,
    gaps: Vec<String>,
    writer: &WriterFacts,
    is_ancestor: &dyn Fn(&str, &str) -> bool,
    now: DateTime<Utc>,
) -> MigrationPlan {
    let lease_plans: Vec<LeasePlan> = leases
        .iter()
        .map(|f| classify(f, writer, is_ancestor, now))
        .collect();
    let trains: Vec<Ownership> = trains.iter().map(classify_train).collect();
    let mut owners: BTreeMap<WorkKey, String> = BTreeMap::new();
    let mut duplicates = Vec::new();
    for p in by_preference(&lease_plans) {
        for rec in &p.records {
            match owners.get(&rec.key) {
                Some(first) if *first != p.lease_id => duplicates.push((
                    rec.key.clone(),
                    format!("lease {} (kept {first})", p.lease_id),
                )),
                Some(_) => {}
                None => {
                    owners.insert(rec.key.clone(), p.lease_id.clone());
                }
            }
        }
    }
    MigrationPlan {
        existing: Vec::new(),
        leases: lease_plans,
        trains,
        accounted,
        duplicates,
        gaps,
    }
}

/// Why `--execute` may not run here, or `None`.
// trace:STORY-2325 | ai:claude
pub(crate) fn execute_refusal(
    stdin_tty: bool,
    pid_ns: Option<u64>,
    contained: bool,
    live_drain: bool,
) -> Option<String> {
    if !stdin_tty {
        return Some(
            "--execute needs an operator at a terminal; run it from an interactive shell"
                .to_string(),
        );
    }
    if contained {
        return Some(
            "--execute refuses from a contained seat; run it from the host shell".to_string(),
        );
    }
    if pid_ns != Some(aida_core::process_probe::INITIAL_PID_NAMESPACE_INODE) {
        return Some(
            "--execute refuses outside the host's pid namespace (a sandboxed shell cannot \
             prove which recorded processes are running); run it from the host shell"
                .to_string(),
        );
    }
    if live_drain {
        return Some("--execute refuses while a drain is running; wait for it to finish".into());
    }
    None
}

/// What `--execute` wrote.
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Applied {
    pub written: usize,
    /// Already recorded (by a dual-write or an earlier run): left as is.
    pub kept: usize,
}

/// Write every planned record that has no record yet, in one locked step.
/// A key that already has a record keeps it: a writer since PR-B recorded
/// newer truth than the migration can reconstruct. Idempotent.
// trace:STORY-2325 | ai:claude
pub(crate) fn apply(store: &OwnershipStore, plan: &MigrationPlan) -> Result<Applied> {
    let records = plan.records();
    let total = records
        .iter()
        .filter(|rec| !plan.existing.iter().any(|old| old.key == rec.key))
        .count()
        + plan.existing.len();
    let mut steps: Vec<Step<'static>> = records
        .into_iter()
        .filter(|rec| !plan.existing.iter().any(|old| old.key == rec.key))
        .map(|rec| Step {
            key: rec.key.clone(),
            op: "migrate".into(),
            expect: Expect::Any,
            transition: Box::new(move |cur| {
                cur.is_none()
                    .then(|| (rec, Some("one-time migration from v0 records".into())))
            }),
        })
        .collect();
    for old in &plan.existing {
        // Existing legacy evidence stays byte-for-byte under its old reader
        // policy. Migration cannot admit an arbitrary old PID, nor should it
        // turn a heartbeat-backed row into a refused holder.
        // trace:STORY-2537 trace:BUG-2458 | ai:codex
        steps.push(Step {
            key: old.key.clone(),
            op: "retain-legacy-holder".into(),
            expect: Expect::Generation(old.generation),
            transition: Box::new(|_| None),
        });
    }
    let out = store.update_all(steps)?;
    let written = out.iter().filter(|c| c.is_some()).count();
    Ok(Applied {
        written,
        kept: total - written,
    })
}

// ---------------------------------------------------------------------------
// Collection (I/O)
// ---------------------------------------------------------------------------

/// The open PRs on origin, by head branch. `Err` when the forge did not
/// answer; the migration then plans without PR facts and says so.
fn open_prs(main: &Path) -> std::result::Result<BTreeMap<String, OpenPr>, String> {
    let out = std::process::Command::new("gh")
        .current_dir(main)
        .args([
            "pr",
            "list",
            "--state",
            "open",
            "--limit",
            "1000",
            "--json",
            "number,headRefName,headRefOid",
        ])
        .output()
        .map_err(|e| format!("gh could not run: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("gh answered malformed JSON: {e}"))?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            Some((
                r.get("headRefName")?.as_str()?.to_string(),
                OpenPr {
                    n: r.get("number")?.as_u64()?,
                    head: r.get("headRefOid")?.as_str()?.to_string(),
                },
            ))
        })
        .collect())
}

fn git_out(dir: &Path, args: &[&str]) -> Option<String> {
    // BUG-1985/BUG-1929: tree-walking git goes through the hardened host builder.
    let out = aida_core::git_ops::host_git_command(dir)
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// What is at a lease's tree, read from the host. Any tree is read the
/// same way except a seat clone: host git refuses to run there
/// (BUG-2169), so it reads Unreadable (kept as salvage), never NotAGitTree.
fn read_tree(path: &Path, branch: &str) -> TreeReading {
    if path.as_os_str().is_empty() || !path.exists() {
        return TreeReading::Missing;
    }
    // trace:STORY-2325 | ai:claude
    if aida_core::seat_clone_detect::seat_clone_root(path).is_some() {
        return TreeReading::Unreadable("a seat clone; host git does not run there".to_string());
    }
    if git_out(path, &["rev-parse", "--is-inside-work-tree"]).as_deref() != Some("true") {
        return TreeReading::NotAGitTree;
    }
    let Some(status) = git_out(path, &["status", "--porcelain"]) else {
        return TreeReading::Unreadable("git status failed".to_string());
    };
    let dirty: Vec<String> = status.lines().map(str::to_string).collect();
    let head = git_out(path, &["rev-parse", "--verify", "-q", "HEAD^{commit}"]);
    let upstream = format!("refs/remotes/origin/{branch}");
    let unpushed = match &head {
        None => vec![],
        Some(_) if branch.is_empty() => vec![],
        Some(_) => match git_out(
            path,
            &[
                "rev-list",
                "--max-count=20",
                "HEAD",
                "--not",
                "--remotes=origin",
            ],
        ) {
            Some(list) => list.lines().map(str::to_string).collect(),
            None => return TreeReading::Unreadable(format!("cannot compare HEAD with {upstream}")),
        },
    };
    TreeReading::Read {
        dirty,
        unpushed,
        head,
    }
}

/// The witnesses of a lease that probe alive right now (A4).
fn live_witnesses(
    lease: &crate::SessionLease,
    writer: &WriterFacts,
    host: &HostFacts<'_>,
    now: DateTime<Utc>,
) -> Vec<Witness> {
    let mut out = dual::holder_for_lease(lease, writer, now).witnesses;
    out.retain(|w| super::judge_witness(w, host, now) == WitnessReading::Alive);
    out
}

fn spec_closed(store: Option<&aida_core::RequirementsStore>, key: &WorkKey) -> bool {
    let WorkKey::Spec { spec } = key else {
        return false;
    };
    store
        .and_then(|s| s.requirements.iter().find(|r| r.id == spec.0))
        .is_some_and(|r| {
            matches!(
                r.status,
                aida_core::RequirementStatus::Completed
                    | aida_core::RequirementStatus::Rejected
                    | aida_core::RequirementStatus::Done
            )
        })
}

fn file_names(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect();
    out.sort();
    out
}

/// Read every v0 record the migration covers and plan it.
// trace:STORY-2325 | ai:claude
pub(crate) fn collect(main: &Path, now: DateTime<Utc>) -> MigrationPlan {
    // trace:STORY-2537 trace:BUG-2458 | ai:codex
    let (existing, malformed) = OwnershipStore::at(store_dir(main)).list();
    let writer = WriterFacts::here();
    // trace:STORY-2537 trace:BUG-2458 | ai:codex
    let probe = |id: &aida_core::process_probe::ProcessIdentity| {
        use aida_core::{
            proc_identity::{ProcIdentity, Verdict},
            process_probe::Probe,
        };
        let Some(id) = ProcIdentity::from_record(id, writer.pid_ns) else {
            return Probe::Unknown("insufficient identity".into());
        };
        match aida_core::proc_identity::observe(&id, true) {
            Verdict::Alive => Probe::Alive,
            Verdict::Dead(why) => Probe::Dead(format!("{why:?}")),
            Verdict::ForeignUnknown(why) => Probe::Unknown(format!("{why:?}")),
        }
    };
    let host = HostFacts {
        node_id: aida_core::proc_identity::local_node_id(),
        host: writer.host.clone(),
        boot: writer.boot.clone(),
        pid_ns: writer.pid_ns,
        probe: &probe,
    };
    let mut gaps = Vec::new();
    let store = crate::load_store_for_lookup(main);
    if store.is_none() {
        gaps.push("no requirement store is reachable: spec scopes stay unresolved".to_string());
    }
    let prs = match open_prs(main) {
        Ok(prs) => prs,
        Err(why) => {
            gaps.push(format!(
                "the forge did not list open PRs ({why}): nothing is classified in-PR"
            ));
            BTreeMap::new()
        }
    };
    let by_number: BTreeMap<u64, OpenPr> = prs.values().map(|p| (p.n, p.clone())).collect();
    let resolve = |label: &str| {
        if let Ok(id) = uuid::Uuid::parse_str(label.trim()) {
            return Some(SpecKey(id));
        }
        store
            .as_ref()
            .and_then(|s| dual::spec_key_in_store(s, label))
    };
    let leases = crate::list_leases(main);
    let facts: Vec<LeaseFacts> = leases
        .iter()
        .map(|lease| {
            let key = dual::key_for_lease(lease, &resolve);
            let pr = dual::change_scope(&lease.scope)
                .and_then(|n| by_number.get(&n).cloned())
                .or_else(|| prs.get(&lease.branch).cloned());
            LeaseFacts {
                spec_closed: key
                    .as_ref()
                    .is_some_and(|(k, _)| spec_closed(store.as_ref(), k)),
                key,
                pr,
                tree: read_tree(&lease.worktree_path, &lease.branch),
                live_witnesses: live_witnesses(lease, &writer, &host, now),
                lease: lease.clone(),
            }
        })
        .collect();

    let mut accounted = Accounted::default();
    let lease_ids: std::collections::BTreeSet<&str> =
        leases.iter().map(|l| l.id.as_str()).collect();
    for name in file_names(&main.join(".aida").join("lanes")) {
        let id = name
            .strip_prefix("headless-")
            .and_then(|n| n.strip_suffix(".json"))
            .unwrap_or(&name);
        if lease_ids.contains(id) {
            accounted.lanes_derived += 1;
        } else {
            accounted.lanes_orphaned.push(name);
        }
    }
    accounted.review_markers = file_names(&crate::gate::review_marker::markers_dir(main));
    let mut trains = Vec::new();
    for (branch, h) in crate::merge_lock::status(main) {
        match h.pr.filter(|_| h.note == crate::pr_train::TRAIN_LEASE_NOTE) {
            Some(train) => {
                // trace:TASK-3364 | ai:codex
                let live_witness = crate::merge_lock::holder_witness(&h, now)
                    .filter(|w| super::judge_witness(w, &host, now) == WitnessReading::Alive);
                trains.push(TrainLeaseFacts {
                    train,
                    pid: h.pid,
                    live_witness,
                });
            }
            None => accounted
                .other_merge_leases
                .push(format!("{branch} ({})", h.note)),
        }
    }
    // BUG-1985/BUG-1929: through the hardened host builder, as `git_out`.
    let is_ancestor = |a: &str, b: &str| {
        aida_core::git_ops::host_git_command(main)
            .args(["merge-base", "--is-ancestor", a, b])
            .status()
            .is_ok_and(|s| s.success())
    };
    for (path, why) in malformed {
        gaps.push(format!("{}: {why}", path.display()));
    }
    let mut result = plan(&facts, &trains, accounted, gaps, &writer, &is_ancestor, now);
    result.existing = existing;
    result
}

/// The store directory the migration writes into.
// trace:STORY-2325 | ai:claude
pub(crate) fn store_dir(main: &Path) -> PathBuf {
    main.join(".aida").join(super::store::DIR)
}

/// Run the migration: plan, print, and with `execute` write.
// trace:STORY-2325 | ai:claude
pub(crate) fn run(main: &Path, execute: bool, json: bool) -> Result<()> {
    use std::io::IsTerminal as _;
    if execute {
        let contained = std::env::var("AIDA_CONTAINED").is_ok_and(|v| v == "1");
        let live_drain = crate::drain_lock::read_pid_live_lock(main).is_some();
        if let Some(why) = execute_refusal(
            std::io::stdin().is_terminal(),
            WriterFacts::here().pid_ns,
            contained,
            live_drain,
        ) {
            bail!("{why}");
        }
    }
    let plan = collect(main, Utc::now());
    let applied = if execute {
        Some(apply(&OwnershipStore::at(store_dir(main)), &plan)?)
    } else {
        None
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report_json(&plan, applied.as_ref()))?
        );
    } else {
        print_report(&plan, applied.as_ref());
    }
    Ok(())
}

fn report_json(plan: &MigrationPlan, applied: Option<&Applied>) -> serde_json::Value {
    serde_json::json!({
        "mode": if applied.is_some() { "execute" } else { "dry-run" },
        "leases": plan.leases.len(),
        "counts": plan.counts().into_iter().map(|(c, n)| (serde_json::to_value(c).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(), n)).collect::<BTreeMap<_, _>>(),
        "records": plan.records().len(),
        "holder_repair_candidates": plan.existing.iter().filter(|rec| rec.holder.as_ref().is_some_and(|h| h.witnesses.iter().any(|w| matches!(w.kind, super::types::WitnessKind::Process { .. }) && w.admission.is_none()))).map(|rec| serde_json::json!({"key": rec.key, "generation": rec.generation, "reason": "legacy evidence retained under existing reader policy; cooperative rebind required before cutover"})).collect::<Vec<_>>(),
        "trains": plan.trains.len(),
        "accounted": plan.accounted,
        "duplicates": plan.duplicates.iter().map(|(k, w)| serde_json::json!({"key": k, "input": w})).collect::<Vec<_>>(),
        "gaps": plan.gaps,
        "listed": plan.leases.iter().filter(|p| matches!(p.class, Class::Salvage | Class::NotAGitTree | Class::Unresolved)).map(|p| serde_json::json!({"lease": p.lease_id, "scope": p.scope, "class": p.class, "detail": p.detail})).collect::<Vec<_>>(),
        "applied": applied,
    })
}

fn print_report(plan: &MigrationPlan, applied: Option<&Applied>) {
    let mode = if applied.is_some() {
        "execute"
    } else {
        "dry run"
    };
    println!(
        "ownership migration ({mode}): {} lease(s)",
        plan.leases.len()
    );
    for (class, n) in plan.counts() {
        println!("  {:<14} {n}", format!("{class:?}"));
    }
    println!("  train leases   {}", plan.trains.len());
    println!(
        "  lanes: {} derived from a lease, {} orphaned; {} review marker(s); {} other merge lease(s)",
        plan.accounted.lanes_derived,
        plan.accounted.lanes_orphaned.len(),
        plan.accounted.review_markers.len(),
        plan.accounted.other_merge_leases.len()
    );
    println!("  records to write: {}", plan.records().len());
    for rec in &plan.existing {
        if rec.holder.as_ref().is_some_and(|h| {
            h.witnesses.iter().any(|w| {
                matches!(w.kind, super::types::WitnessKind::Process { .. }) && w.admission.is_none()
            })
        }) {
            println!("  holder repair {:?} generation {}: legacy evidence retained under existing reader policy; cooperative rebind required before cutover", rec.key, rec.generation);
        }
    }
    for gap in &plan.gaps {
        println!("  note: {gap}");
    }
    for (key, input) in &plan.duplicates {
        println!("  duplicate key {key:?}: {input}");
    }
    let listed: Vec<&LeasePlan> = plan
        .leases
        .iter()
        .filter(|p| {
            matches!(
                p.class,
                Class::Salvage | Class::NotAGitTree | Class::Unresolved
            )
        })
        .collect();
    if !listed.is_empty() {
        println!("for the operator:");
        for p in listed {
            println!("  {:?} {} ({}): {}", p.class, p.lease_id, p.scope, p.detail);
        }
    }
    match applied {
        Some(a) => println!(
            "wrote {} record(s); {} already recorded and kept. Nothing else changed; \
             remove {} to roll back.",
            a.written,
            a.kept,
            super::store::DIR
        ),
        None => println!("nothing was written; run with --execute to write the records"),
    }
}
