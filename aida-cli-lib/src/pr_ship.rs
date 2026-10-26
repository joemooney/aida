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

use crate::gate::ship::*;

/// CLI value enum for `aida pr ship --merge-method <squash|rebase>`.
// trace:STORY-1663 | ai:antigravity
#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShipMergeMethod {
    #[default]
    Squash,
    Rebase,
}

/// Resolve the merge method from `--rebase` and `--merge-method`.
// trace:STORY-1663 | ai:antigravity
pub fn resolved_merge_method(
    rebase: bool,
    merge_method: Option<ShipMergeMethod>,
) -> crate::forge::MergeMethod {
    if rebase || matches!(merge_method, Some(ShipMergeMethod::Rebase)) {
        crate::forge::MergeMethod::Rebase
    } else {
        crate::forge::MergeMethod::Squash
    }
}

/// STORY-1663: resolve the requested merge method and reject `--final-slice`
/// (`--complete-spec`) unless the effective merge method is `rebase`.
// trace:STORY-1663 | ai:antigravity
pub fn validate_ship_merge_method(
    rebase: bool,
    merge_method: Option<ShipMergeMethod>,
    final_slice: bool,
) -> Result<crate::forge::MergeMethod, String> {
    let method = resolved_merge_method(rebase, merge_method);
    if final_slice && method != crate::forge::MergeMethod::Rebase {
        return Err(
            "`--final-slice` (`--complete-spec`) requires `--rebase` or `--merge-method rebase`"
                .to_string(),
        );
    }
    Ok(method)
}

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
    /// Wait for gating CI checks (GitHub uses informational-aware row polling).
    // trace:TASK-1331 | ai:codex
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

/// Derive completion intent from an explicit title trailer, then the branch.
/// PR body prose and mid-title references are never completion directives;
/// a body non-completing marker withholds completion from every source (BUG-1919).
// trace:SPEC-410 TASK-1600 | ai:codex
pub fn derive_squash_subject_spec_ids(pr_title: &str, branch: &str, pr_body: &str) -> Vec<String> {
    derive_squash_subject_spec_ids_held(pr_title, branch, pr_body, |_| None)
}

/// [`derive_squash_subject_spec_ids`] with the closure-hold check ship uses
/// ([`spec_completion_hold_reason`]), so a caller crediting the specs a PR
/// completes cannot credit a spec whose branch-name fallback ship withholds.
// trace:BUG-1957 | ai:claude
pub(crate) fn derive_squash_subject_spec_ids_held(
    pr_title: &str,
    branch: &str,
    pr_body: &str,
    completion_held: impl Fn(&str) -> Option<String>,
) -> Vec<String> {
    derive_squash_trailer(pr_title, branch, pr_body, false, |_| true, completion_held).ids
}

/// BUG-1919: PR-body lines that declare the PR a non-completing slice of a
/// multi-PR spec. A whole line (after stripping list/emphasis markup and
/// trailing punctuation, case-insensitively) must equal one of these or start
/// with one followed (after any closing emphasis) by anything but a word
/// character, `-`, `/`, a backtick, or a `.` that continues into a word (as
/// in `no-complete.md`); a mid-prose mention (or a
/// backticked mention such as a changelog bullet) stays descriptive, exactly as
/// TASK-1600 keeps body ids descriptive.
// trace:BUG-1919 | ai:claude
pub const NO_COMPLETE_BODY_MARKERS: &[&str] = &["aida: no-complete", "non-completing slice"];

/// BUG-1919: does the PR body carry a [`NO_COMPLETE_BODY_MARKERS`] line?
// trace:BUG-1919 | ai:claude
pub fn pr_body_declares_non_completing(pr_body: &str) -> bool {
    pr_body.lines().any(|raw| {
        let line = raw
            .trim()
            .trim_start_matches(|c: char| matches!(c, '-' | '*' | '>' | '+') || c.is_whitespace())
            .trim_matches(|c: char| matches!(c, '*' | '_' | '`'))
            .trim_end_matches(|c: char| matches!(c, '.' | ':' | ';' | '!'))
            .trim()
            .to_ascii_lowercase();
        // After any closing emphasis, the marker ends the declaration unless
        // the next character continues it as a word, path, or quote: a word
        // character, `-`, `/`, or a backtick (the changelog-bullet case). A
        // `.` rejects only when it runs into a word character, so
        // `no-complete.md` stays a mention while `no-complete.`, `.**`, `._`,
        // `...`, `.)` and `.—` declare. Everything else (end of line,
        // whitespace, dashes, `:;,!?)…(`) declares.
        // trace:BUG-1957 | ai:claude
        NO_COMPLETE_BODY_MARKERS.iter().any(|&marker| {
            line.strip_prefix(marker).is_some_and(|rest| {
                let mut chars = rest
                    .trim_start_matches(|c: char| matches!(c, '*' | '_'))
                    .chars();
                match chars.next() {
                    None => true,
                    Some(c) if c.is_alphanumeric() || matches!(c, '-' | '/' | '`') => false,
                    Some('.') => !chars.next().is_some_and(char::is_alphanumeric),
                    Some(_) => true,
                }
            })
        })
    })
}

