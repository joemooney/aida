//! The landing reconciler: one level-triggered pass that derives every open
//! change's state from facts (the forge, the gate ledger, git) and maps each
//! state to exactly one action. Nothing about a change is stored between
//! passes, so no state can go stale.
//!
//! `aida land status` runs the pass and prints the derived table. `aida land
//! run` (see [`run`]) runs the same pass and then carries out each row's
//! action, so the view and the actor cannot disagree.
// trace:STORY-2768 | ai:claude

use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[path = "land_args.rs"]
pub(crate) mod land_args;
#[path = "land_run.rs"]
pub(crate) mod run;

use crate::cmdkit::{CmdPolicy, CommandReg, Handler};
use crate::gate::review_verdict::VerdictKind;
use crate::stakeholder_role::StakeholderAction;

/// Content rounds of requested changes after which a change stops being
/// reworked and goes to the advisor.
const MAX_CONTENT_ROUNDS: usize = 3;
/// The branch whose changes the integration seat lands.
const INTEGRATION_BRANCH: &str = "integration";
const TRAIN_PREFIX: &str = "train/";
/// Where a pass fetches the commits it compares. The refs are shared by
/// every pass and only ever read under [`FETCH_LOCK`], straight after the
/// fetch, to pin commit ids; nothing dereferences them afterwards.
const REF_PREFIX: &str = "refs/aida/land/";
/// Serializes the fetch and the pinning of one pass against another's.
const FETCH_LOCK: &str = ".aida/cache/land-fetch.lock";

/// One open change, as the forge reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pr {
    pub id: u64,
    pub head: String,
    pub base: String,
    pub branch: Option<String>,
    pub title: String,
    /// When the forge last saw the change move.
    pub updated_at: Option<String>,
    /// Whether the forge's merge-hold label is set, when discovery read it.
    pub hold_label: Option<bool>,
    /// CI on the head, when discovery read it.
    pub ci: Option<Ci>,
}

/// CI on a change's head, summed over every check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Ci {
    /// No check has reported.
    None,
    Pending,
    Green,
    /// The failed checks, named.
    Red(String),
}

impl Ci {
    fn label(&self) -> String {
        match self {
            Ci::None => "none".into(),
            Ci::Pending => "pending".into(),
            Ci::Green => "green".into(),
            Ci::Red(names) => format!("red: {names}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Track {
    Main,
    Integration,
}

/// A change's derived state, in the precedence [`derive`] checks them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum State {
    /// A train or promotion branch: it is the landing itself.
    Landing,
    Conflict,
    Reviewing,
    /// A check failed on the head: the head is about to change.
    CiFailed,
    Ready,
    /// Approved, with a hold whose release passes every check except the
    /// mandate's cover, which only the release itself judges. Not READY.
    ReadyPendingCover,
    ChangesRequested,
    NeedsReview,
    Blocked,
    /// Evidence could not be read this pass; the next pass asks again.
    Unknown,
}

impl State {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            State::CiFailed => "CI-FAILED",
            State::Landing => "LANDING",
            State::Conflict => "CONFLICT",
            State::Reviewing => "REVIEWING",
            State::Ready => "READY",
            State::ReadyPendingCover => "READY-PENDING-COVER",
            State::ChangesRequested => "CHANGES-REQUESTED",
            State::NeedsReview => "NEEDS-REVIEW",
            State::Blocked => "BLOCKED",
            State::Unknown => "UNKNOWN",
        }
    }
}

/// The one action a state calls for.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Action {
    /// Nothing to do this pass.
    Wait,
    DispatchRework,
    LaunchReview,
    /// Release a hold whose release condition is met; the next pass lands
    /// the change. The release decides again for itself.
    ClearHoldAndLand,
    /// Add to the next train (main track).
    Train,
    /// Report to the integration seat (integration track).
    ReportToIntegration,
    AdvisorRuling,
}

impl Action {
    /// Who carries the action out.
    pub(crate) fn owner(&self) -> &'static str {
        match self {
            Action::Wait => "-",
            Action::DispatchRework
            | Action::LaunchReview
            | Action::Train
            | Action::ClearHoldAndLand => "reconciler",
            Action::ReportToIntegration => "integration seat",
            Action::AdvisorRuling => "advisor",
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        match self {
            Action::Wait => "wait",
            Action::DispatchRework => "dispatch rework",
            Action::LaunchReview => "launch reviewer",
            Action::ClearHoldAndLand => "release the met hold, then land",
            Action::Train => "add to next train",
            Action::ReportToIntegration => "report to integration seat",
            Action::AdvisorRuling => "advisor ruling",
        }
    }
}

/// A merge hold and whether its release condition is met.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hold {
    pub detail: String,
    pub release: HoldRelease,
    /// When the hold was placed.
    pub since: Option<String>,
}

/// What the hold's own release decision says at the current head. This is a
/// read-only answer: the release itself decides again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HoldRelease {
    /// Every release check passes without a mandate.
    Met,
    /// Every other check passes; a covered spec is sensitive, so the release
    /// also needs this mandate's cover, which only the release judges.
    PendingCover(String),
    Unmet(String),
}

/// Whether an approval covers the head's content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Approval {
    Approved(String),
    /// The gate's settled refusal.
    Refused(String),
}

