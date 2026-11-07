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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Entry {
    version: u32,
    pub(super) branch: String,
    pub(super) train: Built,
    pub(super) change_id: Option<u64>,
    pub(super) step: Step,
    completed_members: Vec<u64>,
    pub(super) rejected: Vec<Ejected>,
    call: String,
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
    fn path(&self, dir: &Path) -> PathBuf {
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

/// Permanent sidecar: serializes entire train runs across linked worktrees.
pub(super) fn lock(dir: &Path) -> Result<File> {
    use fs2::FileExt;
    std::fs::create_dir_all(dir)?;
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
    Ok(lock)
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

    /// Reconcile every unfinished train before admitting any new riders. A
    /// failed read retains the entry and stops admission. Completed mutations
    /// are detected from forge truth, including the call/receipt crash window.
    pub(super) fn resume(&mut self) -> Result<Outcome> {
        let Some(dir) = self.journal_dir.clone() else {
            return Ok(Outcome::default());
        };
        let mut out = Outcome::default();
        for entry in entries(&dir)? {
            if entry.step == Step::Done {
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
