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
/// Two sub-trains test at once while bisecting.
const PARALLEL_SUB_TRAINS: usize = 2;

/// One PR the train may land: its number, the head its approval covers, and
/// the commit message `aida pr ship` would have given its squash merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Member {
    pub pr: u64,
    pub head: String,
    pub subject: String,
    pub body: String,
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
/// members line or any entry is malformed — the gate fails closed on it.
// trace:STORY-1690 | ai:claude
pub(crate) fn parse_members_line(body: &str) -> Option<Vec<(u64, String)>> {
    let rest = body
        .lines()
        .find_map(|l| l.trim().strip_prefix(MEMBERS_LINE))?
        .trim();
    if rest.is_empty() {
        return None;
    }
    rest.split(',')
        .map(|entry| {
            let (n, sha) = entry.trim().split_once('@')?;
            let n: u64 = n.trim().trim_start_matches('#').parse().ok()?;
            let sha = sha.trim();
            (n > 0 && sha.len() >= 7 && sha.bytes().all(|b| b.is_ascii_hexdigit()))
                .then(|| (n, sha.to_string()))
        })
        .collect()
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
    let want = crate::review_carryover::patch_id(repo, &fork, &head)
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
    let got = crate::review_carryover::patch_id(repo, tip, &commit)
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
}

/// The result of a train run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Outcome {
    /// (PR, car commit as built) in landing order.
    pub landed: Vec<(u64, String)>,
    pub ejected: Vec<Ejected>,
    /// CI runs spent.
    pub ci_runs: usize,
}

