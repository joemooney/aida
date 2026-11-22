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

#[path = "land_housekeeping.rs"]
pub(crate) mod housekeeping;
#[path = "land_args.rs"]
pub(crate) mod land_args;
#[path = "land_run.rs"]
pub(crate) mod run;

#[path = "land_ci_base.rs"]
mod ci_base;

#[path = "land_metrics.rs"]
pub(crate) mod metrics;

// trace:STORY-2906 | ai:claude
#[path = "land_stalls.rs"]
pub(crate) mod stalls;

use crate::cmdkit::{CmdPolicy, CommandReg, Handler};
use crate::gate::review_verdict::VerdictKind;
use crate::stakeholder_role::StakeholderAction;

/// Content rounds of requested changes after which a change stops being
/// reworked and goes to the advisor.
const MAX_CONTENT_ROUNDS: usize = 3;
/// The branch whose changes the integration seat lands.
pub(crate) const INTEGRATION_BRANCH: &str = "integration";
/// The branch trains land on.
const MAIN_BRANCH: &str = "main";
const TRAIN_PREFIX: &str = "train/";
/// A promotion of integration to main. It is reviewed and boards a train
/// like any change, and no lane ever reworks it.
const PROMOTE_PREFIX: &str = "promote/";
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
    StaleBase(String),
    /// The failed checks, named.
    Red(String),
}