/// The requested-changes history of a change, counted by content: two heads
/// with the same own diff are one round.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Rounds {
    /// A blocking verdict stands at the current content.
    pub at_content: bool,
    pub content_rounds: usize,
    /// When the newest blocking verdict at the current content was recorded.
    pub since: Option<String>,
}

/// Every fact a derivation reads, behind one seam so tests inject snapshots.
/// `Err` means the fact could not be read, never that it is clear.
pub(crate) trait Evidence {
    /// Whether merging the head into its base conflicts.
    fn conflict(&self, pr: &Pr) -> std::result::Result<bool, String>;
    /// A live review of the head, described.
    fn reviewing(&self, pr: &Pr) -> std::result::Result<Option<String>, String>;
    /// CI on the head.
    fn ci(&self, pr: &Pr) -> std::result::Result<Ci, String>;
    /// Whether an approval covers the head's content. `Err` only when the
    /// evidence could not be read: a refusal is `Ok`.
    fn approval(&self, pr: &Pr) -> std::result::Result<Approval, String>;
    fn hold(&self, pr: &Pr) -> std::result::Result<Option<Hold>, String>;
    fn rounds(&self, pr: &Pr) -> std::result::Result<Rounds, String>;
    /// A live lane working the change's branch, described.
    fn lane(&self, pr: &Pr) -> std::result::Result<Option<String>, String>;
    /// How many reviewers of this head were stopped for running too long.
    fn review_timeouts(&self, _pr: &Pr) -> u32 {
        0
    }
    /// Why no rework lane may be launched for this head, when one refused
    /// in a way a relaunch cannot fix.
    fn rework_stop(&self, _pr: &Pr) -> Option<String> {
        None
    }
}

/// One row of the derived table.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Row {
    pub pr: u64,
    pub track: Track,
    pub state: State,
    pub action: Action,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    pub reason: String,
    /// Who is working the change now: a reviewer or a lane.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// CI on the head, once the derivation read it. CI gates the landing,
    /// never the review: only a red head changes the state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ci: Option<String>,
}

pub(crate) fn track_of(pr: &Pr) -> Track {
    if pr.base == INTEGRATION_BRANCH {
        Track::Integration
    } else {
        Track::Main
    }
}

/// A change's state and its single action, from `evidence` alone. Facts are
/// read in precedence order and the first that decides ends the derivation.
// trace:STORY-2768 | ai:claude
pub(crate) fn derive(pr: &Pr, evidence: &dyn Evidence) -> Row {
    let track = track_of(pr);
    let ci_read: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
    let row = |state: State, action: Action, since: Option<String>, reason: String| Row {
        pr: pr.id,
        track,
        state,
        action,
        since,
        reason,
        actor: None,
        ci: ci_read.borrow().clone(),
    };
    let unknown = |what: &str, why: String| {
        row(
            State::Unknown,
            Action::Wait,
            None,
            format!("{what} unknown ({why}); read again next pass"),
        )
    };
    let branch = pr.branch.as_deref().unwrap_or_default();
    if branch.starts_with(TRAIN_PREFIX) {
        return row(
            State::Landing,
            Action::Wait,
            None,
            "a train; its CI decides".into(),
        );
    }
    if branch == INTEGRATION_BRANCH && track == Track::Main {
        return row(
            State::Landing,
            Action::Wait,
            None,
            "the integration branch itself; the advisor cuts promotions from it".into(),
        );
    }
    // Rework goes to a lane only when none is already on the branch.
    let rework = |state: State, since: Option<String>, reason: String| match evidence.lane(pr) {
        // A lane that refused over unpushed work in its seat is never
        // launched over: someone has to look at the seat first.
        Ok(None) => match evidence.rework_stop(pr) {
            Some(stop) => row(
                State::Blocked,
                Action::AdvisorRuling,
                since,
                format!("{reason}; rework refused: {stop}"),
            ),
            None => row(state, Action::DispatchRework, since, reason),
        },
        Ok(Some(lane)) => Row {
            actor: Some(lane),
            ..row(state, Action::Wait, since, reason)
        },
        Err(why) => unknown("lane liveness", why),
    };
    match evidence.conflict(pr) {
        Ok(false) => {}
        Ok(true) => {
            return rework(
                State::Conflict,
                None,
                format!("conflicts with `{}`", pr.base),
            )
        }
        Err(why) => return unknown("mergeability", why),
    }
    match evidence.reviewing(pr) {
        Ok(None) => {}
        Ok(Some(review)) => {
            return Row {
                actor: Some(review.clone()),
                ..row(State::Reviewing, Action::Wait, None, review)
            }
        }
        // Who holds the review is unknown: neither a second reviewer nor a
        // landing may be scheduled on a guess.
        Err(why) => return unknown("review ownership", why),
    }
    let ci = match evidence.ci(pr) {
        Ok(ci) => ci,
        Err(why) => return unknown("CI", why),
    };
    *ci_read.borrow_mut() = Some(ci.label());
    if let Ci::Red(failed) = &ci {
        return rework(
            State::CiFailed,
            None,
            format!("CI failed on the head: {failed}"),
        );
    }
    let refusal = match evidence.approval(pr) {
        Err(why) => return unknown("approval", why),
        Ok(Approval::Approved(approval)) => {
            return match evidence.hold(pr) {
                Ok(None) => row(
                    State::Ready,
                    match track {
                        Track::Main => Action::Train,
                        Track::Integration => Action::ReportToIntegration,
                    },
                    None,
                    approval,
                ),
                Ok(Some(hold)) => match hold.release {
                    HoldRelease::Met => row(
                        State::Ready,
                        Action::ClearHoldAndLand,
                        None,
                        format!("{approval}; hold met by the approval: {}", hold.detail),
                    ),
                    // Not READY: the mandate's cover is judged only when
                    // the hold is released, so nothing may count this as
                    // landable yet.
                    HoldRelease::PendingCover(mandate) => row(
                        State::ReadyPendingCover,
                        Action::ClearHoldAndLand,
                        hold.since,
                        format!(
                            "{approval}; hold release needs mandate {mandate}'s cover, judged at release: {}",
                            hold.detail
                        ),
                    ),
                    HoldRelease::Unmet(why) => row(
                        State::Blocked,
                        Action::AdvisorRuling,
                        hold.since,
                        format!("approved, held: {} ({why})", hold.detail),
                    ),
                },
                Err(why) => unknown("merge hold", why),
            };
        }
        Ok(Approval::Refused(refusal)) => refusal,
    };
    let rounds = match evidence.rounds(pr) {
        Ok(rounds) => rounds,
        Err(why) => return unknown("review history", why),
    };
    if rounds.content_rounds >= MAX_CONTENT_ROUNDS {
        return row(
            State::Blocked,
            Action::AdvisorRuling,
            rounds.since,
            format!(
                "{} content rounds of requested changes; needs an advisor ruling",
                rounds.content_rounds
            ),
        );
    }
    if rounds.at_content {
        return rework(
            State::ChangesRequested,
            rounds.since,
            format!(
                "changes requested at this content (round {})",
                rounds.content_rounds
            ),
        );
    }
    let timeouts = evidence.review_timeouts(pr);
    if timeouts >= run::MAX_REVIEW_TIMEOUTS {
        return row(
            State::Blocked,
            Action::AdvisorRuling,
            None,
            format!("{timeouts} reviewers of this head were stopped for running too long"),
        );
    }
    row(State::NeedsReview, Action::LaunchReview, None, refusal)
}

