//! Ship readiness: a report-only verdict per reviewed change and a once-only
//! advisor wake. It reads the same evidence `aida pr ship` checks and never
//! acts on it: no merge, push or branch update happens here.
// trace:TASK-1945 | ai:claude
use super::*;

/// Wakes of one kind for a head reach the advisor once.
pub(super) const WAKE_READY: &str = "ready";
pub(super) const WAKE_AFTER_UPDATE: &str = "ready-after-update-branch";

/// A change's readiness to ship. Recomputed on every evaluation from the
/// forge, the verdicts and the holds; stored only for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Readiness {
    Ready,
    /// Behind its base only on paths it does not touch: ready once the
    /// branch is updated, which the watcher never does itself.
    ReadyAfterUpdateBranch {
        behind: u64,
    },
    NotReady(String),
}

impl Readiness {
    pub(super) fn line(&self, change: &forge::Change) -> String {
        match self {
            Readiness::Ready => "ready".into(),
            Readiness::ReadyAfterUpdateBranch { behind } => format!(
                "ready after its branch is updated ({behind} commit(s) behind `{}`, no overlapping paths): {}",
                change.base,
                update_command(change)
            ),
            Readiness::NotReady(reason) => format!("not ready: {reason}"),
        }
    }
    pub(super) fn wake(&self) -> Option<&'static str> {
        match self {
            Readiness::Ready => Some(WAKE_READY),
            Readiness::ReadyAfterUpdateBranch { .. } => Some(WAKE_AFTER_UPDATE),
            Readiness::NotReady(_) => None,
        }
    }
}

fn update_command(change: &forge::Change) -> String {
    if change.gitlab {
        format!("`glab mr rebase {}`", change.id)
    } else {
        format!("`gh pr update-branch {}`", change.id)
    }
}

/// The approval a ready change ships on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Approval {
    /// The commit the approval reviewed.
    pub reviewed: String,
    /// Who recorded it and when: with the head and CI commit, it keys the
    /// wake, so a new approval of the same head is a new wake.
    pub identity: String,
    /// The TASK-1960 carry-over that extends it to the head, when it moved.
    pub carried: Option<String>,
}

/// Every input readiness reads, behind one seam so tests inject snapshots.
/// Each read fails closed: an error is reported as not ready, never clean.
pub(super) trait Evidence {
    /// An authorized, non-author approval covering the head (a clean base
    /// merge may carry it over), and a verdict corpus that does not refuse
    /// the head. `Err` names the refusal.
    fn approval(&self, change: &forge::Change) -> std::result::Result<Approval, String>;
    /// A live review marker covering the head, described.
    fn under_review(&self, change: &forge::Change) -> Option<String>;
    /// Any merge hold (marker or label, of any kind), described. An
    /// unreadable hold is `Err`.
    fn hold(&self, change: &forge::Change) -> Result<Option<String>>;
    /// Every CI check on `sha`.
    fn ci(&self, sha: &str) -> Result<crate::forge::CommitChecks>;
    /// Commits behind the base, and whether the base's changes overlap the
    /// change's paths.
    fn base(&self, change: &forge::Change) -> Result<(u64, bool)>;
    /// The change's head, re-read from the forge.
    fn head(&self, change: &forge::Change) -> Result<String>;
}

