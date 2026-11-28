//! Durable train intent, never authorization. The forge and fresh confirmation
//! decide whether an interrupted operation took effect.
// trace:STORY-2401 | ai:codex
use super::*;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::io::Write;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) enum Step {
    Open,
    Ci,
    Gate,
    Merge,
    Retire { why: String },
    Members,
    Done,
}

impl Step {
    /// The step's name as `aida pr train --status` shows it.
    // trace:STORY-2823 | ai:claude
    pub(super) fn label(&self) -> &'static str {
        match self {
            Step::Open => "Open",
            Step::Ci => "Ci",
            Step::Gate => "Gate",
            Step::Merge => "Merge",
            Step::Retire { .. } => "Retire",
            Step::Members => "Members",
            Step::Done => "Done",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Entry {
    version: u32,
    pub(super) branch: String,
    pub(super) train: Built,
    pub(super) change_id: Option<u64>,
    pub(super) step: Step,
    pub(super) completed_members: Vec<u64>,
    pub(super) rejected: Vec<Ejected>,
    pub(super) call: String,
}
impl Entry {
    pub(super) fn new(train: Built, branch: String) -> Self {
        Self {
            version: 1,
            branch,
            train,
            change_id: None,
            step: Step::Open,
            completed_members: Vec::new(),
            rejected: Vec::new(),
            call: String::new(),
        }
    }
    // trace:BUG-2547 | ai:codex
    pub(super) fn matches(&self, requested: &[u64]) -> bool {
        let members: std::collections::BTreeSet<_> =
            self.train.cars.iter().map(|c| c.member.pr).collect();
        !members.is_empty() && members == requested.iter().copied().collect()
    }

    pub(super) fn path(&self, dir: &Path) -> PathBuf {
        // Generated branches have one slash. Refuse path traversal on reads.
        dir.join(format!(
            "{}.json",
            self.branch.trim_start_matches(BRANCH_PREFIX)
        ))
    }
    fn validate(&self) -> Result<()> {
        let suffix = self.branch.strip_prefix(BRANCH_PREFIX).unwrap_or_default();
        anyhow::ensure!(
            self.version == 1
                && !suffix.is_empty()
                && suffix
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !self.train.tip.is_empty()
                && !self.train.base.is_empty()
                && !self.train.cars.is_empty(),
            "invalid train journal"
        );
        Ok(())
    }
}

thread_local! {
    static ACTIVE: RefCell<Option<(PathBuf, Entry)>> = const { RefCell::new(None) };
}

fn save(dir: &Path, entry: &Entry) -> Result<()> {
    entry.validate()?;
    let path = entry.path(dir);
    let temporary = dir.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(entry)?)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)?;
        File::open(dir)?.sync_all()?;
        note_step(&path, entry);
        // Persist Done before pruning: a crash in between leaves a harmless
        // terminal entry which the next startup also prunes.
        if entry.step == Step::Done {
            std::fs::remove_file(&path)?;
            File::open(dir)?.sync_all()?;
        }
        Ok(())
    })();
    let _ = std::fs::remove_file(temporary);
    result.context("could not persist train intent; forge call refused")
}

/// When a journal entered its current step, kept beside it as
/// `<journal>.step` for `aida pr train --status`. The journal itself is
/// `deny_unknown_fields`, so an older binary could not read a new field
/// there. Advisory only: a failed write never fails the train.
// trace:STORY-2823 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct StepClock {
    pub(super) step: String,
    pub(super) since: String,
}

fn step_clock_path(journal: &Path) -> PathBuf {
    journal.with_extension("step")
}

pub(super) fn step_clock(journal: &Path) -> Option<StepClock> {
    let body = aida_core::seat_clone_detect::read_regular_file(&step_clock_path(journal))?;
    serde_json::from_str(&body).ok()
}