/// One pass over `changes`: a row each, in change order.
pub(crate) fn pass(changes: &[Pr], evidence: &dyn Evidence) -> Vec<Row> {
    let mut rows: Vec<Row> = changes.iter().map(|pr| derive(pr, evidence)).collect();
    rows.sort_by_key(|row| row.pr);
    rows
}

fn one_line(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= limit {
        return flat;
    }
    let mut cut: String = flat.chars().take(limit.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// The derived table.
pub(crate) fn render(rows: &[Row]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{:<6} {:<11} {:<17} {:<36} {:<16} {}\n",
        "PR", "TRACK", "STATE", "ACTION", "SINCE", "REASON"
    ));
    for row in rows {
        out.push_str(&format!(
            "{:<6} {:<11} {:<17} {:<36} {:<16} {}\n",
            row.pr,
            match row.track {
                Track::Main => "main",
                Track::Integration => "integration",
            },
            row.state.label(),
            row.action.label(),
            row.since
                .as_deref()
                .map(|s| s.get(..16).unwrap_or(s))
                .unwrap_or("-"),
            one_line(&row.reason, 110)
        ));
    }
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for row in rows {
        *counts.entry(row.state.label()).or_default() += 1;
    }
    let summary: Vec<String> = counts.iter().map(|(s, n)| format!("{n} {s}")).collect();
    out.push_str(&format!(
        "{} open change(s): {}\n",
        rows.len(),
        if summary.is_empty() {
            "none".to_string()
        } else {
            summary.join(", ")
        }
    ));
    out
}

// ---- live evidence -------------------------------------------------------

/// Git in `root` and nowhere else: inherited routing (`GIT_DIR`,
/// `GIT_WORK_TREE`, `GIT_NAMESPACE`, injected config) is cleared, as the
/// pinned discovery clears it, so a probe cannot run in another repository.
fn git(root: &Path, args: &[&str], timeout_secs: u64) -> Result<std::process::Output> {
    let mut command = std::process::Command::new("git");
    command
        .current_dir(root)
        .args(args)
        .stdin(std::process::Stdio::null());
    crate::review_watch::forge::clear_routing(&mut command);
    crate::command_output_with_timeout(command, std::time::Duration::from_secs(timeout_secs))
        .with_context(|| format!("`git {}` failed to run or timed out", args.join(" ")))
}

fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    let out = git(root, args, 30)?;
    anyhow::ensure!(
        out.status.success(),
        "`git {}` failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn head_ref(id: u64) -> String {
    format!("{REF_PREFIX}pr/{id}")
}

fn base_ref(base: &str) -> String {
    format!("{REF_PREFIX}base/{base}")
}

/// The commits one pass compares, pinned once: every later git call names
/// these ids, never a ref another pass could move.
#[derive(Debug, Default)]
struct Pins {
    heads: BTreeMap<u64, String>,
    bases: BTreeMap<String, String>,
}

