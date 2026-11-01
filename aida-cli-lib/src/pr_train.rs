//! `aida pr train` — the STORY-1690 merge train (signed sketch option (b)).
//!
//! Two PRs that are each green can still break main together: each one's CI
//! ran against a main without the other (#2599/#2597). The train lands a batch
//! of approved PRs only after CI has passed on EXACTLY the commit sequence main
//! will receive:
//!
//! 1. every member passes the member gate — open, not a draft (TASK-1378), on
//!    the default branch, no merge hold, an attested approval covering its
//!    head (with the TASK-1960 carry-over), the verdict corpus clear;
//! 2. [`build`] stacks one squash commit per member on main, written with the
//!    subject `aida pr ship` would write, and proves each commit's patch-id
//!    equals the member's approved diff — the attestation carry-over proof. A
//!    member that conflicts, or whose diff changes when stacked, is dropped
//!    with its reason and never auto-resolved;
//! 3. [`drive`] tests the train tip (a `train/*` PR, so CI and the train mode
//!    of merge-hold-gate run unchanged), lands a green train by a rebase merge
//!    pinned to its tip — only while main is still the train's base — and on
//!    red reruns once, then bisects by halves, two sub-trains at a time. A
//!    one-member train that is red ejects that member. Main never receives an
//!    untested combination.
// trace:STORY-1690 | ai:claude

use anyhow::{Context, Result};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

/// The train PR body line that names its members, `#N@<head>` comma-joined.
/// merge-hold-gate's train mode reads it.
// trace:STORY-1690 | ai:claude
pub(crate) const MEMBERS_LINE: &str = "aida-train-members:";
/// Branch prefix of a train PR; merge-hold-gate keys its train mode on it.
pub(crate) const BRANCH_PREFIX: &str = "train/";
/// The note a train writes into the base's merge-lease; the BUG-2220 rider
/// gate keys live-lease train detection on it.
// trace:BUG-2220 | ai:claude
pub(crate) const TRAIN_LEASE_NOTE: &str = "aida pr train";
/// Two sub-trains test at once while bisecting.
const PARALLEL_SUB_TRAINS: usize = 2;

/// One PR the train may land: its number, the head its approval covers, and
/// the commit message `aida pr ship` would have given its squash merge.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Member {
    pub pr: u64,
    pub head: String,
    pub subject: String,
    pub body: String,
    /// BUG-2214: the mandate cover under which the land step releases this
    /// member's merge-hold, from the gate's most recent cover check (the
    /// pre-land re-gate refreshes it under the merge lease). `None` = no hold
    /// to release.
    // trace:BUG-2214 | ai:claude
    pub release: Option<crate::gate::mandate_ship::MandateCover>,
}

/// A member left off main, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ejected {
    pub pr: u64,
    pub reason: String,
}

/// A member's commit in a built train.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Car {
    pub member: Member,
    pub commit: String,
    pub patch_id: String,
}

/// A train built on `base`: `tip` is `base` plus one commit per car.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Built {
    pub base: String,
    pub tip: String,
    pub cars: Vec<Car>,
    pub dropped: Vec<Ejected>,
}

impl Built {
    pub(crate) fn members(&self) -> Vec<Member> {
        self.cars.iter().map(|c| c.member.clone()).collect()
    }
}

/// The train PR body line for `cars`.
// trace:STORY-1690 | ai:claude
pub(crate) fn members_line(cars: &[Car]) -> String {
    let list: Vec<String> = cars
        .iter()
        .map(|c| format!("{}@{}", c.member.pr, c.member.head))
        .collect();
    format!("{MEMBERS_LINE} {}", list.join(","))
}

/// The members a train PR body names, in order. `None` when the body has no
/// members line, names no member, or any entry is malformed: the merge-hold
/// gate fails closed on it (the rider gate, `train_rider_refusal`, fails
/// open). It parses exactly as merge-hold-gate.yml's bash does (the binding
/// parser); tests/fixtures/train-members-lines.txt holds the cases both are
/// checked against.
// trace:STORY-1690 | ai:claude
pub(crate) fn parse_members_line(body: &str) -> Option<Vec<(u64, String)>> {
    let rest = body
        .lines()
        .find_map(|l| l.trim_start().strip_prefix(MEMBERS_LINE))?;
    let members: Option<Vec<(u64, String)>> = rest
        // The bash gate turns commas into spaces and word-splits.
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let (n, sha) = entry.split_once('@')?;
            let n = n.strip_prefix('#').unwrap_or(n);
            let n_ok =
                !n.is_empty() && !n.starts_with('0') && n.bytes().all(|b| b.is_ascii_digit());
            let sha_ok = sha.len() >= 7 && sha.bytes().all(|b| b.is_ascii_hexdigit());
            if !(n_ok && sha_ok) {
                return None;
            }
            Some((n.parse().ok()?, sha.to_string()))
        })
        .collect();
    members.filter(|m| !m.is_empty())
}

/// The commit message of a member's car: the squash subject (ending in
/// `(#N)`, so `shipped_pr` linkage finds the PR), the squash body, and a
/// trailer naming the approved head the content came from.
// trace:STORY-1690 | ai:claude
pub(crate) fn car_message(m: &Member) -> String {
    let suffix = format!("(#{})", m.pr);
    let subject = m.subject.trim();
    let subject = if subject.ends_with(&suffix) {
        subject.to_string()
    } else {
        format!("{subject} {suffix}")
    };
    let body = m.body.trim();
    let trailer = format!("Aida-Train-Member: #{} at {}", m.pr, m.head);
    if body.is_empty() {
        format!("{subject}\n\n{trailer}\n")
    } else {
        format!("{subject}\n\n{body}\n\n{trailer}\n")
    }
}

/// Build a train on `base` from `members`, in order, in `repo`'s object store
/// (no worktree is touched). For each member: the member's own diff is
/// `merge-base(base, head)..head`; its car is git's merge of that diff onto
/// the train so far, committed with [`car_message`]; and the car's patch-id
/// must equal the member's. A conflict, an empty diff, criss-cross history or
/// a patch-id that changes when stacked drops the member with its reason.
// trace:STORY-1690 | ai:claude
pub(crate) fn build(repo: &Path, base: &str, members: &[Member]) -> Result<Built> {
    let base = rev_parse(repo, base).with_context(|| format!("train base `{base}`"))?;
    let mut tip = base.clone();
    let mut cars = Vec::new();
    let mut dropped = Vec::new();
    for m in members {
        match add_car(repo, &base, &tip, m) {
            Ok(car) => {
                tip = car.commit.clone();
                cars.push(car);
            }
            Err(reason) => dropped.push(Ejected { pr: m.pr, reason }),
        }
    }
    Ok(Built {
        base,
        tip,
        cars,
        dropped,
    })
}