// trace:STORY-2823 | ai:claude
fn note_step(journal: &Path, entry: &Entry) {
    let clock = step_clock_path(journal);
    if entry.step == Step::Done {
        let _ = std::fs::remove_file(clock);
        return;
    }
    let step = entry.step.label();
    if step_clock(journal).is_some_and(|c| c.step == step) {
        return;
    }
    if let Ok(body) = serde_json::to_vec(&StepClock {
        step: step.into(),
        since: chrono::Utc::now().to_rfc3339(),
    }) {
        // trace:STORY-2823 | ai:codex
        let temporary = journal.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let _ = std::fs::write(&temporary, body).and_then(|()| std::fs::rename(&temporary, &clock));
        let _ = std::fs::remove_file(temporary);
    }
}

/// The process holding the run lock, written into the lock file so
/// `aida pr train --status` can name the driver. Advisory only: the lock,
/// not this record, serializes trains.
// trace:STORY-2823 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct DriverRecord {
    pub(super) holder: aida_core::proc_identity::HolderRecord,
    pub(super) since: String,
    // Status evidence only; controllers may be subreapers, which cannot be holders.
    // trace:BUG-2622 | ai:codex
    #[serde(default)]
    pub(super) controller: Option<aida_core::proc_identity::ProcIdentity>,
}

/// This process as a driver record. Identities are read before the lock
/// file is opened: the lock handle is inherited by any
/// child forked while it is open, so nothing slow may run in that window.
fn driver_record_for_self() -> Option<Vec<u8>> {
    static HOLDER: std::sync::OnceLock<aida_core::proc_identity::HolderRecord> =
        std::sync::OnceLock::new();
    serde_json::to_vec(&DriverRecord {
        holder: HOLDER
            .get_or_init(|| aida_core::proc_identity::HolderRecord::capture(std::process::id()))
            .clone(),
        since: chrono::Utc::now().to_rfc3339(),
        controller: super::ADVISOR_LOOP
            .with(std::cell::Cell::get)
            .then(|| aida_core::proc_identity::ProcIdentity::capture(std::process::id()).ok())
            .flatten(),
    })
    .ok()
}

fn record_driver(mut lock: &File, record: Option<Vec<u8>>) {
    if let Some(body) = record {
        let _ = lock.set_len(0).and_then(|()| lock.write_all(&body));
    }
}

pub(super) fn driver_record(dir: &Path) -> Option<DriverRecord> {
    let body = aida_core::seat_clone_detect::read_regular_file(&dir.join("run.lock"))?;
    serde_json::from_str(&body).ok()
}

/// Whether some process holds the run lock right now, without creating the
/// lock file. The probe takes a shared lock for an instant when nobody
/// holds it. Driver records are advisory and never replace this probe.
// trace:STORY-2823 | ai:claude
pub(super) fn lock_held(dir: &Path) -> Result<bool> {
    use fs2::FileExt;
    let lock = match File::open(dir.join("run.lock")) {
        Ok(lock) => lock,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).context("could not open the train run lock"),
    };
    // The std inherent method of the same name has another error type.
    match FileExt::try_lock_shared(&lock) {
        Ok(()) => {
            let _ = FileExt::unlock(&lock);
            Ok(false)
        }
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(true),
        Err(e) => Err(e).context("could not probe the train run lock"),
    }
}

