//! `aida pr ship-batch` gate authority (STORY-1887, STORY-1889).
//!
//! Holds the batch's human-only initial consent check, its per-PR typed
//! confirmation and guarded hold-clear + ship handoff ([`confirm_and_ship`]),
//! its same-head independent review selector ([`review_state`]), its exact-head
//! CI rollup classifier ([`ci_from_rollup`]), and the [`ShipOverrides`] floor.
//! Orchestration, forge reads, branch refresh, polling and CLI stay in
//! [`crate::pr_ship_batch`].
// trace:STORY-1887 | ai:claude
// trace:STORY-1889 | ai:agy
// mandate-gate

use anyhow::Result;

/// C2: the most PRs one batch accepts.
pub(crate) const MAX_BATCH: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrState {
    Open,
    Merged,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mergeable {
    Clean,
    Conflicting,
    Unknown,
}

/// How stale a PR's green is, from the gate `aida pr ship` applies.
// trace:STORY-1690 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Staleness {
    /// The head is missing commits now on its base: ship refuses it.
    pub behind: bool,
    /// CI-definition files changed on the base since the head's green.
    pub definition_files: Vec<String>,
}

/// What the batch needs to know about one PR, read in one forge call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrView {
    pub state: PrState,
    pub mergeable: Mergeable,
    /// The forge says the branch must be brought up to date with its base.
    pub behind: bool,
    pub draft: bool,
    pub head_sha: String,
    pub head_ref: String,
    pub title: String,
    pub body: String,
}

/// CI as observed for one exact head (C3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CiAtHead {
    Pending,
    Passing,
    Failing(String),
    /// The PR's head is no longer the one asked about.
    HeadMoved,
}

/// The review evidence at one exact head, judged by the ship gate's readers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReviewAtHead {
    Approved(String),
    Negative(String),
    Awaiting(Option<String>),
}

/// The ship gate overrides. The batch only ever passes [`ShipOverrides::NONE`];
/// the type exists so tests can assert it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShipOverrides {
    pub stale_check: bool,
    pub stale_approval: bool,
}

impl ShipOverrides {
    pub(crate) const NONE: Self = ShipOverrides {
        stale_check: false,
        stale_approval: false,
    };
}