fn add_car(repo: &Path, base: &str, tip: &str, m: &Member) -> std::result::Result<Car, String> {
    let head = rev_parse(repo, &m.head).map_err(|_| {
        format!(
            "its approved head {} is not available locally",
            short(&m.head)
        )
    })?;
    let fork = git(repo, &["merge-base", "--all", base, &head]).map_err(|e| e.to_string())?;
    let fork = match fork.split_whitespace().collect::<Vec<_>>().as_slice() {
        [one] => one.to_string(),
        [] => return Err("it shares no history with main".to_string()),
        _ => {
            return Err(
                "its history has more than one merge base with main (criss-cross); \
                 merge main into the PR and re-train"
                    .to_string(),
            )
        }
    };
    let want = crate::gate::review_carryover::patch_id(repo, &fork, &head)
        .map_err(|e| format!("its own diff has no patch-id: {}", e.reason()))?;
    let out = git_raw(
        repo,
        &[
            "merge-tree",
            "--write-tree",
            "--no-messages",
            "--merge-base",
            &fork,
            tip,
            &head,
        ],
    )
    .map_err(|e| e.to_string())?;
    let tree = match out.status.code() {
        Some(0) => String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string(),
        Some(1) => {
            return Err(
                "it conflicts with main or an earlier member of this train; \
                 merge main into the PR (resolving by hand) and re-train"
                    .to_string(),
            )
        }
        _ => {
            return Err(format!(
                "`git merge-tree --write-tree --merge-base` failed (needs git 2.40 or later): {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    };
    if tree.is_empty() {
        return Err("`git merge-tree` printed no tree".to_string());
    }
    let commit = commit_tree(repo, &tree, tip, &car_message(m)).map_err(|e| e.to_string())?;
    let got = crate::gate::review_carryover::patch_id(repo, tip, &commit)
        .map_err(|e| format!("its car has no patch-id: {}", e.reason()))?;
    if got != want {
        return Err(format!(
            "its diff changes when stacked on main and the earlier members (patch-id {} \
             approved, {} in the train), so the approval does not cover what would land; \
             merge main into the PR and re-train",
            short(&want),
            short(&got)
        ));
    }
    Ok(Car {
        member: m.clone(),
        commit,
        patch_id: got,
    })
}

/// A train's CI result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CiVerdict {
    Green,
    /// Red, with where to look.
    Red(String),
}

/// What landing a green train did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Landing {
    /// Main received the train; the new main tip.
    Landed(String),
    /// Main moved since the train was built: rebuild and retest.
    Stale,
    /// The pre-land recheck refused these members; the rest rebuild.
    Refused(Vec<Ejected>),
}

/// The side effects a train run needs, so [`drive`] is testable without a
/// forge: read main, build, test several trains at once, land one.
pub(crate) trait Rails {
    fn main_tip(&mut self) -> Result<String>;
    fn build(&mut self, base: &str, members: &[Member]) -> Result<Built>;
    /// Test every train; verdicts in the same order.
    fn test(&mut self, trains: &[Built]) -> Result<Vec<CiVerdict>>;
    fn land(&mut self, train: &Built) -> Result<Landing>;
    /// Told once per member that is not landing, after the run.
    fn ejected(&mut self, _e: &Ejected) {}
    /// The run stopped on `error`: release whatever is still open.
    fn abort(&mut self, _error: &str) {}
}

/// The result of a train run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Outcome {
    /// (PR, car commit as built) in landing order.
    pub landed: Vec<(u64, String)>,
    pub ejected: Vec<Ejected>,
    /// CI runs spent.
    pub ci_runs: usize,
    /// The error that stopped the run early; every member it left in flight
    /// is in `ejected`.
    pub error: Option<String>,
}

/// Drive `members` to main through `rails`: build on the current main, test,
/// land green trains while main is still their base, rerun the first red
/// once, then bisect by halves with up to two sub-trains in flight. Every
/// member ends landed or ejected with a reason.
// trace:STORY-1690 | ai:claude
pub(crate) fn drive(rails: &mut dyn Rails, members: Vec<Member>) -> Result<Outcome> {
    let mut out = Outcome::default();
    // A forge or git error mid-run must not abort with train PRs left open
    // and ejected members never told: release what is open, eject what was
    // in flight, and report the error in the outcome.
    if let Err(e) = drive_into(rails, &members, &mut out) {
        let error = format!("{e:#}");
        rails.abort(&error);
        for m in &members {
            let done = out.landed.iter().any(|(pr, _)| *pr == m.pr)
                || out.ejected.iter().any(|x| x.pr == m.pr);
            if !done {
                out.ejected.push(Ejected {
                    pr: m.pr,
                    reason: format!("the train stopped before landing it ({error}); re-train"),
                });
            }
        }
        out.error = Some(error);
    }
    for e in &out.ejected {
        rails.ejected(e);
    }
    Ok(out)
}

fn drive_into(rails: &mut dyn Rails, members: &[Member], out: &mut Outcome) -> Result<()> {
    if members.is_empty() {
        return Ok(());
    }
    // (group, may this group's red be rerun once?)
    let mut pending: VecDeque<(Vec<Member>, bool)> = VecDeque::from([(members.to_vec(), true)]);
    // A main that keeps moving under the train must not spin it forever.
    let mut waves_left = 4 * members.len() + 8;
    while !pending.is_empty() {
        if waves_left == 0 {
            for (group, _) in pending.drain(..) {
                for m in group {
                    out.ejected.push(Ejected {
                        pr: m.pr,
                        reason: "the train gave up: main kept moving under it; re-train".into(),
                    });
                }
            }
            break;
        }
        waves_left -= 1;
        let take = pending.len().min(PARALLEL_SUB_TRAINS);
        let wave: Vec<(Vec<Member>, bool)> = pending.drain(..take).collect();
        let base = rails.main_tip()?;
        let mut built = Vec::new();
        for (group, rerun) in wave {
            let b = rails.build(&base, &group)?;
            out.ejected.extend(b.dropped.iter().cloned());
            if !b.cars.is_empty() {
                built.push((b, rerun));
            }
        }
        if built.is_empty() {
            continue;
        }
        let trains: Vec<Built> = built.iter().map(|(b, _)| b.clone()).collect();
        let verdicts = rails.test(&trains)?;
        anyhow::ensure!(
            verdicts.len() == trains.len(),
            "train CI returned {} verdicts for {} trains",
            verdicts.len(),
            trains.len()
        );
        out.ci_runs += trains.len();
        // Requeued groups go back to the FRONT, in their wave order.
        let mut requeue: Vec<(Vec<Member>, bool)> = Vec::new();
        for ((train, rerun), verdict) in built.into_iter().zip(verdicts) {
            let members = train.members();
            match verdict {
                CiVerdict::Green => match rails.land(&train)? {
                    Landing::Landed(_) => out
                        .landed
                        .extend(train.cars.iter().map(|c| (c.member.pr, c.commit.clone()))),
                    Landing::Stale => requeue.push((members, false)),
                    Landing::Refused(refused) => {
                        let rest: Vec<Member> = members
                            .into_iter()
                            .filter(|m| !refused.iter().any(|e| e.pr == m.pr))
                            .collect();
                        out.ejected.extend(refused);
                        if !rest.is_empty() {
                            requeue.push((rest, false));
                        }
                    }
                },
                CiVerdict::Red(detail) => {
                    if rerun {
                        requeue.push((members, false));
                    } else if members.len() == 1 {
                        out.ejected.push(Ejected {
                            pr: members[0].pr,
                            reason: format!(
                                "CI is red with this PR alone on top of main ({detail})"
                            ),
                        });
                    } else {
                        let mid = members.len() / 2;
                        requeue.push((members[..mid].to_vec(), false));
                        requeue.push((members[mid..].to_vec(), false));
                    }
                }
            }
        }
        for g in requeue.into_iter().rev() {
            pending.push_front(g);
        }
    }
    Ok(())
}

/// TASK-1378: the forge-metadata half of the member gate. A draft never runs
/// Build, so the train refuses it with a reason instead of waiting.
// trace:STORY-1690 trace:TASK-1378 | ai:claude
pub(crate) fn member_metadata_refusal(
    meta: &crate::forge::ChangeMetadata,
    base: &str,
) -> Option<String> {
    use crate::forge::ChangeState;
    match meta.state {
        ChangeState::Open => {}
        ChangeState::Merged => return Some("it is already merged".to_string()),
        ChangeState::Closed => return Some("it is closed".to_string()),
    }
    if meta.is_draft {
        return Some(
            "it is a draft: CI does not run Build on drafts, so the train will not wait on it; \
             mark it ready (`gh pr ready <N>`) and re-train"
                .to_string(),
        );
    }
    if meta.is_cross_repository {
        return Some("it comes from a fork; the train lands same-repo PRs only".to_string());
    }
    if meta.base_ref.trim() != base {
        return Some(format!(
            "it targets `{}`, not the default branch `{base}`",
            meta.base_ref.trim()
        ));
    }
    if meta.head_ref.starts_with(BRANCH_PREFIX) {
        return Some("it is itself a train PR".to_string());
    }
    None
}

/// The forge-label half of the member gate, the same three rules
/// merge-hold-gate applies. BUG-2214: a hold label is deferred to the mandate
/// cover when a typed marker exists under an active mandate
/// (`marker_under_mandate`); a label-only hold (no marker) stays refused —
/// label-only holds are human-only.
// trace:BUG-2214 | ai:claude
// trace:STORY-1690 | ai:claude
pub(crate) fn member_label_refusal(
    c: &crate::merge_hold::PinnedChange,
    marker_under_mandate: bool,
) -> Option<String> {
    if c.hold_label {
        if !marker_under_mandate {
            return Some(
                "it carries an active aida:merge-hold label; the train never releases a hold"
                    .to_string(),
            );
        }
        // A live deferred hold: `recorded` without `cleared` is its normal
        // state; a clearance label beside a live hold is inconsistent.
        if c.cleared_label {
            return Some(
                "it carries aida:merge-hold and aida:merge-hold-cleared together; the label \
                 state is inconsistent"
                    .to_string(),
            );
        }
        return None;
    }
    if c.recorded_label && !c.cleared_label {
        return Some("it was held before and has no recorded clearance".to_string());
    }
    if c.cleared_label && !c.recorded_label {
        return Some("its clearance label has no matching recorded hold".to_string());
    }
    None
}

/// BUG-2214 (advisor's wait-ci condition): may the train treat this red CI as
/// green-pending-release? Only when every non-green check is
/// `merge-hold-gate`, at least one member is admitted for a mandate release,
/// and every member's live forge labels agree with the member gate's rules:
/// the BUG-1693 pair rule holds for all, and a hold label appears only on a
/// member the mandate covers. Unreadable rows or labels fail closed.
// trace:BUG-2214 | ai:claude
pub(crate) fn hold_gate_tolerance(
    rows: &[crate::ci_gate::CheckRow],
    cars: &[Car],
    mut labels: impl FnMut(u64) -> std::result::Result<Option<crate::merge_hold::PinnedChange>, String>,
) -> bool {
    let hold_gate = |name: &str| name.eq_ignore_ascii_case(crate::ci_gate::HOLD_GATE_CHECK);
    if !rows
        .iter()
        .any(|r| hold_gate(&r.name) && r.bucket != "pass")
    {
        return false;
    }
    if rows
        .iter()
        .any(|r| !hold_gate(&r.name) && !matches!(r.bucket.as_str(), "pass" | "skipping"))
    {
        return false;
    }
    if !cars.iter().any(|c| c.member.release.is_some()) {
        return false;
    }
    cars.iter().all(|car| match labels(car.member.pr) {
        Ok(Some(c)) => member_label_refusal(&c, car.member.release.is_some()).is_none(),
        _ => false,
    })
}

/// Required presence is checked before any informational or hold exception.
// trace:BUG-2401 | ai:codex
#[derive(Debug, PartialEq, Eq)]
enum TrainCiReadiness {
    Missing(Vec<String>),
    Pending(Vec<String>),
    Red(String),
    Green,
}

// trace:BUG-2401 | ai:codex
fn train_ci_readiness(
    rows: &[crate::ci_gate::CheckRow],
    required: &[String],
    cfg: &crate::ci_gate::CiGateConfig,
    tolerate_hold: bool,
) -> TrainCiReadiness {
    let refined = crate::ci_gate::classify_red(rows, required, cfg, tolerate_hold, false);
    // A genuine red remains a red train (including rerun/bisection), even
    // while an aggregate has not registered. Missing evidence never promotes it.
    if refined.is_real() {
        return TrainCiReadiness::Red(format!(
            "failing checks: {}",
            refined
                .real
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let missing: Vec<_> = required
        .iter()
        .filter(|name| !rows.iter().any(|r| r.name == **name))
        .cloned()
        .collect();
    if !missing.is_empty() {
        return TrainCiReadiness::Missing(missing);
    }
    if rows.is_empty() {
        return TrainCiReadiness::Missing(vec!["CI checks".into()]);
    }
    // Required contexts must pass; skipped/unknown required rows are not green.
    let pending: Vec<_> = rows
        .iter()
        .filter(|row| {
            required.contains(&row.name)
                && !matches!(row.bucket.as_str(), "pass" | "fail" | "cancel")
                && !(tolerate_hold && row.name == crate::ci_gate::HOLD_GATE_CHECK)
        })
        .map(|r| r.name.clone())
        .collect();
    if !pending.is_empty() {
        return TrainCiReadiness::Pending(pending);
    }
    if refined.has_pending() {
        return TrainCiReadiness::Pending(refined.pending);
    }
    if rows.iter().any(|r| {
        !matches!(
            r.bucket.as_str(),
            "pass" | "fail" | "cancel" | "skipping" | "pending"
        )
    }) {
        return TrainCiReadiness::Pending(vec!["unclassified CI state".into()]);
    }
    TrainCiReadiness::Green
}

pub(crate) fn short(sha: &str) -> &str {
    &sha[..sha.len().min(10)]
}

fn git_raw(repo: &Path, args: &[&str]) -> Result<std::process::Output> {
    std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| format!("could not run `git {}`", args.first().unwrap_or(&"")))
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let out = git_raw(repo, args)?;
    anyhow::ensure!(
        out.status.success(),
        "`git {}` failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn rev_parse(repo: &Path, rev: &str) -> Result<String> {
    git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ],
    )
}

fn commit_tree(repo: &Path, tree: &str, parent: &str, message: &str) -> Result<String> {
    use std::io::Write;
    let mut child = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["commit-tree", tree, "-p", parent, "-F", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("could not run `git commit-tree`")?;
    child
        .stdin
        .take()
        .context("`git commit-tree` has no stdin")?
        .write_all(message.as_bytes())?;
    let out = child.wait_with_output()?;
    anyhow::ensure!(
        out.status.success(),
        "`git commit-tree` failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Dry-run rails: build in `repo`, test with a local check over a detached
/// checkout of each tip, and "land" by moving a simulated main. Nothing is
/// pushed, opened or merged. `history` records every main the simulation
/// produced, so a caller can show main was never red.
// trace:STORY-1690 | ai:claude
pub(crate) struct LocalRails<'a> {
    pub repo: PathBuf,
    pub main: String,
    pub check: Box<dyn FnMut(&Path) -> std::result::Result<(), String> + 'a>,
    pub history: Vec<String>,
}

impl LocalRails<'_> {
    /// Run the check on a detached checkout of `rev`.
    pub(crate) fn check_rev(&mut self, rev: &str) -> Result<CiVerdict> {
        let dir = tempfile::Builder::new()
            .prefix("aida-train-ci-")
            .tempdir()
            .context("could not create a checkout directory")?;
        let path = dir.path().join("tip");
        let path_s = path.to_string_lossy().to_string();
        git(
            &self.repo,
            &["worktree", "add", "--quiet", "--detach", &path_s, rev],
        )?;
        let result = (self.check)(&path);
        // Retire the checkout's mail first; a refusal keeps it. trace:STORY-1843 | ai:claude
        if let Err(e) =
            crate::mail_retention::remove_worktree(&self.repo, &path, true, "train check")
        {
            eprintln!("warning: kept the train check checkout {path_s}: {e:#}");
        }
        Ok(match result {
            Ok(()) => CiVerdict::Green,
            Err(why) => CiVerdict::Red(format!("local check at {}: {why}", short(rev))),
        })
    }
}

impl Rails for LocalRails<'_> {
    fn main_tip(&mut self) -> Result<String> {
        Ok(self.main.clone())
    }
    fn build(&mut self, base: &str, members: &[Member]) -> Result<Built> {
        build(&self.repo, base, members)
    }
    fn test(&mut self, trains: &[Built]) -> Result<Vec<CiVerdict>> {
        trains.iter().map(|t| self.check_rev(&t.tip)).collect()
    }
    fn land(&mut self, train: &Built) -> Result<Landing> {
        if train.base != self.main {
            return Ok(Landing::Stale);
        }
        self.main = train.tip.clone();
        self.history.push(self.main.clone());
        Ok(Landing::Landed(self.main.clone()))
    }
}

// ─────────────────────────── the live train ────────────────────────────

/// One train network step (origin fetch/push, a `gh` call): run with the
/// shared transient-retry policy — bounded backoff from `RetryConfig::load`
/// (about 3 attempts by default), retry notices on stderr, a non-transient
/// failure (auth, 4xx, conflict) returned at once, and the exhausted or
/// final error naming the step through the caller's `ensure!`.
// trace:BUG-2219 | ai:claude
fn net_output(
    repo: &Path,
    label: &str,
    build: impl FnMut() -> std::process::Command,
) -> Result<std::process::Output> {
    let config = crate::network_retry::RetryConfig::load(repo);
    crate::network_retry::run_with_retry(
        label,
        &config,
        &mut crate::network_retry::StderrSink,
        build,
    )
    .with_context(|| format!("could not run `{label}`"))
}

/// `gh` in the project root, returning its stdout.
// trace:BUG-2214 | ai:claude
fn gh_stdout(repo: &Path, args: &[&str]) -> Result<String> {
    // trace:BUG-2219 | ai:claude
    let out = net_output(repo, &format!("gh {}", args.join(" ")), || {
        let mut cmd = std::process::Command::new("gh");
        cmd.current_dir(repo)
            .args(args)
            .stdin(std::process::Stdio::null());
        cmd
    })?;
    anyhow::ensure!(
        out.status.success(),
        "`gh {}` failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// `git` in the main clone with origin's stored credentials kept out of argv
/// (BUG-2181), for the calls that contact origin.
fn origin_git(repo: &Path, args: &[&str]) -> Result<String> {
    // trace:BUG-2219 | ai:claude
    let out = net_output(repo, &format!("git {}", args.join(" ")), || {
        let mut cmd = std::process::Command::new("git");
        cmd.arg("-C").arg(repo).args(args);
        aida_core::git_ops::configure_remote_credentials(&mut cmd, repo, "origin");
        cmd.stdin(std::process::Stdio::null());
        cmd
    })?;
    anyhow::ensure!(
        out.status.success(),
        "`git {}` failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Fetch `base` from origin and return `origin/<base>`'s commit.
fn fetch_base(repo: &Path, base: &str) -> Result<String> {
    origin_git(
        repo,
        &[
            "fetch",
            "--quiet",
            "--no-tags",
            "origin",
            &format!("+refs/heads/{base}:refs/remotes/origin/{base}"),
        ],
    )?;
    rev_parse(repo, &format!("origin/{base}"))
}

/// The per-member probes the member gate needs, so its order and its
/// fail-closed paths are testable without a forge.
// trace:STORY-1690 | ai:claude
pub(crate) trait MemberChecks {
    fn metadata(&mut self, pr: u64) -> std::result::Result<crate::forge::ChangeMetadata, String>;
    /// BUG-HOME-048: `ids` without those resolving to a spec PR `pr` on
    /// `head_ref` does not cover — the shared gate id filter, so the train
    /// never looks a verdict or spec policy up under a foreign bare id.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
    fn covered_ids(&mut self, pr: u64, head_ref: &str, ids: Vec<String>) -> Vec<String>;
    /// The local merge-hold marker's reason, if any.
    fn hold_marker(&mut self, pr: u64) -> Option<String>;
    fn labels(
        &mut self,
        pr: u64,
    ) -> std::result::Result<Option<crate::merge_hold::PinnedChange>, String>;
    /// Fetch the head branch, so the approval and the subject read it.
    fn fetch_head(&mut self, head_ref: &str) -> std::result::Result<(), String>;
    /// The head the PR's attested approval is pinned to; `None` if none.
    fn approval_pin(
        &mut self,
        pr: u64,
        head_ref: &str,
        head: &str,
    ) -> std::result::Result<Option<String>, String>;
    /// The root whose STORY-1405 review-in-progress markers ship honours.
    fn marker_root(&self) -> &Path;
    fn corpus(&mut self, pr: u64, hints: &[String], head: &str) -> std::result::Result<(), String>;
    /// The (subject, body) `aida pr ship` would give the squash merge.
    fn message(
        &mut self,
        pr: u64,
        meta: &crate::forge::ChangeMetadata,
        head: &str,
    ) -> std::result::Result<(String, String), String>;
    /// TASK-1602: why a live drive owns this PR (or the caller is a drive
    /// seat); `None` when ship's in-drive ownership gate passes.
    fn drive_block(&mut self, pr: u64) -> Option<String>;
    /// The specs the member ships, as ship's STORY-529/BUG-727 gates read them.
    fn spec_records(&mut self, ids: &[String]) -> Vec<crate::pr_cmd::ShipGateSpecRecord>;
    /// BUG-727: a human at an interactive terminal.
    fn interactive_tty(&self) -> bool;
    /// STORY-1698: whether an active mandate covers merging this member;
    /// `Err(None)` when there is no mandate, `Err(Some(why))` when one
    /// refuses. BUG-2214: `Ok` carries the cover so the land step can
    /// release a hold under it.
    // trace:BUG-2214 | ai:claude
    fn mandate_cover(
        &mut self,
        pr: u64,
        ids: &[String],
        head_ref: &str,
        head: &str,
    ) -> std::result::Result<crate::gate::mandate_ship::MandateCover, Option<String>>;
    /// BUG-2214: is a fire-fighting mandate active at this instant? Gates
    /// only whether a held member's judgment defers to the cover; the cover
    /// itself re-reads the mandate.
    // trace:BUG-2214 | ai:claude
    fn mandate_active(&self) -> bool;
}

/// The spec IDs a member ships, read the way ship reads them: its branch and
/// title, the trailing IDs of its squash subject and of each commit subject
/// in its body.
// trace:STORY-1690 | ai:claude
pub(crate) fn member_spec_ids(
    head_ref: &str,
    title: &str,
    subject: &str,
    body: &str,
) -> Vec<String> {
    let mut ids = crate::pr_ship::extract_spec_ids_from_text(&format!("{head_ref} {title}"));
    for line in std::iter::once(subject).chain(body.lines()) {
        ids.extend(crate::pr_ship::extract_trailing_spec_ids_from_subject(line));
    }
    ids.sort();
    ids.dedup();
    ids
}

/// Ship's spec-policy gates for one member, through ship's own filters:
/// STORY-529 `review:draft-only`, then the BUG-727 supervised-merge gate,
/// which only a human at a terminal or (STORY-1698) an active mandate's
/// cover passes. `Some` is the refusal.
// trace:STORY-1690 trace:STORY-529 trace:BUG-727 trace:STORY-1698 | ai:claude
pub(crate) fn member_spec_policy_refusal(
    records: Vec<crate::pr_cmd::ShipGateSpecRecord>,
    interactive_tty: bool,
    mandate_cover: impl FnOnce() -> std::result::Result<(), Option<String>>,
) -> Option<String> {
    let draft_only = crate::pr_cmd::draft_only_specs_in(&records);
    if !draft_only.is_empty() {
        return Some(format!(
            "{} tagged `{}`: a human reviews and merges it; the train does not",
            draft_only.join(", "),
            crate::gate::ship::DRAFT_ONLY_TAG
        ));
    }
    // A human at a terminal needs no mandate; read one only when it matters.
    // A covering mandate is ship's `interactive_tty || mandate_cover.is_ok()`,
    // which passes the supervised gate outright.
    let holds = crate::pr_cmd::supervised_specs_in(records, interactive_tty);
    if holds.is_empty() {
        return None;
    }
    let cover = mandate_cover();
    if cover.is_ok() {
        return None;
    }
    let marked: Vec<String> = holds
        .iter()
        .map(|(id, label)| format!("{id} is marked {label}"))
        .collect();
    let mandate = match cover {
        Err(Some(why)) => format!(" The mandate does not cover it: {why}."),
        _ => String::new(),
    };
    Some(format!(
        "{}: its merge needs a human at a terminal or an active mandate; the train never \
         auto-merges a supervised spec.{mandate}",
        marked.join(", ")
    ))
}

/// The member gate: everything `aida pr ship` checks before it merges a PR,
/// minus the CI wait (the train's own CI replaces it), and with no way to
/// release a hold. `Ok` carries the commit message ship would write.
// trace:STORY-1690 | ai:claude
pub(crate) fn gate_member_with(
    checks: &mut dyn MemberChecks,
    base: &str,
    pr: u64,
) -> std::result::Result<Member, String> {
    let meta = checks.metadata(pr)?;
    if let Some(why) = member_metadata_refusal(&meta, base) {
        return Err(why);
    }
    let head = meta.head_sha.trim().to_string();
    let head_ref = meta.head_ref.trim().to_string();
    if head.is_empty() || head_ref.is_empty() {
        return Err("its head could not be read from the forge; failing closed".to_string());
    }
    // TASK-1602: ship's in-drive ownership gate. trace:STORY-1690 | ai:claude
    if let Some(detail) = checks.drive_block(pr) {
        return Err(format!(
            "a live drive owns it ({detail}); its independent reviewer gates the merge"
        ));
    }
    // BUG-2214 (Q2 ruling): with no active mandate a held member refuses
    // byte-identically to before; under one, the judgment defers to the
    // mandate cover below, where the ids and the head exist.
    // trace:BUG-2214 | ai:claude
    let held = checks.hold_marker(pr);
    if let Some(reason) = &held {
        if !checks.mandate_active() {
            return Err(format!(
                "it is under a merge hold ({}); the train never releases a hold",
                reason.trim()
            ));
        }
    }
    match checks.labels(pr) {
        Ok(Some(c)) => {
            if let Some(why) = member_label_refusal(&c, held.is_some()) {
                return Err(why);
            }
        }
        Ok(None) => {}
        Err(e) => {
            return Err(format!(
                "its labels could not be read ({e}); failing closed"
            ))
        }
    }
    // STORY-1405: a reviewer's claim on this head blocks the train exactly
    // as it blocks ship; the head is known, so the marker cannot be skipped.
    if let crate::gate::review_marker::MergeGate::UnderReview(m) =
        crate::gate::review_marker::merge_gate(checks.marker_root(), pr, || Some(head.clone()))
    {
        return Err(crate::gate::review_marker::refusal_message(&m));
    }
    checks
        .fetch_head(&head_ref)
        .map_err(|e| format!("its head could not be fetched ({e})"))?;
    let pin = match checks.approval_pin(pr, &head_ref, &head) {
        Ok(Some(pin)) => pin,
        Ok(None) => return Err("no approval is recorded for it".to_string()),
        Err(why) => return Err(why),
    };
    if !crate::forge::head_matches_pin(&pin, &head) {
        return Err(format!(
            "its approval is pinned to {}, not its head {}",
            short(&pin),
            short(&head)
        ));
    }
    // BUG-HOME-048: both id lists pass the PR's trailer coverage.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
    let hints = checks.covered_ids(
        pr,
        &head_ref,
        crate::pr_ship::extract_spec_ids_from_text(&format!("{head_ref} {}", meta.title)),
    );
    checks.corpus(pr, &hints, &head)?;
    let (subject, body) = checks.message(pr, &meta, &head)?;
    // STORY-529, BUG-727, STORY-1698: the spec-policy gates ship applies
    // before it merges. trace:STORY-1690 | ai:claude
    let ids = checks.covered_ids(
        pr,
        &head_ref,
        member_spec_ids(&head_ref, &meta.title, &subject, &body),
    );
    let records = checks.spec_records(&ids);
    let tty = checks.interactive_tty();
    // BUG-2214: the cover is read at most once per gate run — the supervised
    // gate and the hold release share it. trace:BUG-2214 | ai:claude
    let mut cover_slot: Option<
        std::result::Result<crate::gate::mandate_ship::MandateCover, Option<String>>,
    > = None;
    if let Some(why) = member_spec_policy_refusal(records, tty, || {
        let cover = checks.mandate_cover(pr, &ids, &head_ref, &head);
        let outcome = cover.as_ref().map(|_| ()).map_err(Clone::clone);
        cover_slot = Some(cover);
        outcome
    }) {
        return Err(why);
    }
    // BUG-2214: a held member is admitted exactly when ship's mandate cover
    // would release its hold; the release itself happens at land time, under
    // the merge lease, through ship's own release sequence.
    // trace:BUG-2214 | ai:claude
    let release = match &held {
        None => None,
        Some(reason) => {
            let cover = match cover_slot {
                Some(cover) => cover,
                None => checks.mandate_cover(pr, &ids, &head_ref, &head),
            };
            match cover {
                Ok(cover) => Some(cover),
                Err(why) => {
                    return Err(format!(
                        "it is under a merge hold ({}); the mandate does not release it{}",
                        reason.trim(),
                        why.map(|w| format!(" — {w}")).unwrap_or_default(),
                    ))
                }
            }
        }
    };
    Ok(Member {
        pr,
        head,
        subject,
        body,
        release,
    })
}

/// [`MemberChecks`] against the project's forge, store and main clone.
// trace:STORY-1690 | ai:claude
pub(crate) struct LiveChecks<'a> {
    pub project_root: &'a Path,
    pub main_worktree: &'a Path,
    pub forge: &'a dyn crate::forge::Forge,
    pub base: &'a str,
    /// The PR was named on the command line, not swept in by `--all-ready`.
    pub named: bool,
}

impl MemberChecks for LiveChecks<'_> {
    fn metadata(&mut self, pr: u64) -> std::result::Result<crate::forge::ChangeMetadata, String> {
        self.forge
            .change_metadata(pr, &mut crate::network_retry::NoopSink)
            .map_err(|e| format!("its metadata could not be read ({e:#})"))
    }
    fn covered_ids(&mut self, pr: u64, head_ref: &str, ids: Vec<String>) -> Vec<String> {
        crate::gate::trailer_coverage::covered_gate_ids(self.project_root, head_ref, Some(pr), ids)
    }
    fn hold_marker(&mut self, pr: u64) -> Option<String> {
        crate::gate::merge_hold_core::read_hold(self.main_worktree, pr)
    }
    fn labels(
        &mut self,
        pr: u64,
    ) -> std::result::Result<Option<crate::merge_hold::PinnedChange>, String> {
        crate::merge_hold::fetch_pinned_change(self.project_root, self.forge.kind(), pr)
            .map_err(|e| e.to_string())
    }
    fn fetch_head(&mut self, head_ref: &str) -> std::result::Result<(), String> {
        origin_git(
            self.main_worktree,
            &[
                "fetch",
                "--quiet",
                "--no-tags",
                "origin",
                &format!("+refs/heads/{head_ref}:refs/remotes/origin/{head_ref}"),
            ],
        )
        .map(|_| ())
        .map_err(|e| format!("{e:#}"))
    }
    fn approval_pin(
        &mut self,
        pr: u64,
        head_ref: &str,
        head: &str,
    ) -> std::result::Result<Option<String>, String> {
        crate::gate::approval::merge_path_approval_gate(
            self.project_root,
            pr,
            &[],
            Some(head_ref),
            Some(head),
        )
    }
    fn marker_root(&self) -> &Path {
        self.main_worktree
    }
    fn corpus(&mut self, pr: u64, hints: &[String], head: &str) -> std::result::Result<(), String> {
        crate::gate::merge_hold_core::enforce_corpus_gate_before_merge(
            self.project_root,
            pr,
            hints,
            Some(self.base),
            || Some(head.to_string()),
        )
        .map_err(|e| format!("{e:#}"))
    }
    fn message(
        &mut self,
        pr: u64,
        meta: &crate::forge::ChangeMetadata,
        head: &str,
    ) -> std::result::Result<(String, String), String> {
        let ship_meta = if self.forge.kind() == crate::forge::ForgeKind::GitHub {
            crate::pr_cmd::fetch_pr_ship_metadata_via_gh(self.project_root, pr)
                .map_err(|e| format!("its title and body could not be read ({e:#})"))?
        } else {
            crate::pr_cmd::PrShipMetadata {
                title: meta.title.clone(),
                body: String::new(),
                number: Some(pr),
            }
        };
        let subject = member_subject_at_head(
            self.project_root,
            self.main_worktree,
            &ship_meta,
            meta.head_ref.trim(),
            head,
        )?;
        let body = member_body(self.project_root, self.base, head, &ship_meta.body)?;
        Ok((subject, body))
    }
    fn drive_block(&mut self, pr: u64) -> Option<String> {
        crate::pr_cmd::probe_ship_drive_block(self.main_worktree, pr).map(|b| b.detail)
    }
    fn spec_records(&mut self, ids: &[String]) -> Vec<crate::pr_cmd::ShipGateSpecRecord> {
        crate::pr_cmd::ship_gate_spec_records_for_ids(self.project_root, ids.to_vec())
    }
    fn interactive_tty(&self) -> bool {
        tty_consents(crate::pr_cmd::ship_interactive_tty(), self.named)
    }
    fn mandate_cover(
        &mut self,
        pr: u64,
        ids: &[String],
        head_ref: &str,
        head: &str,
    ) -> std::result::Result<crate::gate::mandate_ship::MandateCover, Option<String>> {
        let hold_root = crate::pr_cmd::pr_ship_hold_root(self.project_root);
        crate::gate::mandate_ship::mandate_ship_cover_for_ship(
            self.project_root,
            self.main_worktree,
            &hold_root,
            pr,
            ids,
            Some(head_ref),
            || Some(head.to_string()),
            false,
        )
        .map_err(|r| match r {
            crate::gate::mandate::MandateRefusal::NoMandate => None,
            r => {
                // BUG-2214: parity with ship's mandate_refusal_note — a
                // refusing cover is recorded, whichever surface asked.
                // trace:BUG-2214 | ai:claude
                let message = r.message(pr);
                crate::events::record_gate_held(
                    &hold_root,
                    crate::events::GATE_MANDATE_SHIP,
                    None,
                    Some(pr),
                    &message,
                );
                crate::mandate_drive::ship_refused(self.main_worktree, pr, &message);
                Some(message)
            }
        })
    }
    fn mandate_active(&self) -> bool {
        matches!(
            crate::gate::mandate::current_state(),
            crate::gate::mandate::MandateState::Active(_)
        )
    }
}

/// The car subject for a member, read at its gated, approved head sha (the
/// gate fetched it into `repo`). A same-named local branch in the merge
/// seat's checkout may be stale, so the branch is never consulted for the
/// commit message.
// trace:TASK-2272 | ai:claude
pub(crate) fn member_subject_at_head(
    project_root: &Path,
    repo: &Path,
    ship_meta: &crate::pr_cmd::PrShipMetadata,
    head_ref: &str,
    head: &str,
) -> std::result::Result<String, String> {
    let head_msg = git(repo, &["log", "-1", "--format=%B", head, "--"]).map_err(|e| {
        format!(
            "its approved head {} could not be read ({e:#})",
            short(head)
        )
    })?;
    let (subject, _trailer) = crate::pr_cmd::derive_pr_ship_squash_subject_from_message(
        project_root,
        ship_meta,
        head_ref,
        &head_msg,
        false,
    )
    .map_err(|e| format!("its squash subject could not be derived ({e:#})"))?;
    Ok(member_squash_subject(subject, &head_msg, &ship_meta.title))
}

/// The squash subject a member's car carries. Ship's derivation returns
/// `None` when its subject equals the branch head commit's own derived
/// subject, and ship then lets the forge use that commit subject. The train
/// does the same, so the car keeps the commit's `(SPEC-ID)` and `aida pull`
/// completes the spec. The PR title is the last resort.
// trace:STORY-1690 | ai:claude
pub(crate) fn member_squash_subject(
    derived: Option<String>,
    head_commit_message: &str,
    title: &str,
) -> String {
    if let Some(subject) = derived.filter(|s| !s.trim().is_empty()) {
        return subject;
    }
    let from_commit = crate::pr_ship::derive_pr_title_from_commit(head_commit_message);
    if from_commit.trim().is_empty() {
        title.trim().to_string()
    } else {
        from_commit
    }
}

/// Whether the train's PRs were named on its command line: every one is,
/// unless `--all-ready` swept them in. Feeds [`tty_consents`].
// trace:TASK-2272 | ai:claude
pub(crate) fn prs_named_on_command_line(opts: &TrainOptions) -> bool {
    !opts.all_ready
}

/// BUG-727: in ship, a human at a terminal consents to the merge because
/// they typed that PR's number. The train extends that consent only to the
/// PRs named on its command line; a PR swept in by `--all-ready` needs a
/// mandate's cover for a supervised spec.
// trace:STORY-1690 trace:BUG-727 | ai:claude
pub(crate) fn tty_consents(interactive_tty: bool, named: bool) -> bool {
    interactive_tty && named
}

/// BUG-2220: why `pr` already rides a live train, or `None`. `open_trains`
/// are the open `train/*` PRs as `(number, parsed members line)`;
/// `leased_trains` are the train PRs named by live train merge-leases that
/// are not already in the open list, with their members. A members line that
/// does not parse names nobody: the gate cannot tell who rides that train,
/// and blocking every candidate on one malformed body would stop all trains.
// trace:BUG-2220 | ai:claude
pub(crate) fn train_rider_refusal(
    pr: u64,
    open_trains: &[(u64, Option<Vec<(u64, String)>>)],
    leased_trains: &[(u64, Option<Vec<(u64, String)>>)],
) -> Option<String> {
    let rides = |members: &Option<Vec<(u64, String)>>| {
        members
            .as_ref()
            .is_some_and(|m| m.iter().any(|(n, _)| *n == pr))
    };
    if let Some((train, _)) = open_trains.iter().find(|(_, m)| rides(m)) {
        return Some(format!(
            "it is already riding live train PR #{train}; a PR rides one train at a time"
        ));
    }
    if let Some((train, _)) = leased_trains.iter().find(|(_, m)| rides(m)) {
        return Some(format!(
            "it is already riding train PR #{train}, whose merge lease is still held; \
             a PR rides one train at a time"
        ));
    }
    None
}

/// The live trains a second train must not double-carry: every open
/// `train/*` PR with its parsed members line, and every train PR named by a
/// live train merge-lease and not already open (its PR can close mid-land
/// while the lease is still held, so its members are read by number). A
/// closed train PR with no live lease, or a lease whose holder is dead or
/// TTL-expired, is stale and appears in neither list.
// trace:BUG-2220 | ai:claude
#[allow(clippy::type_complexity)]
fn live_trains(
    main_worktree: &Path,
) -> Result<(
    Vec<(u64, Option<Vec<(u64, String)>>)>,
    Vec<(u64, Option<Vec<(u64, String)>>)>,
)> {
    #[derive(serde::Deserialize)]
    struct Row {
        number: u64,
        #[serde(rename = "headRefName")]
        head_ref: String,
        body: String,
    }
    let raw = gh_stdout(
        main_worktree,
        &[
            "pr",
            "list",
            "--state",
            "open",
            // High enough that a busy repo's open PRs never hide a live
            // train from the rider gate (PR-2900 round 1).
            "--limit",
            "1000",
            "--json",
            "number,headRefName,body",
        ],
    )?;
    let rows: Vec<Row> = serde_json::from_str(&raw).context("could not parse `gh pr list` JSON")?;
    let open: Vec<(u64, Option<Vec<(u64, String)>>)> = rows
        .into_iter()
        .filter(|r| r.head_ref.starts_with(BRANCH_PREFIX))
        .map(|r| (r.number, parse_members_line(&r.body)))
        .collect();
    let mut leased: Vec<(u64, Option<Vec<(u64, String)>>)> = Vec::new();
    for (_, holder) in crate::merge_lock::live_status(main_worktree) {
        let Some(train) = holder.pr else { continue };
        if holder.note != TRAIN_LEASE_NOTE
            || open.iter().any(|(n, _)| *n == train)
            || leased.iter().any(|(n, _)| *n == train)
        {
            continue;
        }
        // An unreadable body names nobody (the rider gate fails open, as for
        // a malformed members line), but say so rather than pass silently.
        // trace:TASK-1865 | ai:claude
        let body = gh_stdout(
            main_worktree,
            &[
                "pr",
                "view",
                &train.to_string(),
                "--json",
                "body",
                "--jq",
                ".body",
            ],
        )
        .unwrap_or_else(|e| {
            eprintln!(
                "  warning: could not read leased train PR #{train}'s members; \
                 not checking whether this PR already rides it: {e:#}"
            );
            String::new()
        });
        leased.push((train, parse_members_line(&body)));
    }
    Ok((open, leased))
}

/// The member gate, live.
// trace:STORY-1690 | ai:claude
pub(crate) fn gate_member(
    project_root: &Path,
    main_worktree: &Path,
    forge: &dyn crate::forge::Forge,
    base: &str,
    pr: u64,
    named: bool,
) -> std::result::Result<Member, String> {
    gate_member_with(
        &mut LiveChecks {
            project_root,
            main_worktree,
            forge,
            base,
            named,
        },
        base,
        pr,
    )
}

/// The squash body ship would write: the commit messages (identity-sanitized
/// when the project allowlists emails), plus the non-completing marker when
/// the PR declares one.
fn member_body(
    project_root: &Path,
    base: &str,
    head: &str,
    pr_body: &str,
) -> std::result::Result<String, String> {
    let allowlist =
        crate::identity_gate::read_allowed_emails(project_root).map_err(|e| format!("{e:#}"))?;
    let body = match crate::identity_gate::squash_coauthor_body_override(
        project_root,
        base,
        head,
        &allowlist,
    )
    .map_err(|e| format!("{e:#}"))?
    {
        Some((body, _)) => body,
        None => crate::identity_gate::squash_commit_messages_body(project_root, base, head)
            .map_err(|e| format!("{e:#}"))?,
    };
    Ok(
        if crate::pr_ship::pr_body_declares_non_completing(pr_body) {
            crate::pr_cmd::non_completing_squash_body(&body)
        } else {
            body
        },
    )
}

/// The train PR's body: what it is, its cars, and the members line the
/// merge-hold gate reads.
// trace:STORY-1690 | ai:claude
pub(crate) fn train_pr_body(train: &Built) -> String {
    let mut s = format!(
        "Merge train built by `aida pr train`: `{}` plus one squash commit per member PR. \
         Each commit's patch-id equals its member's approved diff. Do not merge this PR by \
         hand: `aida pr train` rebase-merges it, pinned to this head, once CI is green and \
         main has not moved.\n\n| PR | approved head | commit | patch-id |\n|---|---|---|---|\n",
        short(&train.base)
    );
    for c in &train.cars {
        s.push_str(&format!(
            "| #{} | {} | {} | {} |\n",
            c.member.pr,
            short(&c.member.head),
            short(&c.commit),
            short(&c.patch_id)
        ));
    }
    s.push('\n');
    s.push_str(&members_line(&train.cars));
    s.push('\n');
    s
}

/// The side effects the live train needs, so [`ForgeRails`]' test and land
/// logic is testable against a fake forge.
// trace:STORY-1690 | ai:claude
pub(crate) trait TrainForge {
    /// Fetch the base and return its tip.
    fn fetch_main(&mut self) -> Result<String>;
    /// Take the base's merge lease for train PR `train_pr`; it is held until
    /// the guard drops. `Err` says who holds it.
    fn lease(&mut self, train_pr: u64) -> std::result::Result<Box<dyn std::any::Any>, String>;
    /// Build a train on `base` in the main clone's object store.
    fn build(&mut self, base: &str, members: &[Member]) -> Result<Built>;
    /// The member gate for `pr`, live.
    fn gate(&mut self, pr: u64) -> std::result::Result<Member, String>;
    /// Push `train`'s tip to `branch` and open its train PR.
    fn open(&mut self, train: &Built, branch: &str) -> Result<crate::forge::ChangeRef>;
    /// BUG-2214: `train` lets the live wait tolerate a merge-hold-gate red
    /// that is fully explained by admitted held members the mandate covers.
    fn wait_ci(&mut self, change: &crate::forge::ChangeRef, train: &Built) -> Result<CiVerdict>;
    /// BUG-2214: release `member`'s merge-hold under its mandate cover,
    /// through ship's own release sequence, returning the cleared record for
    /// a re-arm. The default fails closed: a runner that cannot release
    /// ejects the member.
    // trace:BUG-2214 | ai:claude
    fn release_hold(
        &mut self,
        member: &Member,
    ) -> std::result::Result<crate::gate::merge_hold_core::MergeHoldRecord, String> {
        Err(format!(
            "PR-{}: this train runner cannot release a merge-hold",
            member.pr
        ))
    }
    /// BUG-2214: put `record`'s hold back after a failure between its release
    /// and the merge; the returned text describes the outcome for the
    /// ejection reason.
    // trace:BUG-2214 | ai:claude
    fn rearm_hold(&mut self, record: &crate::gate::merge_hold_core::MergeHoldRecord) -> String {
        format!(
            " PR-{}'s released hold was NOT re-armed (this runner cannot); a human must re-place it.",
            record.pr
        )
    }
    /// BUG-2214: after member releases, re-run the train PR's merge-hold-gate
    /// (a body edit re-runs it) and wait for green, re-checking that every
    /// mandate in `covers` is still active. The default fails closed.
    // trace:BUG-2214 | ai:claude
    fn hold_gate_green_after_release(
        &mut self,
        _change: &crate::forge::ChangeRef,
        _covers: &[String],
    ) -> Result<bool> {
        Ok(false)
    }
    /// Close a train PR that will not merge, and delete its branch.
    fn retire(&mut self, change: &crate::forge::ChangeRef, why: &str);
    /// Rebase-merge the train PR, pinned to `tip`.
    fn merge(&mut self, change: &crate::forge::ChangeRef, tip: &str) -> Result<()>;
    /// The commits main received for `cars` between `base` and `main`.
    fn landed(&mut self, base: &str, main: &str, cars: &[Car]) -> Vec<Option<String>>;
    /// The first parent of `commit`.
    fn parent(&mut self, commit: &str) -> Option<String>;
    /// Close a landed member with a pointer and record its bookkeeping.
    fn member_landed(
        &mut self,
        change: &crate::forge::ChangeRef,
        train: &Built,
        car: &Car,
        sha: Option<&str>,
    );
    fn comment(&mut self, pr: u64, note: &str);
    /// STORY-1917 Q2: why a dispatch-originated run lost its authority, once
    /// any port's check failed. The String-returning ports cannot carry the
    /// typed error, so the rails ask here instead of parsing reasons.
    // trace:STORY-1917 trace:TASK-2344 | ai:claude
    fn dispatch_abort(&self) -> Option<&str> {
        None
    }
}

/// [`TrainForge`] through the project's forge and main clone.
// trace:STORY-1690 | ai:claude
pub(crate) struct LiveTrainForge<'a> {
    pub project_root: PathBuf,
    pub main_worktree: PathBuf,
    pub base: String,
    pub forge: &'a dyn crate::forge::Forge,
    /// Members were named on the command line (not `--all-ready`).
    pub named: bool,
    pub dispatch_ctx: Option<&'a crate::gate::dispatch_auth::DispatchExecutionContext>,
    /// The first failed dispatch authority check of this run.
    pub dispatch_abort: Option<String>,
}

impl LiveTrainForge<'_> {
    /// Re-check the dispatch request's authority (a no-op for an ordinary
    /// train), remembering the first failure for [`TrainForge::dispatch_abort`].
    // trace:STORY-1917 trace:TASK-2344 | ai:claude
    pub(crate) fn dispatch_check(&mut self) -> Result<()> {
        crate::gate::dispatch_auth::checkpoint(self.dispatch_ctx, "pr train").inspect_err(|e| {
            if self.dispatch_abort.is_none() {
                self.dispatch_abort = Some(format!("{e:#}"));
            }
        })
    }

    // trace:BUG-2401 | ai:codex
    fn train_ci_snapshot(
        &mut self,
        change: &crate::forge::ChangeRef,
        tip: &str,
        cars: &[Car],
    ) -> Result<TrainCiReadiness> {
        self.dispatch_check()?;
        // Bracket the policy/row reads: rows from a different head or base
        // cannot promote the built train, including after hold release.
        let assert_head = || -> Result<()> {
            let body = gh_stdout(
                &self.project_root,
                &[
                    "pr",
                    "view",
                    &change.id.to_string(),
                    "--json",
                    "headRefOid,baseRefName,state",
                ],
            )?;
            let value: serde_json::Value = serde_json::from_str(&body)?;
            anyhow::ensure!(
                value["headRefOid"].as_str() == Some(tip)
                    && value["baseRefName"].as_str() == Some(change.base.as_str())
                    && value["state"] == "OPEN",
                "train PR #{} head/base/state changed during CI readiness",
                change.id
            );
            Ok(())
        };
        assert_head()?;
        let required =
            crate::status_prs::train_required_status_checks(&self.project_root, &change.base)?;
        let rows = self.forge.check_rows(change)?;
        let cfg = crate::ci_gate::read_ci_gate_config(&self.project_root);
        // Only the existing, live member-label proof may tolerate a hold.
        let tolerated = hold_gate_tolerance(&rows, cars, |pr| {
            crate::merge_hold::fetch_pinned_change(&self.project_root, self.forge.kind(), pr)
                .map_err(|e| e.to_string())
        });
        let state = train_ci_readiness(&rows, &required, &cfg, tolerated);
        assert_head()?;
        self.dispatch_check()?;
        Ok(state)
    }

    fn member_ref(&self, pr: u64) -> crate::forge::ChangeRef {
        crate::forge::ChangeRef {
            id: pr,
            url: String::new(),
            branch: String::new(),
            base: self.base.clone(),
            title: None,
        }
    }
}

impl TrainForge for LiveTrainForge<'_> {
    fn fetch_main(&mut self) -> Result<String> {
        self.dispatch_check()?;
        fetch_base(&self.main_worktree, &self.base)
    }

    fn lease(&mut self, train_pr: u64) -> std::result::Result<Box<dyn std::any::Any>, String> {
        crate::merge_lock::acquire(
            &self.main_worktree,
            &self.base,
            Some(train_pr),
            TRAIN_LEASE_NOTE,
            crate::merge_lock::DEFAULT_WAIT,
        )
        .map(|lease| Box::new(lease) as Box<dyn std::any::Any>)
        .map_err(|e| format!("{e:#}"))
    }

    fn build(&mut self, base: &str, members: &[Member]) -> Result<Built> {
        self.dispatch_check()?;
        build(&self.main_worktree, base, members)
    }

    fn gate(&mut self, pr: u64) -> std::result::Result<Member, String> {
        self.dispatch_check().map_err(|e| format!("{e:#}"))?;
        gate_member(
            &self.project_root,
            &self.main_worktree,
            self.forge,
            &self.base,
            pr,
            self.named,
        )
    }

    fn open(&mut self, train: &Built, branch: &str) -> Result<crate::forge::ChangeRef> {
        self.dispatch_check()?;
        origin_git(
            &self.main_worktree,
            &[
                "push",
                "--quiet",
                "origin",
                &format!("{}:refs/heads/{branch}", train.tip),
            ],
        )?;
        let prs: Vec<String> = train
            .cars
            .iter()
            .map(|c| format!("#{}", c.member.pr))
            .collect();
        // STORY-080 AC2: the cars' specs bind their pre-PR verdicts to the
        // train change. trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
        let specs: Vec<String> = train
            .cars
            .iter()
            .flat_map(|c| crate::pr_ship::extract_trailing_spec_ids_from_subject(&c.member.subject))
            .collect();
        crate::gate::ledger::open_change_bound(
            self.forge,
            &self.main_worktree,
            crate::forge::OpenChange {
                branch: branch.to_string(),
                base: self.base.clone(),
                title: format!("[train] land {}", prs.join(", ")),
                body: train_pr_body(train),
                draft: false,
            },
            &specs,
        )
    }

    fn wait_ci(&mut self, change: &crate::forge::ChangeRef, train: &Built) -> Result<CiVerdict> {
        use crate::forge::CheckRegistration;
        use crate::gate::dispatch_types::CiState;
        self.dispatch_check()?;
        // trace:BUG-2401 | ai:codex
        if self.forge.kind() == crate::forge::ForgeKind::GitHub {
            let started = std::time::Instant::now();
            let settle_limit = crate::ci_idle_timeout::ci_absolute_ceiling_secs(&self.project_root);
            loop {
                let state = self.train_ci_snapshot(change, &train.tip, &train.cars)?;
                match state {
                    TrainCiReadiness::Green => return Ok(CiVerdict::Green),
                    TrainCiReadiness::Red(why) => return Ok(CiVerdict::Red(why)),
                    waiting => {
                        println!("  train PR #{} CI waiting: {waiting:?}", change.id);
                        // Registration keeps its existing 600s bound. An
                        // already running build gets the ordinary CI ceiling,
                        // rather than being cut off by the registration timer.
                        let limit = if matches!(waiting, TrainCiReadiness::Missing(_)) {
                            600
                        } else {
                            settle_limit
                        };
                        anyhow::ensure!(
                            limit == 0 || started.elapsed().as_secs() < limit,
                            "train PR #{} CI not ready within {limit}s: {waiting:?}",
                            change.id
                        );
                        std::thread::sleep(std::time::Duration::from_secs(10));
                    }
                }
            }
        }

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
        loop {
            match self.forge.checks_registered(change)? {
                CheckRegistration::Registered => break,
                CheckRegistration::NoCi => anyhow::bail!(
                    "the forge reports no CI for train PR #{}; a train lands only on a green CI",
                    change.id
                ),
                CheckRegistration::NotYet if std::time::Instant::now() >= deadline => {
                    anyhow::bail!("no CI registered within 600s for train PR #{}", change.id)
                }
                CheckRegistration::NotYet => std::thread::sleep(std::time::Duration::from_secs(10)),
            }
        }
        let ci_state = self.forge.watch_ci(change)?;
        self.dispatch_check()?;
        Ok(match ci_state {
            CiState::Success => CiVerdict::Green,
            other => {
                // BUG-2214: ship tolerates a merge-hold-gate red while the
                // hold it will release is still armed (BUG-1180); the train
                // does the same for admitted held members, re-verifying every
                // member's live labels. trace:BUG-2214 | ai:claude
                let tolerated = self.forge.check_rows(change).is_ok_and(|rows| {
                    hold_gate_tolerance(&rows, &train.cars, |pr| {
                        crate::merge_hold::fetch_pinned_change(
                            &self.project_root,
                            self.forge.kind(),
                            pr,
                        )
                        .map_err(|e| e.to_string())
                    })
                });
                if tolerated {
                    println!(
                        "  train PR #{} is green except `{}` — explained by mandate-covered \
                         member holds; the land step releases them",
                        change.id,
                        crate::ci_gate::HOLD_GATE_CHECK
                    );
                    CiVerdict::Green
                } else {
                    CiVerdict::Red(format!(
                        "train PR #{} CI {other:?}: {}",
                        change.id, change.url
                    ))
                }
            }
        })
    }

    fn retire(&mut self, change: &crate::forge::ChangeRef, why: &str) {
        if let Err(e) = self.forge.close_change(change, why) {
            eprintln!("  warning: could not close train PR #{}: {e:#}", change.id);
        }
        let _ = origin_git(
            &self.main_worktree,
            &["push", "--quiet", "origin", "--delete", &change.branch],
        );
    }

    fn merge(&mut self, change: &crate::forge::ChangeRef, tip: &str) -> Result<()> {
        self.dispatch_check()?;
        // trace:BUG-2401 | ai:codex — fresh policy, rows and head under the
        // merge lease; no member-hold tolerance survives to the merge call.
        if self.forge.kind() == crate::forge::ForgeKind::GitHub {
            let state = self.train_ci_snapshot(change, tip, &[])?;
            anyhow::ensure!(
                state == TrainCiReadiness::Green,
                "train PR #{} final CI recheck refused: {state:?}",
                change.id
            );
        }
        let opts = crate::forge::MergeOptions {
            method: crate::forge::MergeMethod::Rebase,
            squash_subject: None,
            squash_body: None,
            delete_branch: true,
            match_head: Some(tip.to_string()),
        };
        self.forge
            .merge_change(change, &opts, &mut crate::network_retry::StderrSink)
            .map(|_| ())
    }

    fn landed(&mut self, base: &str, main: &str, cars: &[Car]) -> Vec<Option<String>> {
        landed_commits(&self.main_worktree, base, main, cars)
    }

    fn parent(&mut self, commit: &str) -> Option<String> {
        rev_parse(&self.main_worktree, &format!("{commit}^1")).ok()
    }

    // trace:BUG-2214 | ai:claude
    fn release_hold(
        &mut self,
        member: &Member,
    ) -> std::result::Result<crate::gate::merge_hold_core::MergeHoldRecord, String> {
        self.dispatch_check().map_err(|e| format!("{e:#}"))?;
        let Some(cover) = member.release.as_ref() else {
            return Err("no mandate cover was recorded for its hold".into());
        };
        let hold_root = crate::pr_cmd::pr_ship_hold_root(&self.project_root);
        let record = crate::gate::merge_hold_core::read_hold_record(&hold_root, member.pr)
            .ok_or_else(|| {
                "its merge-hold marker vanished between the gate and the release; failing closed"
                    .to_string()
            })?;
        // STORY-1698 parity: the mandate is re-read from disk immediately
        // before the release that precedes the merge.
        if !crate::gate::mandate_ship::mandate_still_active(&cover.id) {
            return Err(format!("mandate {} is no longer active", cover.id));
        }
        let change = crate::forge::ChangeRef {
            id: member.pr,
            url: String::new(),
            branch: String::new(),
            base: self.base.clone(),
            title: None,
        };
        crate::gate::mandate_ship::release_hold_under_mandate(
            &hold_root,
            &change,
            cover,
            &record,
            None,
            Some(&member.head),
            "released by `aida pr train` under the mandate",
        );
        Ok(record)
    }

    // trace:BUG-2214 | ai:claude
    fn rearm_hold(&mut self, record: &crate::gate::merge_hold_core::MergeHoldRecord) -> String {
        crate::gate::mandate_ship::rearm_after_mandate_failure(
            &crate::pr_cmd::pr_ship_hold_root(&self.project_root),
            Some(record),
            record.pr,
        )
    }

    // trace:BUG-2214 | ai:claude
    fn hold_gate_green_after_release(
        &mut self,
        change: &crate::forge::ChangeRef,
        covers: &[String],
    ) -> Result<bool> {
        // Q3 ruling: a member's label change does not re-run the TRAIN PR's
        // merge-hold-gate; a body edit does. One idempotent marker line,
        // replaced rather than stacked, keeps the members line intact.
        const MARKER: &str = "aida-train-gate-refresh:";
        let body = gh_stdout(
            &self.project_root,
            &[
                "pr",
                "view",
                &change.id.to_string(),
                "--json",
                "body",
                "-q",
                ".body",
            ],
        )?;
        let mut kept: Vec<&str> = body
            .lines()
            .filter(|l| !l.trim_start().starts_with(MARKER))
            .collect();
        let refresh = format!("{MARKER} {}", chrono::Utc::now().to_rfc3339());
        kept.push(&refresh);
        let new_body = kept.join("\n");
        gh_stdout(
            &self.project_root,
            &["pr", "edit", &change.id.to_string(), "--body", &new_body],
        )?;
        // Ship's BUG-1180 bounded wait, applied to the train PR.
        let green = crate::ci_gate::wait_hold_gate_green(
            self.forge,
            change,
            std::time::Duration::from_secs(180),
            std::time::Duration::from_secs(10),
        )?;
        self.dispatch_check()?;
        Ok(green
            && covers
                .iter()
                .all(|id| crate::gate::mandate_ship::mandate_still_active(id)))
    }

    fn member_landed(
        &mut self,
        change: &crate::forge::ChangeRef,
        train: &Built,
        car: &Car,
        sha: Option<&str>,
    ) {
        let pr = car.member.pr;
        let note = format!(
            "Landed on `{}` as {} by merge train #{} (rebase-merged at {}). The approved \
             head {} carried over: its patch-id {} is the landed commit's.",
            self.base,
            sha.unwrap_or("(commit not found by subject)"),
            change.id,
            short(&train.tip),
            short(&car.member.head),
            short(&car.patch_id),
        );
        if let Err(e) = self.forge.close_change(&self.member_ref(pr), &note) {
            eprintln!("  warning: could not close PR #{pr} after landing: {e:#}");
        }
        let specs = crate::pr_ship::extract_trailing_spec_ids_from_subject(&car.member.subject);
        crate::pr_cmd::emit_ship_pr_merged(&self.main_worktree, pr as u32, &specs);
        crate::pr_cmd::append_ship_activity_line(
            &self.main_worktree,
            &serde_json::json!({
                "ts": chrono::Utc::now().to_rfc3339(),
                "command": "aida pr train",
                "event": "train_member_landed",
                "pr": pr,
                "train_pr": change.id,
                "approved_head": car.member.head,
                "patch_id": car.patch_id,
                "landed": sha,
            })
            .to_string(),
        );
    }

    fn comment(&mut self, pr: u64, note: &str) {
        if let Err(err) = self.forge.comment(&self.member_ref(pr), note) {
            eprintln!("  warning: could not comment on PR #{pr}: {err:#}");
        }
    }

    fn dispatch_abort(&self) -> Option<&str> {
        self.dispatch_abort.as_deref()
    }
}

/// The typed error a train returns when its dispatch request lost authority.
// trace:STORY-1917 trace:TASK-2344 | ai:claude
fn dispatch_aborted(why: &str) -> anyhow::Error {
    anyhow::Error::new(crate::gate::dispatch_auth::DispatchAuthorityAbortedError(
        why.to_string(),
    ))
}

/// The rails that push, open, test and merge through a [`TrainForge`].
// trace:STORY-1690 | ai:claude
pub(crate) struct ForgeRails<'a> {
    pub ports: &'a mut dyn TrainForge,
    pub stamp: String,
    pub opened: Vec<(String, crate::forge::ChangeRef)>,
    pub count: usize,
    /// Landed (member PR, commit on main).
    pub landed_on_main: Vec<(u64, String)>,
    /// Problems found after a merge, for the run's summary.
    pub warnings: Vec<String>,
    /// Train PRs opened and neither retired nor merged yet.
    live: Vec<crate::forge::ChangeRef>,
}