pub(super) fn activate(dir: &Path, entry: Entry) -> Result<()> {
    save(dir, &entry).map_err(|e| TrainCiUnavailable(format!("{e:#}")))?;
    ACTIVE.with(|a| {
        *a.borrow_mut() = if entry.step == Step::Done {
            None
        } else {
            Some((dir.to_path_buf(), entry))
        };
    });
    Ok(())
}
pub(super) fn clear() {
    ACTIVE.with(|a| *a.borrow_mut() = None);
}
fn update(f: impl FnOnce(&mut Entry)) -> Result<()> {
    ACTIVE.with(|a| {
        let mut active = a.borrow_mut();
        if let Some((dir, entry)) = active.as_mut() {
            f(entry);
            save(dir, entry).map_err(|e| TrainCiUnavailable(format!("{e:#}")))?;
            if entry.step == Step::Done {
                *active = None;
            }
        }
        Ok(())
    })
}
/// Checkpoint even nested reads/polls, preserving the lifecycle intent.
pub(super) fn before(call: &str) -> Result<()> {
    update(|entry| entry.call = call.into())
}
pub(super) fn member_done(pr: u64) -> bool {
    ACTIVE.with(|a| {
        a.borrow()
            .as_ref()
            .is_some_and(|(_, e)| e.completed_members.contains(&pr))
    })
}
pub(super) fn complete_member(pr: u64) -> Result<()> {
    update(|entry| entry.completed_members.push(pr))
}
pub(super) fn rejected(members: &[Ejected]) -> Result<()> {
    update(|entry| entry.rejected = members.to_vec())
}

// Read-only probe: entries() can retire terminal receipts and requires the lock.
// trace:STORY-2857 | ai:codex
pub(super) fn recovery_needed(dir: &Path) -> Result<bool> {
    let listing = match std::fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let mut needed = false;
    for entry in listing {
        needed |= entry?.path().extension().is_some_and(|ext| ext == "json");
    }
    Ok(needed)
}

pub(super) fn entries(dir: &Path) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in std::fs::read_dir(dir)? {
        let path = item?.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let entry: Entry = serde_json::from_slice(
                &std::fs::read(&path)
                    .with_context(|| format!("could not read train journal {}", path.display()))?,
            )
            .with_context(|| format!("unreadable train journal {}", path.display()))?;
            entry
                .validate()
                .with_context(|| format!("invalid train journal {}", path.display()))?;
            anyhow::ensure!(
                entry.path(dir) == path,
                "train journal filename does not match branch"
            );
            if entry.step == Step::Done {
                // Also collect terminal receipts left by older binaries or a
                // crash after the final durable write but before removal.
                std::fs::remove_file(&path)?;
                let _ = std::fs::remove_file(step_clock_path(&path));
                File::open(dir)?.sync_all()?;
            } else {
                entries.push(entry);
            }
        }
    }
    entries.sort_by(|a, b| a.branch.cmp(&b.branch));
    Ok(entries)
}

// trace:STORY-2401 | ai:codex
#[derive(Debug)]
pub(crate) struct TrainJournalBusy;
impl std::fmt::Display for TrainJournalBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("another train owns the train journal; retry when it exits")
    }
}
impl std::error::Error for TrainJournalBusy {}

// trace:BUG-2622 | ai:codex
#[derive(Debug)]
pub(super) struct RunLock(File, PathBuf, std::time::Instant);
impl Drop for RunLock {
    fn drop(&mut self) {
        // Still under the lock, so appends never interleave.
        // trace:STORY-3131 | ai:claude
        note_run(&self.1, self.2.elapsed().as_secs());
        // flock belongs to the open file description. Closing our descriptor
        // alone leaves it held when a forked child retains that description.
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

/// Permanent sidecar: serializes entire train runs across linked worktrees.
pub(super) fn lock(dir: &Path) -> Result<RunLock> {
    use fs2::FileExt;
    std::fs::create_dir_all(dir)?;
    let record = driver_record_for_self();
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("run.lock"))?;
    if let Err(err) = lock.try_lock_exclusive() {
        if err.kind() == std::io::ErrorKind::WouldBlock {
            return Err(TrainJournalBusy.into());
        }
        return Err(err).context("could not lock train journal");
    }
    record_driver(&lock, record);
    Ok(RunLock(lock, dir.to_path_buf(), std::time::Instant::now()))
}

/// How long past train runs held the lock, newest last, for the
/// landing-drought threshold. Best effort; capped at [`RUN_HISTORY_CAP`].
// trace:STORY-3131 | ai:claude
const RUN_HISTORY: &str = "run-history.jsonl";
const RUN_HISTORY_CAP: usize = 50;

#[derive(Serialize, Deserialize)]
struct RunRecord {
    secs: u64,
}

fn note_run(dir: &Path, secs: u64) {
    let path = dir.join(RUN_HISTORY);
    let mut lines: Vec<String> = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    let Ok(line) = serde_json::to_string(&RunRecord { secs }) else {
        return;
    };
    lines.push(line);
    let keep = &lines[lines.len().saturating_sub(RUN_HISTORY_CAP)..];
    let temporary = dir.join(format!("{RUN_HISTORY}.{}.tmp", uuid::Uuid::new_v4()));
    let body = keep.join("\n") + "\n";
    let _ = std::fs::write(&temporary, body).and_then(|()| std::fs::rename(&temporary, &path));
    let _ = std::fs::remove_file(temporary);
}

/// Seconds each recorded train run held the lock, oldest first.
pub(super) fn run_history(dir: &Path) -> Vec<u64> {
    std::fs::read_to_string(dir.join(RUN_HISTORY))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<RunRecord>(l).ok())
        .map(|r| r.secs)
        .collect()
}

