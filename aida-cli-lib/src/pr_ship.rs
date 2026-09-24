//! `aida pr ship [<N>]` — collapse the recurring "push, create-if-needed,
//! watch CI, squash-merge, pull, worktree-cleanup" recipe into one command
//! (TASK-458). Sibling verb to `aida pr rebase` (TASK-308).
//!
//! This is the **direct-publish** path — human-pre-approved work that
//! doesn't need the orchestrator's review phase. `aida queue work PR-N
//! --auto-complete` (TASK-405) is the full-pipeline analogue; this
//! command is intentionally smaller-scope.
//!
//! The module owns the **pure pieces** so they're unit-testable without
//! `git`/`gh`: PR-number extraction from `gh pr create` URLs,
//! commit-message → title/body derivation, dry-run plan formatting,
//! activity-log JSONL formatting. The CLI handler that wires git/gh
//! side-effects lives in `main.rs` next to `pr_rebase_handler` (mirrors
//! `punt.rs` / `pr_rebase.rs`).
//!
//! trace:TASK-458 | ai:claude

/// Flags / mode the handler resolves from the parsed clap subcommand.
/// Kept as a value type so dry-run plan formatting can be exercised in
/// unit tests without invoking the full handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrShipOptions {
    /// `<N>` if the user passed one; otherwise we resolve the PR open
    /// for the current branch (or create one).
    pub pr_number: Option<u64>,
    /// `--no-pull` — skip the post-merge `aida pull` step (used by
    /// compositions that pull separately).
    pub no_pull: bool,
    /// `--no-cleanup` — skip the `aida session end` worktree-cleanup
    /// step (useful when you want to inspect post-merge before
    /// cleanup).
    pub no_cleanup: bool,
    /// `--dry-run` — print the resolved sequence and exit zero.
    pub dry_run: bool,
    /// STORY-439: the implementer's self-assessed actual complexity at
    /// ship time. Captured to the per-spec calibration record alongside
    /// the punt count read from `.aida/punts.jsonl`. Absent ⇒ no
    /// ship-side complexity slot is written; the punt count is still
    /// captured against every spec the PR credits.
    /// trace:STORY-439 | ai:claude
    pub complexity: Option<crate::complexity_calibration::ComplexityLevel>,
    /// STORY-451: implementer's actual effort spent at ship time.
    /// Captured per credited spec in `.aida/effort-calibration/`.
    /// trace:STORY-451 | ai:codex
    pub effort: Option<crate::effort_calibration::EffortBucket>,
}

/// One ordered step in the `aida pr ship` sequence. The variants drive
/// both the dry-run plan output and the per-step status lines printed
/// at run time. Mirrors the structure of the JSONL activity-log entries
/// so a future `aida status` surface can render the same data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShipStep {
    /// Resolve or create the PR. `create_if_needed = true` when no PR
    /// number was supplied and the current branch has no open PR.
    ResolvePr { create_if_needed: bool },
    /// `gh pr checks <N> --watch`.
    WatchCi,
    /// `gh pr merge <N> --squash [--delete-branch]`.
    Merge { delete_branch: bool },
    /// `aida pull` from the main worktree (skipped when `--no-pull`).
    Pull,
    /// `aida session end <lease>` for the worktree the PR was authored
    /// in (skipped when `--no-cleanup` or no lease is found).
    EndLease,
}

/// Extract the PR number from `gh pr create`'s success output. `gh`
/// prints the new PR's URL on the final non-empty stdout line, of the
/// form `https://github.com/<owner>/<repo>/pull/<N>`. We accept any
/// trailing slash or query string so the parser doesn't break on `gh`
/// version drift.
///
/// Returns `None` when no `/pull/<N>` segment is present.
pub fn parse_pr_number_from_create_output(stdout: &str) -> Option<u64> {
    for line in stdout.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(idx) = trimmed.find("/pull/") {
            let rest = &trimmed[idx + "/pull/".len()..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() {
                if let Ok(n) = digits.parse::<u64>() {
                    return Some(n);
                }
            }
        }
    }
    None
}

/// Derive a PR title from a commit message: the first non-empty line,
/// trimmed. Matches the `gh pr create --title "$(git log -1 --format=%s)"`
/// pattern from the user's manual recipe.
pub fn derive_pr_title_from_commit(commit_message: &str) -> String {
    commit_message
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

/// Derive a PR body from a commit message: everything after the first
/// line (and its trailing blank line), with leading/trailing whitespace
/// trimmed. Empty when the commit has no body. Matches `git log -1
/// --format=%b`.
pub fn derive_pr_body_from_commit(commit_message: &str) -> String {
    let mut iter = commit_message.lines();
    // Skip the subject line.
    iter.next();
    // Drop blank line(s) right after the subject, but keep blanks
    // between body paragraphs.
    let mut body: Vec<&str> = iter.collect();
    while body.first().map(|l| l.trim().is_empty()).unwrap_or(false) {
        body.remove(0);
    }
    while body.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
        body.pop();
    }
    body.join("\n")
}

/// Extract AIDA requirement IDs from PR metadata text. This is intentionally
/// broader than commit-subject extraction because PR titles/bodies and branch
/// names are recovery surfaces for malformed local commit subjects.
// trace:SPEC-410 | ai:codex
pub fn extract_spec_ids_from_text(text: &str) -> Vec<String> {
    let re = regex::Regex::new(r"(?i)\b([a-z][a-z0-9]*(?:-[a-z0-9]+)*-\d+(?:-\d+)*)\b")
        .expect("valid spec id regex");
    let mut out = Vec::new();
    for cap in re.captures_iter(text) {
        let id = cap[1].to_ascii_uppercase();
        let prefix = id.split('-').next().unwrap_or("");
        // PR/MR/GH/GL refs are forge IDs, not AIDA requirements.
        if matches!(prefix, "PR" | "MR" | "GH" | "GL") {
            continue;
        }
        if !out.iter().any(|seen| seen == &id) {
            out.push(id);
        }
    }
    out
}

/// Derive the best spec-id set to preserve in a squash subject. Priority:
/// PR title, then branch name, then PR body.
// trace:SPEC-410 | ai:codex
pub fn derive_squash_subject_spec_ids(pr_title: &str, branch: &str, pr_body: &str) -> Vec<String> {
    for source in [pr_title, branch, pr_body] {
        let ids = extract_spec_ids_from_text(source);
        if !ids.is_empty() {
            return ids;
        }
    }
    Vec::new()
}

/// Ensure the squash subject carries every derived spec ID exactly once.
/// Existing well-formed subjects are left untouched.
// trace:SPEC-410 BUG-339 | ai:codex
pub fn squash_subject_with_spec_ids(subject: &str, ids: &[String]) -> String {
    let subject = subject.trim();
    if ids.is_empty() || subject.is_empty() {
        return subject.to_string();
    }
    let existing = extract_trailing_spec_ids_from_subject(subject);
    let missing: Vec<&String> = ids
        .iter()
        .filter(|id| !existing.iter().any(|seen| seen == *id))
        .collect();
    if missing.is_empty() {
        return subject.to_string();
    }
    let joined = missing
        .into_iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    append_spec_ids_before_pr_suffix(subject, &joined)
}

/// Choose the explicit squash-merge subject `aida pr ship` should pass.
///
/// GitHub's default squash subject can be the branch HEAD commit. If a feature
/// branch's HEAD is a merge commit, that default is mechanically correct but
/// loses the PR's descriptive title in main history. Prefer the PR title as the
/// subject base, then fall back to the branch-head commit subject when the PR
/// title is empty.
// trace:TASK-142 | ai:codex
pub fn derive_squash_subject(
    pr_title: &str,
    branch: &str,
    pr_body: &str,
    branch_head_commit_message: &str,
) -> Option<String> {
    let pr_title_subject = derive_pr_title_from_commit(pr_title);
    let branch_subject = derive_pr_title_from_commit(branch_head_commit_message);
    let mut ids = derive_squash_subject_spec_ids(pr_title, branch, pr_body);
    if ids.is_empty() {
        ids = extract_spec_ids_from_text(&branch_subject);
    }
    let base = if pr_title_subject.is_empty() {
        branch_subject
    } else {
        pr_title_subject
    };
    if base.is_empty() {
        return None;
    }
    Some(squash_subject_with_spec_ids(&base, &ids))
}

/// Extract the exact shape the auto-bump scanner recognizes: a trailing
/// `(SPEC-ID[, SPEC-ID...])` group, optionally followed by GitHub's `(#N)`.
// trace:BUG-339 | ai:codex
pub fn extract_trailing_spec_ids_from_subject(subject: &str) -> Vec<String> {
    let mut tail = subject.trim();
    if let Some((head, inner)) = trailing_paren_group(tail) {
        let pr_suffix = inner
            .strip_prefix('#')
            .filter(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()));
        if pr_suffix.is_some() {
            tail = head.trim_end();
        }
    }
    let Some((_, inner)) = trailing_paren_group(tail) else {
        return Vec::new();
    };
    parse_spec_id_group(inner)
}

fn append_spec_ids_before_pr_suffix(subject: &str, joined_ids: &str) -> String {
    if let Some((head, inner)) = trailing_paren_group(subject) {
        let pr_suffix = inner
            .strip_prefix('#')
            .filter(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()));
        if pr_suffix.is_some() {
            return format!("{} ({}) ({})", head.trim_end(), joined_ids, inner);
        }
    }
    format!("{subject} ({joined_ids})")
}

fn trailing_paren_group(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim();
    if !trimmed.ends_with(')') {
        return None;
    }
    let open_at = trimmed.rfind('(')?;
    Some((
        &trimmed[..open_at],
        &trimmed[open_at + 1..trimmed.len() - 1],
    ))
}

fn parse_spec_id_group(inner: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut tok_count = 0;
    for tok in inner.split(|c: char| c == ',' || c.is_whitespace()) {
        let tok = tok.trim();
        if tok.is_empty() {
            continue;
        }
        tok_count += 1;
        let ids = extract_spec_ids_from_text(tok);
        if ids.len() == 1 && ids[0].eq_ignore_ascii_case(tok) {
            out.push(ids[0].clone());
        } else {
            return Vec::new();
        }
    }
    if tok_count == 0 {
        Vec::new()
    } else {
        out
    }
}

/// BUG-434: decide whether `aida pr ship` may pass `--delete-branch`.
///
/// Deleting the just-merged branch is the convenient default, but it has two
/// footguns the substrate already has the data to prevent:
///   - `branch_in_sibling` (BUG-289): the branch is checked out in a sibling
///     worktree, so `gh pr merge --delete-branch`'s local-cleanup step fails;
///     `aida session end` removes the worktree-bound branch instead.
///   - `stacked_child_count` / `open_child_pr_count` (BUG-434): one or more
///     branches/PRs are stacked ON this branch. Deleting it orphans the local
///     children and GitHub auto-CLOSES the stacked PRs (the #439 slip).
///
/// `force` is the operator's explicit `--force-delete-branch` override — it
/// deletes regardless (deliberately orphaning children). Kept pure so the
/// guard is unit-testable without a worktree or `gh`. trace:BUG-434 | ai:claude
pub fn should_delete_branch(
    branch_in_sibling: bool,
    stacked_child_count: usize,
    open_child_pr_count: usize,
    force: bool,
) -> bool {
    if force {
        return true;
    }
    !branch_in_sibling && stacked_child_count == 0 && open_child_pr_count == 0
}

/// BUG-733: for an explicit `aida pr ship <N>`, the current checkout branch may
/// be the PR base (`main`) rather than the source branch being merged. Use the
/// resolved PR head when available, and never count the target PR itself as a
/// stacked child. Kept pure so the head/base swap regression stays pinned.
// trace:BUG-733 | ai:codex
pub fn ship_branch_context(
    current_branch: &str,
    target_change: Option<&crate::forge::ChangeRef>,
    open_prs_based_on_delete_branch: &[u64],
) -> (String, String, Vec<u64>) {
    let branch_to_delete = target_change
        .and_then(|c| (!c.branch.is_empty()).then_some(c.branch.as_str()))
        .unwrap_or(current_branch)
        .to_string();
    let retarget_base = target_change
        .and_then(|c| (!c.base.is_empty()).then_some(c.base.as_str()))
        .unwrap_or("main")
        .to_string();
    let target_id = target_change.map(|c| c.id);
    let child_prs = open_prs_based_on_delete_branch
        .iter()
        .copied()
        .filter(|n| Some(*n) != target_id)
        .collect();

    (branch_to_delete, retarget_base, child_prs)
}

/// Why `aida pr ship` must leave a PR open for the drive's independent reviewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShipMergeBlockReason {
    DriveSeat,
    DriveOwnedPr,
}

/// TASK-1292: whether a reviewer phase is currently live against a given PR
/// number — the pure decision behind `aida pr ship`'s drive-ownership check.
///
/// The 2026-09-18 near-miss: a reviewer phase running under BUG-1236 was
/// live on PR #1948, but the drive's "current spec" bookkeeping pointed at
/// STORY-1221 (shelved). A check that asks "does the drive's current spec
/// own this PR" misses that case entirely; the only question that can't miss
/// it is "is ANY live member — whichever spec it belongs to — phase-bound to
/// THIS PR". That's what this function answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewerLiveness {
    /// A live (`in-phase-*`) member is phase-bound to exactly this PR.
    OnThisPr,
    /// At least one member is live and phase-bound to a PR — just not this
    /// one. Not grounds to block a ship of an unrelated PR.
    Elsewhere,
    /// No live member is phase-bound to any PR (including: no drive at all).
    None,
}