impl<'a> ForgeRails<'a> {
    pub(crate) fn new(ports: &'a mut dyn TrainForge, stamp: String) -> Self {
        ForgeRails {
            ports,
            stamp,
            opened: Vec::new(),
            count: 0,
            landed_on_main: Vec::new(),
            warnings: Vec::new(),
            live: Vec::new(),
        }
    }

    fn retire(&mut self, change: &crate::forge::ChangeRef, why: &str) {
        self.live.retain(|c| c.id != change.id);
        self.ports.retire(change, why);
    }

    fn change_for(&self, tip: &str) -> Result<crate::forge::ChangeRef> {
        self.opened
            .iter()
            .rev()
            .find(|(t, _)| t == tip)
            .map(|(_, c)| c.clone())
            .with_context(|| format!("no train PR is open for {}", short(tip)))
    }

    fn warn(&mut self, line: String) {
        eprintln!("  warning: {line}");
        self.warnings.push(line);
    }

    /// BUG-2214: re-arm every hold released during a landing that then
    /// failed; the returned text (empty when nothing was released) appends to
    /// the ejection reason or warning that caused it.
    // trace:BUG-2214 | ai:claude
    fn rearm_released(
        &mut self,
        released: &[crate::gate::merge_hold_core::MergeHoldRecord],
    ) -> String {
        released
            .iter()
            .map(|record| self.ports.rearm_hold(record))
            .collect()
    }
}