/// A recovery waiting on its train's CI outside every lock: what its
/// re-admitted pass re-validates. It withholds train admission from others
/// and never grants anything. No `.json`/`.step` extension, so journal
/// readers and older binaries ignore it. trace:TASK-3920 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Parked {
    pub(crate) version: u32,
    pub(crate) token: String,
    pub(crate) branch: String,
    pub(crate) change_id: Option<u64>,
    pub(crate) step: String,
    pub(crate) tip: String,
    pub(crate) expected_main: String,
    pub(crate) member_heads: Vec<(u64, String)>,
    pub(crate) owner: aida_core::proc_identity::ProcIdentity,
    /// When the parking pass took admission: its fairness position.
    pub(crate) position: Time,
    pub(crate) parked_at: Time,
    pub(crate) deadline: Time,
    pub(crate) expires_at: Time,
}
type Time = chrono::DateTime<chrono::Utc>;
const PARKED: &str = "recovery.parked";
/// The longest wait, the longest re-acquire grace, and the intent slack.
const PARK_MAX_LIFETIME: i64 = 3600 + 3600 + 30;

impl Parked {
    /// Q1 (advisor): a ticketed resume matches by branch, tip and member
    /// heads, only while the car set is exactly the parked one.
    #[cfg_attr(not(test), allow(dead_code))] // the resume (slice 2) calls it
    pub(super) fn matches(&self, entry: &Entry) -> bool {
        let mut cars: Vec<_> = (entry.train.cars.iter())
            .map(|c| (c.member.pr, c.member.head.clone()))
            .collect();
        let mut parked = self.member_heads.clone();
        cars.sort();
        parked.sort();
        entry.branch == self.branch
            && entry.train.tip == self.tip
            && !cars.is_empty()
            && cars == parked
    }

    pub(crate) fn describe(&self) -> String {
        let at = |t: &Time| t.with_timezone(&chrono::Local).format("%H:%M").to_string();
        let train = self
            .change_id
            .map_or(self.branch.clone(), |id| format!("PR #{id}"));
        format!(
            "auto-train recovery of train {train} parked since {} waiting on CI (position {}, expires {}; holds no lock)",
            at(&self.parked_at), at(&self.position), at(&self.expires_at)
        )
    }
}