/// Drive `members` to main through `rails`: build on the current main, test,
/// land green trains while main is still their base, rerun the first red
/// once, then bisect by halves with up to two sub-trains in flight. Every
/// member ends landed or ejected with a reason.
// trace:STORY-1690 | ai:claude
pub(crate) fn drive(rails: &mut dyn Rails, members: Vec<Member>) -> Result<Outcome> {
    let mut out = Outcome::default();
    if members.is_empty() {
        return Ok(out);
    }
    // (group, may this group's red be rerun once?)
    let mut pending: VecDeque<(Vec<Member>, bool)> = VecDeque::from([(members.clone(), true)]);
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
    for e in &out.ejected {
        rails.ejected(e);
    }
    Ok(out)
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
/// merge-hold-gate applies. The train never releases a hold.
// trace:STORY-1690 | ai:claude
pub(crate) fn member_label_refusal(c: &crate::merge_hold::PinnedChange) -> Option<String> {
    if c.hold_label {
        return Some(
            "it carries an active aida:merge-hold label; the train never releases a hold"
                .to_string(),
        );
    }
    if c.recorded_label && !c.cleared_label {
        return Some("it was held before and has no recorded clearance".to_string());
    }
    if c.cleared_label && !c.recorded_label {
        return Some("its clearance label has no matching recorded hold".to_string());
    }
    None
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
        let _ = git(&self.repo, &["worktree", "remove", "--force", &path_s]);
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

/// `git` in the main clone with origin's stored credentials kept out of argv
/// (BUG-2181), for the calls that contact origin.
fn origin_git(repo: &Path, args: &[&str]) -> Result<String> {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg(repo).args(args);
    aida_core::git_ops::configure_remote_credentials(&mut cmd, repo, "origin");
    let out = cmd
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| format!("could not run `git {}`", args.join(" ")))?;
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
    if let Some(reason) = checks.hold_marker(pr) {
        return Err(format!(
            "it is under a merge hold ({}); the train never releases a hold",
            reason.trim()
        ));
    }
    match checks.labels(pr) {
        Ok(Some(c)) => {
            if let Some(why) = member_label_refusal(&c) {
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
    if let crate::review_marker::MergeGate::UnderReview(m) =
        crate::review_marker::merge_gate(checks.marker_root(), pr, || Some(head.clone()))
    {
        return Err(crate::review_marker::refusal_message(&m));
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
    let hints = crate::pr_ship::extract_spec_ids_from_text(&format!("{head_ref} {}", meta.title));
    checks.corpus(pr, &hints, &head)?;
    let (subject, body) = checks.message(pr, &meta, &head)?;
    Ok(Member {
        pr,
        head,
        subject,
        body,
    })
}

/// [`MemberChecks`] against the project's forge, store and main clone.
// trace:STORY-1690 | ai:claude
pub(crate) struct LiveChecks<'a> {
    pub project_root: &'a Path,
    pub main_worktree: &'a Path,
    pub forge: &'a dyn crate::forge::Forge,
    pub base: &'a str,
}

impl MemberChecks for LiveChecks<'_> {
    fn metadata(&mut self, pr: u64) -> std::result::Result<crate::forge::ChangeMetadata, String> {
        self.forge
            .change_metadata(pr, &mut crate::network_retry::NoopSink)
            .map_err(|e| format!("its metadata could not be read ({e:#})"))
    }
    fn hold_marker(&mut self, pr: u64) -> Option<String> {
        crate::merge_hold::read_hold(self.main_worktree, pr)
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
        crate::pr_cmd::merge_path_approval_gate(
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
        crate::merge_hold::enforce_corpus_gate_before_merge(
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
            }
        };
        let (subject, _trailer) = crate::pr_cmd::derive_pr_ship_squash_subject_from(
            self.project_root,
            &ship_meta,
            meta.head_ref.trim(),
            false,
        )
        .map_err(|e| format!("its squash subject could not be derived ({e:#})"))?;
        let subject = subject.unwrap_or_else(|| ship_meta.title.trim().to_string());
        let body = member_body(self.project_root, self.base, head, &ship_meta.body)?;
        Ok((subject, body))
    }
}

/// The member gate, live.
// trace:STORY-1690 | ai:claude
pub(crate) fn gate_member(
    project_root: &Path,
    main_worktree: &Path,
    forge: &dyn crate::forge::Forge,
    base: &str,
    pr: u64,
) -> std::result::Result<Member, String> {
    gate_member_with(
        &mut LiveChecks {
            project_root,
            main_worktree,
            forge,
            base,
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
    fn wait_ci(&mut self, change: &crate::forge::ChangeRef) -> Result<CiVerdict>;
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
}

/// [`TrainForge`] through the project's forge and main clone.
// trace:STORY-1690 | ai:claude
pub(crate) struct LiveTrainForge<'a> {
    pub project_root: PathBuf,
    pub main_worktree: PathBuf,
    pub base: String,
    pub forge: &'a dyn crate::forge::Forge,
}

impl LiveTrainForge<'_> {
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
        fetch_base(&self.main_worktree, &self.base)
    }

    fn lease(&mut self, train_pr: u64) -> std::result::Result<Box<dyn std::any::Any>, String> {
        crate::merge_lock::acquire(
            &self.main_worktree,
            &self.base,
            Some(train_pr),
            "aida pr train",
            crate::merge_lock::DEFAULT_WAIT,
        )
        .map(|lease| Box::new(lease) as Box<dyn std::any::Any>)
        .map_err(|e| format!("{e:#}"))
    }

    fn build(&mut self, base: &str, members: &[Member]) -> Result<Built> {
        build(&self.main_worktree, base, members)
    }

    fn gate(&mut self, pr: u64) -> std::result::Result<Member, String> {
        gate_member(
            &self.project_root,
            &self.main_worktree,
            self.forge,
            &self.base,
            pr,
        )
    }

    fn open(&mut self, train: &Built, branch: &str) -> Result<crate::forge::ChangeRef> {
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
        self.forge.open_change(crate::forge::OpenChange {
            branch: branch.to_string(),
            base: self.base.clone(),
            title: format!("[train] land {}", prs.join(", ")),
            body: train_pr_body(train),
            draft: false,
        })
    }

    fn wait_ci(&mut self, change: &crate::forge::ChangeRef) -> Result<CiVerdict> {
        use crate::forge::{CheckRegistration, CiState};
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
        Ok(match self.forge.watch_ci(change)? {
            CiState::Success => CiVerdict::Green,
            other => CiVerdict::Red(format!(
                "train PR #{} CI {other:?}: {}",
                change.id, change.url
            )),
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
        }
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
        for change in &changes {
            let v = match self.ports.wait_ci(change) {
                Ok(v) => v,
                Err(e) => CiVerdict::Red(format!("{e:#}")),
            };
            if let CiVerdict::Red(why) = &v {
                println!("  train PR #{} is red: {why}", change.id);
                self.ports.retire(
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
                self.ports.retire(
                    &change,
                    "Another merger holds the merge lease; the train rebuilds.",
                );
                return Ok(Landing::Stale);
            }
        };
        if self.ports.fetch_main()? != train.base {
            self.ports.retire(
                &change,
                "Main moved after this train was built; the train rebuilds.",
            );
            return Ok(Landing::Stale);
        }
        // Re-gate every member right before the merge: a hold, a new push or
        // a withdrawn approval since the build ejects that member.
        let mut refused = Vec::new();
        for car in &train.cars {
            let pr = car.member.pr;
            match self.ports.gate(pr) {
                Ok(m) if m.head == car.member.head => {}
                Ok(m) => refused.push(Ejected {
                    pr,
                    reason: format!(
                        "its head moved from {} to {} after the train was built; re-train",
                        short(&car.member.head),
                        short(&m.head)
                    ),
                }),
                Err(reason) => refused.push(Ejected { pr, reason }),
            }
        }
        if !refused.is_empty() {
            self.ports.retire(
                &change,
                "A member failed its pre-land recheck; the train rebuilds.",
            );
            return Ok(Landing::Refused(refused));
        }
        // The re-gate took several forge round trips, and a merge that does
        // not take the lease (the web UI, a raw `gh pr merge`) could have
        // moved main meanwhile; strict is off, so the forge would rebase the
        // train onto an untested main. Look again right before merging.
        if self.ports.fetch_main()? != train.base {
            self.ports.retire(
                &change,
                "Main moved while the train was re-checked; the train rebuilds.",
            );
            return Ok(Landing::Stale);
        }
        self.ports
            .merge(&change, &train.tip)
            .with_context(|| format!("train PR #{} was not merged", change.id))?;
        let main = match self.ports.fetch_main() {
            Ok(main) => main,
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

/// `aida pr train`.
// trace:STORY-1690 | ai:claude
pub(crate) fn pr_train_handler(opts: &TrainOptions<'_>) -> Result<()> {
    let project_root = crate::find_project_root()?;
    let main_worktree = crate::main_worktree_root_from(&project_root);
    let kind = crate::forge::resolve_forge_kind(&project_root);
    anyhow::ensure!(
        kind != crate::forge::ForgeKind::None,
        "`aida pr train` needs a forge (GitHub or GitLab): a train is tested by CI on a train PR"
    );
    let forge = crate::forge::forge_for(&project_root);
    let base = crate::review_carryover::protected_base(&main_worktree)
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
    println!(
        "merge train onto `{base}`: gating {} PR(s){}",
        candidates.len(),
        if opts.dry_run { " (dry run)" } else { "" }
    );
    let mut members = Vec::new();
    let mut refused = Vec::new();
    let mut waiting = Vec::new();
    for pr in candidates {
        if members.len() >= opts.max.max(1) {
            waiting.push(pr);
            continue;
        }
        match gate_member(&project_root, &main_worktree, forge.as_ref(), &base, pr) {
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
        };
        let mut rails = ForgeRails::new(
            &mut ports,
            chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string(),
        );
        let outcome = drive(&mut rails, members)?;
        warnings = rails.warnings;
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
    let not_landed = refused.len() + outcome.ejected.len();
    if not_landed > 0 && !opts.all_ready {
        anyhow::bail!("{not_landed} PR(s) were not landed (reasons above)");
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/story_1690_train_tests.rs"]
mod tests;