/// Fetch every change's head and base in one call, drop the refs of changes
/// that are no longer open, and pin what was fetched. The fetch and the
/// pinning hold [`FETCH_LOCK`], so a concurrent pass cannot move a ref
/// between the two; `repo` is the origin discovery pinned, rechecked before
/// the network is touched.
fn fetch(
    root: &Path,
    repo: &crate::review_watch::forge::Repository,
    changes: &[Pr],
    gitlab: bool,
) -> Result<Pins> {
    use aida_core::git_ops::RemoteCredentialsExt as _;
    use fs2::FileExt as _;
    let lock_path = root.join(FETCH_LOCK);
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("opening {}", lock_path.display()))?;
    lock.lock_exclusive()
        .with_context(|| format!("locking {}", lock_path.display()))?;
    // Released when `lock` drops, on every return below.
    let mut specs: BTreeSet<String> = BTreeSet::new();
    for pr in changes {
        git_text(
            root,
            &["check-ref-format", &format!("refs/heads/{}", pr.base)],
        )
        .context("a change's base is not a valid branch name")?;
        specs.insert(format!("+refs/heads/{}:{}", pr.base, base_ref(&pr.base)));
        let remote = if gitlab {
            format!("refs/merge-requests/{}/head", pr.id)
        } else {
            format!("refs/pull/{}/head", pr.id)
        };
        specs.insert(format!("+{remote}:{}", head_ref(pr.id)));
    }
    let wanted: BTreeSet<String> = specs
        .iter()
        .filter_map(|spec| spec.split_once(':').map(|(_, local)| local.to_string()))
        .collect();
    let existing = git_text(root, &["for-each-ref", "--format=%(refname)", REF_PREFIX])?;
    for stale in existing.lines().filter(|name| !wanted.contains(*name)) {
        let _ = git_text(root, &["update-ref", "-d", stale]);
    }
    if specs.is_empty() {
        return Ok(Pins::default());
    }
    anyhow::ensure!(
        crate::review_watch::forge::Repository::load(root)?.identity == repo.identity,
        "origin changed since the open changes were discovered"
    );
    let mut command = std::process::Command::new("git");
    command
        .current_dir(root)
        .args([
            "fetch",
            "--no-write-fetch-head",
            "--no-tags",
            "--",
            "origin",
        ])
        .args(&specs)
        .stdin(std::process::Stdio::null());
    // Routing first: the credentials are passed through the environment.
    crate::review_watch::forge::clear_routing(&mut command);
    command.remote_credentials(root, "origin");
    let out = crate::command_output_with_timeout(command, std::time::Duration::from_secs(120))
        .context("the fetch of open changes failed to run or timed out")?;
    anyhow::ensure!(
        out.status.success(),
        "the fetch of open changes failed: {}",
        one_line(&String::from_utf8_lossy(&out.stderr), 300)
    );
    // A ref that does not resolve leaves its change unpinned, which reads
    // as unknown for that change alone.
    let commit = |name: &str| {
        git_text(
            root,
            &["rev-parse", "--verify", &format!("{name}^{{commit}}")],
        )
        .ok()
        .filter(|sha| sha.len() == 40 && sha.bytes().all(|c| c.is_ascii_hexdigit()))
    };
    let mut pins = Pins::default();
    for pr in changes {
        if let Some(sha) = commit(&head_ref(pr.id)) {
            pins.heads.insert(pr.id, sha);
        }
        if !pins.bases.contains_key(&pr.base) {
            if let Some(sha) = commit(&base_ref(&pr.base)) {
                pins.bases.insert(pr.base.clone(), sha);
            }
        }
    }
    Ok(pins)
}

/// The production evidence: the readers the merge gates use, all read-only.
struct Live {
    root: PathBuf,
    /// The commits this pass compares; `Err` when its fetch failed, which
    /// makes every git-backed fact unknown.
    pins: std::result::Result<Pins, String>,
    /// The forge keys its changes `MR-N`.
    gitlab: bool,
    leases: std::cell::OnceCell<std::result::Result<Vec<crate::leases::SessionLease>, String>>,
    /// Live claims from other clones and hosts, read once per pass from the
    /// synced store: never a process probe of another machine.
    claims: Vec<crate::coordination::Claim>,
    /// The children `aida land run` started that are still alive, and what
    /// it recorded about the ones it stopped.
    book: run::Book,
    /// The tree each clean merge of a head into its base produced.
    merge_trees: std::cell::RefCell<BTreeMap<u64, String>>,
}

impl Live {
    /// The pinned head and base of `pr`, as commit ids. The head must be
    /// the one discovery reported: a result is never attached to a change
    /// whose head it was not computed from.
    fn pinned(&self, pr: &Pr) -> std::result::Result<(&str, &str), String> {
        let pins = self.pins.as_ref().map_err(Clone::clone)?;
        let head = pins
            .heads
            .get(&pr.id)
            .ok_or("the change's head was not fetched")?;
        if head != &pr.head {
            return Err("the head moved during the pass".into());
        }
        let base = pins
            .bases
            .get(&pr.base)
            .ok_or("the change's base was not fetched")?;
        Ok((head, base))
    }