impl Rails for ForgeRails<'_> {
    fn main_tip(&mut self) -> Result<String> {
        self.ports.fetch_main()
    }

    fn build(&mut self, base: &str, members: &[Member]) -> Result<Built> {
        self.ports.build(base, members)
    }

    fn test(&mut self, trains: &[Built]) -> Result<Vec<CiVerdict>> {
        let mut changes = Vec::new();
        for t in trains {
            self.count += 1;
            let branch = format!("{BRANCH_PREFIX}{}-{}", self.stamp, self.count);
            let change = self.ports.open(t, &branch)?;
            self.live.push(change.clone());
            let prs: Vec<String> = t.cars.iter().map(|c| format!("#{}", c.member.pr)).collect();
            println!(
                "  train PR #{} opened for {} ({branch} at {})",
                change.id,
                prs.join(", "),
                short(&t.tip)
            );
            self.opened.push((t.tip.clone(), change.clone()));
            changes.push(change);
        }
        // Every train's CI runs at once; wait for each in turn.
        let mut verdicts = Vec::new();
        for (change, train) in changes.iter().zip(trains) {
            // A CI that cannot be read (none registered, an API error) is not
            // a red verdict: bisecting it would spend CI runs and eject every
            // member as "red alone". It stops the run with the real reason.
            let v = self
                .ports
                .wait_ci(change, train)
                .with_context(|| format!("train PR #{}'s CI could not be read", change.id))?;
            if let CiVerdict::Red(why) = &v {
                println!("  train PR #{} is red: {why}", change.id);
                self.retire(
                    change,
                    &format!("Train CI red ({why}); the train reruns or bisects."),
                );
            } else {
                println!("  train PR #{} is green", change.id);
            }
            verdicts.push(v);
        }
        Ok(verdicts)
    }

    fn land(&mut self, train: &Built) -> Result<Landing> {
        let change = self.change_for(&train.tip)?;
        // Another merger holding the lease is about to move main: rebuild on
        // it rather than abort the run with open train PRs and no summary.
        let _lease = match self.ports.lease(change.id) {
            Ok(lease) => lease,
            Err(holder) => {
                println!("  the merge lease is held ({holder}); the train rebuilds");
                self.retire(
                    &change,
                    "Another merger holds the merge lease; the train rebuilds.",
                );
                return Ok(Landing::Stale);
            }
        };
        if self.ports.fetch_main()? != train.base {
            self.retire(
                &change,
                "Main moved after this train was built; the train rebuilds.",
            );
            return Ok(Landing::Stale);
        }
        // Re-gate every member right before the merge: a hold, a new push or
        // a withdrawn approval since the build ejects that member. BUG-2214:
        // the fresh gate re-runs the mandate cover under the lease, so a
        // lapsed mandate ejects a held member here with its hold intact.
        // trace:BUG-2214 | ai:claude
        let mut refused = Vec::new();
        let mut fresh: Vec<Member> = Vec::new();
        for car in &train.cars {
            let pr = car.member.pr;
            match self.ports.gate(pr) {
                Ok(m) if m.head == car.member.head => fresh.push(m),
                Ok(m) => refused.push(Ejected {
                    pr,
                    reason: format!(
                        "its head moved from {} to {} after the train was built; re-train",
                        short(&car.member.head),
                        short(&m.head)
                    ),
                }),
                Err(why) => {
                    if self.ports.dispatch_abort().is_some() {
                        self.retire(
                            &change,
                            "Dispatch authority was revoked before landing; the train aborts.",
                        );
                        return Err(dispatch_aborted(&why));
                    }
                    refused.push(Ejected { pr, reason: why });
                }
            }
        }
        if !refused.is_empty() {
            self.retire(
                &change,
                "A member failed its pre-land recheck; the train rebuilds.",
            );
            return Ok(Landing::Refused(refused));
        }
        // BUG-2214: release each admitted held member exactly as ship would —
        // under the lease, after the fresh cover — then the train PR's
        // merge-hold-gate must re-run green before the merge. Any failure
        // after a release re-arms every released hold. trace:BUG-2214 | ai:claude
        let mut released: Vec<crate::gate::merge_hold_core::MergeHoldRecord> = Vec::new();
        for member in fresh.iter().filter(|m| m.release.is_some()) {
            match self.ports.release_hold(member) {
                Ok(record) => released.push(record),
                Err(why) => {
                    let rearm = self.rearm_released(&released);
                    if self.ports.dispatch_abort().is_some() {
                        self.retire(
                            &change,
                            "Dispatch authority was revoked during hold release; the train aborts.",
                        );
                        if !rearm.is_empty() {
                            self.warn(format!("{why}.{rearm}"));
                        }
                        return Err(dispatch_aborted(&format!("{why}{rearm}")));
                    }
                    self.retire(
                        &change,
                        "A member's merge-hold was not released; the train rebuilds.",
                    );
                    return Ok(Landing::Refused(vec![Ejected {
                        pr: member.pr,
                        reason: format!("its merge-hold was not released ({why}); re-train{rearm}"),
                    }]));
                }
            }
        }
        if !released.is_empty() {
            let covers: Vec<String> = fresh
                .iter()
                .filter_map(|m| m.release.as_ref().map(|c| c.id.clone()))
                .collect();
            match self.ports.hold_gate_green_after_release(&change, &covers) {
                Ok(true) => {}
                Err(e) if crate::gate::dispatch_auth::is_dispatch_authority_aborted(&e) => {
                    let rearm = self.rearm_released(&released);
                    self.retire(
                        &change,
                        "Dispatch authority was revoked during the hold-gate wait; the train aborts.",
                    );
                    if !rearm.is_empty() {
                        self.warn(format!("{e:#}.{rearm}"));
                    }
                    return Err(e);
                }
                outcome => {
                    let why = match outcome {
                        Ok(_) => "the train PR's merge-hold-gate did not re-run green in time, \
                                  or the mandate lapsed"
                            .to_string(),
                        Err(e) => {
                            format!("the train PR's merge-hold-gate could not be read ({e:#})")
                        }
                    };
                    let rearm = self.rearm_released(&released);
                    self.retire(
                        &change,
                        &format!(
                            "{why}; the released member holds are re-armed; the train rebuilds."
                        ),
                    );
                    self.warn(format!("{why}.{rearm}"));
                    return Ok(Landing::Stale);
                }
            }
        }
        // The re-gate took several forge round trips, and a merge that does
        // not take the lease (the web UI, a raw `gh pr merge`) could have
        // moved main meanwhile; strict is off, so the forge would rebase the
        // train onto an untested main. Look again right before merging.
        if self.ports.fetch_main()? != train.base {
            let rearm = self.rearm_released(&released);
            if !rearm.is_empty() {
                self.warn(format!("main moved after the hold release.{rearm}"));
            }
            self.retire(
                &change,
                "Main moved while the train was re-checked; the train rebuilds.",
            );
            return Ok(Landing::Stale);
        }
        if let Err(e) = self
            .ports
            .merge(&change, &train.tip)
            .with_context(|| format!("train PR #{} was not merged", change.id))
        {
            // trace:BUG-2214 | ai:claude
            let rearm = self.rearm_released(&released);
            if !rearm.is_empty() {
                self.warn(format!("the merge failed after the hold release.{rearm}"));
            }
            return Err(e);
        }
        self.live.retain(|c| c.id != change.id);
        let main = match self.ports.fetch_main() {
            Ok(main) => main,
            // The merge landed; only the dispatch re-check before the fetch
            // failed, so say that rather than blame main.
            // trace:STORY-1917 | ai:claude
            Err(e) if crate::gate::dispatch_auth::is_dispatch_authority_aborted(&e) => {
                self.warn(format!(
                    "train PR #{} merged, then its dispatch request lost authority ({e:#}), \
                     so main was not re-read; check its commits on main by hand",
                    change.id
                ));
                train.tip.clone()
            }
            Err(e) => {
                self.warn(format!(
                    "train PR #{} merged, but main could not be read after it ({e:#}); \
                     check its commits on main by hand",
                    change.id
                ));
                train.tip.clone()
            }
        };
        let landed = self.ports.landed(&train.base, &main, &train.cars);
        // The first car must sit directly on the base the train was tested
        // on; anything else means another commit slipped in underneath.
        let first_parent = landed
            .first()
            .and_then(|sha| sha.as_deref())
            .and_then(|sha| self.ports.parent(sha));
        if first_parent.as_deref() != Some(train.base.as_str()) {
            self.warn(format!(
                "train PR #{} landed, but its first commit's parent is {}, not the tested \
                 base {}: main received a combination CI never ran. Check main's CI now.",
                change.id,
                first_parent.as_deref().map(short).unwrap_or("unknown"),
                short(&train.base)
            ));
        }
        for (car, sha) in train.cars.iter().zip(&landed) {
            self.ports
                .member_landed(&change, train, car, sha.as_deref());
            self.landed_on_main
                .push((car.member.pr, sha.clone().unwrap_or_default()));
        }
        Ok(Landing::Landed(main))
    }

    fn ejected(&mut self, e: &Ejected) {
        self.ports.comment(
            e.pr,
            &format!("Not landed by the merge train: {}.", e.reason),
        );
    }

    fn abort(&mut self, error: &str) {
        for change in std::mem::take(&mut self.live) {
            self.ports.retire(
                &change,
                &format!("The train run stopped ({error}); re-train the members."),
            );
        }
    }
}