/// The parked recovery in effect at `now`: unexpired, its owner not dead,
/// and no more than the lifetime left. With `prune` (under the admission
/// serializer) an expired or dead owner's record is removed, an unparsable
/// one after 30 s. trace:TASK-3920 | ai:claude
pub(super) fn parked(dir: &Path, now: Time, prune: bool) -> Result<Option<Parked>> {
    let path = dir.join(PARKED);
    let body = match aida_core::seat_clone_detect::read_regular_file_result_with_limit(&path, 16384)
    {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        body => body.context("the parked recovery record is unreadable")?,
    };
    let row = serde_json::from_str::<Parked>(&body).ok();
    let stale = row.as_ref().map_or_else(
        || {
            let modified = std::fs::metadata(&path).and_then(|meta| meta.modified());
            modified.is_ok_and(|at| at.elapsed().is_ok_and(|age| age.as_secs() >= 30))
        },
        |row| {
            let dead = aida_core::proc_identity::observe(&row.owner, true);
            row.expires_at <= now || matches!(dead, aida_core::proc_identity::Verdict::Dead(_))
        },
    );
    if stale && prune {
        match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error)?,
            _ => {}
        }
    }
    let max = chrono::Duration::seconds(PARK_MAX_LIFETIME);
    Ok(row.filter(|row| {
        row.version == 1
            && !stale
            && row.expires_at.signed_duration_since(now) <= max
            && row.expires_at.signed_duration_since(row.parked_at) <= max
    }))
}

/// Persist durably (temporary file, fsync, rename, directory fsync), or
/// with `None` remove the record only while it carries `token`; whether it
/// changed anything. Callers hold the admission serializer.
// trace:TASK-3920 | ai:claude
#[cfg_attr(not(test), allow(dead_code))] // the park point (slice 2) calls it
pub(super) fn park(dir: &Path, record: Option<&Parked>, token: &str) -> Result<bool> {
    let path = dir.join(PARKED);
    let Some(record) = record else {
        let body = aida_core::seat_clone_detect::read_regular_file(&path).unwrap_or_default();
        if serde_json::from_str::<Parked>(&body).is_ok_and(|r| r.token == token) {
            std::fs::remove_file(&path)?;
            File::open(dir)?.sync_all()?;
            return Ok(true);
        }
        return Ok(false);
    };
    std::fs::create_dir_all(dir)?;
    let temporary = dir.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(record)?)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)?;
        Ok(File::open(dir)?.sync_all()?)
    })();
    let _ = std::fs::remove_file(temporary);
    result
        .context("could not persist the parked recovery record")
        .map(|()| true)
}