/// The readiness verdict, and the approval it relies on when ready. The
/// cheap local reads run first; the first refusal ends the evaluation.
pub(super) fn evaluate(
    change: &forge::Change,
    evidence: &dyn Evidence,
) -> (Readiness, Option<Approval>) {
    let not = |reason: String| (Readiness::NotReady(reason), None);
    if let Some(review) = evidence.under_review(change) {
        return not(format!("under review ({review})"));
    }
    match evidence.hold(change) {
        Ok(Some(hold)) => return not(format!("held: {hold}")),
        Ok(None) => {}
        Err(error) => return not(format!("merge-hold evidence unreadable: {error:#}")),
    }
    let (behind, overlap) = match evidence.base(change) {
        Ok(base) => base,
        Err(error) => return not(format!("base evidence unknown: {error:#}")),
    };
    if behind > 0 && overlap {
        return not(format!(
            "{behind} commit(s) behind `{}` with overlapping paths; merge `{}` into the branch",
            change.base, change.base
        ));
    }
    let approval = match evidence.approval(change) {
        Ok(approval) => approval,
        Err(refusal) => return not(bounded_diagnostic(&refusal)),
    };
    let checks = match evidence.ci(&change.head) {
        Ok(checks) => checks,
        Err(error) => return not(format!("CI evidence unknown: {error:#}")),
    };
    if !checks.sha.eq_ignore_ascii_case(&change.head) {
        return not(format!(
            "CI evidence is for {}, not the head",
            short(&checks.sha)
        ));
    }
    let names = |names: &[String]| bounded_diagnostic(&names.join(", "));
    match crate::forge::judge_commit_checks(&checks, &[crate::forge::CI_AGGREGATOR_CHECK]) {
        crate::forge::CommitCi::Green => {}
        crate::forge::CommitCi::NoChecks => return not("no CI checks on the head".into()),
        crate::forge::CommitCi::MissingRequired(name) => {
            return not(format!(
                "CI check `{name}` has not reported success on the head"
            ))
        }
        crate::forge::CommitCi::Pending(pending) => {
            return not(format!("CI pending: {}", names(&pending)))
        }
        crate::forge::CommitCi::Failed(failed) => {
            return not(format!("CI failed: {}", names(&failed)))
        }
    }
    match evidence.head(change) {
        Ok(head) if head == change.head => {}
        Ok(head) => return not(format!("head moved to {} during evaluation", short(&head))),
        Err(error) => return not(format!("head re-read refused: {error:#}")),
    }
    let readiness = if behind > 0 {
        Readiness::ReadyAfterUpdateBranch { behind }
    } else {
        Readiness::Ready
    };
    (readiness, Some(approval))
}

fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

/// The advisor's ready wake. Its ID binds the change, head (the commit CI
/// passed on), readiness and approval, so a retry re-delivers the same
/// message and new evidence is a new wake. It names only ordinary commands.
pub(super) fn wake_message(
    repository: &str,
    change: &forge::Change,
    readiness: &Readiness,
    approval: &Approval,
) -> aida_core::mailbox::Message {
    use aida_core::mailbox::{Intent, Message, Recipient, SenderSource};
    let key = change.key();
    let kind = readiness.wake().unwrap_or(WAKE_READY);
    let id = format!(
        "review-watch-{:x}",
        Sha256::digest(format!(
            "{repository}\0{key}\0{}\0{kind}\0{}",
            change.head, approval.identity
        ))
    );
    let carried = approval
        .carried
        .as_deref()
        .map(|c| format!(" ({c})"))
        .unwrap_or_default();
    let ship = format!("`aida pr ship {}`", change.id);
    let (state, next) = match readiness {
        Readiness::ReadyAfterUpdateBranch { behind } => (
            format!(
                "{behind} commit(s) behind `{}` on paths it does not touch",
                change.base
            ),
            format!(
                "Update the branch with {}, then run {ship} once CI on the new head is green.",
                update_command(change)
            ),
        ),
        _ => (
            format!("0 commits behind `{}`", change.base),
            format!("Run {ship} to merge it."),
        ),
    };
    let body = format!(
        "{key} at {head} is ready to ship: an authorized approval of {reviewed}{carried} covers the head, \
         every CI check on {head} passed, it is {state}, and no merge hold or review in progress is \
         recorded. {next} The watcher only reports; it did not merge or update the branch.",
        head = change.head,
        reviewed = approval.reviewed,
    );
    Message {
        id: id.clone(),
        thread_id: id,
        from: "review-watch".into(),
        to: Recipient::Agent("advisor".into()),
        timestamp: chrono::Utc::now().timestamp(),
        in_reply_to: None,
        body,
        subject: Some(match readiness {
            Readiness::ReadyAfterUpdateBranch { .. } => {
                format!("{key}: ready to ship after its branch is updated")
            }
            _ => format!("{key}: ready to ship"),
        }),
        urgent: false,
        intent: Intent::Request,
        retracted: false,
        deleted: false,
        archived: false,
        from_source: SenderSource::Explicit,
        from_role: None,
        relayed_from: None,
    }
}

/// The production evidence: the readers `aida pr ship` uses, pinned to the
/// watcher's repository, all read-only.
pub(super) struct Live<'a> {
    pub root: &'a Path,
    pub repo: &'a forge::Repository,
}