/// `members` is `(is_running, pr)` per drive member. The caller derives
/// liveness through `DrainMember::is_running`, so this guard cannot drift from
/// the state producer's format. `pr` is the member's currently-bound PR
/// (STORY-1033/TASK-1292 binds it live, at phase-entry, not only at the
/// member's terminal outcome).
// trace:TASK-1292 | ai:claude
// trace:BUG-1429 | ai:codex
pub fn reviewer_liveness_for_pr(
    members: impl IntoIterator<Item = (bool, Option<u32>)>,
    pr: u32,
) -> ReviewerLiveness {
    let mut saw_other_live_pr = false;
    for (is_running, member_pr) in members {
        if !is_running {
            continue;
        }
        match member_pr {
            Some(bound) if bound == pr => return ReviewerLiveness::OnThisPr,
            Some(_) => saw_other_live_pr = true,
            None => {}
        }
    }
    if saw_other_live_pr {
        ReviewerLiveness::Elsewhere
    } else {
        ReviewerLiveness::None
    }
}

/// BUG-1566: `aida pr ship` releasing a supervised merge-hold is the SAME
/// integrity floor as `aida merge-hold clear` — a human at an interactive
/// terminal. Without that authority the ship refuses before touching the hold,
/// and points at the human release path. There is no carve-out: dispatch,
/// advisor, reviewer and headless seats all hit this, so the two release
/// paths can never disagree on who may clear a hold. `Some(msg)` = refuse.
// trace:BUG-1566 | ai:claude
pub fn ship_hold_release_refusal(
    hold_present: bool,
    integrity_floor_authority: bool,
    pr: u64,
) -> Option<String> {
    if !hold_present || integrity_floor_authority {
        return None;
    }
    Some(format!(
        "PR-{pr} is under a supervised merge-hold; `aida pr ship` will not release it without \
         a human at an interactive terminal (the same integrity floor as `aida merge-hold clear`). \
         A human must run `aida merge-hold clear {pr}`, then re-run `aida pr ship {pr}`."
    ))
}

/// BUG-1499: the release-step decision. A hold that exists ONLY as the forge
/// label is refused for every caller, a human included: `pr ship` would drop
/// the label with no clearance record, so it is sent to
/// `aida merge-hold clear <PR>` (human floor + recorded clearance). A marker
/// hold falls through to the BUG-1566 floor. `Some(msg)` = refuse.
// trace:BUG-1499 | ai:claude
pub fn ship_hold_release_refusal_for(
    marker_present: bool,
    label_only: bool,
    integrity_floor_authority: bool,
    pr: u64,
) -> Option<String> {
    if !marker_present && label_only {
        return Some(format!(
            "PR-{pr} is held by the `aida:merge-hold` label alone (no marker, no recorded reason); \
             `aida pr ship` will not release it. A human clears it with `aida merge-hold clear {pr}` \
             (which records the clearance), then re-runs `aida pr ship {pr}`."
        ));
    }
    ship_hold_release_refusal(marker_present, integrity_floor_authority, pr)
}

/// BUG-1532: ANY merge-hold marker binds `aida pr ship` — whether or not a
/// drive is live and whatever its reason text says (the old guard honoured a
/// marker only when its prose named a running drive member's spec, so with
/// the lane stopped no hold bound the client side at all).
///
/// The question is "is this command the decision the hold is waiting for?":
///   - no human at a terminal → never (BUG-1566 floor): refuse, before CI
///     is watched, the red gate discounted, or the hold touched;
///   - a human, on a SUPERVISION or DECISION hold → yes: the explicit merge
///     is that decision, so the BUG-1167 release proceeds;
///   - a human, on a REFUSAL hold (typed rework, or an untyped legacy marker
///     — unknown is not permission) → only when a FRESH APPROVED verdict is
///     recorded at the PR's current head (criterion 4, decided by
///     `release_of`, i.e. [`crate::merge_hold::refusal_release`]). A person
///     running a merge is not the reviewer's answer; a new verdict at the
///     head is. Otherwise refuse, naming the verdict on record and its sha —
///     or its missing sha (criterion 11);
///   - a human, on a RECUSAL hold or a malformed marker → no: those are
///     released by an independent reader / a deliberate
///     `aida merge-hold clear <PR>` (recorded).
///
/// `release_of` is called only for a refusal hold with a human present, so
/// the forge head read it needs is never made otherwise.
/// `AIDA_PR_SHIP_ALLOW_IN_DRIVE` does not reach this gate. `None` = proceed.
// trace:BUG-1532 | ai:claude
pub(crate) fn ship_hold_gate(
    hold: Option<&crate::merge_hold::MergeHoldRecord>,
    integrity_floor_authority: bool,
    pr: u64,
    release_of: impl FnOnce(&crate::merge_hold::MergeHoldRecord) -> crate::merge_hold::RefusalRelease,
) -> Option<String> {
    use crate::merge_hold::{HoldReasonKind, RefusalRelease};
    let hold = hold?;
    if !integrity_floor_authority {
        return ship_hold_release_refusal(true, false, pr);
    }
    let releasable = !hold.legacy
        && matches!(
            hold.reason_kind,
            HoldReasonKind::Supervision | HoldReasonKind::Decision
        );
    if releasable {
        return None;
    }
    let kind = if hold.legacy {
        "an untyped legacy hold (read as a refusal)".to_string()
    } else {
        format!("a {} hold", hold.reason_kind.as_str())
    };
    if hold.is_refusal() {
        return match release_of(hold) {
            RefusalRelease::Released(_) => None,
            RefusalRelease::Held(why) => Some(format!(
                "PR-{pr} is under {kind}. A reviewer refusal is released only by a fresh APPROVED \
                 verdict recorded at the PR's current head, and {why}. (Marker summary from \
                 placement, not current: {}.) Once that verdict is recorded, re-run \
                 `aida pr ship {pr}`. A human may instead override deliberately with \
                 `aida merge-hold clear {pr}` (recorded).",
                hold.detail.trim()
            )),
        };
    }
    Some(format!(
        "PR-{pr} is under {kind}: {}. `aida pr ship` releases only a supervision or decision \
         hold, or a refusal answered by a fresh approving verdict at the current head — merging \
         is not the decision this one waits for. Once its release condition is met, a human \
         clears it with `aida merge-hold clear {pr}` (recorded), then re-runs `aida pr ship {pr}`.",
        hold.detail.trim()
    ))
}

/// BUG-1532: the merge pin when a refusal hold is released by a fresh
/// verdict. The verdict covers exactly `head_sha`, so the merge is pinned
/// there (overriding the TASK-1448 pin, which is None when the verdict's key
/// is not among its candidates); with no readable head the release is
/// refused. Not verdict-released → the TASK-1448 pin stands unchanged.
// trace:BUG-1532 | ai:claude
pub(crate) fn refusal_release_pin(
    released_by_verdict: bool,
    match_head: Option<String>,
    head_sha: Option<&str>,
    pr: u64,
) -> Result<Option<String>, String> {
    if !released_by_verdict {
        return Ok(match_head);
    }
    match head_sha.map(str::trim).filter(|h| !h.is_empty()) {
        Some(head) => Ok(Some(head.to_string())),
        None => Err(format!(
            "PR-{pr}: the reviewer-refusal hold's release rests on a verdict at the PR head, but \
             the head could not be read to pin the merge to it; the hold stays. Re-run \
             `aida pr ship {pr}`."
        )),
    }
}

/// BUG-710/BUG-716/TASK-1253: a drive seat may not merge any PR, and no caller
/// may merge the live drive's own PR. Merely observing an unrelated live drain
/// is not grounds to block: those merges serialize on the merge lease.
// trace:BUG-710 trace:BUG-716 trace:TASK-1253 | ai:codex
pub fn ship_merge_block_reason(
    caller_is_drive_seat: bool,
    pr_is_drive_owned: bool,
    override_allow: bool,
) -> Option<ShipMergeBlockReason> {
    if override_allow {
        None
    } else if caller_is_drive_seat {
        Some(ShipMergeBlockReason::DriveSeat)
    } else if pr_is_drive_owned {
        Some(ShipMergeBlockReason::DriveOwnedPr)
    } else {
        None
    }
}

/// BUG-732: after a forge merge command returns non-zero, decide whether the
/// caller should keep going because a fresh forge-state probe shows the squash
/// merge actually landed and only post-merge cleanup failed.
// trace:BUG-732 | ai:codex
pub fn merge_error_landed_despite_failure(pr_is_merged_after_error: Option<bool>) -> bool {
    matches!(pr_is_merged_after_error, Some(true))
}

/// Build the `gh pr merge` argv. Kept pure so SPEC-410 can pin the
/// contract that the wrapper passes `--subject` when it repairs a squash
/// subject.
// trace:SPEC-410 | ai:codex
pub fn merge_args(pr_number: u64, delete_branch: bool, subject: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "pr".to_string(),
        "merge".to_string(),
        pr_number.to_string(),
        "--squash".to_string(),
    ];
    if delete_branch {
        args.push("--delete-branch".to_string());
    }
    if let Some(subject) = subject {
        args.push("--subject".to_string());
        args.push(subject.to_string());
    }
    args
}

/// True when `gh pr checks <N>` output indicates at least one CI check
/// exists. The startup path treats "no checks yet" separately from
/// "checks ran and failed" so `aida pr ship` does not bail during the
/// small GitHub Actions registration window after a push.
// trace:BUG-344 | ai:codex
/// STORY-1166: classify one `gh pr checks <n>` invocation into the forge-
/// neutral registration state. `Err` = gh failed for a reason other than
/// "no checks yet". Shared by `GitHubForge::checks_registered` and the
/// fake-gh test harness so the two cannot drift.
// trace:STORY-1166 | ai:claude
pub(crate) fn classify_gh_pr_checks_registration(
    pr: u64,
    stdout: &str,
    stderr: &str,
    success: bool,
) -> anyhow::Result<crate::forge::CheckRegistration> {
    // external-prose-classifier: pr_ship::classify_gh_pr_checks_registration
    if gh_pr_checks_output_has_registered_checks(stdout, stderr) {
        return Ok(crate::forge::CheckRegistration::Registered);
    }
    if !success
        && !gh_pr_checks_output_is_unregistered(stdout, stderr)
        && (!stdout.trim().is_empty() || !stderr.trim().is_empty())
    {
        anyhow::bail!(
            "`gh pr checks {pr}` failed before CI registration could be inspected: {}",
            stderr.trim()
        );
    }
    Ok(crate::forge::CheckRegistration::NotYet)
}

pub fn gh_pr_checks_output_has_registered_checks(stdout: &str, stderr: &str) -> bool {
    let combined = format!("{stdout}\n{stderr}");
    let trimmed = combined.trim();
    if trimmed.is_empty() {
        return false;
    }
    if aida_core::external_tool_output::contains_any_case_insensitive(
        trimmed,
        aida_core::external_tool_output::GH_NO_REGISTERED_CHECKS,
    ) {
        return false;
    }
    true
}

/// True when a failing `gh pr checks <N>` invocation is the expected
/// "checks not registered yet" state rather than a real gh/auth/network
/// failure.
// trace:BUG-344 | ai:codex
pub fn gh_pr_checks_output_is_unregistered(stdout: &str, stderr: &str) -> bool {
    let combined = format!("{stdout}\n{stderr}");
    aida_core::external_tool_output::contains_any_case_insensitive(
        &combined,
        aida_core::external_tool_output::GH_NO_REGISTERED_CHECKS,
    )
}

/// BUG-417: parse the PR base branch from `gh repo view --json
/// defaultBranchRef -q .defaultBranchRef.name`. `gh` prints the bare branch
/// name on its own line (e.g. `master\n`). Returns the first non-empty trimmed
/// line, or `None` when the output is empty / unusable — the caller then falls
/// back to the local origin/HEAD probe and finally `main`. Pure so the parse
/// contract is unit-testable without invoking `gh`. trace:BUG-417 | ai:claude
pub fn parse_gh_default_branch(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .map(|l| l.to_string())
}

/// BUG-417: true when the repository has at least one GitHub Actions workflow
/// file configured (a `.yml`/`.yaml` under `.github/workflows/`). When this is
/// false, `aida pr ship` skips the blocking CI-wait instead of hanging for the
/// full timeout waiting for checks that will never register. Pure over the file
/// names so the "is this a CI workflow file?" rule is unit-testable without a
/// real directory. trace:BUG-417 | ai:claude
pub fn workflow_files_indicate_ci<I, S>(file_names: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    file_names.into_iter().any(|name| {
        let lower = name.as_ref().to_ascii_lowercase();
        lower.ends_with(".yml") || lower.ends_with(".yaml")
    })
}