/// The commits a landed train became on main, matched to its cars by
/// subject (a rebase merge may rewrite them). `None` for a car not found.
fn landed_commits(repo: &Path, base: &str, main: &str, cars: &[Car]) -> Vec<Option<String>> {
    let log = git(
        repo,
        &[
            "log",
            "--reverse",
            "--format=%H%x09%s",
            &format!("{base}..{main}"),
        ],
    )
    .unwrap_or_default();
    let rows: Vec<(&str, &str)> = log.lines().filter_map(|l| l.split_once('\t')).collect();
    cars.iter()
        .map(|c| {
            let subject = car_message(&c.member);
            let subject = subject.lines().next().unwrap_or("");
            rows.iter()
                .find(|(_, s)| *s == subject)
                .map(|(h, _)| h.to_string())
        })
        .collect()
}

/// `aida pr train` options.
// trace:STORY-1690 | ai:claude
pub(crate) struct TrainOptions<'a> {
    pub prs: &'a [u64],
    pub all_ready: bool,
    pub dry_run: bool,
    pub local_ci: Option<&'a str>,
    pub max: usize,
    pub no_pull: bool,
}

/// A local CI command as a [`LocalRails`] check.
fn shell_check(cmd: &str) -> impl FnMut(&Path) -> std::result::Result<(), String> + '_ {
    move |dir: &Path| {
        let (shell, flag) = if cfg!(windows) {
            ("cmd", "/C")
        } else {
            ("sh", "-c")
        };
        let status = std::process::Command::new(shell)
            .args([flag, cmd])
            .current_dir(dir)
            .status()
            .map_err(|e| format!("could not run `{cmd}`: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("`{cmd}` exited {status}"))
        }
    }
}