impl ForgeRails<'_> {
    pub(super) fn with_journal(mut self, dir: &Path) -> Self {
        self.journal_dir = Some(dir.to_path_buf());
        self._journal_scope = Some(Scope);
        self
    }
    pub(super) fn intent(
        &mut self,
        change: &crate::forge::ChangeRef,
        train: &Built,
        step: Option<Step>,
    ) -> Result<()> {
        let Some(dir) = &self.journal_dir else {
            return Ok(());
        };
        let path = Entry::new(train.clone(), change.branch.clone()).path(dir);
        let mut entry: Entry = serde_json::from_slice(&std::fs::read(path)?)?;
        anyhow::ensure!(
            entry.train.tip == train.tip && entry.change_id.is_none_or(|id| id == change.id),
            "train journal pin changed"
        );
        entry.change_id = Some(change.id);
        if let Some(step) = step {
            entry.step = step;
        }
        activate(dir, entry)
    }

    /// Reconcile matching unfinished trains before admitting new riders. A
    /// failed read retains the entry and stops admission. Completed mutations
    /// are detected from forge truth, including the call/receipt crash window.
    #[cfg(test)]
    pub(super) fn resume(&mut self) -> Result<Outcome> {
        self.resume_for(None)
    }

    // trace:BUG-2547 | ai:codex
    pub(super) fn resume_for(&mut self, requested: Option<&[u64]>) -> Result<Outcome> {
        let Some(dir) = self.journal_dir.clone() else {
            return Ok(Outcome::default());
        };
        let mut out = Outcome::default();
        for entry in entries(&dir)? {
            if entry.step == Step::Done {
                continue;
            }
            if requested.is_some_and(|prs| !entry.matches(prs)) {
                eprintln!("Skipping retained train journal {}: member PR set differs from requested PRs; starting fresh. Use `aida pr train --abandon {}` to discard it.", entry.path(&dir).display(), entry.path(&dir).file_name().unwrap().to_string_lossy());
                continue;
            }
            activate(&dir, entry.clone())?;
            before("recover train")?;
            let (change, state) = self.ports.recover_change(&entry)?;
            self.intent(&change, &entry.train, None)?;
            self.opened.push((entry.train.tip.clone(), change.clone()));
            self.built.insert(change.id, entry.train.clone());
            self.live.push(change.clone());
            before("confirm recovered landing")?;
            match self.ports.confirm_not_landed(&change, &entry.train) {
                crate::forge_observed::Confirmed::Positive => {
                    self.finish_landed(&change, &entry.train)?;
                    out.landed.extend(
                        self.landed_on_main
                            .iter()
                            .filter(|(pr, _)| entry.train.cars.iter().any(|c| c.member.pr == *pr))
                            .cloned(),
                    );
                }
                crate::forge_observed::Confirmed::Unconfirmed => anyhow::bail!(
                    "train PR #{} recovery unconfirmed; members retained",
                    change.id
                ),
                crate::forge_observed::Confirmed::Negative => {
                    if matches!(entry.step, Step::Members) {
                        anyhow::bail!(
                            "journaled landing disagrees with forge truth; members retained"
                        );
                    }
                    if let Step::Retire { why } = &entry.step {
                        // The journal remembers the decision, not permission to
                        // eject a member whose live policy/head has changed.
                        let mut fresh_rejections = Vec::new();
                        for saved in &entry.rejected {
                            let delay = self.ports.confirmation_backoff();
                            let gate = crate::forge_observed::confirm_snapshot(
                                || self.ports.observe_gate(saved.pr),
                                |_| crate::forge_observed::Observed::Known(true),
                                || std::thread::sleep(delay),
                            );
                            match gate {
                                crate::forge_observed::Observed::Known(Err(reason)) => {
                                    fresh_rejections.push(Ejected {
                                        pr: saved.pr,
                                        reason,
                                    });
                                }
                                crate::forge_observed::Observed::Known(Ok(member)) => {
                                    let pinned = entry
                                        .train
                                        .cars
                                        .iter()
                                        .find(|c| c.member.pr == saved.pr)
                                        .context("journal rejection names no train member")?;
                                    if member.head != pinned.member.head {
                                        anyhow::ensure!(
                                            self.ports.confirm_member_head(saved.pr, &member.head),
                                            "recovered member head change unconfirmed"
                                        );
                                        fresh_rejections.push(saved.clone());
                                    }
                                    // A lifted refusal leaves the member eligible
                                    // for the replacement train after retirement.
                                }
                                _ => anyhow::bail!(
                                    "recovered member refusal unconfirmed; members retained"
                                ),
                            }
                        }
                        rejected(&fresh_rejections)?;
                        self.retire(&change, why)?;
                        out.ejected.extend(fresh_rejections);
                    } else if state == crate::forge::ChangeState::Closed {
                        // A closed train did not land; do not replay the close.
                        self.intent(&change, &entry.train, Some(Step::Done))?;
                        self.live.retain(|c| c.id != change.id);
                    } else {
                        // Resume the existing tested head. No second open/push,
                        // and land() still takes the lease and fresh live gates.
                        // Reuse normal waiting, red confirmation and retirement.
                        // A prior green receipt never freezes CI at that result.
                        let verdict = self.test(std::slice::from_ref(&entry.train))?.remove(0);
                        out.ci_runs += 1;
                        if verdict == CiVerdict::Green {
                            match self.land(&entry.train)? {
                                Landing::Landed(_) => out.landed.extend(
                                    self.landed_on_main
                                        .iter()
                                        .filter(|(pr, _)| {
                                            entry.train.cars.iter().any(|c| c.member.pr == *pr)
                                        })
                                        .cloned(),
                                ),
                                Landing::Refused(ejected) => out.ejected.extend(ejected),
                                Landing::Stale => {}
                            }
                        }
                    }
                }
            }
        }
        clear();
        Ok(out)
    }
}
pub(super) struct Scope;
impl Drop for Scope {
    fn drop(&mut self) {
        clear();
    }
}