/// Format the dry-run plan: one line per resolved step, in execution
/// order, prefixed by an arrow. Pure so the contract-visible output is
/// pinned by tests — drift here is what makes the CLI feel inconsistent
/// (mirrors `pr_rebase::manual_recipe`).
pub fn format_dry_run_plan(
    opts: &PrShipOptions,
    steps: &[ShipStep],
    forge: crate::forge::ForgeKind,
) -> String {
    // STORY-508/TASK-651: forge-aware noun + commands so a GitLab dry-run names
    // glab/MR, not gh/PR. `<N>` placeholder since the plan is pre-resolution.
    let noun = forge.change_noun();
    let mut out = String::from("aida pr ship — dry-run plan:\n");
    for (idx, step) in steps.iter().enumerate() {
        let n = idx + 1;
        let desc = match step {
            ShipStep::ResolvePr {
                create_if_needed: true,
            } => format!("resolve {noun} for current branch (create one if none exists)"),
            ShipStep::ResolvePr {
                create_if_needed: false,
            } => match opts.pr_number {
                Some(n) => format!("use {noun}-{n} (explicit)"),
                None => format!("resolve {noun} for current branch"),
            },
            ShipStep::WatchCi => forge
                .ci_watch_cmd("<N>")
                .unwrap_or_else(|| "watch CI to completion".to_string()),
            ShipStep::Merge {
                delete_branch: true,
            } => forge
                .merge_cmd("<N>")
                .unwrap_or_else(|| format!("merge the {noun} and delete the branch")),
            ShipStep::Merge {
                delete_branch: false,
            } => {
                // Name each forge's actual branch-delete flag in the skip note.
                let flag = match forge {
                    crate::forge::ForgeKind::GitHub => "--delete-branch",
                    crate::forge::ForgeKind::GitLab => "--remove-source-branch",
                    crate::forge::ForgeKind::None => "branch delete",
                };
                forge
                    .change_cmd_hint("merge", "<N> --squash")
                    .map(|c| format!("{c}  (skip {flag}: branch protected — sibling worktree or stacked children)"))
                    .unwrap_or_else(|| format!("merge the {noun} (keep the branch: protected — sibling worktree or stacked children)"))
            }
            ShipStep::Pull => "aida pull (from main worktree)".to_string(),
            ShipStep::EndLease => "aida session end <lease>".to_string(),
        };
        out.push_str(&format!("  {n}. {desc}\n"));
    }
    if opts.no_pull {
        out.push_str("  · --no-pull: aida pull step skipped\n");
    }
    if opts.no_cleanup {
        out.push_str("  · --no-cleanup: aida session end step skipped\n");
    }
    if let Some(level) = opts.complexity {
        out.push_str(&format!(
            "  · --complexity {level}: capture implementer self-assessment + punt count to .aida/complexity-calibration/\n"
        ));
    }
    if let Some(effort) = opts.effort {
        out.push_str(&format!(
            "  · --effort {effort}: capture implementation effort to .aida/effort-calibration/\n"
        ));
    }
    out
}

/// Outcome of one ship step, used for both the per-step status line on
/// stderr and the JSONL activity-log entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepOutcome {
    Ok,
    Skipped(String),
    Failed(String),
}

/// Decision for no-arg `aida pr ship` after probing the current branch's
/// open PR. Only a definitive "no change" may fall through to create; lookup
/// failures are inconclusive because the branch may already have an open PR.
// trace:TASK-141 | ai:codex
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchPrResolution {
    Found(u64),
    Create,
    LookupFailed(String),
}

/// Preserve the forge lookup's transient-vs-definitive distinction for
/// `aida pr ship` resume. The old Option collapse treated auth/network/parse
/// failures the same as "no PR", then tried `gh pr create` and produced the
/// misleading already-exists failure this task fixes.
// trace:TASK-141 | ai:codex
pub fn branch_pr_resolution_from_lookup(lookup: &crate::forge::ChangeLookup) -> BranchPrResolution {
    match lookup {
        crate::forge::ChangeLookup::Found(c) => BranchPrResolution::Found(c.id),
        crate::forge::ChangeLookup::NoChange => BranchPrResolution::Create,
        crate::forge::ChangeLookup::CliMissing => BranchPrResolution::LookupFailed(
            "could not resolve an open PR for this branch because the forge CLI is missing; \
             install/authenticate it or re-run `aida pr ship <N>` with the PR number"
                .to_string(),
        ),
        crate::forge::ChangeLookup::CliFailed(reason) => BranchPrResolution::LookupFailed(format!(
            "could not resolve an open PR for this branch; lookup failed: {}. \
             Re-run `aida pr ship <N>` with the existing PR number, or fix the forge CLI lookup.",
            reason.trim()
        )),
        crate::forge::ChangeLookup::Unreachable(reason) => {
            BranchPrResolution::LookupFailed(format!(
                "could not resolve an open PR for this branch because the forge API was unreachable: {}. \
                 Re-run `aida pr ship <N>` with the existing PR number, or retry once the API is reachable.",
                reason.trim()
            ))
        }
    }
}

/// Build a single JSONL activity-log entry (STORY-405 composes here).
/// `now_iso` is injected so tests can pin the timestamp; production
/// callers pass `chrono::Utc::now().to_rfc3339()`.
pub fn format_activity_event(
    now_iso: &str,
    pr_number: Option<u64>,
    step: &ShipStep,
    outcome: &StepOutcome,
) -> String {
    let kind = match step {
        ShipStep::ResolvePr { .. } => "pr-resolve",
        ShipStep::WatchCi => "pr-watch-ci",
        ShipStep::Merge { .. } => "pr-merge",
        ShipStep::Pull => "pr-pull",
        ShipStep::EndLease => "pr-cleanup",
    };
    let (status, detail) = match outcome {
        StepOutcome::Ok => ("ok", String::new()),
        StepOutcome::Skipped(reason) => ("skipped", reason.clone()),
        StepOutcome::Failed(reason) => ("failed", reason.clone()),
    };
    let mut obj = serde_json::Map::new();
    obj.insert(
        "ts".to_string(),
        serde_json::Value::String(now_iso.to_string()),
    );
    obj.insert(
        "command".to_string(),
        serde_json::Value::String("aida pr ship".to_string()),
    );
    obj.insert(
        "step".to_string(),
        serde_json::Value::String(kind.to_string()),
    );
    obj.insert(
        "status".to_string(),
        serde_json::Value::String(status.to_string()),
    );
    if let Some(n) = pr_number {
        obj.insert("pr".to_string(), serde_json::Value::Number(n.into()));
    }
    if !detail.is_empty() {
        obj.insert("detail".to_string(), serde_json::Value::String(detail));
    }
    serde_json::Value::Object(obj).to_string()
}

/// Recovery hint printed when a step fails. Keeps the failure message
/// actionable rather than leaving the user to guess the next move.
/// Pulled out so the hint text is contract-pinned by tests (mirrors
/// `pr_rebase::manual_recipe`).
pub fn recovery_hint(
    step: &ShipStep,
    pr_number: Option<u64>,
    forge: crate::forge::ForgeKind,
) -> String {
    let n = pr_number
        .map(|n| n.to_string())
        .unwrap_or_else(|| "<N>".to_string());
    let noun = forge.change_noun();
    match step {
        ShipStep::ResolvePr { create_if_needed } => {
            if *create_if_needed {
                let create = forge
                    .create_cmd()
                    .unwrap_or_else(|| "create a change request".to_string());
                let list = forge
                    .change_cmd_hint("list", "--head <branch>")
                    .unwrap_or_else(|| "inspect the branch in your forge".to_string());
                format!(
                    "Inspect the branch with `{list}` or run `{create}` manually to debug the create failure."
                )
            } else {
                let view = forge
                    .change_cmd_hint("view", "<N>")
                    .unwrap_or_else(|| format!("inspect the {noun} in your forge"));
                format!("Verify the {noun} number with `{view}` and that it targets this repo.")
            }
        }
        ShipStep::WatchCi => {
            let ci = forge
                .ci_watch_cmd(&n)
                .unwrap_or_else(|| "inspect CI in your forge".to_string());
            let fallback = match forge {
                crate::forge::ForgeKind::GitHub => {
                    " or `gh run list --branch <branch>`".to_string()
                }
                crate::forge::ForgeKind::GitLab => {
                    " or `glab ci list --branch <branch>`".to_string()
                }
                crate::forge::ForgeKind::None => String::new(),
            };
            format!(
                "CI failed or was cancelled. Inspect with `{ci}`{fallback}, fix, push, and re-run `aida pr ship`."
            )
        }
        // The by-hand retry suggestion carries no --delete-branch: a live
        // worktree may still hold the branch, and the refused local delete
        // reads as a merge failure (branch deletion belongs to worktree
        // cleanup). `;` keeps the auto-bump pull from being dropped.
        // trace:BUG-758 trace:TASK-1240 | ai:claude+codex
        ShipStep::Merge { .. } => {
            let merge = forge
                .change_cmd_hint("merge", &format!("{n} --squash"))
                .map(|cmd| format!("{cmd}; aida pull"))
                .unwrap_or_else(|| format!("merge the {noun}; aida pull"));
            format!(
                "Merge step failed (may be transient — the retry wrapper already tried). Re-run `{merge}` once the cause is resolved."
            )
        }
        ShipStep::Pull => "`aida pull` failed. Run it from the main worktree directly to \
             see the underlying git/store error; the merge already landed, \
             so the auto-bump can be replayed via `aida db reconcile-status`."
            .to_string(),
        ShipStep::EndLease => "`aida session end` failed. End the lease manually with \
             `aida session leases` + `aida session end <id>` after \
             investigating."
            .to_string(),
    }
}

/// STORY-529: a spec carrying this tag must NOT be auto-merged by `aida pr
/// ship` — the PR is left a draft for a human to review + merge. Enforces the
/// draft-for-review handoff that briefs alone couldn't (handed-off agents kept
/// self-merging draft-for-review work). trace:STORY-529 | ai:claude
pub const DRAFT_ONLY_TAG: &str = "review:draft-only";

/// True iff any of `tags` marks the spec draft-only (case-insensitive match on
/// [`DRAFT_ONLY_TAG`]). The pure heart of the ship-time draft gate.
/// trace:STORY-529 | ai:claude
pub fn is_draft_only_tagged(tags: &[String]) -> bool {
    tags.iter()
        .any(|t| t.trim().eq_ignore_ascii_case(DRAFT_ONLY_TAG))
}

/// The supervised-merge gate's pure heart. The keystone "Gate + PR, do NOT
/// merge — advisor reviews first" directive used to live only in spec PROSE,
/// which the ship cadence never read — it auto-merged keystone work straight
/// past it. The machine-readable marker is the spec's `execution_mode` (the
/// advisor's bless-time routing, STORY-776): only `drain` licenses an
/// unattended merge. Every other mode — and, fail safe, NO mode at all —
/// holds the merge for an explicit human/advisor.
// trace:BUG-727 | ai:claude
pub fn merge_requires_supervision(mode: Option<aida_core::ExecutionMode>) -> bool {
    mode != Some(aida_core::ExecutionMode::Drain)
}

/// The mode label used in the supervised-merge refusal message — the mode's
/// name, or `unset (supervised)` for a spec that never had one (an unset mode
/// is supervised, fail safe).
// trace:BUG-727 | ai:claude
pub fn supervision_mode_label(mode: Option<aida_core::ExecutionMode>) -> String {
    match mode {
        Some(m) => m.to_string(),
        None => "unset (supervised)".to_string(),
    }
}

/// The `aida pr ship` supervised-merge decision: which of the specs behind the
/// commits about to ship hold the auto-merge, as `(spec-id, mode-label)`
/// pairs. Empty ⇒ the merge may proceed. An interactive human at a TTY IS the
/// explicit human/advisor merge the marker asks for, so the gate never fires
/// there — it fires for non-interactive (automation) callers whose specs carry
/// any `execution_mode` but `drain` (or none at all, the fail-safe).
// trace:BUG-727 | ai:claude
pub fn supervised_merge_holds(
    specs: &[(String, Option<aida_core::ExecutionMode>)],
    interactive_tty: bool,
) -> Vec<(String, String)> {
    if interactive_tty {
        return Vec::new();
    }
    specs
        .iter()
        .filter(|(_, mode)| merge_requires_supervision(*mode))
        .map(|(id, mode)| (id.clone(), supervision_mode_label(*mode)))
        .collect()
}

// ============================================================================
// Cluster PR — the terminal handoff of a coupled single-branch drain
//
// A `--single-branch` drain accumulates EVERY batch member's commits on ONE
// shared branch and then opens ONE pull request for the whole cluster. Two
// things must hold for that one PR to be honest:
//
//   1. a reviewer opening it can see the whole cluster — every member id is
//      named, in drain order, with the shared branch it landed on;
//   2. merging it completes EVERY member, not just the one whose id happens to
//      reach the squash subject.
//
// (2) is why the title carries the ids rather than only the body. The merge
// path already derives its squash subject from the PR title
// (`derive_squash_subject` → `squash_subject_with_spec_ids`), and the
// `aida pull` Done→Completed scan harvests every id from a trailing
// `(ID ID ID)` group on that subject. So a title that ends in the full member
// group makes the multi-spec bump fall out of the existing single-spec
// machinery — no parallel completion path. The body's `## Covers` list is the
// second belt: it repeats one `(SPEC-ID)` per line in exactly the shape the
// referenced-id extractor recognizes, so a squash body carrying the PR body
// credits the same set.
// ============================================================================

/// Trailing `(ID ID ID)` group naming every member of a cluster, in drain
/// order — the exact shape the Done→Completed scan harvests. Empty when there
/// are no members.
// trace:TASK-1136 | ai:claude
fn cluster_spec_id_group(members: &[String]) -> String {
    let ids: Vec<&str> = members
        .iter()
        .map(|m| m.trim())
        .filter(|m| !m.is_empty())
        .collect();
    if ids.is_empty() {
        return String::new();
    }
    format!("({})", ids.join(" "))
}