/// Why `aida pr train` refuses forge `kind`, if it does. GitLab's merge-hold
/// gate template has no train mode, so a train MR's server-side gate would
/// check only its own labels, never its members'.
// trace:STORY-1690 | ai:claude
pub(crate) fn forge_refusal(kind: crate::forge::ForgeKind) -> Option<String> {
    use crate::forge::ForgeKind;
    match kind {
        ForgeKind::GitHub => None,
        ForgeKind::GitLab => Some(
            "`aida pr train` supports GitHub only for now: the GitLab merge-hold gate has no \
             train mode, so a train MR's gate could not check its members' holds. Ship GitLab \
             MRs one at a time with `aida pr ship N`."
                .to_string(),
        ),
        ForgeKind::None => Some(
            "`aida pr train` needs a forge (GitHub): a train is tested by CI on a train PR"
                .to_string(),
        ),
    }
}

/// An explicit `aida pr train N...` naming more PRs than `--max` would leave
/// some never attempted, so it is refused up front. `--all-ready` instead
/// leaves the rest for the next train.
// trace:STORY-1690 | ai:claude
pub(crate) fn explicit_over_max_refusal(
    named: usize,
    max: usize,
    all_ready: bool,
) -> Option<String> {
    let max = max.max(1);
    (!all_ready && named > max).then(|| {
        format!(
            "{named} PRs named but --max is {max}: name at most {max}, or raise --max to {named}"
        )
    })
}