/// Report reservations without the recovery reader's terminal-file cleanup.
// trace:STORY-2688 | ai:codex
pub(super) fn pr_state_members(dir: &Path) -> Result<Vec<u64>> {
    let mut members = Vec::new();
    for (_, entry) in peek(dir)? {
        members.extend(entry.train.cars.iter().map(|c| c.member.pr).filter(|pr| {
            !entry.completed_members.contains(pr) && !entry.rejected.iter().any(|r| r.pr == *pr)
        }));
    }
    Ok(members)
}

/// Unfinished journals with their paths, in branch order. Read-only: unlike
/// [`entries`] it removes nothing and takes no lock.
// trace:STORY-2688 | ai:codex
// trace:STORY-2823 | ai:claude
pub(super) fn peek(dir: &Path) -> Result<Vec<(PathBuf, Entry)>> {
    peek_snapshot(dir, &|| {})
}

// Running journals include accumulated CI evidence. Allow 16 MiB, matching the
// train evidence ceiling, rather than the 64 KiB seat-marker limit.
// trace:BUG-2595 | ai:codex
pub(super) const MAX_JOURNAL_BYTES: u64 = 16 * 1024 * 1024;

// One finite listing; concurrent retirement does not trigger rediscovery.
// trace:STORY-2823 | ai:codex
pub(super) fn peek_snapshot(dir: &Path, after_listing: &dyn Fn()) -> Result<Vec<(PathBuf, Entry)>> {
    const MAX_LISTING: usize = 4096;
    let listing = match std::fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut paths = Vec::new();
    for item in listing.take(MAX_LISTING + 1) {
        paths.push(item?.path());
        anyhow::ensure!(
            paths.len() <= MAX_LISTING,
            "train journal listing exceeds status snapshot budget"
        );
    }
    after_listing();
    let mut found = Vec::new();
    for path in paths {
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let body = match aida_core::seat_clone_detect::read_regular_file_result_with_limit(
            &path,
            MAX_JOURNAL_BYTES,
        ) {
            Ok(body) => body,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(e).with_context(|| {
                    format!(
                        "unsafe or unreadable train receipt {} (cap: {} bytes)",
                        path.display(),
                        MAX_JOURNAL_BYTES
                    )
                })
            }
        };
        let entry: Entry = serde_json::from_str(&body)?;
        entry.validate()?;
        anyhow::ensure!(entry.path(dir) == path, "train receipt filename mismatch");
        if entry.step != Step::Done {
            found.push((path, entry));
        }
    }
    found.sort_by(|a, b| a.1.branch.cmp(&b.1.branch));
    Ok(found)
}

// trace:BUG-2547 | ai:codex
pub(super) fn abandon(dir: &Path, name: &str) -> Result<()> {
    anyhow::ensure!(
        Path::new(name).components().count() == 1
            && name.ends_with(".json")
            && !name.starts_with('.'),
        "name a train journal filename, such as <uuid>-1.json"
    );
    let _lock = lock(dir)?;
    let path = dir.join(name);
    let metadata = std::fs::symlink_metadata(&path)?;
    anyhow::ensure!(
        metadata.file_type().is_file(),
        "train journal must be a regular file"
    );
    std::fs::remove_file(&path)?;
    let _ = std::fs::remove_file(step_clock_path(&path));
    File::open(dir)?.sync_all()?;
    println!(
        "Abandoned local train journal {}; forge PRs and branches are unchanged.",
        path.display()
    );
    Ok(())
}

// trace:BUG-2622 | ai:codex
#[cfg(test)]
mod run_lock_tests {
    use super::*;

