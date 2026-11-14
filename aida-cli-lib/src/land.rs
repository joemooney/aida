//! The landing reconciler: one level-triggered pass that derives every open
//! change's state from facts (the forge, the gate ledger, git) and maps each
//! state to exactly one action. Nothing about a change is stored between
//! passes, so no state can go stale.
//!
//! This slice is read-only: `aida land status` runs the pass and prints the
//! derived table. The acting loop calls the same [`derive`], so the view and
//! the actor cannot disagree.
// trace:STORY-2768 | ai:claude

use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[path = "land_args.rs"]
pub(crate) mod land_args;

use crate::cmdkit::{CmdPolicy, CommandReg, Handler};
use crate::gate::review_verdict::VerdictKind;
use crate::stakeholder_role::StakeholderAction;

/// Content rounds of requested changes after which a change stops being
/// reworked and goes to the advisor.
const MAX_CONTENT_ROUNDS: usize = 3;
/// The branch whose changes the integration seat lands.
const INTEGRATION_BRANCH: &str = "integration";
const TRAIN_PREFIX: &str = "train/";
/// Where a pass keeps the commits it compares; rewritten on every pass.
const REF_PREFIX: &str = "refs/aida/land/";

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
    /// Clear a hold whose release condition is met, then land. Clearing a
    /// hold takes a human at a terminal, so this is the operator's.
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
            Action::DispatchRework | Action::LaunchReview | Action::Train => "reconciler",
            Action::ClearHoldAndLand => "operator",
            Action::ReportToIntegration => "integration seat",
            Action::AdvisorRuling => "advisor",
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        match self {
            Action::Wait => "wait",
            Action::DispatchRework => "dispatch rework",
            Action::LaunchReview => "launch reviewer",
            Action::ClearHoldAndLand => "operator clears met hold, then land",
            Action::Train => "add to next train",
            Action::ReportToIntegration => "report to integration seat",
            Action::AdvisorRuling => "advisor ruling",
        }
    }
}

/// A merge hold and whether the reconciler may release it itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hold {
    pub detail: String,
    /// Its release condition is an approval covering the content (a rework
    /// hold), so an approved change has met it whatever head placed it.
    pub released_by_approval: bool,
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
    fn reviewing(&self, pr: &Pr) -> Option<String>;
    /// CI on the head.
    fn ci(&self, pr: &Pr) -> std::result::Result<Ci, String>;
    /// An approval covering the head's content, described; `Err` is the
    /// gate's refusal.
    fn approval(&self, pr: &Pr) -> std::result::Result<String, String>;
    fn hold(&self, pr: &Pr) -> std::result::Result<Option<Hold>, String>;
    fn rounds(&self, pr: &Pr) -> std::result::Result<Rounds, String>;
    /// A live lane working the change's branch, described.
    fn lane(&self, pr: &Pr) -> std::result::Result<Option<String>, String>;
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
        Ok(None) => row(state, Action::DispatchRework, since, reason),
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
    if let Some(review) = evidence.reviewing(pr) {
        return Row {
            actor: Some(review.clone()),
            ..row(State::Reviewing, Action::Wait, None, review)
        };
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
        Ok(approval) => {
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
                Ok(Some(hold)) if hold.released_by_approval => row(
                    State::Ready,
                    Action::ClearHoldAndLand,
                    None,
                    format!("{approval}; hold met by the approval: {}", hold.detail),
                ),
                Ok(Some(hold)) => row(
                    State::Blocked,
                    Action::AdvisorRuling,
                    None,
                    format!("approved, held: {}", hold.detail),
                ),
                Err(why) => unknown("merge hold", why),
            }
        }
        Err(refusal) => refusal,
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

fn git(root: &Path, args: &[&str], timeout_secs: u64) -> Result<std::process::Output> {
    let mut command = std::process::Command::new("git");
    command
        .current_dir(root)
        .args(args)
        .stdin(std::process::Stdio::null());
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

/// Fetch every change's head and base into this pass's refs in one call, and
/// drop the refs of changes that are no longer open.
fn fetch(root: &Path, changes: &[Pr], gitlab: bool) -> Result<()> {
    use aida_core::git_ops::RemoteCredentialsExt as _;
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
        return Ok(());
    }
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
    command.remote_credentials(root, "origin");
    let out = crate::command_output_with_timeout(command, std::time::Duration::from_secs(120))
        .context("the fetch of open changes failed to run or timed out")?;
    anyhow::ensure!(
        out.status.success(),
        "the fetch of open changes failed: {}",
        one_line(&String::from_utf8_lossy(&out.stderr), 300)
    );
    Ok(())
}

/// The production evidence: the readers the merge gates use, all read-only.
struct Live {
    root: PathBuf,
    /// Set when the pass's fetch failed: every git-backed fact is unknown.
    fetch_error: Option<String>,
    leases: std::cell::OnceCell<std::result::Result<Vec<crate::leases::SessionLease>, String>>,
    /// Live claims from other clones and hosts, read once per pass from the
    /// synced store: never a process probe of another machine.
    claims: Vec<crate::coordination::Claim>,
}

impl Live {
    fn fetched(&self, pr: &Pr) -> std::result::Result<(), String> {
        if let Some(error) = &self.fetch_error {
            return Err(error.clone());
        }
        let at = git_text(&self.root, &["rev-parse", "--verify", &head_ref(pr.id)])
            .map_err(|e| format!("{e:#}"))?;
        if at != pr.head {
            return Err("the head moved during the pass".into());
        }
        Ok(())
    }