    /// The identity of a commit's own diff against the pinned base: equal
    /// for two heads that differ only by merges of the base.
    fn content_id(&self, base: &str, sha: &str) -> std::result::Result<String, String> {
        crate::gate::review_carryover::single_merge_base(&self.root, base, sha)
            .and_then(|fork| crate::gate::review_carryover::patch_id(&self.root, &fork, sha))
            .map_err(|refusal| refusal.reason())
    }

    fn local_leases(&self) -> std::result::Result<&[crate::leases::SessionLease], String> {
        self.leases
            .get_or_init(|| crate::status_prs::live_owned_leases_strict(&self.root))
            .as_ref()
            .map(Vec::as_slice)
            .map_err(Clone::clone)
    }
}

/// How a local lease's holder reads in the table.
fn lease_actor(lease: &crate::leases::SessionLease) -> String {
    format!(
        "{}@{} ({})",
        lease.role.as_deref().unwrap_or("lane"),
        lease.hostname,
        lease.id
    )
}

/// How another clone's claim reads in the table.
pub(crate) fn claim_actor(claim: &crate::coordination::Claim) -> String {
    format!(
        "{}@{}",
        if claim.agent.is_empty() {
            "lane"
        } else {
            &claim.agent
        },
        claim.host
    )
}

/// Whether a claim's scope names one of the change's specs, or its branch
/// (a lane's branch starts with its spec's canonical ID).
pub(crate) fn claim_covers(scope: &str, pr: &Pr) -> bool {
    let scope = scope.trim();
    if scope.is_empty() {
        return false;
    }
    if pr.branch.as_deref().is_some_and(|branch| {
        branch
            .to_ascii_uppercase()
            .starts_with(&scope.to_ascii_uppercase())
    }) {
        return true;
    }
    specs_of(pr)
        .iter()
        .any(|id| aida_core::id_identity::same_id(id, scope))
}

/// The specs a change's title names: those in its trailing parentheses when
/// it has them, else every ID in the title.
pub(crate) fn specs_of(pr: &Pr) -> Vec<String> {
    let title = pr.title.trim();
    let trailer = title
        .strip_suffix(')')
        .and_then(|rest| rest.rfind('(').map(|at| &rest[at + 1..]));
    let ids = trailer
        .map(crate::pr_ship::extract_spec_ids_from_text)
        .unwrap_or_default();
    if ids.is_empty() {
        crate::pr_ship::extract_spec_ids_from_text(title)
    } else {
        ids
    }
}

impl Evidence for Live {
    fn conflict(&self, pr: &Pr) -> std::result::Result<bool, String> {
        let (head, base) = self.pinned(pr)?;
        let out = git(
            &self.root,
            &["merge-tree", "--write-tree", "--no-messages", base, head],
            60,
        )
        .map_err(|e| format!("{e:#}"))?;
        match out.status.code() {
            Some(0) => {
                let tree = String::from_utf8_lossy(&out.stdout);
                if let Some(tree) = tree.lines().next().filter(|tree| !tree.is_empty()) {
                    self.merge_trees
                        .borrow_mut()
                        .insert(pr.id, tree.trim().to_string());
                }
                Ok(false)
            }
            Some(1) => Ok(true),
            _ => Err(one_line(&String::from_utf8_lossy(&out.stderr), 200)),
        }
    }

    fn reviewing(&self, pr: &Pr) -> std::result::Result<Option<String>, String> {
        // Our own reviewer is known to be alive before it claims anything.
        if let Some(child) = self.book.reviewer(pr) {
            return Ok(Some(child.describe()));
        }
        // A marker that is there but unreadable is not "no reviewer".
        match crate::gate::review_marker::live_description_checked(
            &self.root,
            pr.id,
            Some(&pr.head),
        ) {
            Ok(Some(claimed)) => return Ok(Some(claimed)),
            Ok(None) => {}
            Err(error) => return Err(format!("the review marker could not be read ({error})")),
        }
        // A reviewer that has launched but not yet claimed the head holds a
        // lease scoped to the change.
        let scope = format!("PR-{}", pr.id);
        let leases = self
            .local_leases()
            .map_err(|why| format!("reviewer leases unreadable ({why})"))?;
        Ok(leases
            .iter()
            .find(|lease| lease.scope.eq_ignore_ascii_case(&scope))
            .map(|lease| format!("{} launched, no claim yet", lease_actor(lease)))
            .or_else(|| {
                self.claims
                    .iter()
                    .find(|claim| claim.scope.eq_ignore_ascii_case(&scope))
                    .map(|claim| format!("{} reviewing", claim_actor(claim)))
            }))
    }

    fn ci(&self, pr: &Pr) -> std::result::Result<Ci, String> {
        pr.ci
            .clone()
            .ok_or_else(|| "discovery did not report checks".to_string())
    }