/// A batch name is free-form and may itself be spec-id-shaped (`epic-54`).
/// Dropped verbatim into a PR title it would be harvested as a *shipped* spec by
/// the broad title/branch/body sweep the squash-subject repair runs, and the
/// cluster merge would then falsely complete it alongside the real members.
/// Neutralize the shape (`epic-54` → `epic_54`) while keeping the name legible;
/// names that can't parse as an id are left exactly as written.
// trace:TASK-1136 | ai:claude
fn cluster_scope_token(batch_name: &str) -> String {
    if extract_spec_ids_from_text(batch_name).is_empty() {
        batch_name.to_string()
    } else {
        batch_name.replace('-', "_")
    }
}

/// Title for the ONE cluster PR a `--single-branch` drain opens. Conventional
/// AIDA commit shape, ending in the trailing spec-id group that names EVERY
/// member — so the squash subject the merge writes onto the default branch
/// carries all of them and the auto-bump completes all of them.
// trace:TASK-1136 | ai:claude
pub fn cluster_pr_title(batch_name: &str, members: &[String]) -> String {
    let n = members.iter().filter(|m| !m.trim().is_empty()).count();
    let base = format!(
        "[AI:claude] feat({}): coupled cluster — {} member{} on one branch",
        cluster_scope_token(batch_name),
        n,
        if n == 1 { "" } else { "s" }
    );
    let group = cluster_spec_id_group(members);
    if group.is_empty() {
        base
    } else {
        format!("{base} {group}")
    }
}

/// Body for the ONE cluster PR: what the mode did, then a `## Covers` list with
/// one member per line ending in its own `(SPEC-ID)` so every member is both
/// human-visible to the reviewer and machine-harvestable by the completion
/// scan. Members are listed in drain order — the order their commits stack on
/// the shared branch.
// trace:TASK-1136 | ai:claude
pub fn cluster_pr_body(batch_name: &str, branch: &str, members: &[String]) -> String {
    let ids: Vec<&str> = members
        .iter()
        .map(|m| m.trim())
        .filter(|m| !m.is_empty())
        .collect();
    let mut out = String::new();
    out.push_str(&format!(
        "Coupled single-branch drain of `batch:{batch_name}`. Every member below was \
         implemented and CI'd in order, committing in place on `{branch}` — no \
         per-member merge to the default branch — so this ONE pull request carries \
         the whole cluster.\n\n"
    ));
    out.push_str("## Covers\n\n");
    if ids.is_empty() {
        out.push_str("_No members landed on the branch._\n");
    } else {
        for (i, id) in ids.iter().enumerate() {
            out.push_str(&format!("- member {} ({})\n", i + 1, id));
        }
    }
    out.push_str(
        "\nMerging this pull request completes every member above: its squash subject \
         names all of them, so the post-merge scan bumps each from Done to Completed \
         in one pass.\n",
    );
    out
}

// ── TASK-1448: merge-time approval-covers-head gate ─────────────────────────

/// Why a merge path refuses a PR whose newest recorded APPROVAL does not
/// provably cover the PR's current head (PRIN-5: an approval that cannot be
/// placed against the head is not evidence the head was reviewed).
///
/// Derived ONLY from `awaiting_you::classify_pr_review` (BUG-1549) — the
/// same classifier the awaiting surface uses — so the merge gate and the
/// "stale approval" row cannot disagree about which approvals are stale.
// trace:TASK-1448 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ApprovalHeadRefusal {
    /// The approval was recorded at a commit the head has since moved past.
    Moved {
        reviewed_sha: String,
        head_sha: String,
    },
    /// The approval records no `reviewed_sha`, so it cannot be tied to any
    /// commit. `head_sha` is empty when the head was unreadable too.
    NoReviewedSha { head_sha: String },
    /// The PR's current head could not be read, so no approval can be shown
    /// to cover it.
    HeadUnreadable { reviewed_sha: String },
    /// Both shas are present but too short to compare with confidence.
    Incomparable {
        reviewed_sha: String,
        head_sha: String,
    },
}

/// TASK-1448: does the newest recorded APPROVAL among `candidates` cover
/// `head`? `None` = the merge may proceed on this gate: either the newest
/// approval was recorded at the head, or there is no approval at all (a PR
/// merged without delegated review is governed by the other ship gates, not
/// this one). `Some` = refuse.
///
/// Only approvals are classified: a refusal is a different question with a
/// different gate (TASK-1169 / the orchestrator's own reviewer phase), and
/// letting a refusal shadow the approval here would let a stale approval
/// merge whenever a refusal also exists.
// trace:TASK-1448 | ai:claude
pub(crate) fn approval_head_refusal(
    candidates: &[crate::review_verdict::RecordedVerdict],
    head: Option<&str>,
) -> Option<ApprovalHeadRefusal> {
    use crate::awaiting_you::{classify_pr_review, PrReviewRow};
    let approvals = open_approvals(candidates);
    let head_sha = head.map(str::trim).unwrap_or("").to_string();
    let row = classify_pr_review(&approvals, head).row?;
    let reviewed_sha = match &row {
        PrReviewRow::Stale { reviewed_sha }
        | PrReviewRow::Unverifiable { reviewed_sha }
        | PrReviewRow::Blocked { reviewed_sha, .. }
        | PrReviewRow::ReworkReady { reviewed_sha, .. } => reviewed_sha.trim().to_string(),
    };
    Some(match row {
        PrReviewRow::Stale { .. } => ApprovalHeadRefusal::Moved {
            reviewed_sha,
            head_sha,
        },
        // Every other row fails closed. Only `Unverifiable` is reachable from
        // an approvals-only candidate set; the rest are listed so a future
        // classifier change cannot silently turn into "merge".
        _ if reviewed_sha.is_empty() => ApprovalHeadRefusal::NoReviewedSha { head_sha },
        _ if head_sha.is_empty() => ApprovalHeadRefusal::HeadUnreadable { reviewed_sha },
        _ => ApprovalHeadRefusal::Incomparable {
            reviewed_sha,
            head_sha,
        },
    })
}

/// TASK-1458: the approvals a merge gate weighs — every APPROVED verdict not
/// yet closed by a merge. A closed approval belongs to a PR that already
/// landed (BUG-1529 treats closed refusals the same way), so it is history,
/// not evidence about this PR's head.
// trace:TASK-1458 | ai:claude
fn open_approvals(
    candidates: &[crate::review_verdict::RecordedVerdict],
) -> Vec<crate::review_verdict::RecordedVerdict> {
    candidates
        .iter()
        .filter(|v| v.kind == crate::review_verdict::VerdictKind::Approved && !v.is_closed())
        .cloned()
        .collect()
}

/// TASK-1458: the commit a merge must be pinned to (`MergeOptions.match_head`)
/// once the approval gate has passed. `Some` only when an open approval
/// exists AND the newest one covers `head` — then the pin is that approved
/// commit, spelled as the longer of the reviewed and head shas (the forge
/// wants a full sha). `None` when there is no approval to pin to, when the
/// head is unknown, or when the gate would refuse.
// trace:TASK-1458 | ai:claude
pub(crate) fn approved_match_head(
    candidates: &[crate::review_verdict::RecordedVerdict],
    head: Option<&str>,
) -> Option<String> {
    let head = head.map(str::trim).filter(|h| !h.is_empty())?;
    let approvals = open_approvals(candidates);
    if approvals.is_empty() || approval_head_refusal(candidates, Some(head)).is_some() {
        return None;
    }
    let reviewed = approvals
        .iter()
        .filter_map(|v| v.reviewed_sha.as_deref().map(str::trim))
        .filter(|sha| crate::forge::head_matches_pin(sha, head))
        .max_by_key(|sha| sha.len())
        .unwrap_or(head);
    Some(if reviewed.len() > head.len() {
        reviewed.to_string()
    } else {
        head.to_string()
    })
}

/// TASK-1458: the reviewed and head shas a refusal names, for the durable
/// `--override-stale-approval` record. Empty string = that sha was missing.
// trace:TASK-1458 | ai:claude
pub(crate) fn approval_head_refusal_shas(refusal: &ApprovalHeadRefusal) -> (String, String) {
    match refusal {
        ApprovalHeadRefusal::Moved {
            reviewed_sha,
            head_sha,
        }
        | ApprovalHeadRefusal::Incomparable {
            reviewed_sha,
            head_sha,
        } => (reviewed_sha.clone(), head_sha.clone()),
        ApprovalHeadRefusal::NoReviewedSha { head_sha } => (String::new(), head_sha.clone()),
        ApprovalHeadRefusal::HeadUnreadable { reviewed_sha } => {
            (reviewed_sha.clone(), String::new())
        }
    }
}

/// TASK-1458: the `.aida/advisor-activity.jsonl` line that durably records an
/// `aida pr ship --override-stale-approval` — which PR, the approval's
/// reviewed sha, the head it was overridden onto, and why the gate refused.
/// Written BEFORE the merge, so the override is on record even when the
/// merge then fails.
// trace:TASK-1458 | ai:claude
pub(crate) fn format_stale_approval_override_event(
    now_iso: &str,
    pr_number: u64,
    refusal: &ApprovalHeadRefusal,
) -> String {
    let (reviewed_sha, head_sha) = approval_head_refusal_shas(refusal);
    serde_json::json!({
        "ts": now_iso,
        "command": "aida pr ship",
        "step": "pr-merge-override-stale-approval",
        "status": "overridden",
        "pr": pr_number,
        "reviewed_sha": reviewed_sha,
        "head_sha": head_sha,
        "detail": approval_head_refusal_message(pr_number, refusal),
    })
    .to_string()
}

/// TASK-1448: the refusal text both merge paths print. Names both shas (or
/// says which is missing) and says what to do: re-review the current head.
// trace:TASK-1448 | ai:claude
pub(crate) fn approval_head_refusal_message(pr: u64, refusal: &ApprovalHeadRefusal) -> String {
    let short = |s: &str| crate::review_verdict::short_sha(s).to_string();
    let why = match refusal {
        ApprovalHeadRefusal::Moved {
            reviewed_sha,
            head_sha,
        } => format!(
            "its approval was recorded at {} but the PR head is now {} — the new commits were never reviewed",
            short(reviewed_sha),
            short(head_sha)
        ),
        ApprovalHeadRefusal::NoReviewedSha { head_sha } => format!(
            "its approval records no reviewed commit, so it cannot be shown to cover the PR head ({})",
            if head_sha.is_empty() {
                "unreadable".to_string()
            } else {
                short(head_sha)
            }
        ),
        ApprovalHeadRefusal::HeadUnreadable { reviewed_sha } => format!(
            "its approval was recorded at {} but the PR's current head could not be read, so the \
             approval cannot be shown to cover it",
            short(reviewed_sha)
        ),
        ApprovalHeadRefusal::Incomparable {
            reviewed_sha,
            head_sha,
        } => format!(
            "its approval sha {} cannot be compared with the PR head {} (too short to tell)",
            reviewed_sha, head_sha
        ),
    };
    format!(
        "PR-{pr} was not merged: {why}. Re-review the current head — e.g. \
         `aida queue work PR-{pr} --role reviewer`, or `aida review record <SPEC> --verdict approved \
         --sha <head>` after reviewing it — then merge again."
    )
}