impl Ci {
    fn label(&self) -> String {
        match self {
            Ci::None => "none".into(),
            Ci::Pending => "pending".into(),
            Ci::Green => "green".into(),
            Ci::StaleBase(_) => "stale-base".into(),
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
    StaleCiBase,
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
            State::StaleCiBase => "STALE-CI-BASE",
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
    UpdateBranch,
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
            | Action::UpdateBranch
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
            Action::UpdateBranch => "update branch from base",
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
    /// The failed checks at the exact head, named, whatever base the run
    /// tested: `ci` answers stale or unknown when the base moved, and that
    /// answer hides a red head. `None` when no check failed there.
    // trace:STORY-2906 | ai:claude
    fn head_red(&self, pr: &Pr) -> std::result::Result<Option<String>, String> {
        Ok(match self.ci(pr)? {
            Ci::Red(names) => Some(names),
            _ => None,
        })
    }
    /// Whether red CI on the head blocks landing: true in `ci-gated` mode;
    /// in `review-gated` mode pre-merge CI is informational and red CI
    /// never turns an approved head into rework on its own.
    // trace:STORY-2897 | ai:claude
    fn ci_gates_landing(&self) -> bool {
        true
    }
    /// Whether an approval covers the head's content. `Err` only when the
    /// evidence could not be read: a refusal is `Ok`.
    fn approval(&self, pr: &Pr) -> std::result::Result<Approval, String>;
    fn hold(&self, pr: &Pr) -> std::result::Result<Option<Hold>, String>;
    fn rounds(&self, pr: &Pr) -> std::result::Result<Rounds, String>;
    /// A live lane working the change's branch, described.
    fn lane(&self, pr: &Pr) -> std::result::Result<Option<String>, String>;
    /// [`Evidence::lane`], leaving out a claim another host recorded. This
    /// host can probe its own lanes; what another host's record says is
    /// judged in one place ([`crate::lane_hosts`]), which also knows when
    /// the answer is "unknown".
    // trace:TASK-3827 | ai:claude
    fn lane_on_this_host(&self, pr: &Pr) -> std::result::Result<Option<String>, String> {
        self.lane(pr)
    }
    /// How many reviewers of this head were stopped for running too long.
    fn review_timeouts(&self, _pr: &Pr) -> u32 {
        0
    }
    /// Why no rework lane may be launched for this head, when one refused
    /// in a way a relaunch cannot fix.
    fn rework_stop(&self, _pr: &Pr) -> Option<String> {
        None
    }
    /// For an unapproved integration-track change: whether it may merge
    /// before its review, and why. `None` when that path is switched off.
    // trace:STORY-2772 | ai:claude
    fn review_after_merge(&self, _pr: &Pr) -> Option<String> {
        None
    }
    /// When the head was committed (RFC 3339): the throughput guard counts
    /// an unreviewed head's wait from it. `None` when it could not be read.
    // trace:STORY-2906 | ai:claude
    fn head_at(&self, _pr: &Pr) -> Option<String> {
        None
    }
    /// When the approval that covers the head was recorded.
    // trace:STORY-2906 | ai:claude
    fn approved_at(&self, _pr: &Pr) -> Option<String> {
        None
    }
    /// When the base first moved to a commit the head conflicts with.
    // trace:STORY-2906 | ai:claude
    fn conflict_since(&self, _pr: &Pr) -> Option<String> {
        None
    }
}

/// One pass's evidence with each fact read once per change: the derivation
/// and a second reader of the same pass (the throughput guard) share the
/// reads, and so the same answers.
// trace:STORY-2906 | ai:claude
struct Memo<'a> {
    inner: &'a dyn Evidence,
    conflict: MemoCell<bool>,
    reviewing: MemoCell<Option<String>>,
    ci: MemoCell<Ci>,
    approval: MemoCell<Approval>,
    hold: MemoCell<Option<Hold>>,
    rounds: MemoCell<Rounds>,
    lane: MemoCell<Option<String>>,
}

type MemoCell<T> = std::cell::RefCell<BTreeMap<u64, std::result::Result<T, String>>>;

fn memo<T: Clone>(
    cell: &MemoCell<T>,
    id: u64,
    read: impl FnOnce() -> std::result::Result<T, String>,
) -> std::result::Result<T, String> {
    if let Some(known) = cell.borrow().get(&id) {
        return known.clone();
    }
    let value = read();
    cell.borrow_mut().insert(id, value.clone());
    value
}

impl<'a> Memo<'a> {
    fn new(inner: &'a dyn Evidence) -> Self {
        Self {
            inner,
            conflict: Default::default(),
            reviewing: Default::default(),
            ci: Default::default(),
            approval: Default::default(),
            hold: Default::default(),
            rounds: Default::default(),
            lane: Default::default(),
        }
    }
}

impl Evidence for Memo<'_> {
    fn conflict(&self, pr: &Pr) -> std::result::Result<bool, String> {
        memo(&self.conflict, pr.id, || self.inner.conflict(pr))
    }
    fn reviewing(&self, pr: &Pr) -> std::result::Result<Option<String>, String> {
        memo(&self.reviewing, pr.id, || self.inner.reviewing(pr))
    }
    fn ci(&self, pr: &Pr) -> std::result::Result<Ci, String> {
        memo(&self.ci, pr.id, || self.inner.ci(pr))
    }
    fn head_red(&self, pr: &Pr) -> std::result::Result<Option<String>, String> {
        self.inner.head_red(pr)
    }
    fn ci_gates_landing(&self) -> bool {
        self.inner.ci_gates_landing()
    }
    fn approval(&self, pr: &Pr) -> std::result::Result<Approval, String> {
        memo(&self.approval, pr.id, || self.inner.approval(pr))
    }
    fn hold(&self, pr: &Pr) -> std::result::Result<Option<Hold>, String> {
        memo(&self.hold, pr.id, || self.inner.hold(pr))
    }
    fn rounds(&self, pr: &Pr) -> std::result::Result<Rounds, String> {
        memo(&self.rounds, pr.id, || self.inner.rounds(pr))
    }
    fn lane(&self, pr: &Pr) -> std::result::Result<Option<String>, String> {
        memo(&self.lane, pr.id, || self.inner.lane(pr))
    }
    fn lane_on_this_host(&self, pr: &Pr) -> std::result::Result<Option<String>, String> {
        self.inner.lane_on_this_host(pr)
    }
    fn review_timeouts(&self, pr: &Pr) -> u32 {
        self.inner.review_timeouts(pr)
    }
    fn rework_stop(&self, pr: &Pr) -> Option<String> {
        self.inner.rework_stop(pr)
    }
    fn review_after_merge(&self, pr: &Pr) -> Option<String> {
        self.inner.review_after_merge(pr)
    }
    fn head_at(&self, pr: &Pr) -> Option<String> {
        self.inner.head_at(pr)
    }
    fn approved_at(&self, pr: &Pr) -> Option<String> {
        self.inner.approved_at(pr)
    }
    fn conflict_since(&self, pr: &Pr) -> Option<String> {
        self.inner.conflict_since(pr)
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
    // A promotion is cut from integration: a lane that reworked it would
    // move its head off the commit that was checked. The advisor cuts it
    // again.
    // trace:STORY-2768 | ai:claude
    let promotion = branch.starts_with(PROMOTE_PREFIX);
    // Rework goes to a lane only when none is already on the branch.
    let rework = |state: State, since: Option<String>, reason: String| match evidence.lane(pr) {
        _ if promotion => row(
            State::Blocked,
            Action::AdvisorRuling,
            since,
            format!(
                "{reason}; a promotion is never reworked by a lane: cut it again from integration"
            ),
        ),
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
    // trace:BUG-2690 | ai:codex
    if let Ci::StaleBase(base) = &ci {
        let update = !promotion
            && matches!(evidence.lane(pr), Ok(None))
            && matches!(evidence.hold(pr), Ok(None));
        return row(
            State::StaleCiBase,
            if update {
                Action::UpdateBranch
            } else {
                Action::Wait
            },
            None,
            format!("{base}. Refresh CI with a clean update-branch"),
        );
    }
    if let (Ci::Red(failed), true) = (&ci, evidence.ci_gates_landing()) {
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
                // A change stacked on another change's branch is reviewed
                // like any other, and lands only once its base has: a train
                // merges into the main branch and nowhere else.
                Ok(None) if pr.base != MAIN_BRANCH && pr.base != INTEGRATION_BRANCH => row(
                    State::Ready,
                    Action::Wait,
                    None,
                    format!(
                        "{approval}; stacked on `{}`: retarget it when that lands",
                        pr.base
                    ),
                ),
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
    let reason = match (track, evidence.review_after_merge(pr)) {
        (Track::Integration, Some(note)) => format!("{refusal}; {note}"),
        _ => refusal,
    };
    row(State::NeedsReview, Action::LaunchReview, None, reason)
}

/// One pass over `changes`: a row each, in change order.
pub(crate) fn pass(changes: &[Pr], evidence: &dyn Evidence) -> Vec<Row> {
    let mut rows: Vec<Row> = changes.iter().map(|pr| derive(pr, evidence)).collect();
    rows.sort_by_key(|row| row.pr);
    rows
}

/// The change `pr` and its own row. [`pass`] sorts rows by PR number while
/// discovery keeps the forge's order, so the two lists are joined by id,
/// never by position: another change's row would admit this one.
// trace:STORY-2772 | ai:claude
pub(crate) fn change_and_row<'a>(
    changes: &'a [Pr],
    rows: &'a [Row],
    pr: u64,
) -> Option<(&'a Pr, &'a Row)> {
    let change = changes.iter().find(|change| change.id == pr)?;
    let row = rows.iter().find(|row| row.pr == pr)?;
    Some((change, row))
}

pub(crate) fn one_line(text: &str, limit: usize) -> String {
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

/// A git command in `root` that contacts origin: routing cleared first,
/// then origin's credentials passed through the environment.
fn origin_command(root: &Path, args: &[&str]) -> std::process::Command {
    use aida_core::git_ops::RemoteCredentialsExt as _;
    let mut command = std::process::Command::new("git");
    command
        .current_dir(root)
        .args(args)
        .stdin(std::process::Stdio::null());
    crate::review_watch::forge::clear_routing(&mut command);
    command.remote_credentials(root, "origin");
    command
}

/// The output of a git command that contacts origin, bounded by
/// `timeout_secs`. A command that fails, or does not finish, is an error.
// trace:STORY-2768 | ai:claude
fn origin_git(root: &Path, args: &[&str], timeout_secs: u64) -> Result<String> {
    let out = crate::command_output_with_timeout(
        origin_command(root, args),
        std::time::Duration::from_secs(timeout_secs),
    )
    .with_context(|| format!("`git {}` failed to run or timed out", args.join(" ")))?;
    anyhow::ensure!(
        out.status.success(),
        "`git {}` failed: {}",
        args.join(" "),
        one_line(&String::from_utf8_lossy(&out.stderr), 300)
    );
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
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

/// The paths `head` changes since it forked from `base`. Renames are not
/// detected: a file moved out of a protected path is listed at both its old
/// and its new place, so the move cannot hide it.
// trace:STORY-2772 | ai:claude
pub(crate) fn changed_paths(
    root: &Path,
    base: &str,
    head: &str,
) -> std::result::Result<Vec<String>, String> {
    let range = format!("{base}...{head}");
    let args = ["diff", "--name-only", "--no-renames", "-z", &range];
    let out = git(root, &args, 30).map_err(|error| format!("{error:#}"))?;
    if !out.status.success() {
        return Err(format!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    nul_separated_paths(&out.stdout)
}

/// Paths from `-z` output, exactly as git stores them: no quoting, no
/// trimming, a newline or tab inside a name kept. A name that is not UTF-8
/// cannot be matched against a protected prefix and is refused, never
/// guessed at.
// trace:STORY-2772 | ai:claude
pub(crate) fn nul_separated_paths(out: &[u8]) -> std::result::Result<Vec<String>, String> {
    out.split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .map(|name| {
            String::from_utf8(name.to_vec()).map_err(|_| {
                format!(
                    "a changed path is not UTF-8 ({}); it cannot be checked against the protected paths",
                    String::from_utf8_lossy(name)
                )
            })
        })
        .collect()
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
    let mut command = origin_command(
        root,
        &[
            "fetch",
            "--no-write-fetch-head",
            "--no-tags",
            "--",
            "origin",
        ],
    );
    command.args(&specs);
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
    repo: crate::review_watch::forge::Repository,
    github_checks: GitHubChecks,
    leases: std::cell::OnceCell<std::result::Result<Vec<crate::leases::SessionLease>, String>>,
    /// Live claims from other clones and hosts, read once per pass from the
    /// synced store: never a process probe of another machine.
    claims: Vec<crate::coordination::Claim>,
    /// The children `aida land run` started that are still alive, and what
    /// it recorded about the ones it stopped.
    book: run::Book,
    /// The tree each clean merge of a head into its base produced.
    merge_trees: std::cell::RefCell<BTreeMap<u64, String>>,
    /// What review-after-merge eligibility reads, once per pass.
    review_after_merge: crate::integration_cmd::review::Reader,
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

    /// A commit's committer date, RFC 3339.
    // trace:STORY-2906 | ai:claude
    fn commit_time(&self, sha: &str) -> Option<String> {
        let out = git(&self.root, &["log", "-1", "--format=%cI", sha], 10)
            .ok()
            .filter(|out| out.status.success())?;
        let at = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!at.is_empty()).then_some(at)
    }

    /// A live lane on the change: a child of `aida land run`, a lease of
    /// this clone, or another clone's claim that `counts` accepts.
    // trace:TASK-3827 | ai:claude
    fn lane_among(
        &self,
        pr: &Pr,
        counts: impl Fn(&crate::coordination::Claim) -> bool,
    ) -> std::result::Result<Option<String>, String> {
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
            .find(|claim| counts(claim) && claim_covers(&claim.scope, pr))
            .map(claim_actor))
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

/// Whether a claim's scope names one of the change's specs, or its branch.
/// A lane's branch is its spec's canonical ID, alone or followed by a
/// delimiter and more (`task-70-work`). The scope has to match that whole
/// ID: `TASK-7` does not cover `task-70-work`, and `TASK-70` does.
// trace:TASK-3827 | ai:claude
pub(crate) fn claim_covers(scope: &str, pr: &Pr) -> bool {
    let scope = scope.trim();
    if scope.is_empty() {
        return false;
    }
    if pr
        .branch
        .as_deref()
        .is_some_and(|branch| branch_names(branch, scope))
    {
        return true;
    }
    specs_of(pr)
        .iter()
        .any(|id| aida_core::id_identity::same_id(id, scope))
}

/// `branch` is `scope`, or starts with `scope` where `scope` is a whole ID
/// (`PREFIX-…`, not a bare prefix such as `task`) and the next character is
/// one no ID continues with. Compared without regard to ASCII case.
fn branch_names(branch: &str, scope: &str) -> bool {
    let (branch, scope) = (branch.as_bytes(), scope.as_bytes());
    if branch.len() < scope.len() || !branch[..scope.len()].eq_ignore_ascii_case(scope) {
        return false;
    }
    match branch.get(scope.len()) {
        None => true,
        Some(next) => {
            !next.is_ascii_alphanumeric()
                && scope.contains(&b'-')
                && scope.last().is_some_and(u8::is_ascii_alphanumeric)
        }
    }
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

/// The merge hold armed on `pr`, with its release condition, read-only.
pub(crate) fn hold_of(root: &Path, pr: &Pr) -> std::result::Result<Option<Hold>, String> {
    use crate::gate::merge_hold_core::{hold_placed_at, read_hold, read_hold_record};
    use crate::gate::rework_hold_release::{release_condition, ReleaseCondition};
    let since = || {
        hold_placed_at(root, pr.id).map(|at| chrono::DateTime::<chrono::Utc>::from(at).to_rfc3339())
    };
    let human_only = |what: &str| HoldRelease::Unmet(format!("{what}; a human clears it"));
    if let Some(record) = read_hold_record(root, pr.id) {
        let detail = format!(
            "{} hold: {}",
            format!("{:?}", record.reason_kind).to_lowercase(),
            record.detail
        );
        // The release's own decision, read-only: the hold's kind alone
        // says nothing about composed holds, sensitive specs or grants.
        let release = match release_condition(root, pr.id) {
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
    if let Some(detail) = read_hold(root, pr.id) {
        return Ok(Some(Hold {
            detail,
            release: human_only("an untyped hold has no release condition"),
            since: since(),
        }));
    }
    let labelled = match pr.hold_label {
        Some(labelled) => labelled,
        None => crate::merge_hold::label_present(root, pr.id)?,
    };
    Ok(labelled.then(|| Hold {
        detail: "the merge-hold label is set on the forge".into(),
        release: human_only("the label has no recorded hold behind it"),
        since: None,
    }))
}

// trace:TASK-3827 | ai:codex
pub(super) fn reviewer_key(gitlab: bool, number: u64) -> String {
    format!("{}-{number}", if gitlab { "MR" } else { "PR" })
}

impl Evidence for Live {
    // trace:STORY-2897 | ai:claude
    fn ci_gates_landing(&self) -> bool {
        crate::landing_policy::load(&self.root).waits_for_ci()
    }
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
        let scope = reviewer_key(self.gitlab, pr.id);
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
        if let Some(rollup) = self.github_checks.get(&pr.id) {
            let (_, base) = self.pinned(pr)?;
            return ci_base::observe(&self.root, &self.repo, rollup, pr, base);
        }
        pr.ci
            .clone()
            .ok_or_else(|| "discovery did not report checks".to_string())
    }

    // trace:STORY-2906 | ai:claude
    fn head_red(&self, pr: &Pr) -> std::result::Result<Option<String>, String> {
        let ci = match self.github_checks.get(&pr.id) {
            Some(rollup) => ci_base::head_ci(&self.root, &self.repo, rollup, pr)?,
            None => pr
                .ci
                .clone()
                .ok_or_else(|| "discovery did not report checks".to_string())?,
        };
        Ok(match ci {
            Ci::Red(names) => Some(names),
            _ => None,
        })
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
        hold_of(&self.root, pr)
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
        self.lane_among(pr, |_| true)
    }

    // trace:TASK-3827 | ai:claude
    fn lane_on_this_host(&self, pr: &Pr) -> std::result::Result<Option<String>, String> {
        let here = crate::coordination::hostname();
        self.lane_among(pr, |claim| claim.host == here)
    }

    fn review_timeouts(&self, pr: &Pr) -> u32 {
        self.book.timeouts(pr)
    }

    fn review_after_merge(&self, pr: &Pr) -> Option<String> {
        use crate::integration_cmd::review::{Eligibility, Mode};
        let reader = &self.review_after_merge;
        match reader.config() {
            Ok(config) if config.mode == Mode::Off => return None,
            Ok(_) => {}
            Err(why) => return Some(format!("review after merge: unknown ({why})")),
        }
        let verdict = self
            .pinned(pr)
            .and_then(|(head, base)| changed_paths(&self.root, base, head))
            .and_then(|paths| reader.eligibility(&specs_of(pr), &paths));
        Some(match verdict {
            Ok(Eligibility::Eligible(why)) => format!("may merge before review: {why}"),
            Ok(Eligibility::Ineligible(why)) => format!("review before merge: {why}"),
            Err(why) => format!("review after merge: unknown ({why})"),
        })
    }

    fn rework_stop(&self, pr: &Pr) -> Option<String> {
        self.book.stop(pr)
    }

    // trace:STORY-2906 | ai:claude
    fn head_at(&self, pr: &Pr) -> Option<String> {
        let (head, _) = self.pinned(pr).ok()?;
        self.commit_time(head)
    }

    // trace:STORY-2906 | ai:claude
    fn approved_at(&self, pr: &Pr) -> Option<String> {
        let change = crate::gate::ledger::ChangeKey {
            forge: if self.gitlab {
                crate::gate::ledger::ChangeForge::Mr
            } else {
                crate::gate::ledger::ChangeForge::Pr
            },
            number: pr.id,
        };
        // The newest approval the ledger took at the head; for an approval
        // carried over clean merges of the base, the newest approval of the
        // change. The ledger's own time: every event has one.
        let mut at_head: Option<String> = None;
        let mut any: Option<String> = None;
        for (taken, record) in
            crate::gate::ledger::project::change_verdicts_checked(&self.root, change).ok()?
        {
            let key = crate::gate::review_verdict::recording_key(&record);
            if !VerdictKind::parse(&key.2).approves() {
                continue;
            }
            let at = Some(taken);
            if key.0.as_deref() == Some(pr.head.as_str()) && at > at_head {
                at_head = at.clone();
            }
            if at > any {
                any = at;
            }
        }
        at_head.or(any)
    }

    // trace:STORY-2906 | ai:claude
    fn conflict_since(&self, pr: &Pr) -> Option<String> {
        let (head, base) = self.pinned(pr).ok()?;
        let fork = crate::gate::review_carryover::single_merge_base(&self.root, base, head).ok()?;
        let range = format!("{fork}..{base}");
        let out = git(
            &self.root,
            &[
                "log",
                "--first-parent",
                "--reverse",
                "--format=%H %cI",
                &range,
            ],
            20,
        )
        .ok()
        .filter(|out| out.status.success())?;
        let text = String::from_utf8_lossy(&out.stdout);
        let commits: Vec<(&str, &str)> = text
            .lines()
            .filter_map(|line| line.trim().split_once(' '))
            .collect();
        let conflicts = |sha: &str| -> Option<bool> {
            let out = git(
                &self.root,
                &["merge-tree", "--write-tree", "--no-messages", sha, head],
                20,
            )
            .ok()?;
            match out.status.code() {
                Some(0) => Some(false),
                Some(1) => Some(true),
                _ => None,
            }
        };
        // The base's tip conflicts (the pass found that). The first base
        // commit that does is searched for by halving: a conflict, once
        // there, is taken to stay.
        let (mut low, mut high) = (0, commits.len().checked_sub(1)?);
        while low < high {
            let mid = (low + high) / 2;
            if conflicts(commits[mid].0)? {
                high = mid;
            } else {
                low = mid + 1;
            }
        }
        Some(commits[low].1.to_string())
    }
}

fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

/// CI summed over a GitHub `statusCheckRollup`.
#[cfg(test)]
fn ci_of_rollup(rollup: &serde_json::Value) -> Ci {
    ci_of_checks(rollup.as_array().into_iter().flatten())
}

fn ci_of_checks<'a>(checks: impl IntoIterator<Item = &'a serde_json::Value>) -> Ci {
    let newest = newest_checks(checks);
    if newest.is_empty() {
        return Ci::None;
    }
    let mut failed: Vec<String> = Vec::new();
    let mut pending = false;
    for check in newest {
        let name = check_name(check);
        // A check run reports `status` + `conclusion`; a commit status
        // reports `state`.
        let outcome = match check_text(check, "conclusion") {
            done if !done.is_empty() => done,
            _ => check_text(check, "state"),
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

fn check_text(check: &serde_json::Value, key: &str) -> String {
    check
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_ascii_uppercase()
}

fn check_name(check: &serde_json::Value) -> String {
    check
        .get("name")
        .or_else(|| check.get("context"))
        .and_then(|v| v.as_str())
        .unwrap_or("check")
        .to_string()
}

fn newest_checks<'a>(
    checks: impl IntoIterator<Item = &'a serde_json::Value>,
) -> Vec<&'a serde_json::Value> {
    // A check that ran more than once at this head counts once, by its
    // newest run: a cancelled run that a later run replaced says nothing.
    let mut newest: BTreeMap<(String, String), &serde_json::Value> = BTreeMap::new();
    for check in checks {
        let key = (check_text(check, "workflowName"), check_name(check));
        let started = |check: &serde_json::Value| check_text(check, "startedAt");
        match newest.get(&key) {
            Some(kept) if started(kept) > started(check) => {}
            _ => {
                newest.insert(key, check);
            }
        }
    }
    newest.into_values().collect()
}

// trace:BUG-2692 | ai:codex
#[cfg(test)]
fn ci_of_github_rollup(
    rollup: &serde_json::Value,
    head: &str,
    metadata: impl FnMut(u64) -> Result<serde_json::Value>,
) -> Ci {
    summarize_github_rollup(rollup, head, metadata).0
}

fn summarize_github_rollup<'a>(
    rollup: &'a serde_json::Value,
    head: &str,
    metadata: impl FnMut(u64) -> Result<serde_json::Value>,
) -> (Ci, Vec<&'a serde_json::Value>) {
    let Some(rows) = rollup.as_array() else {
        return (Ci::None, Vec::new());
    };
    let run_id = |row: &serde_json::Value| {
        crate::ci_gate::run_id_from_check_link(row["detailsUrl"].as_str()?)
    };
    let candidates: Vec<_> = rows
        .iter()
        .filter_map(|row| Some((row["name"].as_str()?, run_id(row)?)))
        .collect();
    let replaced = crate::forge::replaced_fast_runs(
        candidates.iter().map(|(name, id)| (*name, id.as_str())),
        Some(head),
        metadata,
    );
    let kept: Vec<_> = rows
        .iter()
        .filter(|row| !crate::forge::is_replaced_run(&replaced, run_id(row).as_deref()))
        .collect();
    let aggregate_passed = replaced.values().all(|replacement| {
        kept.iter().any(|row| {
            row["name"] == crate::forge::CI_AGGREGATOR_CHECK
                && row["conclusion"] == "SUCCESS"
                && run_id(row).and_then(|id| id.parse::<u64>().ok()) == Some(*replacement)
        })
    });
    let mut ci = ci_of_checks(kept.iter().copied());
    if ci == Ci::Green && !aggregate_passed {
        ci = Ci::Pending;
    }
    (ci, kept)
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
type GitHubChecks = BTreeMap<u64, serde_json::Value>;

fn discover_rich(
    root: &Path,
    repo: &crate::review_watch::forge::Repository,
) -> Result<Option<(Vec<Pr>, GitHubChecks)>> {
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
    let mut checks = BTreeMap::new();
    let mut changes = Vec::new();
    for mut row in rows {
        if row.get("isDraft").and_then(|v| v.as_bool()) == Some(true) {
            continue;
        }
        let head = text(&row, "headRefOid")?;
        anyhow::ensure!(
            head.len() == 40 && head.bytes().all(|c| c.is_ascii_hexdigit()),
            "forge listing row has an invalid head"
        );
        let id = row["number"]
            .as_u64()
            .context("forge listing row has no number")?;
        checks.insert(id, row["statusCheckRollup"].take());
        changes.push(Pr {
            id,
            head,
            base: text(&row, "baseRefName")?,
            branch: Some(text(&row, "headRefName")?),
            title: text(&row, "title")?,
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
            ci: None,
        });
    }
    Ok(Some((changes, checks)))
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

/// How many open changes the forge lists now, on every base, drafts left
/// out: the number a pass would show, without the pass.
// trace:STORY-2768 | ai:claude
pub(crate) fn open_change_count(start: &Path) -> Result<usize> {
    let root = crate::review_watch::forge::main_root(start)?;
    let repo = crate::review_watch::forge::Repository::load(&root)?;
    Ok(match discover_rich(&root, &repo)? {
        Some((changes, _)) => changes.len(),
        None => repo.discover(&root)?.len(),
    })
}

/// Discover the open changes and derive a row for each.
pub(crate) fn live_pass(start: &Path) -> Result<Pass> {
    live_pass_with(start, |_, _, _| ()).map(|(pass, ())| pass)
}

/// [`live_pass`], with a second reader of the same pass: `also` sees the
/// changes, their rows and the evidence the rows were derived from, each
/// fact read once.
// trace:STORY-2906 | ai:claude
pub(crate) fn live_pass_with<T>(
    start: &Path,
    also: impl FnOnce(&[Pr], &[Row], &dyn Evidence) -> T,
) -> Result<(Pass, T)> {
    // Every change's approval reads verdicts under several keys, and each
    // read used to rediscover the whole gate ledger: minutes for twenty open
    // changes. One discovery is shared for the pass; it is re-validated
    // against the ledger before each reuse, and every key's events are still
    // read fresh.
    // trace:STORY-2768 | ai:claude
    let _discovery = crate::gate::ledger::checked::DiscoveryScope::enter();
    let root = crate::review_watch::forge::main_root(start)?;
    let repo = crate::review_watch::forge::Repository::load(&root)?;
    let (changes, github_checks) = match discover_rich(&root, &repo)? {
        Some(discovered) => discovered,
        None => {
            // The watcher's discovery carries no title; the specs a change
            // ships are read from it, so each is asked for.
            let forge = crate::forge::forge_for(&root);
            let changes = repo
                .discover(&root)?
                .into_iter()
                .map(|change| {
                    let title = forge
                        .change_title_and_body(change.id)
                        .map(|(title, _)| title)
                        .unwrap_or_default();
                    plain_change(change, title)
                })
                .collect();
            (changes, BTreeMap::new())
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
        repo,
        github_checks,
        root: root.clone(),
        leases: std::cell::OnceCell::new(),
        claims,
        review_after_merge: crate::integration_cmd::review::Reader::new(&root),
    };
    // trace:STORY-2906 | ai:claude
    let (rows, extra) = {
        let memo = Memo::new(&live);
        let rows = pass(&changes, &memo);
        let extra = also(&changes, &rows, &memo);
        (rows, extra)
    };
    // trace:TASK-3813 | ai:codex
    metrics::record(&root, &changes, &rows, chrono::Utc::now())?;
    // Lanes that could not be read are said to be missing, never shown as
    // "no lanes".
    let (local_lanes, warnings) = match live.local_leases() {
        Ok(leases) => (leases.to_vec(), Vec::new()),
        Err(why) => (Vec::new(), vec![lanes_unreadable(&why)]),
    };
    Ok((
        Pass {
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
        },
        extra,
    ))
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
        land_args::LandCommand::Metrics { since, until, json } => metrics::run(
            &crate::find_project_root()?,
            &since,
            until.as_deref(),
            json || ctx.globals.and_then(|g| g.format) == Some(crate::cmdkit::OutputFormat::Json),
        ),
        land_args::LandCommand::Run(args) => run::run(&crate::find_project_root()?, &args),
        // trace:STORY-2906 | ai:claude
        land_args::LandCommand::Stalls { json } => stalls::run(
            &crate::find_project_root()?,
            json || ctx.globals.and_then(|g| g.format) == Some(crate::cmdkit::OutputFormat::Json),
        ),
        // trace:STORY-2897 | ai:claude
        land_args::LandCommand::Mode { action } => {
            let root = crate::find_project_root()?;
            match action {
                land_args::ModeCommand::Status => crate::land_mode::status(&root),
                land_args::ModeCommand::Set { mode } => crate::land_mode::set(
                    &root,
                    crate::landing_policy::LandingMode::parse(&mode).map_err(anyhow::Error::msg)?,
                ),
                land_args::ModeCommand::Reconcile => crate::land_mode::reconcile_effective(&root),
            }
        }
    }
}

/// `land mode set|reconcile` change branch protection and config: writes.
/// Every other verb reads. An unparseable invocation is a write (fail
/// closed). trace:STORY-2897 | ai:claude
fn land_policy(matches: &clap::ArgMatches) -> CmdPolicy {
    let action = <LandArgs as clap::FromArgMatches>::from_arg_matches(matches)
        .ok()
        .map(|args| args.action);
    let read = matches!(
        action,
        Some(
            land_args::LandCommand::Status
                | land_args::LandCommand::Metrics { .. }
                | land_args::LandCommand::Run(_)
                | land_args::LandCommand::Stalls { .. }
                | land_args::LandCommand::Mode {
                    action: land_args::ModeCommand::Status
                }
        )
    );
    CmdPolicy {
        stakeholder: if read {
            StakeholderAction::Read
        } else {
            StakeholderAction::Write
        },
        per_write_auto_push: false,
        skip_role_restore_prompt: true,
        team_identity_write: false,
    }
}

// trace:STORY-2768 | ai:claude
pub(crate) static LAND: CommandReg = CommandReg {
    name: "land",
    help_after: "lane",
    command: <LandArgs as clap::CommandFactory>::command,
    policy: land_policy,
    run: Handler::Early(run),
};

#[cfg(test)]
#[path = "tests/story_2768_land_tests.rs"]
mod tests;