    fn approval(&self, pr: &Pr) -> std::result::Result<Approval, String> {
        use crate::gate::approval::HeadDecision;
        use crate::gate::merge_hold_core::CorpusMergeGate;
        let root = self.root.as_path();
        let head = pr.head.as_str();
        let (candidates, policy, ids, forge_answered) =
            crate::gate::approval::merge_approval_subject_observed(
                &[root],
                root,
                pr.id,
                &[],
                pr.branch.as_deref(),
                Some(head),
            );
        // Without the forge's answer the verdicts were looked up under only
        // the specs the title and branch claim: "none found" proves nothing.
        if !forge_answered {
            return Err("the forge did not report the change's commits".into());
        }
        // The corpus is read before any refusal is considered, so a verdict
        // artifact that cannot be read is unknown, never "not approved".
        let corpus = crate::gate::merge_hold_core::corpus_gate_decision(
            root,
            pr.id,
            &ids,
            Some(&pr.base),
            || Some(pr.head.clone()),
            false,
        )
        .map_err(|error| format!("{error:#}"))?;
        let site = crate::gate::review_carryover::CarryOverSite::read_only(root, Some(&pr.base));
        let approval = match crate::gate::approval::approval_decision(
            pr.id,
            &candidates,
            Some(head),
            &policy,
            Some(&site),
        ) {
            Err(refusal) => return Ok(Approval::Refused(refusal)),
            Ok(HeadDecision::Covered(Some(sha))) => format!("approved at {}", short(&sha)),
            Ok(HeadDecision::Covered(None)) => {
                return Ok(Approval::Refused("no approval covers the head".into()))
            }
            Ok(HeadDecision::CarriedOver(proof, _)) => format!(
                "approved at {}, carried over clean merges of `{}`",
                short(&proof.approved_sha),
                proof.base_branch()
            ),
            Ok(HeadDecision::Stale { message, .. }) => return Ok(Approval::Refused(message)),
        };
        // A blocking verdict the approval does not answer refuses the head.
        Ok(match corpus {
            CorpusMergeGate::Clear => Approval::Approved(approval),
            CorpusMergeGate::Refuse { message, .. } => Approval::Refused(message),
        })
    }

    fn hold(&self, pr: &Pr) -> std::result::Result<Option<Hold>, String> {
        use crate::gate::merge_hold_core::{hold_placed_at, read_hold, read_hold_record};
        use crate::gate::rework_hold_release::{release_condition, ReleaseCondition};
        let since = || {
            hold_placed_at(&self.root, pr.id)
                .map(|at| chrono::DateTime::<chrono::Utc>::from(at).to_rfc3339())
        };
        let human_only = |what: &str| HoldRelease::Unmet(format!("{what}; a human clears it"));
        if let Some(record) = read_hold_record(&self.root, pr.id) {
            let detail = format!(
                "{} hold: {}",
                format!("{:?}", record.reason_kind).to_lowercase(),
                record.detail
            );
            // The release's own decision, read-only: the hold's kind alone
            // says nothing about composed holds, sensitive specs or grants.
            let release = match release_condition(&self.root, pr.id) {
                ReleaseCondition::Met => HoldRelease::Met,
                ReleaseCondition::PendingCover(mandate) => HoldRelease::PendingCover(mandate),
                ReleaseCondition::Unmet(why) => HoldRelease::Unmet(why),
                ReleaseCondition::Unknown(why) => return Err(why),
                // Released between the two reads: the next pass sees it gone.
                ReleaseCondition::NoHold => return Err("the hold changed during the pass".into()),
            };
            return Ok(Some(Hold {
                detail,
                release,
                since: since(),
            }));
        }
        if let Some(detail) = read_hold(&self.root, pr.id) {
            return Ok(Some(Hold {
                detail,
                release: human_only("an untyped hold has no release condition"),
                since: since(),
            }));
        }
        let labelled = match pr.hold_label {
            Some(labelled) => labelled,
            None => crate::merge_hold::label_present(&self.root, pr.id)?,
        };
        Ok(labelled.then(|| Hold {
            detail: "the merge-hold label is set on the forge".into(),
            release: human_only("the label has no recorded hold behind it"),
            since: None,
        }))
    }

    fn rounds(&self, pr: &Pr) -> std::result::Result<Rounds, String> {
        let (head, base) = self.pinned(pr)?;
        // The current content must be identified: every comparison below is
        // against it.
        let current = self
            .content_id(base, head)
            .map_err(|why| format!("the head's own diff could not be identified: {why}"))?;
        let change = crate::gate::ledger::ChangeKey {
            forge: if self.gitlab {
                crate::gate::ledger::ChangeForge::Mr
            } else {
                crate::gate::ledger::ChangeForge::Pr
            },
            number: pr.id,
        };
        let mut contents: BTreeSet<String> = BTreeSet::new();
        let mut rounds = Rounds::default();
        // The strict reader: a corrupt, unreadable or over-budget ledger is
        // an error here, never an empty history.
        for record in
            crate::gate::ledger::project::change_verdict_records_checked(&self.root, change)?
        {
            let key = crate::gate::review_verdict::recording_key(&record);
            if !VerdictKind::parse(&key.2).requires_changes() {
                continue;
            }
            let Some(sha) = key.0 else { continue };
            // An older head that can no longer be compared (its commits are
            // gone after a force push) counts as content of its own.
            let content = self.content_id(base, &sha).unwrap_or_else(|_| sha.clone());
            if content == current {
                rounds.at_content = true;
                let at = record
                    .get("recorded_at")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                if at > rounds.since {
                    rounds.since = at;
                }
            }
            contents.insert(content);
        }
        rounds.content_rounds = contents.len();
        Ok(rounds)
    }