/// TASK-1448: every recorded verdict a merge gate must weigh for `pr` —
/// the PR-keyed record plus the spec-keyed record for each id in `spec_ids`,
/// read under each of `roots` (the calling worktree and the main clone can
/// both hold `.aida/review-verdicts/`). A file seen under two roots is read
/// once. Unreadable/absent files contribute nothing.
///
/// TASK-1460: when the PR `head` is known, a key whose current file was
/// recorded at some OTHER commit also contributes its archived verdict for
/// `head` (BUG-1539's per-sha archive), so a later round at a different sha
/// cannot hide the review of the commit about to merge. The current file is
/// still read first and always counts — the archive only adds evidence.
// trace:TASK-1448 | ai:claude
// trace:TASK-1460 | ai:claude
pub(crate) fn merge_gate_verdict_candidates(
    roots: &[&std::path::Path],
    pr: u64,
    spec_ids: &[String],
    head: Option<&str>,
) -> Vec<crate::review_verdict::RecordedVerdict> {
    let head = head.map(str::trim).filter(|h| !h.is_empty());
    let mut keys: Vec<String> = vec![format!("PR-{pr}")];
    for id in spec_ids {
        let id = id.trim().to_ascii_uppercase();
        if !id.is_empty() && !keys.contains(&id) {
            keys.push(id);
        }
    }
    let mut seen: Vec<std::path::PathBuf> = Vec::new();
    let mut out = Vec::new();
    for root in roots {
        for key in &keys {
            let path = crate::review_verdict::verdict_path(root, key);
            let canon = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if seen.contains(&canon) {
                continue;
            }
            seen.push(canon);
            let current = std::fs::read_to_string(&path)
                .ok()
                .and_then(|body| crate::review_verdict::parse_recorded_verdict(&body));
            let current_at_head = match (&current, head) {
                (Some(v), Some(h)) => v
                    .reviewed_sha
                    .as_deref()
                    .is_some_and(|r| crate::review_verdict::same_reviewed_sha(r, h)),
                _ => false,
            };
            if let Some(v) = current {
                out.push(v);
            }
            if let (false, Some(h)) = (current_at_head, head) {
                if let Some(archived) = crate::review_verdict::read_verdict_for_sha(root, key, h) {
                    out.push(archived);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    // BUG-1566: pr ship's hold release obeys the integrity floor exactly like
    // `merge-hold clear` — no hold → no gate; hold + human → release; hold
    // without a human → refuse, naming the human release path.
    #[test]
    fn ship_hold_release_requires_the_integrity_floor() {
        assert_eq!(ship_hold_release_refusal(false, false, 7), None);
        assert_eq!(ship_hold_release_refusal(false, true, 7), None);
        assert_eq!(ship_hold_release_refusal(true, true, 7), None);
        let msg = ship_hold_release_refusal(true, false, 7).expect("must refuse");
        assert!(msg.contains("aida merge-hold clear 7"), "{msg}");
        assert!(msg.contains("human"), "{msg}");
    }

    // BUG-1566: the real ship path must consult the floor BEFORE it clears
    // the marker or drops the label — never after, never not at all.
    #[test]
    fn ship_release_site_gates_on_the_floor_before_clearing() {
        let src = include_str!("pr_cmd.rs");
        let gate = src
            .find("pr_ship::ship_hold_release_refusal_for(")
            .expect("pr ship must gate the hold release");
        let floor = src[gate..]
            .find("crate::has_integrity_floor_authority()")
            .expect("the gate must be fed the integrity-floor authority");
        assert!(floor < 400, "authority must be the gate's own argument");
        let clear = src
            .find("crate::merge_hold::clear_hold(&hold_root, pr_number)")
            .expect("release site present");
        let unlabel = src
            .find("crate::merge_hold::sync_label(&hold_root, pr_number, false)")
            .expect("label drop present");
        assert!(
            gate < clear && gate < unlabel,
            "gate must precede the release"
        );
    }

    use super::*;

    // BUG-1499: a label-only hold is refused even for a HUMAN at a terminal
    // (no clearance record would exist); a marker hold keeps the BUG-1566
    // floor behaviour; no hold → no gate.
    // trace:BUG-1499 | ai:claude
    #[test]
    fn label_only_hold_is_refused_even_for_a_human() {
        let msg = ship_hold_release_refusal_for(false, true, true, 7).expect("human refused");
        assert!(msg.contains("aida merge-hold clear 7"), "{msg}");
        assert!(msg.contains("label alone"), "{msg}");
        assert!(ship_hold_release_refusal_for(false, true, false, 7).is_some());
        assert_eq!(ship_hold_release_refusal_for(true, false, true, 7), None);
        assert_eq!(ship_hold_release_refusal_for(true, true, true, 7), None);
        assert!(ship_hold_release_refusal_for(true, false, false, 7).is_some());
        assert_eq!(ship_hold_release_refusal_for(false, false, false, 7), None);
    }

    // BUG-1532: a hold binds `pr ship` with NO drive running — the gate takes
    // no drive input at all. Refusal holds refuse even a human; supervision
    // holds still release for a human (BUG-1167); nothing releases headless.
    // trace:BUG-1532 | ai:claude
    #[test]
    fn any_hold_binds_pr_ship_without_a_live_drive() {
        use crate::merge_hold::{typed_hold, HoldReasonKind, RefusalRelease, VerdictRef};
        let held = |_: &crate::merge_hold::MergeHoldRecord| {
            RefusalRelease::Held(
                "the verdict on record is CHANGES REQUESTED for BUG-1 at 3acf3671fd7a".into(),
            )
        };
        let never = |_: &crate::merge_hold::MergeHoldRecord| -> RefusalRelease {
            panic!("release_of must only be consulted for a refusal hold with a human present")
        };
        assert_eq!(ship_hold_gate(None, false, 9, never), None);
        assert_eq!(ship_hold_gate(None, true, 9, never), None);

        let rework = typed_hold(
            9,
            HoldReasonKind::Rework,
            "CHANGES REQUESTED for BUG-1 at 3acf3671fd7a",
            None,
        );
        let msg = ship_hold_gate(Some(&rework), true, 9, held).expect("refusal refuses a human");
        assert!(msg.contains("rework hold"), "{msg}");
        assert!(msg.contains("CHANGES REQUESTED for BUG-1"), "{msg}");
        assert!(msg.contains("aida merge-hold clear 9"), "{msg}");
        assert!(msg.contains("fresh APPROVED"), "{msg}");
        assert!(msg.contains("at 3acf3671fd7a"), "{msg}");
        assert!(ship_hold_gate(Some(&rework), false, 9, never).is_some());
        // Criterion 4: a fresh APPROVED verdict at the head releases a
        // refusal for a human — and never for a headless seat.
        let fresh = |_: &crate::merge_hold::MergeHoldRecord| {
            RefusalRelease::Released(VerdictRef::new(
                "BUG-1",
                Some(9),
                Some("cd21a1dc0a9e".into()),
                None,
            ))
        };
        assert_eq!(ship_hold_gate(Some(&rework), true, 9, fresh), None);
        assert!(ship_hold_gate(Some(&rework), false, 9, fresh).is_some());

        let mut recusal = typed_hold(9, HoldReasonKind::Recusal, "author", Some("a".into()));
        recusal.recused_principals = vec!["agent:author".into()];
        assert!(ship_hold_gate(Some(&recusal), true, 9, never).is_some());
        let unknown = typed_hold(9, HoldReasonKind::Unknown, "malformed", None);
        assert!(ship_hold_gate(Some(&unknown), true, 9, never).is_some());

        for kind in [HoldReasonKind::Supervision, HoldReasonKind::Decision] {
            let supervised = typed_hold(9, kind, "STORY-1 is marked drive", None);
            assert_eq!(
                ship_hold_gate(Some(&supervised), true, 9, never),
                None,
                "a human's explicit ship IS the decision a {kind:?} hold awaits"
            );
            let headless = ship_hold_gate(Some(&supervised), false, 9, never).expect("floor");
            assert!(headless.contains("human"), "{headless}");
        }

        let dir = tempfile::tempdir().unwrap();
        crate::merge_hold::write_hold(dir.path(), 9, "STORY-1 is marked drive").unwrap();
        let legacy = crate::merge_hold::read_hold_record(dir.path(), 9).unwrap();
        let msg = ship_hold_gate(Some(&legacy), true, 9, held).expect("untyped is not permission");
        assert!(msg.contains("legacy"), "{msg}");
    }

    // BUG-1532: a verdict-released hold merges PINNED to the head the verdict
    // covers, even when the TASK-1448 pin is None (the verdict's key was not a
    // candidate); no readable head → refused. Otherwise the pin is untouched.
    // trace:BUG-1532 | ai:claude
    #[test]
    fn verdict_released_hold_always_merges_pinned_to_the_head() {
        assert_eq!(
            refusal_release_pin(true, None, Some("cd21a1dc0a9e"), 7),
            Ok(Some("cd21a1dc0a9e".to_string()))
        );
        assert_eq!(
            refusal_release_pin(true, Some("old".into()), Some("cd21a1dc0a9e"), 7),
            Ok(Some("cd21a1dc0a9e".to_string()))
        );
        let err = refusal_release_pin(true, None, None, 7).unwrap_err();
        assert!(err.contains("hold stays"), "{err}");
        assert!(refusal_release_pin(true, None, Some("  "), 7).is_err());
        assert_eq!(refusal_release_pin(false, None, None, 7), Ok(None));
        assert_eq!(
            refusal_release_pin(false, Some("abc".into()), Some("def"), 7),
            Ok(Some("abc".to_string()))
        );
        // Wiring: the pin is applied to merge_opts BEFORE the hold is cleared.
        let src = include_str!("pr_cmd.rs");
        let pin = src
            .find("pr_ship::refusal_release_pin(")
            .expect("ship applies the release pin");
        let assign = src
            .find("Ok(pin) => merge_opts.match_head = pin")
            .expect("pin assigned to the merge");
        let clear = src
            .find("crate::merge_hold::clear_hold(&hold_root, pr_number)")
            .expect("release site");
        let merge = src[clear..]
            .find("&merge_opts,")
            .map(|i| i + clear)
            .expect("merge call");
        assert!(pin < assign && assign < clear && clear < merge);
    }

    // BUG-1532: the real ship path consults the hold gate BEFORE the CI watch
    // (so a held PR's red gate is never discounted) and the gate is fed the
    // marker itself, not a drive-liveness or reason-substring predicate.
    #[test]
    fn ship_hold_gate_runs_before_ci_and_ignores_drive_liveness() {
        let src = include_str!("pr_cmd.rs");
        let gate = src
            .find("pr_ship::ship_hold_gate(")
            .expect("pr ship must consult the hold gate");
        let args = &src[gate..gate + 300];
        assert!(args.contains("read_hold_record(&main_worktree, pr_number)"));
        assert!(args.contains("crate::has_integrity_floor_authority()"));
        assert!(!args.contains("drive_specs"));
        let ci = src
            .find("crate::ci_gate::wait_for_checks_to_register(")
            .expect("ci watch present");
        let clear = src
            .find("crate::merge_hold::clear_hold(&hold_root, pr_number)")
            .expect("release site present");
        assert!(
            gate < ci && gate < clear,
            "hold gate must precede CI + release"
        );
    }

    // ── TASK-1448: approval-covers-head merge gate ─────────────────────────
    // trace:TASK-1448 | ai:claude
    const HEAD: &str = "1aca4e3e9251aaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OLD: &str = "08834c6045a9bbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn verdict(
        kind: &str,
        sha: Option<&str>,
        at: Option<&str>,
    ) -> crate::review_verdict::RecordedVerdict {
        crate::review_verdict::RecordedVerdict {
            kind: crate::review_verdict::VerdictKind::parse(kind),
            raw: kind.to_string(),
            reviewed_sha: sha.map(str::to_string),
            recorded_at: at.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn task_1448_approval_at_head_proceeds() {
        let v = [verdict(
            "approved",
            Some(HEAD),
            Some("2026-09-21T04:00:00Z"),
        )];
        assert_eq!(approval_head_refusal(&v, Some(HEAD)), None);
        // An abbreviated approval sha matching the head's prefix is the same commit.
        let short = [verdict("approved", Some(&HEAD[..12]), None)];
        assert_eq!(approval_head_refusal(&short, Some(HEAD)), None);
    }

    #[test]
    fn task_1448_approval_behind_head_refuses_naming_both_shas() {
        let v = [verdict("approved", Some(OLD), Some("2026-09-21T04:00:00Z"))];
        let refusal = approval_head_refusal(&v, Some(HEAD)).expect("must refuse");
        assert_eq!(
            refusal,
            ApprovalHeadRefusal::Moved {
                reviewed_sha: OLD.to_string(),
                head_sha: HEAD.to_string()
            }
        );
        let msg = approval_head_refusal_message(2048, &refusal);
        assert!(msg.contains("PR-2048"), "{msg}");
        assert!(
            msg.contains(&OLD[..12]) && msg.contains(&HEAD[..12]),
            "{msg}"
        );
        assert!(msg.contains("Re-review the current head"), "{msg}");
    }

    #[test]
    fn task_1448_approval_without_sha_refuses() {
        let v = [verdict("approved", None, Some("2026-09-21T04:00:00Z"))];
        let refusal = approval_head_refusal(&v, Some(HEAD)).expect("must refuse");
        assert_eq!(
            refusal,
            ApprovalHeadRefusal::NoReviewedSha {
                head_sha: HEAD.to_string()
            }
        );
        // A blank sha is the same as none.
        let blank = [verdict("approved", Some("  "), None)];
        assert!(matches!(
            approval_head_refusal(&blank, Some(HEAD)),
            Some(ApprovalHeadRefusal::NoReviewedSha { .. })
        ));
        let msg = approval_head_refusal_message(7, &refusal);
        assert!(msg.contains("records no reviewed commit"), "{msg}");
        assert!(msg.contains("Re-review the current head"), "{msg}");
    }

    #[test]
    fn task_1448_head_unreadable_refuses() {
        let v = [verdict("approved", Some(HEAD), None)];
        for head in [None, Some(""), Some("   ")] {
            let refusal = approval_head_refusal(&v, head).expect("must refuse");
            assert_eq!(
                refusal,
                ApprovalHeadRefusal::HeadUnreadable {
                    reviewed_sha: HEAD.to_string()
                }
            );
        }
        let msg = approval_head_refusal_message(
            7,
            &ApprovalHeadRefusal::HeadUnreadable {
                reviewed_sha: HEAD.to_string(),
            },
        );
        assert!(msg.contains("could not be read"), "{msg}");
    }

    #[test]
    fn task_1448_incomparable_sha_refuses() {
        let v = [verdict("approved", Some("1ac"), None)];
        assert_eq!(
            approval_head_refusal(&v, Some(HEAD)),
            Some(ApprovalHeadRefusal::Incomparable {
                reviewed_sha: "1ac".to_string(),
                head_sha: HEAD.to_string()
            })
        );
    }

    #[test]
    fn task_1448_no_approval_is_not_this_gates_business() {
        assert_eq!(approval_head_refusal(&[], Some(HEAD)), None);
        assert_eq!(approval_head_refusal(&[], None), None);
        // A refusal alone is gated elsewhere; this gate classifies approvals only.
        let refusal_only = [verdict("request-changes", Some(OLD), None)];
        assert_eq!(approval_head_refusal(&refusal_only, Some(HEAD)), None);
    }

    #[test]
    fn task_1448_refusal_does_not_shadow_a_stale_approval() {
        let v = [
            verdict("request-changes", Some(HEAD), Some("2026-09-21T05:00:00Z")),
            verdict("approved", Some(OLD), Some("2026-09-21T04:00:00Z")),
        ];
        assert!(matches!(
            approval_head_refusal(&v, Some(HEAD)),
            Some(ApprovalHeadRefusal::Moved { .. })
        ));
    }

    #[test]
    fn task_1448_newest_approval_decides() {
        // Re-review at the new head clears an older stale approval…
        let cleared = [
            verdict("approved", Some(OLD), Some("2026-09-21T04:00:00Z")),
            verdict("approved", Some(HEAD), Some("2026-09-21T05:00:00Z")),
        ];
        assert_eq!(approval_head_refusal(&cleared, Some(HEAD)), None);
        // …but an older at-head approval does not clear a newer stale one.
        let stale = [
            verdict("approved", Some(HEAD), Some("2026-09-21T04:00:00Z")),
            verdict("approved", Some(OLD), Some("2026-09-21T05:00:00Z")),
        ];
        assert!(approval_head_refusal(&stale, Some(HEAD)).is_some());
    }

    // ── TASK-1458: closed approvals, merge pin, override audit ─────────────
    // trace:TASK-1458 | ai:claude

    fn closed(
        mut v: crate::review_verdict::RecordedVerdict,
    ) -> crate::review_verdict::RecordedVerdict {
        v.closed_by_merge = Some("PR-9".into());
        v
    }

    #[test]
    fn task_1458_closed_stale_approval_does_not_refuse() {
        // A stale approval closed by an earlier PR's merge is history.
        let v = [closed(verdict(
            "approved",
            Some(OLD),
            Some("2026-09-21T05:00:00Z"),
        ))];
        assert_eq!(approval_head_refusal(&v, Some(HEAD)), None);
    }

    #[test]
    fn task_1458_closed_approval_cannot_shadow_or_cover() {
        // A newer CLOSED approval at the head must not hide an open stale one…
        let shadow = [
            verdict("approved", Some(OLD), Some("2026-09-21T04:00:00Z")),
            closed(verdict(
                "approved",
                Some(HEAD),
                Some("2026-09-21T05:00:00Z"),
            )),
        ];
        assert!(matches!(
            approval_head_refusal(&shadow, Some(HEAD)),
            Some(ApprovalHeadRefusal::Moved { .. })
        ));
        // …and a closed approval alone is not an approval to pin the merge to.
        let only_closed = [closed(verdict(
            "approved",
            Some(HEAD),
            Some("2026-09-21T05:00:00Z"),
        ))];
        assert_eq!(approved_match_head(&only_closed, Some(HEAD)), None);
    }

    #[test]
    fn task_1458_match_head_is_the_approved_head() {
        let v = [verdict(
            "approved",
            Some(HEAD),
            Some("2026-09-21T04:00:00Z"),
        )];
        assert_eq!(approved_match_head(&v, Some(HEAD)).as_deref(), Some(HEAD));
        // An abbreviated reviewed sha still pins the FULL head sha.
        let short = [verdict(
            "approved",
            Some(&HEAD[..12]),
            Some("2026-09-21T04:00:00Z"),
        )];
        assert_eq!(
            approved_match_head(&short, Some(HEAD)).as_deref(),
            Some(HEAD)
        );
    }

    #[test]
    fn task_1458_no_match_head_without_a_covering_approval() {
        let stale = [verdict("approved", Some(OLD), Some("2026-09-21T04:00:00Z"))];
        assert_eq!(approved_match_head(&stale, Some(HEAD)), None);
        assert_eq!(approved_match_head(&[], Some(HEAD)), None);
        let ok = [verdict(
            "approved",
            Some(HEAD),
            Some("2026-09-21T04:00:00Z"),
        )];
        assert_eq!(approved_match_head(&ok, None), None);
    }

    #[test]
    fn task_1458_override_event_names_both_shas() {
        let refusal = ApprovalHeadRefusal::Moved {
            reviewed_sha: OLD.into(),
            head_sha: HEAD.into(),
        };
        let line = format_stale_approval_override_event("2026-09-23T00:00:00Z", 77, &refusal);
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["step"], "pr-merge-override-stale-approval");
        assert_eq!(v["status"], "overridden");
        assert_eq!(v["pr"], 77);
        assert_eq!(v["reviewed_sha"], OLD);
        assert_eq!(v["head_sha"], HEAD);
        assert!(!line.contains('\n'), "one JSONL line");
    }

    #[test]
    fn task_1448_candidates_read_pr_and_spec_keyed_verdicts_once() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join(".aida").join("review-verdicts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("PR-12.json"),
            format!(r#"{{"verdict":"approved","reviewed_sha":"{OLD}"}}"#),
        )
        .unwrap();
        std::fs::write(
            dir.join("TASK-9.json"),
            format!(r#"{{"verdict":"approved","reviewed_sha":"{HEAD}"}}"#),
        )
        .unwrap();
        std::fs::write(dir.join("TASK-10.json"), "not json").unwrap();
        // The same root twice (worktree == main clone) reads each file once.
        let got = merge_gate_verdict_candidates(
            &[tmp.path(), tmp.path()],
            12,
            &[
                "task-9".to_string(),
                "TASK-10".to_string(),
                "TASK-9".to_string(),
            ],
            None,
        );
        let mut shas: Vec<_> = got.iter().filter_map(|v| v.reviewed_sha.clone()).collect();
        shas.sort();
        let mut want = vec![OLD.to_string(), HEAD.to_string()];
        want.sort();
        assert_eq!(shas, want);
    }

    #[test]
    fn draft_only_tag_detection_is_case_insensitive() {
        // STORY-529: the ship-time draft gate fires on this tag, case-insensitive.
        assert!(is_draft_only_tagged(&["review:draft-only".to_string()]));
        assert!(is_draft_only_tagged(&[
            "batch:x".to_string(),
            "Review:Draft-Only".to_string()
        ]));
        assert!(!is_draft_only_tagged(&[
            "review".to_string(),
            "draft-only".to_string(),
            "papercut".to_string()
        ]));
        assert!(!is_draft_only_tagged(&[]));
    }

    /// BUG-727: only `drain` licenses an unattended merge; every other
    /// execution mode — and NO mode at all (fail safe) — requires supervision.
    #[test]
    fn merge_requires_supervision_truth_table() {
        use aida_core::ExecutionMode as M;
        assert!(!merge_requires_supervision(Some(M::Drain)));
        assert!(merge_requires_supervision(Some(M::Drive)));
        assert!(merge_requires_supervision(Some(M::Guided)));
        assert!(merge_requires_supervision(Some(M::Operator)));
        assert!(merge_requires_supervision(Some(M::Decide)));
        // Fail safe: a spec with no mode set is treated as supervised.
        assert!(merge_requires_supervision(None));
    }

    /// BUG-727: a marked (non-drain) spec shipped by automation holds the
    /// merge — this is the refusal that parks the PR open for the advisor.
    #[test]
    fn supervised_merge_holds_refuses_marked_spec_for_automation() {
        use aida_core::ExecutionMode as M;
        let specs = vec![
            ("TASK-1080".to_string(), Some(M::Drive)),
            ("BUG-714".to_string(), None),
        ];
        let holds = supervised_merge_holds(&specs, false);
        assert_eq!(
            holds,
            vec![
                ("TASK-1080".to_string(), "drive".to_string()),
                ("BUG-714".to_string(), "unset (supervised)".to_string()),
            ]
        );
    }

    /// BUG-727 regression guard: a `drain`-mode spec preserves the current
    /// auto-merge — the normal automation path is unaffected.
    #[test]
    fn supervised_merge_holds_drain_mode_spec_still_auto_merges() {
        use aida_core::ExecutionMode as M;
        let specs = vec![("TASK-500".to_string(), Some(M::Drain))];
        assert!(supervised_merge_holds(&specs, false).is_empty());
        // No specs behind the ship (no store / no trailers) ⇒ nothing to hold.
        assert!(supervised_merge_holds(&[], false).is_empty());
    }

    /// BUG-727: an interactive human at a TTY IS the explicit human/advisor
    /// merge — the gate never fires there, whatever the mode.
    #[test]
    fn supervised_merge_holds_interactive_tty_may_always_merge() {
        use aida_core::ExecutionMode as M;
        let specs = vec![
            ("TASK-1080".to_string(), Some(M::Guided)),
            ("BUG-714".to_string(), None),
        ];
        assert!(supervised_merge_holds(&specs, true).is_empty());
    }

    #[test]
    fn parse_pr_number_from_create_output_canonical() {
        let out = "https://github.com/joemooney/aida/pull/458\n";
        assert_eq!(parse_pr_number_from_create_output(out), Some(458));
    }

    /// BUG-434: the `--delete-branch` guard truth table. Delete only when
    /// nothing protects the branch; `force` overrides every guard.
    #[test]
    fn should_delete_branch_truth_table() {
        // Clean: no sibling, no children → delete.
        assert!(should_delete_branch(false, 0, 0, false));
        // Sibling worktree (BUG-289) → keep.
        assert!(!should_delete_branch(true, 0, 0, false));
        // Stacked child branch (stacks.json) → keep.
        assert!(!should_delete_branch(false, 1, 0, false));
        // Open child PR (gh) → keep.
        assert!(!should_delete_branch(false, 0, 2, false));
        // Both stacked signals → keep.
        assert!(!should_delete_branch(false, 3, 2, false));
        // force overrides sibling + children → delete (deliberate orphan).
        assert!(should_delete_branch(true, 5, 5, true));
        assert!(should_delete_branch(false, 1, 1, true));
    }

    #[test]
    fn explicit_plain_pr_based_on_main_does_not_count_itself_as_child() {
        let target = crate::forge::ChangeRef {
            id: 1489,
            url: String::new(),
            branch: "task-1155-work".to_string(),
            base: "main".to_string(),
            title: None,
        };

        let (branch, retarget_base, child_prs) =
            ship_branch_context("main", Some(&target), &[1489]);

        assert_eq!(branch, "task-1155-work");
        assert_eq!(retarget_base, "main");
        assert!(child_prs.is_empty());
        assert!(should_delete_branch(false, 0, child_prs.len(), false));
    }

    #[test]
    fn explicit_stacked_child_pr_still_protects_source_branch() {
        let target = crate::forge::ChangeRef {
            id: 12,
            url: String::new(),
            branch: "parent-work".to_string(),
            base: "main".to_string(),
            title: None,
        };

        let (branch, retarget_base, child_prs) =
            ship_branch_context("main", Some(&target), &[12, 13]);

        assert_eq!(branch, "parent-work");
        assert_eq!(retarget_base, "main");
        assert_eq!(child_prs, vec![13]);
        assert!(!should_delete_branch(false, 0, child_prs.len(), false));
    }

    #[test]
    fn unrelated_pr_from_plain_shell_is_allowed_during_drain() {
        assert_eq!(ship_merge_block_reason(false, false, false), None);
    }

    #[test]
    fn drive_owned_pr_from_plain_shell_is_blocked_with_owned_reason() {
        assert_eq!(
            ship_merge_block_reason(false, true, false),
            Some(ShipMergeBlockReason::DriveOwnedPr)
        );
    }

    #[test]
    fn drive_seat_blocks_any_pr_with_seat_reason() {
        assert_eq!(
            ship_merge_block_reason(true, false, false),
            Some(ShipMergeBlockReason::DriveSeat)
        );
        assert_eq!(ship_merge_block_reason(true, true, true), None);
    }

    // TASK-1292: the pure PR-keyed liveness decision. trace:TASK-1292 | ai:claude
    #[test]
    fn reviewer_liveness_on_this_pr_when_a_live_member_is_bound_to_it() {
        let members = [(true, Some(1948))];
        assert_eq!(
            reviewer_liveness_for_pr(members, 1948),
            ReviewerLiveness::OnThisPr
        );
    }

    #[test]
    fn reviewer_liveness_elsewhere_when_live_member_bound_to_a_different_pr() {
        let members = [(true, Some(1965))];
        assert_eq!(
            reviewer_liveness_for_pr(members, 1948),
            ReviewerLiveness::Elsewhere
        );
    }

    #[test]
    fn reviewer_liveness_none_with_no_drive_members() {
        let members: [(bool, Option<u32>); 0] = [];
        assert_eq!(
            reviewer_liveness_for_pr(members, 1948),
            ReviewerLiveness::None
        );
    }

    #[test]
    fn reviewer_liveness_none_when_the_only_member_bound_to_this_pr_is_terminal() {
        // A member that finished (`completed`/`failed`, not `in-phase-*`)
        // keeps its `pr` field, but it is no longer LIVE — its reviewer
        // is not running, so it must not block a ship.
        let members = [(false, Some(1948))];
        assert_eq!(
            reviewer_liveness_for_pr(members, 1948),
            ReviewerLiveness::None
        );
    }

    #[test]
    fn reviewer_liveness_ignores_live_member_with_no_pr_bound_yet() {
        // An implementer phase that hasn't discovered a PR yet is live but
        // unbound — it must not read as "elsewhere" (which would be a false
        // "some other reviewer is busy" signal) or "on this PR".
        let members = [(true, None)];
        assert_eq!(
            reviewer_liveness_for_pr(members, 1948),
            ReviewerLiveness::None
        );
    }

    // TASK-1292 regression fixture: the sibling-spec case from the spec's
    // acceptance criteria — a reviewer live on PR-N under spec A must refuse
    // a ship of PR-N even though the drive's "current spec" is B.
    // trace:TASK-1292 | ai:claude
    #[test]
    fn sibling_spec_reviewer_still_blocks_the_pr_it_is_reviewing() {
        // spec A (BUG-1236) is live-reviewing PR #1948; spec B (STORY-1221)
        // is the drive's nominal "current" member but is shelved/terminal
        // and carries no PR binding of its own — exactly the 2026-09-18
        // near-miss shape.
        let members = [(false, None), (true, Some(1948))];
        assert_eq!(
            reviewer_liveness_for_pr(members, 1948),
            ReviewerLiveness::OnThisPr,
            "a live reviewer on PR-1948 under a sibling spec must still be found"
        );
        let is_owned = reviewer_liveness_for_pr(members, 1948) == ReviewerLiveness::OnThisPr;
        assert!(
            ship_merge_block_reason(false, is_owned, false).is_some(),
            "aida pr ship must refuse PR-1948 while the sibling-spec reviewer is live"
        );
        // A ship of an UNRELATED PR (not the one being reviewed) still goes
        // through — this is not a blanket "any live drive blocks everything".
        let unrelated_owned = reviewer_liveness_for_pr(members, 9999) == ReviewerLiveness::OnThisPr;
        assert!(ship_merge_block_reason(false, unrelated_owned, false).is_none());
    }

    #[test]
    fn merge_error_landed_despite_failure_only_when_reprobe_confirms_merged() {
        // BUG-732: a non-zero `gh pr merge` can mean the remote squash merge
        // landed but a post-merge cleanup step failed. Only continue when a
        // fresh PR-state probe confirms MERGED; unknown remains a real failure.
        assert!(merge_error_landed_despite_failure(Some(true)));
        assert!(!merge_error_landed_despite_failure(Some(false)));
        assert!(!merge_error_landed_despite_failure(None));
    }

    #[test]
    fn parse_pr_number_from_create_output_with_preamble() {
        // `gh pr create` sometimes prints "Creating pull request..." lines first.
        let out = "Creating pull request for task-458 into main in joemooney/aida\n\
                   \n\
                   https://github.com/joemooney/aida/pull/458\n";
        assert_eq!(parse_pr_number_from_create_output(out), Some(458));
    }

    #[test]
    fn parse_pr_number_from_create_output_trailing_whitespace() {
        let out = "https://github.com/joemooney/aida/pull/123   \n";
        assert_eq!(parse_pr_number_from_create_output(out), Some(123));
    }

    #[test]
    fn parse_pr_number_from_create_output_query_string() {
        // Defensive: gh occasionally appends a query string in some flows.
        let out = "https://github.com/joemooney/aida/pull/77?foo=bar\n";
        assert_eq!(parse_pr_number_from_create_output(out), Some(77));
    }

    #[test]
    fn parse_pr_number_from_create_output_missing() {
        let out = "no url here\n";
        assert_eq!(parse_pr_number_from_create_output(out), None);
    }

    #[test]
    fn parse_pr_number_from_create_output_empty() {
        assert_eq!(parse_pr_number_from_create_output(""), None);
    }

    #[test]
    fn derive_pr_title_takes_first_nonempty_line() {
        let msg = "feat(pr): add aida pr ship (TASK-458)\n\nLong body here.\n";
        assert_eq!(
            derive_pr_title_from_commit(msg),
            "feat(pr): add aida pr ship (TASK-458)"
        );
    }

    #[test]
    fn derive_pr_title_skips_leading_blank_lines() {
        let msg = "\n\n  subject only after blanks  \nbody\n";
        assert_eq!(
            derive_pr_title_from_commit(msg),
            "subject only after blanks"
        );
    }

    #[test]
    fn derive_pr_body_drops_subject_and_separator() {
        let msg = "subject\n\nfirst body line\nsecond body line\n";
        assert_eq!(
            derive_pr_body_from_commit(msg),
            "first body line\nsecond body line"
        );
    }

    #[test]
    fn derive_pr_body_empty_when_no_body() {
        assert_eq!(derive_pr_body_from_commit("subject only\n"), "");
    }

    #[test]
    fn derive_pr_body_preserves_internal_blank_lines() {
        let msg = "subj\n\npara1\n\npara2\n";
        assert_eq!(derive_pr_body_from_commit(msg), "para1\n\npara2");
    }

    #[test]
    fn extracts_spec_ids_from_pr_title() {
        let title = "feat(store): add cadence (STORY-284)";
        assert_eq!(
            extract_spec_ids_from_text(title),
            vec!["STORY-284".to_string()]
        );
    }

    #[test]
    fn derives_spec_id_from_branch_when_title_has_none() {
        assert_eq!(
            derive_squash_subject_spec_ids("feat(store): add cadence", "task-310", ""),
            vec!["TASK-310".to_string()]
        );
        assert_eq!(
            derive_squash_subject_spec_ids("fix(queue): preserve id", "task-1-127", ""),
            vec!["TASK-1-127".to_string()]
        );
    }

    #[test]
    fn derive_spec_ids_returns_none_when_metadata_has_none() {
        assert!(derive_squash_subject_spec_ids(
            "feat(store): add cadence",
            "feature/store-cadence",
            "no requirement id here",
        )
        .is_empty());
    }

    #[test]
    fn extracts_multiple_spec_ids_in_order() {
        let title = "fix(queue): preserve credits (SPEC-1, SPEC-2)";
        assert_eq!(
            extract_spec_ids_from_text(title),
            vec!["SPEC-1".to_string(), "SPEC-2".to_string()]
        );
    }

    #[test]
    fn squash_subject_appends_missing_spec_id() {
        assert_eq!(
            squash_subject_with_spec_ids(
                "[AI:codex] feat(store): add cadence",
                &["STORY-284".to_string()]
            ),
            "[AI:codex] feat(store): add cadence (STORY-284)"
        );
    }

    #[test]
    fn squash_subject_does_not_duplicate_existing_spec_id() {
        assert_eq!(
            squash_subject_with_spec_ids(
                "[AI:antigravity] fix(mcp): normalize ids (BUG-332)",
                &["BUG-332".to_string()]
            ),
            "[AI:antigravity] fix(mcp): normalize ids (BUG-332)"
        );
    }

    #[test]
    fn trailing_spec_ids_match_auto_bump_shape() {
        assert_eq!(
            extract_trailing_spec_ids_from_subject(
                "[AI:codex] fix(pr-ship): preserve subject (SPEC-410) (#204)"
            ),
            vec!["SPEC-410".to_string()]
        );
        assert!(extract_trailing_spec_ids_from_subject(
            "docs(competitive): implement maintained surface for STORY-260 (#203)"
        )
        .is_empty());
    }

    #[test]
    fn squash_subject_repairs_mid_text_spec_id_before_pr_suffix() {
        let subject = "docs(competitive): implement maintained competitive analysis surface for STORY-260 (#203)";
        assert_eq!(
            squash_subject_with_spec_ids(subject, &["STORY-260".to_string()]),
            "docs(competitive): implement maintained competitive analysis surface for STORY-260 (STORY-260) (#203)"
        );
    }

    #[test]
    fn squash_subject_repairs_missing_codex_subject_before_pr_suffix() {
        let subject = "[AI:codex] feat(store): add configurable auto-push cadence (#201)";
        assert_eq!(
            squash_subject_with_spec_ids(subject, &["STORY-284".to_string()]),
            "[AI:codex] feat(store): add configurable auto-push cadence (STORY-284) (#201)"
        );
    }

    #[test]
    fn squash_subject_prefers_pr_title_over_branch_head_merge_commit() {
        let pr_title = "[AI:codex] feat(autopilot): add drift guard (TASK-1020)";
        let branch_head =
            "Merge main into task-1020 (bring current + pick up drift-guard fix) (TASK-1020)";
        assert_eq!(
            derive_squash_subject(pr_title, "task-1020", "", branch_head),
            Some(pr_title.to_string())
        );
    }

    #[test]
    fn squash_subject_falls_back_to_branch_head_when_pr_title_empty() {
        let branch_head = "[AI:codex] fix(pr-ship): preserve branch subject";
        assert_eq!(
            derive_squash_subject("", "task-142", "", branch_head),
            Some("[AI:codex] fix(pr-ship): preserve branch subject (TASK-142)".to_string())
        );
    }

    #[test]
    fn squash_subject_can_recover_spec_id_from_branch_head() {
        let branch_head = "[AI:codex] fix(pr-ship): preserve subject (TASK-140)";
        assert_eq!(
            derive_squash_subject(
                "[AI:codex] fix(pr-ship): preserve subject",
                "feature/pr-ship-subject",
                "",
                branch_head,
            ),
            Some("[AI:codex] fix(pr-ship): preserve subject (TASK-140)".to_string())
        );
    }

    #[test]
    fn merge_args_include_repaired_subject() {
        let args = merge_args(
            201,
            false,
            Some("[AI:codex] feat(store): add cadence (STORY-284)"),
        );
        assert_eq!(
            args,
            vec![
                "pr",
                "merge",
                "201",
                "--squash",
                "--subject",
                "[AI:codex] feat(store): add cadence (STORY-284)"
            ]
        );
    }

    #[test]
    fn merge_args_without_repair_keep_existing_shape() {
        let args = merge_args(197, true, None);
        assert_eq!(
            args,
            vec!["pr", "merge", "197", "--squash", "--delete-branch"]
        );
    }

    #[test]
    fn gh_checks_empty_output_means_not_registered_yet() {
        assert!(!gh_pr_checks_output_has_registered_checks("", ""));
    }

    #[test]
    fn gh_checks_no_checks_message_means_not_registered_yet() {
        let stderr = "no checks reported on the 'bug-344' branch";
        assert!(!gh_pr_checks_output_has_registered_checks("", stderr));
        assert!(gh_pr_checks_output_is_unregistered("", stderr));
    }

    #[test]
    fn gh_checks_pending_line_means_registered() {
        let stdout = "build\tpending\t0\thttps://github.com/joemooney/aida/actions/runs/1\n";
        assert!(gh_pr_checks_output_has_registered_checks(stdout, ""));
        assert!(!gh_pr_checks_output_is_unregistered(stdout, ""));
    }

    #[test]
    fn gh_checks_failed_line_still_means_registered() {
        let stdout = "test\tfail\t1\thttps://github.com/joemooney/aida/actions/runs/2\n";
        assert!(gh_pr_checks_output_has_registered_checks(stdout, ""));
    }

    // BUG-417: PR base resolves from origin's default branch, not hardcoded main.
    #[test]
    fn parse_gh_default_branch_reads_bare_name() {
        assert_eq!(
            parse_gh_default_branch("master\n").as_deref(),
            Some("master")
        );
        assert_eq!(parse_gh_default_branch("main").as_deref(), Some("main"));
        assert_eq!(
            parse_gh_default_branch("  develop  \n").as_deref(),
            Some("develop")
        );
    }

    #[test]
    fn parse_gh_default_branch_none_on_empty() {
        assert_eq!(parse_gh_default_branch(""), None);
        assert_eq!(parse_gh_default_branch("   \n\n"), None);
    }

    #[test]
    fn parse_gh_default_branch_takes_first_nonempty_line() {
        assert_eq!(
            parse_gh_default_branch("\n\nmaster\nextra").as_deref(),
            Some("master")
        );
    }

    // BUG-417: no .github/workflows YAML ⇒ no CI configured ⇒ skip CI-wait.
    #[test]
    fn workflow_files_indicate_ci_true_for_yaml() {
        assert!(workflow_files_indicate_ci(["ci.yml"]));
        assert!(workflow_files_indicate_ci(["release.yaml"]));
        assert!(workflow_files_indicate_ci(["README.md", "ci.yml"]));
        assert!(workflow_files_indicate_ci(["CI.YML"])); // case-insensitive
    }

    #[test]
    fn workflow_files_indicate_ci_false_when_no_yaml() {
        let empty: [&str; 0] = [];
        assert!(!workflow_files_indicate_ci(empty));
        assert!(!workflow_files_indicate_ci(["README.md", "notes.txt"]));
        assert!(!workflow_files_indicate_ci([".gitkeep"]));
    }

    #[test]
    fn dry_run_plan_full_sequence() {
        let opts = PrShipOptions {
            pr_number: None,
            no_pull: false,
            no_cleanup: false,
            dry_run: true,
            complexity: None,
            effort: None,
        };
        let steps = vec![
            ShipStep::ResolvePr {
                create_if_needed: true,
            },
            ShipStep::WatchCi,
            ShipStep::Merge {
                delete_branch: true,
            },
            ShipStep::Pull,
            ShipStep::EndLease,
        ];
        let plan = format_dry_run_plan(&opts, &steps, crate::forge::ForgeKind::GitHub);
        assert!(plan.contains("1. resolve PR"), "{plan}");
        assert!(plan.contains("2. gh pr checks"), "{plan}");
        assert!(plan.contains("3. gh pr merge"), "{plan}");
        assert!(plan.contains("4. aida pull"), "{plan}");
        assert!(plan.contains("5. aida session end"), "{plan}");
    }

    #[test]
    fn dry_run_plan_explicit_pr_number() {
        let opts = PrShipOptions {
            pr_number: Some(182),
            no_pull: false,
            no_cleanup: false,
            dry_run: true,
            complexity: None,
            effort: None,
        };
        let steps = vec![ShipStep::ResolvePr {
            create_if_needed: false,
        }];
        let plan = format_dry_run_plan(&opts, &steps, crate::forge::ForgeKind::GitHub);
        assert!(plan.contains("PR-182"), "{plan}");
        assert!(plan.contains("explicit"), "{plan}");
    }

    #[test]
    fn dry_run_plan_notes_skipped_flags() {
        let opts = PrShipOptions {
            pr_number: None,
            no_pull: true,
            no_cleanup: true,
            dry_run: true,
            complexity: None,
            effort: None,
        };
        let plan = format_dry_run_plan(&opts, &[], crate::forge::ForgeKind::GitHub);
        assert!(plan.contains("--no-pull"), "{plan}");
        assert!(plan.contains("--no-cleanup"), "{plan}");
    }

    #[test]
    fn dry_run_plan_worktree_aware_merge() {
        let opts = PrShipOptions {
            pr_number: Some(1),
            no_pull: false,
            no_cleanup: false,
            dry_run: true,
            complexity: None,
            effort: None,
        };
        let steps = vec![ShipStep::Merge {
            delete_branch: false,
        }];
        let plan = format_dry_run_plan(&opts, &steps, crate::forge::ForgeKind::GitHub);
        assert!(plan.contains("skip --delete-branch"), "{plan}");
        assert!(plan.contains("sibling worktree"), "{plan}");
    }

    // STORY-508/TASK-651: the dry-run plan is forge-aware — a GitLab project
    // sees glab/MR commands and the glab branch-delete flag, never gh.
    #[test]
    fn dry_run_plan_is_forge_aware_for_gitlab() {
        let opts = PrShipOptions {
            pr_number: None,
            no_pull: false,
            no_cleanup: false,
            dry_run: true,
            complexity: None,
            effort: None,
        };
        let steps = vec![
            ShipStep::ResolvePr {
                create_if_needed: true,
            },
            ShipStep::WatchCi,
            ShipStep::Merge {
                delete_branch: true,
            },
            ShipStep::Merge {
                delete_branch: false,
            },
        ];
        let plan = format_dry_run_plan(&opts, &steps, crate::forge::ForgeKind::GitLab);
        assert!(plan.contains("resolve MR for current branch"), "{plan}");
        assert!(plan.contains("glab ci status"), "{plan}");
        assert!(
            plan.contains("glab mr merge <N> --squash --remove-source-branch"),
            "{plan}"
        );
        assert!(plan.contains("skip --remove-source-branch"), "{plan}");
        assert!(!plan.contains("gh pr"), "{plan}");
    }

    #[test]
    fn dry_run_plan_includes_complexity_capture_when_set() {
        let opts = PrShipOptions {
            pr_number: Some(7),
            no_pull: false,
            no_cleanup: false,
            dry_run: true,
            complexity: Some(crate::complexity_calibration::ComplexityLevel::High),
            effort: None,
        };
        let plan = format_dry_run_plan(&opts, &[], crate::forge::ForgeKind::GitHub);
        assert!(plan.contains("--complexity high"), "{plan}");
        assert!(plan.contains(".aida/complexity-calibration/"), "{plan}");
    }

    #[test]
    fn dry_run_plan_omits_complexity_line_when_absent() {
        let opts = PrShipOptions {
            pr_number: Some(7),
            no_pull: false,
            no_cleanup: false,
            dry_run: true,
            complexity: None,
            effort: None,
        };
        let plan = format_dry_run_plan(&opts, &[], crate::forge::ForgeKind::GitHub);
        assert!(!plan.contains("complexity"), "{plan}");
    }

    #[test]
    fn dry_run_plan_includes_effort_capture_when_set() {
        let opts = PrShipOptions {
            pr_number: Some(7),
            no_pull: false,
            no_cleanup: false,
            dry_run: true,
            complexity: None,
            effort: Some(crate::effort_calibration::EffortBucket::OneDay),
        };
        let plan = format_dry_run_plan(&opts, &[], crate::forge::ForgeKind::GitHub);
        assert!(plan.contains("--effort 1d"), "{plan}");
        assert!(plan.contains(".aida/effort-calibration/"), "{plan}");
    }

    #[test]
    fn activity_event_includes_step_status_and_pr() {
        let ev = format_activity_event(
            "2026-05-22T18:30:00Z",
            Some(458),
            &ShipStep::Merge {
                delete_branch: true,
            },
            &StepOutcome::Ok,
        );
        // Must be valid JSON.
        let v: serde_json::Value = serde_json::from_str(&ev).unwrap();
        assert_eq!(v["command"], "aida pr ship");
        assert_eq!(v["step"], "pr-merge");
        assert_eq!(v["status"], "ok");
        assert_eq!(v["pr"], 458);
        assert_eq!(v["ts"], "2026-05-22T18:30:00Z");
        assert!(v.get("detail").is_none(), "ok should not emit detail");
    }

    #[test]
    fn activity_event_failed_carries_detail() {
        let ev = format_activity_event(
            "2026-05-22T18:30:00Z",
            Some(7),
            &ShipStep::WatchCi,
            &StepOutcome::Failed("CI red on build job".into()),
        );
        let v: serde_json::Value = serde_json::from_str(&ev).unwrap();
        assert_eq!(v["status"], "failed");
        assert_eq!(v["detail"], "CI red on build job");
    }

    #[test]
    fn activity_event_skipped_carries_reason() {
        let ev = format_activity_event(
            "2026-05-22T18:30:00Z",
            None,
            &ShipStep::Pull,
            &StepOutcome::Skipped("--no-pull".into()),
        );
        let v: serde_json::Value = serde_json::from_str(&ev).unwrap();
        assert_eq!(v["status"], "skipped");
        assert_eq!(v["detail"], "--no-pull");
        assert!(v.get("pr").is_none());
    }

    #[test]
    fn branch_pr_resolution_uses_existing_open_pr() {
        // TASK-141: no-arg `pr ship` resumes the branch PR when lookup finds it.
        let lookup = crate::forge::ChangeLookup::Found(crate::forge::ChangeRef {
            id: 1420,
            url: "https://github.com/joemooney/aida/pull/1420".to_string(),
            branch: "task-1020".to_string(),
            base: "main".to_string(),
            title: Some("fix".to_string()),
        });
        assert_eq!(
            branch_pr_resolution_from_lookup(&lookup),
            BranchPrResolution::Found(1420)
        );
    }

    #[test]
    fn branch_pr_resolution_creates_only_after_definitive_no_change() {
        // TASK-141: only a clean empty lookup means it is safe to create.
        assert_eq!(
            branch_pr_resolution_from_lookup(&crate::forge::ChangeLookup::NoChange),
            BranchPrResolution::Create
        );
    }

    #[test]
    fn branch_pr_resolution_does_not_create_after_lookup_failure() {
        // TASK-141: auth/network/parser failures are inconclusive; falling
        // through to create hides the real resume problem behind gh's
        // "a pull request already exists" error.
        for lookup in [
            crate::forge::ChangeLookup::CliMissing,
            crate::forge::ChangeLookup::CliFailed("auth required".to_string()),
            crate::forge::ChangeLookup::Unreachable("connection refused".to_string()),
        ] {
            match branch_pr_resolution_from_lookup(&lookup) {
                BranchPrResolution::LookupFailed(msg) => {
                    assert!(msg.contains("aida pr ship <N>"), "{msg}");
                }
                other => panic!("expected LookupFailed, got {other:?}"),
            }
        }
    }

    #[test]
    fn recovery_hint_names_step_pr_and_action() {
        let h = recovery_hint(
            &ShipStep::Merge {
                delete_branch: true,
            },
            Some(458),
            crate::forge::ForgeKind::GitHub,
        );
        assert!(h.contains("458"), "{h}");
        assert!(h.contains("gh pr merge"), "{h}");
    }

    #[test]
    fn recovery_hint_uses_gitlab_merge_vocabulary() {
        let h = recovery_hint(
            &ShipStep::Merge {
                delete_branch: true,
            },
            Some(458),
            crate::forge::ForgeKind::GitLab,
        );
        assert!(h.contains("458"), "{h}");
        assert!(h.contains("glab mr merge"), "{h}");
        assert!(!h.contains("gh pr"), "{h}");
    }

    #[test]
    fn recovery_hint_pull_mentions_reconcile() {
        let h = recovery_hint(&ShipStep::Pull, Some(1), crate::forge::ForgeKind::GitHub);
        assert!(h.contains("aida db reconcile-status"), "{h}");
    }

    // --- Cluster PR (coupled single-branch drain) --------------------------
    // trace:TASK-1136 | ai:claude

    fn cluster_members() -> Vec<String> {
        vec![
            "TASK-1134".to_string(),
            "TASK-1135".to_string(),
            "TASK-1138".to_string(),
        ]
    }

    /// The ONE cluster PR's title names EVERY member, in drain order, in the
    /// trailing spec-id group the completion scan reads.
    #[test]
    fn cluster_pr_title_names_every_member_in_drain_order() {
        let title = cluster_pr_title("tui-redesign", &cluster_members());
        assert!(
            title.ends_with("(TASK-1134 TASK-1135 TASK-1138)"),
            "{title}"
        );
        assert!(title.contains("3 members"), "{title}");
        assert_eq!(
            extract_trailing_spec_ids_from_subject(&title),
            cluster_members()
        );
    }

    /// One member is still a cluster of one — singular wording, same shape.
    #[test]
    fn cluster_pr_title_handles_single_member() {
        let title = cluster_pr_title("solo", &["BUG-777".to_string()]);
        assert!(title.contains("1 member on one branch"), "{title}");
        assert!(title.ends_with("(BUG-777)"), "{title}");
    }

    /// A halted drain that committed nothing yields a title with no trailing
    /// group rather than an empty `()` the id parsers would choke on.
    #[test]
    fn cluster_pr_title_with_no_members_has_no_empty_group() {
        let title = cluster_pr_title("empty", &[]);
        assert!(!title.contains("()"), "{title}");
        assert!(extract_trailing_spec_ids_from_subject(&title).is_empty());
    }

    /// The body's `## Covers` list gives the reviewer the whole cluster and the
    /// shared branch it landed on.
    #[test]
    fn cluster_pr_body_covers_every_member_and_names_the_branch() {
        let body = cluster_pr_body(
            "tui-redesign",
            "single-branch/tui-redesign-abc1234",
            &cluster_members(),
        );
        assert!(body.contains("## Covers"), "{body}");
        assert!(
            body.contains("single-branch/tui-redesign-abc1234"),
            "{body}"
        );
        for (i, id) in cluster_members().iter().enumerate() {
            assert!(
                body.contains(&format!("- member {} ({})", i + 1, id)),
                "{body}"
            );
        }
    }

    /// THE load-bearing round-trip: the cluster PR title → the squash subject
    /// the merge writes onto the default branch → the `aida pull`
    /// Done→Completed scan. Every member must come back out, not just the
    /// first. This is what makes ONE cluster merge complete ALL N specs
    /// through the existing single-spec auto-bump machinery.
    #[test]
    fn cluster_pr_title_round_trips_all_members_through_the_completion_scan() {
        let members = cluster_members();
        let branch = "single-branch/tui-redesign-abc1234";
        let title = cluster_pr_title("tui-redesign", &members);
        let subject = derive_squash_subject(&title, branch, "", "")
            .expect("cluster title yields a squash subject");
        // GitHub appends its `(#N)` suffix to the landed squash commit.
        let landed = format!("{subject} (#1590)");
        assert_eq!(crate::extract_spec_ids_from_commit(&landed), members);
    }

    /// Second belt: when the squash body carries the PR body, the `## Covers`
    /// list credits the same member set through the referenced-id extractor
    /// (the path a squash-merged umbrella PR's folded specs already ride).
    #[test]
    fn cluster_pr_body_covers_list_is_harvestable_by_the_referenced_id_scan() {
        let members = cluster_members();
        let title = cluster_pr_title("tui-redesign", &members);
        let body = cluster_pr_body("tui-redesign", "single-branch/x", &members);
        // A squash commit: subject line, blank line, then the body.
        let commit = format!("{title}\n\n{body}");
        let mut seen = crate::extract_spec_ids_from_commit(&commit);
        seen.extend(crate::extract_referenced_spec_ids_from_commit(&commit));
        for id in &members {
            assert!(seen.contains(id), "{id} missing from {seen:?}");
        }
    }

    /// A spec-id-shaped batch name (`epic-54`) must NOT be mined as a shipped
    /// spec — otherwise the cluster merge would falsely complete it alongside
    /// the real members. The scope is neutralized; only the members survive the
    /// squash-subject id sweep.
    #[test]
    fn cluster_pr_title_does_not_leak_a_spec_id_shaped_batch_name() {
        let members = cluster_members();
        let title = cluster_pr_title("epic-54", &members);
        assert!(title.contains("feat(epic_54)"), "{title}");
        // The broad sweep the squash-subject repair runs sees ONLY the members.
        assert_eq!(extract_spec_ids_from_text(&title), members);
        let subject = derive_squash_subject(&title, "single-branch/epic-54-abc1234", "", "")
            .expect("cluster title yields a squash subject");
        assert_eq!(
            crate::extract_spec_ids_from_commit(&format!("{subject} (#1590)")),
            members
        );
    }

    /// A batch name that cannot parse as a spec id is left exactly as written.
    #[test]
    fn cluster_pr_title_keeps_an_ordinary_batch_name_verbatim() {
        let title = cluster_pr_title("tui-redesign", &cluster_members());
        assert!(title.contains("feat(tui-redesign)"), "{title}");
    }

    /// A cluster title must not lose members when the merge path re-derives the
    /// subject from the branch name instead (branch names carry no ids), and
    /// must not duplicate ids when it re-appends.
    #[test]
    fn cluster_squash_subject_is_idempotent() {
        let members = cluster_members();
        let title = cluster_pr_title("tui-redesign", &members);
        let once = squash_subject_with_spec_ids(&title, &members);
        let twice = squash_subject_with_spec_ids(&once, &members);
        assert_eq!(once, title);
        assert_eq!(twice, title);
    }
}