/// Everything the step machine reads or changes.
pub(crate) trait BatchPorts {
    /// The integrity floor: a human at an interactive terminal.
    fn human_at_terminal(&self) -> bool;
    fn view(&mut self, pr: u64) -> Result<PrView>;
    /// C4: what makes `aida pr ship` refuse this PR's green, from the stale
    /// gate's own predicate (STORY-1690: a head behind its base refuses even
    /// when the forge does not report BEHIND). `fetch` false (`--dry-run`)
    /// judges against the local tracking refs as they are.
    fn staleness(&mut self, pr: u64, view: &PrView, fetch: bool) -> Result<Staleness>;
    fn update_branch(&mut self, pr: u64, expected_head: &str) -> Result<()>;
    /// CI at `head`; `held` is the PR's hold state, read by the caller
    /// (see [`crate::pr_ship_batch`]) so a hold-label error is not reported as CI.
    fn ci_at_head(&mut self, pr: u64, head: &str, held: bool) -> Result<CiAtHead>;
    /// Enqueue a review of the current head; never launches or records one.
    fn enqueue_review(&mut self, pr: u64, view: &PrView);
    fn review_at_head(&mut self, pr: u64, view: &PrView) -> ReviewAtHead;
    /// The merge-hold on `pr`, described with its release condition. An
    /// error means the hold state could not be read (not a hold to clear).
    fn hold(&mut self, pr: u64) -> Result<Option<String>>;
    /// Which spec IDs the ship would complete.
    fn completion_preview(&mut self, pr: u64, view: &PrView) -> Option<String>;
    /// One line typed by the operator; `None` at end of input.
    fn ask(&mut self, prompt: &str) -> Option<String>;
    /// Clear `pr`'s merge-hold through `aida merge-hold clear`.
    fn clear_hold(&mut self, pr: u64) -> Result<()>;
    fn ship(&mut self, pr: u64, overrides: ShipOverrides) -> Result<()>;
    fn sleep(&mut self, secs: u64);
    fn elapsed_secs(&self) -> u64;
    fn say(&mut self, line: &str);
    fn record(&mut self, pr: Option<u64>, outcome: &str, detail: &str);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrOutcome {
    Shipped,
    AlreadyMerged,
    Stopped(String),
    /// `--dry-run`: what the batch would do.
    Planned(String),
}

/// C2: the PR list is non-empty, at most [`MAX_BATCH`] long, without repeats.
pub(crate) fn validate_order(prs: &[u64]) -> std::result::Result<(), String> {
    if prs.is_empty() {
        return Err("give at least one PR number".to_string());
    }
    if prs.len() > MAX_BATCH {
        return Err(format!(
            "{} PRs given; one batch ships at most {MAX_BATCH}",
            prs.len()
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for pr in prs {
        if !seen.insert(*pr) {
            return Err(format!("PR #{pr} is listed twice; list each PR once"));
        }
    }
    Ok(())
}

pub(crate) fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

/// Initial human-only check before any batch action (including `--dry-run`).
pub(crate) fn ensure_human_at_terminal(
    ports: &mut dyn BatchPorts,
) -> std::result::Result<(), String> {
    if !ports.human_at_terminal() {
        let refusal = "`aida pr ship-batch` ships PRs that need a human merge, so it runs only \
                       for a human at an interactive terminal";
        ports.record(None, "refused", refusal);
        return Err(refusal.to_string());
    }
    Ok(())
}

/// Up-front `go` confirmation before starting a non-dry-run batch.
pub(crate) fn confirm_batch_start(
    ports: &mut dyn BatchPorts,
    order: &str,
) -> std::result::Result<(), String> {
    ports.say(&format!("Ship in this order: {order}"));
    ports.say("Each PR is refreshed from main if needed, then waits for CI and an independent approval at its new head, then asks you to confirm.");
    if ports.ask("Type 'go' to start: ").as_deref().map(str::trim) != Some("go") {
        return Err("not started: 'go' was not typed".to_string());
    }
    Ok(())
}

/// `Some` when `view` ends this PR's turn before any write.
pub(crate) fn open_or_outcome(pr: u64, view: &PrView) -> Option<PrOutcome> {
    match view.state {
        PrState::Merged => Some(PrOutcome::AlreadyMerged),
        PrState::Closed => Some(PrOutcome::Stopped(format!(
            "PR #{pr} is closed without merging"
        ))),
        PrState::Open if view.draft => Some(PrOutcome::Stopped(format!(
            "PR #{pr} is a draft; mark it ready (`gh pr ready {pr}`) and re-run"
        ))),
        PrState::Open if view.mergeable == Mergeable::Conflicting => Some(PrOutcome::Stopped(
            format!("PR #{pr} conflicts with main; resolve the conflict, then re-run the batch"),
        )),
        PrState::Open => None,
    }
}

pub(crate) fn confirm_and_ship(
    ports: &mut dyn BatchPorts,
    pr: u64,
    view: &PrView,
    approval: &str,
) -> PrOutcome {
    if !ports.human_at_terminal() {
        return PrOutcome::Stopped(
            "no human at an interactive terminal any more; nothing shipped".to_string(),
        );
    }
    // A hold state that cannot be read is not offered as a hold to clear.
    // trace:STORY-1887 | ai:claude
    let hold = match ports.hold(pr) {
        Ok(hold) => hold,
        Err(e) => {
            return PrOutcome::Stopped(format!(
                "could not read the hold label on PR #{pr} ({e:#}); nothing shipped"
            ))
        }
    };
    ports.say(&format!("  ready: PR #{pr} {}", view.title));
    ports.say(&format!(
        "    head {}, CI green, approved: {approval}",
        short(&view.head_sha)
    ));
    if let Some(preview) = ports.completion_preview(pr, view) {
        ports.say(&format!("    {preview}"));
    }
    if let Some(hold) = &hold {
        ports.say(&format!("    merge-hold: {hold}"));
    }
    let typed = ports.ask(&format!(
        "Type {pr} to ship PR #{pr} (anything else stops the batch): "
    ));
    if typed.as_deref().map(str::trim) != Some(pr.to_string().as_str()) {
        return PrOutcome::Stopped(format!("PR #{pr} was not confirmed; nothing shipped"));
    }
    let mut cleared = false;
    if hold.is_some() {
        let typed = ports.ask(&format!(
            "PR #{pr} has a merge-hold. Type 'clear' to clear it and ship (anything else stops): "
        ));
        if typed.as_deref().map(str::trim) != Some("clear") {
            return PrOutcome::Stopped(format!(
                "PR #{pr}'s merge-hold was not cleared; nothing shipped"
            ));
        }
        if let Err(e) = ports.clear_hold(pr) {
            return PrOutcome::Stopped(format!("could not clear PR #{pr}'s merge-hold: {e:#}"));
        }
        cleared = true;
    }
    match ports.ship(pr, ShipOverrides::NONE) {
        Ok(()) => PrOutcome::Shipped,
        // A refusal after the clear leaves the PR unheld; say so.
        // trace:TASK-2172 | ai:claude
        Err(e) if cleared => PrOutcome::Stopped(format!(
            "`aida pr ship {pr}` refused: {e:#}; PR #{pr}'s merge-hold was already cleared"
        )),
        Err(e) => PrOutcome::Stopped(format!("`aida pr ship {pr}` refused: {e:#}")),
    }
}

/// The review state at `head`, judged by the readers `aida pr ship` uses.
/// The newest verdict recorded at this head wins; only an approval the ship
/// gate would pin to (no carry-over) and an authorized recorder count.
pub(crate) fn review_state(
    pr: u64,
    candidates: &[crate::gate::review_verdict::RecordedVerdict],
    policy: &crate::gate::ship::ShipApprovalPolicy,
    head: &str,
) -> ReviewAtHead {
    let newest_at_head = candidates
        .iter()
        .filter(|v| !v.is_closed())
        .filter(|v| {
            v.reviewed_sha
                .as_deref()
                .is_some_and(|sha| crate::forge::head_matches_pin(sha.trim(), head))
        })
        .max_by_key(|v| {
            v.recorded_at
                .as_deref()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t.trim()).ok())
                .map(|t| t.with_timezone(&chrono::Utc))
        });
    let Some(newest) = newest_at_head else {
        return ReviewAtHead::Awaiting(None);
    };
    if !newest.kind.approves() {
        return ReviewAtHead::Negative(format!("the reviewer recorded {}", newest.kind.label()));
    }
    if crate::gate::ship::approved_match_head(candidates, Some(head)).is_none() {
        return ReviewAtHead::Awaiting(None);
    }
    if let Some(refusal) = crate::gate::ship::approval_authority_refusal(candidates, policy) {
        return ReviewAtHead::Awaiting(Some(
            crate::gate::ship::approval_authority_refusal_message(pr, &refusal),
        ));
    }
    let who = newest
        .recorded_by
        .clone()
        .unwrap_or_else(|| "recorded".to_string());
    ReviewAtHead::Approved(match newest.summary.as_deref() {
        Some(summary) if !summary.trim().is_empty() => format!("{who}: {}", summary.trim()),
        _ => who,
    })
}

fn json_str(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Map `gh pr view --json state,mergeable,mergeStateStatus,isDraft,...`.
pub(crate) fn parse_pr_view(v: &serde_json::Value) -> PrView {
    PrView {
        state: match json_str(v, "state").as_str() {
            "MERGED" => PrState::Merged,
            "CLOSED" => PrState::Closed,
            _ => PrState::Open,
        },
        mergeable: match json_str(v, "mergeable").as_str() {
            "MERGEABLE" => Mergeable::Clean,
            "CONFLICTING" => Mergeable::Conflicting,
            _ => Mergeable::Unknown,
        },
        behind: json_str(v, "mergeStateStatus") == "BEHIND",
        draft: v.get("isDraft").and_then(|d| d.as_bool()).unwrap_or(false),
        head_sha: json_str(v, "headRefOid"),
        head_ref: json_str(v, "headRefName"),
        title: json_str(v, "title"),
        body: json_str(v, "body"),
    }
}

/// C3: classify a rollup only when it belongs to `head`.
pub(crate) fn ci_from_rollup(
    v: &serde_json::Value,
    head: &str,
    cfg: &crate::ci_gate::CiGateConfig,
    held: bool,
) -> CiAtHead {
    if !crate::forge::head_matches_pin(head, &json_str(v, "headRefOid")) {
        return CiAtHead::HeadMoved;
    }
    let rollup = v
        .get("statusCheckRollup")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    // A held PR's red merge-hold-gate is the hold, which the confirm step
    // offers to clear; it is not a CI failure. trace:STORY-1887 | ai:claude
    let status = crate::pr_list::github_rollup_ci_status_for(&rollup, cfg, held);
    if status == "Failing" {
        CiAtHead::Failing("a required check failed".to_string())
    } else if status.starts_with("Passing") {
        CiAtHead::Passing
    } else {
        CiAtHead::Pending
    }
}

#[cfg(test)]
#[path = "tests/story_1887_pr_ship_batch_tests.rs"]
mod tests;