    fn lane(&self, pr: &Pr) -> std::result::Result<Option<String>, String> {
        if let Some(child) = self.book.lane(pr) {
            return Ok(Some(child.describe()));
        }
        let Some(branch) = pr.branch.as_deref() else {
            return Err("the change's branch is unknown".into());
        };
        if let Some(lease) = self
            .local_leases()?
            .iter()
            .find(|lease| lease.branch == branch)
        {
            return Ok(Some(lease_actor(lease)));
        }
        Ok(self
            .claims
            .iter()
            .find(|claim| claim_covers(&claim.scope, pr))
            .map(claim_actor))
    }

    fn review_timeouts(&self, pr: &Pr) -> u32 {
        self.book.timeouts(pr)
    }

    fn rework_stop(&self, pr: &Pr) -> Option<String> {
        self.book.stop(pr)
    }
}

fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

/// CI summed over a GitHub `statusCheckRollup`.
pub(crate) fn ci_of_rollup(rollup: &serde_json::Value) -> Ci {
    let Some(checks) = rollup.as_array().filter(|checks| !checks.is_empty()) else {
        return Ci::None;
    };
    let text = |check: &serde_json::Value, key: &str| {
        check
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_ascii_uppercase()
    };
    let name_of = |check: &serde_json::Value| {
        check
            .get("name")
            .or_else(|| check.get("context"))
            .and_then(|v| v.as_str())
            .unwrap_or("check")
            .to_string()
    };
    // A check that ran more than once at this head counts once, by its
    // newest run: a cancelled run that a later run replaced says nothing.
    let mut newest: BTreeMap<(String, String), &serde_json::Value> = BTreeMap::new();
    for check in checks {
        let key = (text(check, "workflowName"), name_of(check));
        let started = |check: &serde_json::Value| text(check, "startedAt");
        match newest.get(&key) {
            Some(kept) if started(kept) > started(check) => {}
            _ => {
                newest.insert(key, check);
            }
        }
    }
    let mut failed: Vec<String> = Vec::new();
    let mut pending = false;
    for check in newest.values() {
        let name = name_of(check);
        // A check run reports `status` + `conclusion`; a commit status
        // reports `state`.
        let outcome = match text(check, "conclusion") {
            done if !done.is_empty() => done,
            _ => text(check, "state"),
        };
        match outcome.as_str() {
            "SUCCESS" | "NEUTRAL" | "SKIPPED" => {}
            "" | "PENDING" | "EXPECTED" | "QUEUED" | "IN_PROGRESS" => pending = true,
            // A cancelled run did not test the head: CI is not green, and
            // it is not a failure of the change either.
            "CANCELLED" => pending = true,
            _ => failed.push(name),
        }
    }
    if !failed.is_empty() {
        failed.sort();
        failed.dedup();
        return Ci::Red(one_line(&failed.join(", "), 80));
    }
    if pending {
        Ci::Pending
    } else {
        Ci::Green
    }
}

/// A change from the watcher's discovery, which reports no checks. That is
/// "no check has reported", the same as a GitHub change with an empty
/// rollup: unknown CI would make every change on such a forge UNKNOWN.
pub(crate) fn plain_change(change: crate::review_watch::forge::Change, title: String) -> Pr {
    Pr {
        id: change.id,
        head: change.head,
        base: change.base,
        branch: change.head_branch,
        title,
        updated_at: None,
        hold_label: None,
        ci: Some(Ci::None),
    }
}

/// One forge read for every open change, with its title, labels and checks.
/// `None` on a forge without it (GitLab): the caller falls back to the
/// watcher's discovery and reads the rest per change.
fn discover_rich(
    root: &Path,
    repo: &crate::review_watch::forge::Repository,
) -> Result<Option<Vec<Pr>>> {
    let Some(slug) = repo.github_slug() else {
        return Ok(None);
    };
    let rows = crate::forge::github_open_changes_detailed(root, &slug, &|command| {
        repo.pin_command(command)
    })?;
    let text = |row: &serde_json::Value, key: &str| -> Result<String> {
        row.get(key)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .with_context(|| format!("forge listing row has no `{key}`"))
    };
    let mut changes = Vec::new();
    for row in &rows {
        if row.get("isDraft").and_then(|v| v.as_bool()) == Some(true) {
            continue;
        }
        let head = text(row, "headRefOid")?;
        anyhow::ensure!(
            head.len() == 40 && head.bytes().all(|c| c.is_ascii_hexdigit()),
            "forge listing row has an invalid head"
        );
        changes.push(Pr {
            id: row
                .get("number")
                .and_then(|v| v.as_u64())
                .context("forge listing row has no number")?,
            head,
            base: text(row, "baseRefName")?,
            branch: Some(text(row, "headRefName")?),
            title: text(row, "title")?,
            updated_at: row
                .get("updatedAt")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            hold_label: Some(
                row.get("labels")
                    .and_then(|v| v.as_array())
                    .is_some_and(|labels| {
                        labels.iter().any(|label| {
                            label.get("name").and_then(|v| v.as_str())
                                == Some(crate::merge_hold::HOLD_LABEL)
                        })
                    }),
            ),
            ci: Some(ci_of_rollup(
                row.get("statusCheckRollup")
                    .unwrap_or(&serde_json::Value::Null),
            )),
        });
    }
    Ok(Some(changes))
}