impl Evidence for Live<'_> {
    fn approval(&self, change: &forge::Change) -> std::result::Result<Approval, String> {
        let head = change.head.as_str();
        let (candidates, policy, ids) = crate::pr_cmd::merge_approval_subject(
            &[self.root],
            self.root,
            change.id,
            &[],
            None,
            Some(head),
        );
        let site = crate::review_carryover::CarryOverSite::quiet(self.root, Some(&change.base));
        let approval = approval_at(change.id, &candidates, head, &policy, &site)?;
        match crate::merge_hold::corpus_gate_decision(
            self.root,
            change.id,
            &ids,
            Some(&change.base),
            || Some(head.to_string()),
        ) {
            Ok(crate::merge_hold::CorpusMergeGate::Clear) => {}
            Ok(crate::merge_hold::CorpusMergeGate::Refuse { message, .. }) => return Err(message),
            Err(error) => return Err(format!("{error:#}")),
        }
        Ok(approval)
    }
    fn under_review(&self, change: &forge::Change) -> Option<String> {
        crate::review_marker::live_description(self.root, change.id, Some(&change.head))
    }
    fn hold(&self, change: &forge::Change) -> Result<Option<String>> {
        hold_at(self.root, change.id, || {
            crate::merge_hold::label_present(self.root, change.id)
        })
    }
    fn ci(&self, sha: &str) -> Result<crate::forge::CommitChecks> {
        self.repo.commit_checks(self.root, sha)
    }
    fn base(&self, change: &forge::Change) -> Result<(u64, bool)> {
        if let (Some(behind), Some(overlap)) = (change.behind, change.overlap) {
            return Ok((behind, overlap));
        }
        let mut probed = change.clone();
        self.repo.probe(self.root, &mut probed)?;
        Ok((
            probed.behind.context("behind count unknown")?,
            probed.overlap.context("overlap unknown")?,
        ))
    }
    fn head(&self, change: &forge::Change) -> Result<String> {
        Ok(self.repo.change(self.root, change.id)?.head)
    }
}

/// The approval gate `aida pr ship` applies, decided read-only: an
/// authorized, non-author approval that covers `head` or carries over to it
/// through `site` (which must be quiet). `Err` is the gate's refusal.
pub(super) fn approval_at(
    pr: u64,
    candidates: &[crate::review_verdict::RecordedVerdict],
    head: &str,
    policy: &crate::pr_ship::ShipApprovalPolicy,
    site: &crate::review_carryover::CarryOverSite<'_>,
) -> std::result::Result<Approval, String> {
    debug_assert!(site.quiet, "readiness never records a carry-over");
    let carried =
        match crate::pr_cmd::approval_decision(pr, candidates, Some(head), policy, Some(site))? {
            crate::pr_cmd::HeadDecision::Covered(Some(_)) => None,
            crate::pr_cmd::HeadDecision::Covered(None) => {
                return Err("no approval covers the head".into())
            }
            crate::pr_cmd::HeadDecision::CarriedOver(proof, _) => Some(format!(
                "carried over clean merges of `{}` to the head, the PR's own diff unchanged",
                proof.base_branch()
            )),
            crate::pr_cmd::HeadDecision::Stale { message, .. } => return Err(message),
        };
    let mut approval = approval_of(candidates).ok_or("no approval covers the head")?;
    approval.carried = carried;
    Ok(approval)
}

/// Any merge hold on `pr`, described: a marker of any kind (an unreadable
/// one reads as an unknown-kind hold), a marker removed without a record,
/// or the forge label, read through `label`. An unreadable label is `Err`.
pub(super) fn hold_at(
    root: &Path,
    pr: u64,
    label: impl FnOnce() -> std::result::Result<bool, String>,
) -> Result<Option<String>> {
    if let Some(record) = crate::merge_hold::read_hold_record(root, pr) {
        return Ok(Some(format!(
            "{} hold: {}",
            format!("{:?}", record.reason_kind).to_lowercase(),
            record.detail
        )));
    }
    if let Some(reason) = crate::merge_hold::read_hold(root, pr) {
        return Ok(Some(reason));
    }
    match label() {
        Ok(true) => Ok(Some("the merge-hold label is set on the forge".into())),
        Ok(false) => Ok(None),
        Err(error) => anyhow::bail!("merge-hold label unreadable: {error}"),
    }
}

/// The newest open approval among `candidates`, as a wake identity.
pub(super) fn approval_of(
    candidates: &[crate::review_verdict::RecordedVerdict],
) -> Option<Approval> {
    let open: Vec<_> = candidates
        .iter()
        .filter(|v| v.kind.approves() && !v.is_closed())
        .cloned()
        .collect();
    let newest = crate::pr_ship::newest_approval(&open)?;
    let reviewed = newest.reviewed_sha.clone()?;
    Some(Approval {
        identity: format!(
            "{reviewed}\0{}\0{}",
            newest.recorded_by.as_deref().unwrap_or(""),
            newest.recorded_at.as_deref().unwrap_or("")
        ),
        reviewed,
        carried: None,
    })
}