/// BUG-1919: where a squash subject's completing trailer came from, so ship
/// can say so before it merges.
// trace:BUG-1919 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrailerSource {
    /// An explicit trailing `(SPEC-ID)` group on the PR title.
    Title,
    /// Recovered from the branch name because the title carried no trailer.
    Branch,
    /// No trailer: nothing resolvable, or completion was withheld.
    None,
}

/// BUG-1919: the completing trailer `aida pr ship` will write, with its
/// provenance and the reasons any candidate id was withheld.
// trace:BUG-1919 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SquashTrailer {
    pub ids: Vec<String>,
    pub source: TrailerSource,
    /// Human-readable reasons shown to the operator (a non-completing
    /// declaration, a branch-name id whose spec is closure-held, ...).
    pub notes: Vec<String>,
}

impl SquashTrailer {
    fn none(notes: Vec<String>) -> Self {
        SquashTrailer {
            ids: Vec::new(),
            source: TrailerSource::None,
            notes,
        }
    }
}

/// BUG-1919: derive the completing trailer from the title, then the branch
/// name, with two ways to withhold completion:
///
/// - the PR declares itself a non-completing slice (`no_complete`, the
///   `--no-complete` flag, or a [`NO_COMPLETE_BODY_MARKERS`] line in the
///   body) — then no source grants a trailer and the merge completes no spec;
/// - `completion_held(id)` names a reason the spec must stay open (it
///   carries `closure:pending` or has open slice children) — then the
///   branch-name fallback is skipped for that id. An explicit title trailer
///   is still honoured: it is a deliberate directive, and the merge-time
///   closure hold keeps such a spec at Done rather than Completed.
///
/// `resolves` keeps TASK-1600's rule: only ids that resolve unambiguously in
/// the store can carry completion authority.
// trace:BUG-1919 | ai:claude
pub(crate) fn derive_squash_trailer(
    pr_title: &str,
    branch: &str,
    pr_body: &str,
    no_complete: bool,
    resolves: impl Fn(&str) -> bool,
    completion_held: impl Fn(&str) -> Option<String>,
) -> SquashTrailer {
    if no_complete || pr_body_declares_non_completing(pr_body) {
        let how = if no_complete {
            "`--no-complete`"
        } else {
            "PR-body marker"
        };
        // trace:BUG-1957 | ai:claude
        return SquashTrailer::none(vec![format!(
            "non-completing slice ({how}) — no spec is completed by this merge"
        )]);
    }
    // BUG-2203: a title ending in `(Refs X[, Y])` declares a non-completing
    // reference, so neither the title nor the branch-name fallback may grant a
    // trailer (the fallback turned `(Refs BUG-2169)` into `... (BUG-2169)` and
    // completed it). An id named in any `(Refs …)` group of the title never
    // gains completion authority from another source either. Fail closed.
    // trace:BUG-2203 | ai:claude
    let trailing_refs = extract_trailing_refs_spec_ids_from_subject(pr_title);
    if !trailing_refs.is_empty() {
        return SquashTrailer::none(vec![format!(
            "PR title ends in `(Refs {})` — a reference, not a completion trailer; \
             no spec is completed by this merge",
            trailing_refs.join(", ")
        )]);
    }
    let refs_ids = refs_spec_ids_in_text(pr_title);
    let is_ref = |id: &str| refs_ids.iter().any(|r| r.eq_ignore_ascii_case(id));
    let mut notes = Vec::new();
    let title_ids: Vec<String> = extract_trailing_spec_ids_from_subject(pr_title)
        .into_iter()
        .filter(|id| resolves(id))
        .filter(|id| {
            let referenced = is_ref(id);
            if referenced {
                notes.push(format!(
                    "{id} is also named in a `(Refs …)` group — not completed by this merge"
                ));
            }
            !referenced
        })
        .collect();
    if !title_ids.is_empty() {
        return SquashTrailer {
            ids: title_ids,
            source: TrailerSource::Title,
            notes,
        };
    }
    if !notes.is_empty() {
        // The title's only trailer ids were referenced ones: withhold the
        // branch-name fallback too rather than guess. trace:BUG-2203 | ai:claude
        return SquashTrailer::none(notes);
    }
    let mut branch_ids = Vec::new();
    for id in extract_spec_ids_from_text(branch)
        .into_iter()
        .filter(|id| resolves(id))
        .filter(|id| !is_ref(id))
    {
        match completion_held(&id) {
            Some(reason) => notes.push(format!(
                "branch-name fallback withheld for {id}: {reason} — not completed by this merge; \
                 add an explicit `({id})` title trailer to complete it deliberately"
            )),
            None => branch_ids.push(id),
        }
    }
    if branch_ids.is_empty() {
        SquashTrailer::none(notes)
    } else {
        SquashTrailer {
            ids: branch_ids,
            source: TrailerSource::Branch,
            notes,
        }
    }
}