    // trace:TASK-3128 | ai:codex
    #[test]
    fn task3128_retained_description_does_not_delay_or_release_successor_lock() {
        use fs2::FileExt;

        let dir = tempfile::tempdir().unwrap();
        let first = lock(dir.path()).unwrap();
        // dup shares the open file description just as fork does, without
        // timing-dependent concurrent spawns in the threaded test runner.
        let retained = first.0.try_clone().unwrap();
        drop(first);
        let successor = lock(dir.path()).unwrap();
        assert!(lock_held(dir.path()).unwrap());
        drop(retained);
        assert!(lock_held(dir.path()).unwrap());
        let error = lock(dir.path()).unwrap_err();
        assert!(error.downcast_ref::<TrainJournalBusy>().is_some());
        drop(successor);

        // Positive control: closing a bare File really does retain flock
        // ownership through its duplicate. The guard's explicit unlock is
        // what makes immediate reacquisition above safe.
        let bare = File::open(dir.path().join("run.lock")).unwrap();
        bare.try_lock_exclusive().unwrap();
        let retained = bare.try_clone().unwrap();
        drop(bare);
        assert!(lock_held(dir.path()).unwrap());
        // Release explicitly here too: another test thread may fork while
        // the control is open, retaining an additional description.
        FileExt::unlock(&retained).unwrap();
        drop(retained);
        assert!(lock(dir.path()).is_ok());
    }

    #[test]
    fn bug2622_pass_releases_lock_even_with_an_inherited_description() {
        let dir = tempfile::tempdir().unwrap();
        for fail in [false, true] {
            let mut retained = None;
            let result = (|| -> Result<()> {
                let pass = lock(dir.path())?;
                retained = Some(pass.0.try_clone()?);
                assert!(lock(dir.path()).is_err());
                if fail {
                    anyhow::bail!("gate refused");
                }
                Ok(())
            })();
            assert_eq!(result.is_err(), fail);
            assert!(!lock_held(dir.path()).unwrap());
            let path = dir.path().join("closed-1.json");
            std::fs::write(&path, "closed journal").unwrap();
            abandon(dir.path(), "closed-1.json").unwrap();
            assert!(!path.exists());
            drop(retained);
        }
    }
}

#[cfg(test)]
mod task3920_parked_tests {
    use super::*;

    /// Q1: a parked record matches its entry only with the exact car set and
    /// heads; it is read by no journal reader and removed only by its token.
    // trace:TASK-3920 | ai:claude
    #[test]
    fn task3920_parked_matches_exact_cars_and_unparks_by_token() {
        let entry = |prs: &[u64]| {
            let cars = prs.iter().map(|pr| Car {
                member: Member {
                    pr: *pr,
                    head: format!("{pr:040}"),
                    ..Default::default()
                },
                commit: String::new(),
                patch_id: String::new(),
            });
            let (base, tip, dropped) = ("b".repeat(40), "c".repeat(40), Vec::new());
            Entry::new(
                Built {
                    base,
                    tip,
                    cars: cars.collect(),
                    dropped,
                },
                "train/abc-1".into(),
            )
        };
        let me = aida_core::proc_identity::ProcIdentity::capture(std::process::id()).unwrap();
        let (parked, now) = (test_parked(me, 0, 60), chrono::Utc::now());
        assert!(parked.matches(&entry(&[12, 11])));
        assert!(!parked.matches(&entry(&[11])), "a member removed");
        assert!(!parked.matches(&entry(&[11, 12, 13])), "a member added");
        let mut moved = entry(&[11, 12]);
        moved.train.cars[0].member.head = "f".repeat(40);
        assert!(!parked.matches(&moved), "a member head moved");
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        assert!(park(dir, Some(&parked), "").unwrap());
        assert_eq!(super::parked(dir, now, true).unwrap(), Some(parked.clone()));
        assert!(!recovery_needed(dir).unwrap() && entries(dir).unwrap().is_empty());
        assert!(!park(dir, None, "another token").unwrap());
        assert!(park(dir, None, &parked.token).unwrap());
        assert_eq!(super::parked(dir, now, true).unwrap(), None);
    }
}
