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

/// BUG-710/BUG-716: substrate-as-bouncer decision — should `aida pr ship`
/// REFUSE its merge step? An implementer running inside an orchestrated drive
/// must not self-merge its own PR: `aida zen` promises an INDEPENDENT reviewer
/// before the auto-merge, and a phase-1 self-merge bypasses it (the failure the
/// codex TASK-1115/1119 *headless* and TASK-1123 *supervised* drives exposed).
/// The implementer's job is to OPEN the PR; the orchestrator's CI + reviewer +
/// merge phases finish it.
///
/// BUG-710 gated only on `AIDA_HEADLESS=1`, so a `--supervised` (interactive)
/// implementer slipped past and self-merged (BUG-716). The caller now passes
/// `in_orchestrated_drive` = `AIDA_HEADLESS` OR a LIVE drain lock
/// (`probe_lock == Running`) — present in every drive mode (headless AND
/// supervised), absent for a plain `aida queue work <spec>` session and for a
/// stale post-crash lock (BUG-712). One explicit opt-in
/// (`AIDA_PR_SHIP_ALLOW_IN_DRIVE=1`) covers a deliberate in-drive direct-publish.
/// Pure so the decision is unit-testable without the process env or a live drive.
// trace:BUG-710 trace:BUG-716 | ai:claude
pub fn should_block_ship_merge(in_orchestrated_drive: bool, override_allow: bool) -> bool {
    in_orchestrated_drive && !override_allow
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
pub fn gh_pr_checks_output_has_registered_checks(stdout: &str, stderr: &str) -> bool {
    let combined = format!("{stdout}\n{stderr}");
    let trimmed = combined.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("no checks reported")
        || lower.contains("no checks found")
        || lower.contains("no check runs")
        || lower.contains("no checks have been reported")
    {
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
    let lower = combined.to_ascii_lowercase();
    lower.contains("no checks reported")
        || lower.contains("no checks found")
        || lower.contains("no check runs")
        || lower.contains("no checks have been reported")
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
pub fn recovery_hint(step: &ShipStep, pr_number: Option<u64>) -> String {
    let n = pr_number
        .map(|n| n.to_string())
        .unwrap_or_else(|| "<N>".to_string());
    match step {
        ShipStep::ResolvePr { create_if_needed } => {
            if *create_if_needed {
                "Inspect the branch with `gh pr list --head <branch>` or run `gh pr create` \
                 manually to debug the create failure."
                    .to_string()
            } else {
                "Verify the PR number with `gh pr view <N>` and that it targets this repo."
                    .to_string()
            }
        }
        ShipStep::WatchCi => format!(
            "CI failed or was cancelled. Inspect with `gh pr checks {n}` or \
             `gh run list --branch <branch>`, fix, push, and re-run `aida pr ship`."
        ),
        ShipStep::Merge { .. } => format!(
            "Merge step failed (may be transient — the retry wrapper already \
             tried). Re-run `gh pr merge {n} --squash --delete-branch` once \
             the cause is resolved."
        ),
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn should_block_ship_merge_truth_table() {
        // BUG-710/BUG-716: the first arg is "inside an orchestrated drive" —
        // AIDA_HEADLESS OR a live drain lock — so it now covers BOTH the
        // headless AND the supervised/interactive implementer.
        // Inside a drive (headless or supervised) → REFUSE the self-merge.
        assert!(should_block_ship_merge(true, false));
        // Not in a drive (plain session / human at the keyboard) → allow.
        assert!(!should_block_ship_merge(false, false));
        // In a drive but the explicit opt-in is set → allow (deliberate publish).
        assert!(!should_block_ship_merge(true, true));
        // Override with no drive context is a no-op → still allowed.
        assert!(!should_block_ship_merge(false, true));
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
    fn recovery_hint_names_step_pr_and_action() {
        let h = recovery_hint(
            &ShipStep::Merge {
                delete_branch: true,
            },
            Some(458),
        );
        assert!(h.contains("458"), "{h}");
        assert!(h.contains("gh pr merge"), "{h}");
    }

    #[test]
    fn recovery_hint_pull_mentions_reconcile() {
        let h = recovery_hint(&ShipStep::Pull, Some(1));
        assert!(h.contains("aida db reconcile-status"), "{h}");
    }
}