/// STORY-1663: completion trailer derivation for `aida pr ship` across merge
/// methods. Rebase merges produce no squash subject, so slices never
/// auto-complete their spec from the PR title or branch name unless
/// `--final-slice` is explicitly passed.
// trace:STORY-1663 | ai:antigravity
#[allow(clippy::too_many_arguments)]
pub(crate) fn derive_ship_trailer_for_method(
    method: crate::forge::MergeMethod,
    final_slice: bool,
    pr_title: &str,
    branch: &str,
    pr_body: &str,
    no_complete: bool,
    resolves: impl Fn(&str) -> bool,
    completion_held: impl Fn(&str) -> Option<String>,
) -> SquashTrailer {
    if method == crate::forge::MergeMethod::Rebase && !final_slice {
        return SquashTrailer::none(vec![
            "rebase merge (slice; pass `--final-slice` to complete the spec) — no spec is completed by this merge"
                .to_string(),
        ]);
    }
    derive_squash_trailer(
        pr_title,
        branch,
        pr_body,
        no_complete,
        resolves,
        completion_held,
    )
}

/// STORY-1663: true when a commit subject matches the STORY-1488 slice-stack
/// move signature (`refactor(cli-lib): extract <m>.rs` or `extend <m>.rs`).
// trace:STORY-1663 | ai:antigravity
pub fn is_slice_stack_commit_subject(subject: &str) -> bool {
    let trimmed = subject.trim();
    let Some(rest) = trimmed
        .strip_prefix("refactor(cli-lib): extract ")
        .or_else(|| trimmed.strip_prefix("refactor(cli-lib): extend "))
    else {
        return false;
    };
    let module_part = rest.split_whitespace().next().unwrap_or("");
    let Some(stem) = module_part.strip_suffix(".rs") else {
        return false;
    };
    !stem.is_empty()
        && stem
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// STORY-1663: marker recorded on a spec when a rebase merge lands without
/// `--final-slice`, so `aida pull` and `aida db reconcile-status` leave the
/// spec's status unchanged even if a rebased commit subject carried `(SPEC-ID)`.
// trace:STORY-1663 | ai:antigravity
pub const NON_COMPLETING_REBASE_MARKER: &str = "[aida:non-completing-rebase]";

/// STORY-1663: true when a commit subject starts with a `[k/N]` slice prefix
/// (e.g. `[1/3] feat: ...`) where `1 <= k <= N` and `N > 1`.
// trace:STORY-1663 | ai:antigravity
pub fn is_numbered_slice_commit_subject(subject: &str) -> bool {
    let Some(rest) = subject.trim().strip_prefix('[') else {
        return false;
    };
    let Some((inside, after_bracket)) = rest.split_once(']') else {
        return false;
    };
    if !after_bracket.starts_with(char::is_whitespace) {
        return false;
    }
    let Some((k_str, n_str)) = inside.split_once('/') else {
        return false;
    };
    let (Ok(k), Ok(n)) = (k_str.parse::<u32>(), n_str.parse::<u32>()) else {
        return false;
    };
    k >= 1 && n > 1 && k <= n
}

/// STORY-1663: true when a commit subject is a slice commit (`refactor(cli-lib):
/// extract|extend <m>.rs` or `[k/N] ...`) that should not auto-complete its
/// owning spec during `aida pull` / `aida db reconcile-status`.
// trace:STORY-1663 | ai:antigravity
pub fn is_non_completing_slice_commit_subject(subject: &str) -> bool {
    is_slice_stack_commit_subject(subject) || is_numbered_slice_commit_subject(subject)
}

/// BUG-1919: the reason `spec` must not be completed by a branch-name
/// fallback, or `None` when nothing holds it: the spec carries the
/// `closure:pending` tag, or its slice subtree still has open children
/// (anything not Done / Completed / Rejected / Superseded / Deferred). A Done
/// child is finished on its branch but not yet merged; it is treated as
/// resolved anyway, accepting that the parent can be completed while that
/// child's merge is still pending, rather than holding the parent on work
/// that is already finished. An id that does not resolve yields `None`;
/// `resolves` already filters those out.
// trace:BUG-1919 | ai:claude
pub(crate) fn spec_completion_hold_reason(
    store: &aida_core::RequirementsStore,
    spec: &str,
) -> Option<String> {
    let req = store.get_requirement_unambiguous(spec).ok().flatten()?;
    let tag = aida_core::pickability::CLOSURE_PENDING_TAG;
    if req.tags.iter().any(|t| t.trim().eq_ignore_ascii_case(tag)) {
        return Some(format!("the spec carries the `{tag}` tag"));
    }
    let rollup = aida_core::graph_walk::child_status_rollup(store, req.id);
    // trace:BUG-1957 | ai:claude
    let open = rollup.total.saturating_sub(
        rollup.done + rollup.completed + rollup.rejected + rollup.superseded + rollup.deferred,
    );
    if open > 0 {
        return Some(format!(
            "{open} of {} slice child(ren) still open",
            rollup.total
        ));
    }
    None
}

/// BUG-1919: the one-line (plus notes) provenance ship prints before the
/// merge: which ids the squash subject will carry and where they came from.
// trace:BUG-1919 | ai:claude
pub fn format_trailer_provenance(trailer: &SquashTrailer) -> String {
    let joined = trailer.ids.join(" ");
    let mut out = match trailer.source {
        // BUG-2203: name the ids this merge will complete, not just the trailer.
        // trace:BUG-2203 | ai:claude
        TrailerSource::Title => format!(
            "completion trailer ({joined}) — from the PR title; this merge will complete {joined}"
        ),
        TrailerSource::Branch => format!(
            "completion trailer ({joined}) — from the branch name; this merge will complete {joined}"
        ),
        TrailerSource::None => {
            "completion trailer: none — this merge completes no spec".to_string()
        }
    };
    for note in &trailer.notes {
        out.push_str("\n    ");
        out.push_str(note);
    }
    out
}

/// The `--dry-run` plan's trailer-provenance line: the same text the real
/// ship prints before it merges, or why it could not be resolved yet (the PR
/// does not exist, or its metadata could not be read).
// trace:BUG-1957 | ai:claude
pub fn format_dry_run_trailer_provenance(trailer: Result<&SquashTrailer, &str>) -> String {
    match trailer {
        Ok(trailer) => format!("  · {}\n", format_trailer_provenance(trailer)),
        Err(why) => format!("  · completion trailer: not resolved — {why}\n"),
    }
}

/// Ensure the squash subject carries every derived spec ID exactly once.
/// Existing well-formed subjects are left untouched.
// trace:SPEC-410 BUG-339 | ai:codex
pub fn squash_subject_with_spec_ids(subject: &str, ids: &[String]) -> String {
    let subject = subject.trim();
    if ids.is_empty() || subject.is_empty() {
        return subject.to_string();
    }
    // BUG-2203: never turn a `(Refs X)` subject into a completing one.
    // trace:BUG-2203 | ai:claude
    if !extract_trailing_refs_spec_ids_from_subject(subject).is_empty() {
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
    derive_squash_subject_resolving(
        pr_title,
        branch,
        pr_body,
        branch_head_commit_message,
        |_| true,
    )
}

/// The shipping path supplies store resolution so ID-shaped prose such as
/// SLICE-1 cannot acquire completion authority. Remove unresolved IDs even
/// when they already appear in a title trailer.
// trace:TASK-1600 | ai:codex
pub(crate) fn derive_squash_subject_resolving(
    pr_title: &str,
    branch: &str,
    pr_body: &str,
    branch_head_commit_message: &str,
    resolves: impl Fn(&str) -> bool,
) -> Option<String> {
    derive_squash_subject_with_trailer(
        pr_title,
        branch,
        pr_body,
        branch_head_commit_message,
        false,
        resolves,
        |_| None,
    )
    .map(|(subject, _)| subject)
}

/// BUG-1919: [`derive_squash_subject_resolving`] with the suppression inputs
/// and the trailer's provenance returned alongside the subject, so ship can
/// print where the completing trailer came from before it merges.
// trace:BUG-1919 | ai:claude
pub(crate) fn derive_squash_subject_with_trailer(
    pr_title: &str,
    branch: &str,
    pr_body: &str,
    branch_head_commit_message: &str,
    no_complete: bool,
    resolves: impl Fn(&str) -> bool,
    completion_held: impl Fn(&str) -> Option<String>,
) -> Option<(String, SquashTrailer)> {
    let title = derive_pr_title_from_commit(pr_title);
    let base = if title.is_empty() {
        derive_pr_title_from_commit(branch_head_commit_message)
    } else {
        title
    };
    if base.is_empty() {
        return None;
    }
    let trailer = derive_squash_trailer(
        &base,
        branch,
        pr_body,
        no_complete,
        resolves,
        completion_held,
    );
    let clean_base = without_spec_trailer(&base);
    Some((
        squash_subject_with_spec_ids(&clean_base, &trailer.ids),
        trailer,
    ))
}

fn without_spec_trailer(subject: &str) -> String {
    let (tail, suffix) = match trailing_paren_group(subject) {
        Some((head, inner))
            if inner
                .strip_prefix('#')
                .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) =>
        {
            (head.trim_end(), format!(" ({inner})"))
        }
        _ => (subject, String::new()),
    };
    match trailing_paren_group(tail) {
        Some((head, inner)) if !parse_spec_id_group(inner).is_empty() => {
            format!("{}{suffix}", head.trim_end())
        }
        _ => subject.to_string(),
    }
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

/// BUG-2199: the spec ids a commit subject names for AUTHORSHIP — its
/// trailing completion group (see [`extract_trailing_spec_ids_from_subject`])
/// plus any `(Refs X[, Y...])` group, which names the work's spec without
/// completing it. Never use this for completion: `Refs` must not close a spec.
// trace:BUG-2199 | ai:claude
pub(crate) fn extract_authorship_spec_ids_from_subject(subject: &str) -> Vec<String> {
    let mut out = extract_trailing_spec_ids_from_subject(subject);
    let mut rest = subject.trim();
    while let Some((head, inner)) = trailing_paren_group(rest) {
        // One Refs grammar for authorship and completion (BUG-2203).
        // trace:BUG-2199 BUG-2203 | ai:claude
        out.extend(refs_group_spec_ids(inner));
        rest = head.trim_end();
    }
    out
}

/// BUG-2203: the ids of a subject's trailing `(Refs X[, Y])` group (the
/// non-completing reference form), optionally followed by GitHub's `(#N)`.
/// Empty when the trailing group is anything else.
// trace:BUG-2203 | ai:claude
pub fn extract_trailing_refs_spec_ids_from_subject(subject: &str) -> Vec<String> {
    let mut tail = subject.trim();
    if let Some((head, inner)) = trailing_paren_group(tail) {
        let pr_suffix = inner
            .strip_prefix('#')
            .filter(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()));
        if pr_suffix.is_some() {
            tail = head.trim_end();
        }
    }
    match trailing_paren_group(tail) {
        Some((_, inner)) => refs_group_spec_ids(inner),
        None => Vec::new(),
    }
}

/// BUG-2203: every id named in any `(Refs …)` group of `text` (a subject or a
/// whole commit message). Merge-time completion skips these: a commit that
/// says `Refs X` anywhere is never evidence that X is finished, even when a
/// bare `(X)` trailer was appended after it. Upper-cased, deduplicated.
// trace:BUG-2203 | ai:claude
pub fn refs_spec_ids_in_text(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (open_at, _) in text.match_indices('(') {
        let rest = &text[open_at + 1..];
        let Some(close_at) = rest.find(')') else {
            break;
        };
        for id in refs_group_spec_ids(&rest[..close_at]) {
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

/// The ids of one paren group's inner text when it reads `Refs X[, Y]` (also
/// `Ref`, `Refs:`, any case); empty otherwise.
fn refs_group_spec_ids(inner: &str) -> Vec<String> {
    let inner = inner.trim_start();
    let word_len = inner
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(inner.len());
    let word = &inner[..word_len];
    if !(word.eq_ignore_ascii_case("refs") || word.eq_ignore_ascii_case("ref")) {
        return Vec::new();
    }
    let rest = inner[word_len..].trim_start();
    let rest = rest.strip_prefix(':').unwrap_or(rest);
    extract_spec_ids_from_text(rest)
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

/// BUG-732: after a forge merge command returns non-zero, decide whether the
/// caller should keep going because a fresh forge-state probe shows the squash
/// merge actually landed and only post-merge cleanup failed.
// trace:BUG-732 | ai:codex
pub fn merge_error_landed_despite_failure(pr_is_merged_after_error: Option<bool>) -> bool {
    matches!(pr_is_merged_after_error, Some(true))
}

/// Build the `gh pr merge` argv. Kept pure so SPEC-410 can pin the
/// contract that the wrapper passes `--subject` when it repairs a squash
/// subject. TASK-1330: an explicit `body` (the identity-sanitized squash
/// body) is passed as `--body`, which also suppresses GitHub's auto-derived
/// `Co-authored-by:` trailers.
// trace:SPEC-410 | ai:codex
// trace:TASK-1330 | ai:claude
pub fn merge_args(
    pr_number: u64,
    delete_branch: bool,
    subject: Option<&str>,
    body: Option<&str>,
) -> Vec<String> {
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
    if let Some(body) = body {
        args.push("--body".to_string());
        args.push(body.to_string());
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

/// TASK-1582: the placeholder the dry-run plan prints for the TASK-1458
/// approval pin (`MergeOptions.match_head`). The plan runs before the
/// approval gate reads the head, so the literal sha is not yet known.
pub const DRY_RUN_PIN_PLACEHOLDER: &str = "<approved-head-sha>";

/// TASK-1582: the merge step as the real ship runs it, with the PR number
/// and pin as placeholders. GitHub's line is rendered from
/// [`crate::forge::github_merge_argv`] — the argv builder the real merge
/// uses — so the flag shape tracks it (the old plan printed a bare
/// `gh pr merge <N> --squash` and omitted `--match-head-commit`). The plan's
/// footnote covers what a pre-resolution plan cannot know: whether a pin
/// applies and whether a repaired `--subject` is appended. GitLab merges
/// through the merge API rather than `glab mr merge`, so its line is the
/// familiar CLI equivalent plus an explicit abbreviation note.
/// `None` = no forge command (pure git).
// trace:TASK-1582 | ai:antigravity
pub fn dry_run_merge_cmd(forge: crate::forge::ForgeKind, delete_branch: bool) -> Option<String> {
    dry_run_merge_cmd_for_method(forge, crate::forge::MergeMethod::Squash, delete_branch)
}

// trace:STORY-1663 | ai:antigravity
pub fn dry_run_merge_cmd_for_method(
    forge: crate::forge::ForgeKind,
    method: crate::forge::MergeMethod,
    delete_branch: bool,
) -> Option<String> {
    match forge {
        crate::forge::ForgeKind::GitHub => {
            let opts = crate::forge::MergeOptions {
                method,
                squash_subject: None,
                squash_body: None, // trace:TASK-1330 | ai:claude
                delete_branch,
                match_head: Some(DRY_RUN_PIN_PLACEHOLDER.to_string()),
            };
            let argv: Vec<String> = crate::forge::github_merge_argv(0, &opts)
                .into_iter()
                .enumerate()
                .map(|(i, a)| if i == 2 { "<N>".to_string() } else { a })
                .collect();
            Some(format!("gh {}", argv.join(" ")))
        }
        crate::forge::ForgeKind::GitLab => {
            let method_flag = match method {
                crate::forge::MergeMethod::Squash => "--squash",
                crate::forge::MergeMethod::Rebase => "--rebase",
                crate::forge::MergeMethod::Merge => "",
            };
            let base = if delete_branch && method == crate::forge::MergeMethod::Squash {
                forge.merge_cmd("<N>")?
            } else if method_flag.is_empty() {
                forge.change_cmd_hint("merge", "<N>")?
            } else {
                forge.change_cmd_hint("merge", &format!("<N> {method_flag}"))?
            };
            Some(format!(
                "{base}  (abbreviated: AIDA merges through the GitLab merge API with sha=<MR head read at merge time>, checked against {DRY_RUN_PIN_PLACEHOLDER})"
            ))
        }
        crate::forge::ForgeKind::None => None,
    }
}

/// TASK-1582: how step 5 identifies a lease in operator-facing text — its id
/// plus the SCOPE, ROLE and WORKTREE PATH, so an agent can tell a stale
/// implementer lease from its own live coordination session before acting
/// (the id alone once told a mid-cascade integrator to end its own session).
// trace:TASK-1582 | ai:antigravity
pub fn describe_lease_for_step5(
    id: &str,
    scope: &str,
    role: Option<&str>,
    worktree: &std::path::Path,
) -> String {
    let role = role
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .unwrap_or("unrecorded");
    format!(
        "lease {id} (scope {scope}, role {role}, worktree {})",
        worktree.display()
    )
}

/// TASK-1582: step 5's instruction when this shell sits inside the shipped
/// branch's lease worktree, so `aida session end` would refuse. Names the
/// lease's scope/role/worktree and tells a caller whose OWN session it is
/// to leave it alone. Returns `(stderr line, activity-log detail)`.
// trace:TASK-1582 | ai:antigravity
pub fn step5_inside_worktree_messages(
    id: &str,
    scope: &str,
    role: Option<&str>,
    worktree: &std::path::Path,
    branch: &str,
) -> (String, String) {
    let lease = describe_lease_for_step5(id, scope, role, worktree);
    let line = format!(
        "  step 5: {lease} holds the shipped branch {branch}, and this shell is inside its worktree, \
         so it was not ended.\n          If that is your own active session, leave it running; otherwise \
         exit this shell, then run `aida session end {id}` (check `aida session leases` first)."
    );
    let detail = format!(
        "shell inside {lease} for branch {branch} — not ended; run `aida session end {id}` after \
         exiting unless it is the caller's own session"
    );
    (line, detail)
}

/// BUG-1953: step 5's report when no lease holds the shipped branch. Names
/// the PR head branch (`ship_branch`), not the cwd branch — shipping by
/// number from `main` once printed "no AIDA lease for branch main" while the
/// head branch's lease survived the merge. Returns `(stderr line, activity-log
/// detail)`.
// trace:BUG-1953 | ai:claude
pub fn step5_no_lease_messages(ship_branch: &str) -> (String, String) {
    (
        format!("  step 5: no AIDA lease for branch {ship_branch} — nothing to clean up"),
        format!("no lease for branch {ship_branch}"),
    )
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
    format_dry_run_plan_with_method(opts, steps, forge, crate::forge::MergeMethod::Squash)
}

// trace:STORY-1663 | ai:antigravity
pub fn format_dry_run_plan_with_method(
    opts: &PrShipOptions,
    steps: &[ShipStep],
    forge: crate::forge::ForgeKind,
    method: crate::forge::MergeMethod,
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
            } => dry_run_merge_cmd_for_method(forge, method, true)
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
                dry_run_merge_cmd_for_method(forge, method, false)
                    .map(|c| format!("{c}  (skip {flag}: branch protected — sibling worktree or stacked children)"))
                    .unwrap_or_else(|| format!("merge the {noun} (keep the branch: protected — sibling worktree or stacked children)"))
            }
            ShipStep::Pull => "aida pull (from main worktree)".to_string(),
            ShipStep::EndLease => "aida session end <lease>".to_string(),
        };
        out.push_str(&format!("  {n}. {desc}\n"));
    }
    // TASK-1582: say what the pin placeholder stands for. trace:TASK-1582 | ai:antigravity
    if steps.iter().any(|s| matches!(s, ShipStep::Merge { .. }))
        && forge != crate::forge::ForgeKind::None
    {
        out.push_str(&format!(
            "  · {DRY_RUN_PIN_PLACEHOLDER}: the head the merge is pinned to, resolved at merge time — the approved head when an approval covers it (the forge then refuses a moved head); with no covering approval the pin is omitted. A repaired `--subject` is appended when the squash subject needs its trailer.\n"
        ));
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

#[cfg(test)]
#[path = "tests/pr_ship_tests.rs"]
mod tests;