/// Everything one pass derived, with the facts the snapshot is built from.
pub(crate) struct Pass {
    pub root: PathBuf,
    pub changes: Vec<Pr>,
    pub rows: Vec<Row>,
    /// Live lanes on this clone and, through the synced store, on others.
    pub local_lanes: Vec<crate::leases::SessionLease>,
    pub foreign_lanes: Vec<crate::coordination::Claim>,
    /// The forge keys its changes `MR-N`.
    pub gitlab: bool,
    /// The tree of each clean merge of a head into its base.
    pub merge_trees: BTreeMap<u64, String>,
    /// The record of `aida land run`'s children, settled against what is
    /// alive now. Only `aida land run` writes it back.
    pub book: run::Book,
    /// What settling the record found.
    pub notes: Vec<String>,
    /// What the pass could not read and so does not show.
    pub warnings: Vec<String>,
}

/// The warning for a lease store that could not be read.
pub(crate) fn lanes_unreadable(why: &str) -> String {
    format!(
        "this clone's leases could not be read ({}): live lanes are missing from this view",
        one_line(why, 200)
    )
}

/// Discover the open changes and derive a row for each.
pub(crate) fn live_pass(start: &Path) -> Result<Pass> {
    let root = crate::review_watch::forge::main_root(start)?;
    let repo = crate::review_watch::forge::Repository::load(&root)?;
    let changes: Vec<Pr> = match discover_rich(&root, &repo)? {
        Some(changes) => changes,
        None => {
            // The watcher's discovery carries no title; the specs a change
            // ships are read from it, so each is asked for.
            let forge = crate::forge::forge_for(&root);
            repo.discover(&root)?
                .into_iter()
                .map(|change| {
                    let title = forge
                        .change_title_and_body(change.id)
                        .map(|(title, _)| title)
                        .unwrap_or_default();
                    plain_change(change, title)
                })
                .collect()
        }
    };
    let gitlab = repo.github_slug().is_none();
    let our_clone = root.display().to_string();
    let foreign: Vec<crate::coordination::Claim> =
        crate::coordination::list_claims(&root.join(".aida-store"))
            .into_iter()
            .filter(|claim| claim.clone_path.is_empty() || claim.clone_path != our_clone)
            .collect();
    let (claims, _stale) = crate::coordination::partition_claims(
        foreign,
        chrono::Utc::now(),
        &crate::coordination::hostname(),
    );
    let mut book = run::Book::read(&root)?;
    let notes = book.settle(&changes, &run::alive, &run::log_tail);
    let live = Live {
        book,
        merge_trees: Default::default(),
        pins: fetch(&root, &repo, &changes, gitlab).map_err(|e| format!("{e:#}")),
        gitlab,
        root: root.clone(),
        leases: std::cell::OnceCell::new(),
        claims,
    };
    let rows = pass(&changes, &live);
    // Lanes that could not be read are said to be missing, never shown as
    // "no lanes".
    let (local_lanes, warnings) = match live.local_leases() {
        Ok(leases) => (leases.to_vec(), Vec::new()),
        Err(why) => (Vec::new(), vec![lanes_unreadable(&why)]),
    };
    Ok(Pass {
        root,
        changes,
        rows,
        local_lanes,
        foreign_lanes: live.claims,
        gitlab,
        merge_trees: live.merge_trees.into_inner(),
        book: live.book,
        notes,
        warnings,
    })
}

/// The landing reconciler: every open change's state, derived from the forge,
/// the gate ledger and git on each run, and the one action each state calls
/// for.
// trace:STORY-2768 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "land")]
pub(crate) struct LandArgs {
    #[clap(subcommand)]
    pub action: land_args::LandCommand,
}

fn run(matches: &clap::ArgMatches, ctx: &crate::cmdkit::DispatchCtx) -> Result<()> {
    let args = <LandArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match args.action {
        land_args::LandCommand::Status => {
            let root = crate::find_project_root()?;
            let started = std::time::Instant::now();
            let pass = live_pass(&root)?;
            // Every pass refreshes the snapshot `aida now` renders.
            let snapshot = crate::now::build(&pass, started.elapsed());
            if let Err(error) = crate::now::write(&pass.root, &snapshot) {
                eprintln!("the `aida now` snapshot was not written: {error:#}");
            }
            let rows = pass.rows;
            if ctx.globals.and_then(|g| g.format) == Some(crate::cmdkit::OutputFormat::Json) {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                print!("{}", render(&rows));
            }
            Ok(())
        }
        land_args::LandCommand::Run(args) => run::run(&crate::find_project_root()?, &args),
    }
}

// trace:STORY-2768 | ai:claude
pub(crate) static LAND: CommandReg = CommandReg {
    name: "land",
    help_after: "lane",
    command: <LandArgs as clap::CommandFactory>::command,
    policy: |_| CmdPolicy {
        stakeholder: StakeholderAction::Read,
        per_write_auto_push: false,
        skip_role_restore_prompt: true,
        team_identity_write: false,
    },
    run: Handler::Early(run),
};

#[cfg(test)]
#[path = "tests/story_2768_land_tests.rs"]
mod tests;