/// `aida pr train`.
// trace:STORY-1690 | ai:claude
pub(crate) fn pr_train_handler(opts: &TrainOptions<'_>) -> Result<()> {
    pr_train_handler_with_dispatch_ctx(opts, None)
}

// trace:STORY-1917 | ai:agy
pub(crate) fn pr_train_handler_with_dispatch_ctx(
    opts: &TrainOptions<'_>,
    dispatch_ctx: Option<&crate::gate::dispatch_auth::DispatchExecutionContext>,
) -> Result<()> {
    crate::gate::dispatch_auth::checkpoint(dispatch_ctx, "pr train")?;
    // Flow tests stand in for the rest of the train here, after the real
    // entry check, so a context dropped on the way in still fails them.
    #[cfg(test)]
    if let Some(ctx) = dispatch_ctx {
        let maybe_res = crate::dispatch::dispatch_flow::TEST_FLOW_TRAIN_HOOK
            .with(|cell| cell.borrow_mut().as_mut().map(|hook| hook(opts.prs, ctx)));
        if let Some(res) = maybe_res {
            return res;
        }
    }
    // STORY-1662: a standalone implementer stops at an open PR; the train is
    // a merge path like any other. trace:STORY-1690 | ai:claude
    crate::standalone_implementer::refuse_under_marker("pr train")?;
    let project_root = crate::find_project_root()?;
    let main_worktree = crate::main_worktree_root_from(&project_root);
    let kind = crate::forge::resolve_forge_kind(&project_root);
    if let Some(why) = forge_refusal(kind) {
        anyhow::bail!(why);
    }
    let forge = crate::forge::forge_for(&project_root);
    let base = crate::gate::review_carryover::protected_base(&main_worktree)
        .map_err(|e| anyhow::anyhow!("could not read origin's default branch: {}", e.reason()))?;
    let mut candidates: Vec<u64> = if opts.all_ready {
        let mut ids: Vec<u64> = forge
            .list_changes(crate::forge::ChangeFilter {
                base: Some(base.clone()),
                open_only: true,
            })?
            .into_iter()
            .filter(|c| !c.branch.starts_with(BRANCH_PREFIX))
            .map(|c| c.id)
            .collect();
        ids.sort_unstable();
        ids
    } else {
        opts.prs.to_vec()
    };
    let mut seen = std::collections::BTreeSet::new();
    candidates.retain(|n| seen.insert(*n));
    anyhow::ensure!(
        !candidates.is_empty(),
        "no PRs to train: name them (`aida pr train 12 13`) or pass --all-ready"
    );
    if let Some(why) = explicit_over_max_refusal(candidates.len(), opts.max, opts.all_ready) {
        anyhow::bail!(why);
    }
    println!(
        "merge train onto `{base}`: gating {} PR(s){}",
        candidates.len(),
        if opts.dry_run { " (dry run)" } else { "" }
    );
    // BUG-2220: the merge lease serializes landings but never stopped a
    // second train from carrying a PR already riding a live one — wasted CI
    // and a confused "who landed it". Read the live trains once and refuse
    // such candidates, naming the other train. trace:BUG-2220 | ai:claude
    let (open_trains, leased_trains) = live_trains(&main_worktree)?;
    let mut members = Vec::new();
    let mut refused = Vec::new();
    let mut waiting = Vec::new();
    for pr in candidates {
        crate::gate::dispatch_auth::checkpoint(dispatch_ctx, "pr train")?;
        if members.len() >= opts.max.max(1) {
            waiting.push(pr);
            continue;
        }
        if let Some(reason) = train_rider_refusal(pr, &open_trains, &leased_trains) {
            println!("  ✗ #{pr} not eligible: {reason}");
            refused.push(Ejected { pr, reason });
            continue;
        }
        match gate_member(
            &project_root,
            &main_worktree,
            forge.as_ref(),
            &base,
            pr,
            prs_named_on_command_line(opts),
        ) {
            Ok(m) => {
                println!("  ✓ #{pr} ready at {}: {}", short(&m.head), m.subject);
                members.push(m);
            }
            Err(reason) => {
                println!("  ✗ #{pr} not eligible: {reason}");
                refused.push(Ejected { pr, reason });
            }
        }
    }
    if !waiting.is_empty() {
        let list: Vec<String> = waiting.iter().map(|p| format!("#{p}")).collect();
        println!(
            "  {} PR(s) wait for the next train (--max {}): {}",
            waiting.len(),
            opts.max,
            list.join(", ")
        );
    }
    let main = fetch_base(&main_worktree, &base)?;
    let mut warnings = Vec::new();
    let mut dispatch_abort: Option<String> = None;
    let outcome = if opts.dry_run {
        let built = build(&main_worktree, &main, &members)?;
        println!(
            "\nplan: {} car(s) on {} → train tip {}",
            built.cars.len(),
            short(&built.base),
            short(&built.tip)
        );
        for c in &built.cars {
            println!(
                "  #{} {} patch-id {} — {}",
                c.member.pr,
                short(&c.commit),
                short(&c.patch_id),
                c.member.subject
            );
        }
        for d in &built.dropped {
            println!("  dropped #{}: {}", d.pr, d.reason);
        }
        let Some(cmd) = opts.local_ci else {
            println!("\ndry run: nothing pushed, opened or merged.");
            return Ok(());
        };
        println!("\nsimulating with the local check `{cmd}` (nothing leaves this clone):");
        let mut rails = LocalRails {
            repo: main_worktree.clone(),
            main,
            check: Box::new(shell_check(cmd)),
            history: Vec::new(),
        };
        drive(&mut rails, members)?
    } else {
        let mut ports = LiveTrainForge {
            project_root: project_root.clone(),
            main_worktree: main_worktree.clone(),
            base: base.clone(),
            forge: forge.as_ref(),
            named: prs_named_on_command_line(opts),
            dispatch_ctx,
            dispatch_abort: None,
        };
        let mut rails = ForgeRails::new(
            &mut ports,
            chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string(),
        );
        let outcome = drive(&mut rails, members)?;
        warnings = rails.warnings;
        dispatch_abort = ports.dispatch_abort.take();
        outcome
    };
    println!(
        "\n{} landed, {} ejected, {} CI run(s){}",
        outcome.landed.len(),
        outcome.ejected.len(),
        outcome.ci_runs,
        if opts.dry_run { " (simulated)" } else { "" }
    );
    for (pr, commit) in &outcome.landed {
        println!("  landed #{pr} ({})", short(commit));
    }
    for e in &outcome.ejected {
        println!("  ejected #{}: {}", e.pr, e.reason);
    }
    for w in &warnings {
        println!("  warning: {w}");
    }
    if !opts.dry_run && !outcome.landed.is_empty() && !opts.no_pull {
        let status = std::process::Command::new(crate::pr_cmd::pr_ship_post_merge_aida_exe())
            .arg("pull")
            .current_dir(&main_worktree)
            .status();
        if !matches!(status, Ok(s) if s.success()) {
            eprintln!("  warning: `aida pull` after the train did not finish; run it by hand");
        }
    }
    if let Some(error) = &outcome.error {
        println!("  the train stopped early: {error}");
    }
    // trace:STORY-1917 trace:TASK-2344 | ai:claude
    if let Some(why) = dispatch_abort {
        return Err(dispatch_aborted(&why));
    }
    let not_landed = refused.len() + outcome.ejected.len();
    if not_landed > 0 && !opts.all_ready {
        let reasons: Vec<String> = refused
            .iter()
            .chain(outcome.ejected.iter())
            .map(|e| format!("#{}: {}", e.pr, e.reason))
            .collect();
        anyhow::bail!(
            "{not_landed} PR(s) were not landed ({})",
            reasons.join("; ")
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/story_1690_train_tests.rs"]
mod tests;