    /// The identity of a commit's own diff against the change's base: equal
    /// for two heads that differ only by merges of the base. A commit that
    /// cannot be compared is its own identity.
    fn content_id(&self, pr: &Pr, sha: &str) -> String {
        let base = base_ref(&pr.base);
        crate::gate::review_carryover::single_merge_base(&self.root, &base, sha)
            .and_then(|fork| crate::gate::review_carryover::patch_id(&self.root, &fork, sha))
            .unwrap_or_else(|_| sha.to_string())
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
        self.fetched(pr)?;
        let out = git(
            &self.root,
            &[
                "merge-tree",
                "--write-tree",
                "--no-messages",
                &base_ref(&pr.base),
                &head_ref(pr.id),
            ],
            60,
        )
        .map_err(|e| format!("{e:#}"))?;
        match out.status.code() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(one_line(&String::from_utf8_lossy(&out.stderr), 200)),
        }
    }

    fn reviewing(&self, pr: &Pr) -> Option<String> {
        if let Some(claimed) =
            crate::gate::review_marker::live_description(&self.root, pr.id, Some(&pr.head))
        {
            return Some(claimed);
        }
        // A reviewer that has launched but not yet claimed the head holds a
        // lease scoped to the change. Unreadable leases read as a review in
        // progress: a second reviewer must never be launched on a guess.
        let scope = format!("PR-{}", pr.id);
        match self.local_leases() {
            Ok(leases) => leases
                .iter()
                .find(|lease| lease.scope.eq_ignore_ascii_case(&scope))
                .map(|lease| format!("{} launched, no claim yet", lease_actor(lease)))
                .or_else(|| {
                    self.claims
                        .iter()
                        .find(|claim| claim.scope.eq_ignore_ascii_case(&scope))
                        .map(|claim| format!("{} reviewing", claim_actor(claim)))
                }),
            Err(why) => Some(format!("reviewer leases unreadable ({why})")),
        }
    }

    fn ci(&self, pr: &Pr) -> std::result::Result<Ci, String> {
        pr.ci
            .clone()
            .ok_or_else(|| "discovery did not report checks".to_string())
    }

    fn approval(&self, pr: &Pr) -> std::result::Result<String, String> {
        let root = self.root.as_path();
        let head = pr.head.as_str();
        let (candidates, policy, ids) = crate::gate::approval::merge_approval_subject(
            &[root],
            root,
            pr.id,
            &[],
            pr.branch.as_deref(),
            Some(head),
        );
        let site = crate::gate::review_carryover::CarryOverSite::read_only(root, Some(&pr.base));
        let approval = match crate::gate::approval::approval_decision(
            pr.id,
            &candidates,
            Some(head),
            &policy,
            Some(&site),
        )? {
            crate::gate::approval::HeadDecision::Covered(Some(sha)) => {
                format!("approved at {}", short(&sha))
            }
            crate::gate::approval::HeadDecision::Covered(None) => {
                return Err("no approval covers the head".into())
            }
            crate::gate::approval::HeadDecision::CarriedOver(proof, _) => format!(
                "approved at {}, carried over clean merges of `{}`",
                short(&proof.approved_sha),
                proof.base_branch()
            ),
            crate::gate::approval::HeadDecision::Stale { message, .. } => return Err(message),
        };
        // A blocking verdict the approval does not answer refuses the head.
        match crate::gate::merge_hold_core::corpus_gate_decision(
            root,
            pr.id,
            &ids,
            Some(&pr.base),
            || Some(pr.head.clone()),
            false,
        ) {
            Ok(crate::gate::merge_hold_core::CorpusMergeGate::Clear) => Ok(approval),
            Ok(crate::gate::merge_hold_core::CorpusMergeGate::Refuse { message, .. }) => {
                Err(message)
            }
            Err(error) => Err(format!("{error:#}")),
        }
    }

    fn hold(&self, pr: &Pr) -> std::result::Result<Option<Hold>, String> {
        use crate::gate::merge_hold_core::{read_hold, read_hold_record, HoldReasonKind};
        if let Some(record) = read_hold_record(&self.root, pr.id) {
            return Ok(Some(Hold {
                detail: format!(
                    "{} hold: {}",
                    format!("{:?}", record.reason_kind).to_lowercase(),
                    record.detail
                ),
                released_by_approval: record.reason_kind == HoldReasonKind::Rework,
            }));
        }
        if let Some(detail) = read_hold(&self.root, pr.id) {
            return Ok(Some(Hold {
                detail,
                released_by_approval: false,
            }));
        }
        let labelled = match pr.hold_label {
            Some(labelled) => labelled,
            None => crate::merge_hold::label_present(&self.root, pr.id)?,
        };
        Ok(labelled.then(|| Hold {
            detail: "the merge-hold label is set on the forge".into(),
            released_by_approval: false,
        }))
    }

    fn rounds(&self, pr: &Pr) -> std::result::Result<Rounds, String> {
        self.fetched(pr)?;
        let current = self.content_id(pr, &pr.head);
        let mut contents: BTreeSet<String> = BTreeSet::new();
        let mut rounds = Rounds::default();
        for record in
            crate::gate::ledger::project::verdict_records(&self.root, &format!("PR-{}", pr.id))
        {
            let key = crate::gate::review_verdict::recording_key(&record);
            if !VerdictKind::parse(&key.2).requires_changes() {
                continue;
            }
            let Some(sha) = key.0 else { continue };
            let content = self.content_id(pr, &sha);
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
    let live = Live {
        fetch_error: fetch(&root, &changes, gitlab)
            .err()
            .map(|e| format!("{e:#}")),
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
