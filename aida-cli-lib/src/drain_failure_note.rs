//! A drain phase failure is recorded as a **comment on the parent spec**, not
//! as its own draft spec.
//!
//! Before this module a `--auto-complete` phase failure filed a Draft BUG
//! (TASK-266). A draft spec demands a disposition from a human; a comment
//! simply informs the next reader of the parent. The narrative record moves,
//! and nothing else does: the non-zero exit, the telemetry ledger line, and
//! the seat routing are untouched.
//!
//! Everything here is pure string work so it can be asserted without a store.
// trace:TASK-1564 | ai:claude

/// Stable HTML-comment marker every drain-failure note carries, so the
/// recurrence scan can tell its own notes from an operator's prose.
pub(crate) const MARKER: &str = "<!-- aida:drain-phase-failure -->";

pub(crate) const ATTEMPTS_PREFIX: &str = "Attempts:";
pub(crate) const LATEST_PREFIX: &str = "Latest recurrence:";

/// Everything a note carries. The caller resolves these; this module only
/// renders and re-reads them.
pub(crate) struct NoteContext<'a> {
    /// The parent spec the drain was driving — the spec this note lands on.
    pub spec: &'a str,
    /// 1-based phase index, as the orchestrator numbers phases.
    pub phase_index: u8,
    /// Phase slug (`implementer`, `ci`, `reviewer`, …) — also the seat name.
    pub phase_slug: &'a str,
    /// The seat the failed phase ran as, plus the session role when one is set.
    pub seat: &'a str,
    /// Stable failure-kind slug (`ci-red`, `no-pr`, `spawn`, …).
    pub failure_kind: &'a str,
    /// The captured failure text the orchestrator classified.
    pub failure_reason: &'a str,
    /// The recovery hint the orchestrator showed the operator.
    pub hint: &'a str,
    /// Branch under test, when it could be resolved.
    pub branch: Option<&'a str>,
    /// Commit under test, when it could be resolved.
    pub commit: Option<&'a str>,
    /// RFC3339 — when the drain finished.
    pub completed_at: &'a str,
    /// Pre-rendered per-phase wall times.
    pub durations: &'a str,
    /// The JSONL ledger line for this run.
    pub telemetry_json: &'a str,
}

/// The `(phase, failure-kind)` fingerprint a recurrence of the same failure
/// matches on. Carried verbatim in the note body — the recurrence scan reads
/// it back rather than re-parsing prose.
pub(crate) fn signature(phase_index: u8, phase_slug: &str, failure_kind: &str) -> String {
    format!("Signature: phase {phase_index} ({phase_slug}) · kind `{failure_kind}`")
}

/// Whether `body` is one of our notes for this exact signature.
pub(crate) fn matches_signature(body: &str, sig: &str) -> bool {
    body.contains(MARKER) && body.contains(sig)
}

/// The explicit promotion path, embedded in every note: the case where the
/// failure really is a distinct defect and does want its own spec.
pub(crate) fn promotion_recipe(spec: &str, phase_index: u8, phase_slug: &str) -> String {
    format!(
        "If this is a distinct defect rather than a drain hiccup, promote it to its own spec \
         (it is NOT promoted automatically):\n\n\
         1. Save this note's text to a file, e.g. `/tmp/note.md`.\n\
         2. `aida add --type bug --priority medium --parent {spec} --title \"drain phase \
         {phase_index} ({phase_slug}) failure on {spec}\" --description-from-file /tmp/note.md`\n\
         3. `aida edit <NEW-ID> --status approved`\n"
    )
}

/// Render a fresh note. A recurrence re-uses the same body through
/// [`increment_auto_failure_attempts`] rather than appending a second note.
pub(crate) fn render(ctx: &NoteContext<'_>) -> String {
    let branch = ctx.branch.unwrap_or("(unresolved)");
    let commit = ctx.commit.unwrap_or("(unresolved)");
    format!(
        "{MARKER}\n\
         ## Drain phase failure — phase {phase_index} ({phase_slug})\n\n\
         Recorded by `aida queue work {spec} --auto-complete`. This is a narrative record on \
         the parent spec, not a new spec: it informs the next reader and asks nothing of \
         anyone. The drain still exited non-zero and still wrote its telemetry line.\n\n\
         - {sig}\n\
         - Seat: {seat}\n\
         - Parent spec: {spec}\n\
         - Branch under test: {branch}\n\
         - Commit under test: {commit}\n\
         - Recorded at: {completed_at}\n\n\
         ### Failure\n\n\
         {failure_reason}\n\n\
         Failure kind: `{failure_kind}`\n\n\
         ### Recovery hint shown to the operator\n\n\
         {hint}\n\n\
         ## Recurrence\n\n\
         {ATTEMPTS_PREFIX} 1\n\
         {LATEST_PREFIX} {completed_at}\n\n\
         ### Phase durations\n\n\
         {durations}\n\n\
         ### Telemetry entry\n\n\
         The line appended to `~/.aida/auto-complete.jsonl` for this run:\n\n\
         ```json\n{telemetry_json}\n```\n\n\
         ### Promoting this to its own spec\n\n\
         {recipe}",
        phase_index = ctx.phase_index,
        phase_slug = ctx.phase_slug,
        spec = ctx.spec,
        sig = signature(ctx.phase_index, ctx.phase_slug, ctx.failure_kind),
        seat = ctx.seat,
        branch = branch,
        commit = commit,
        completed_at = ctx.completed_at,
        failure_reason = ctx.failure_reason,
        failure_kind = ctx.failure_kind,
        hint = ctx.hint,
        durations = ctx.durations,
        telemetry_json = ctx.telemetry_json,
        recipe = promotion_recipe(ctx.spec, ctx.phase_index, ctx.phase_slug),
    )
}

/// Bump the recurrence counter and latest-recurrence stamp in an existing
/// record, adding the `## Recurrence` block when the record predates it.
///
/// Shared by the note path here and the legacy auto-drafted-BUG absorb path in
/// `lib.rs`, which is why it lives in this module rather than beside either
/// caller.
// trace:BUG-864 trace:TASK-1564 | ai:claude
pub(crate) fn increment_auto_failure_attempts(description: &str, latest_at: &str) -> String {
    let mut attempts_seen = false;
    let mut latest_seen = false;
    let mut current_attempts = 1;
    for line in description.lines() {
        if let Some(raw) = line.trim().strip_prefix(ATTEMPTS_PREFIX) {
            current_attempts = raw.trim().parse::<u32>().unwrap_or(1);
            break;
        }
    }
    let next_attempts = current_attempts.saturating_add(1);
    let mut lines = Vec::new();
    for line in description.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(ATTEMPTS_PREFIX) {
            lines.push(format!("{ATTEMPTS_PREFIX} {next_attempts}"));
            attempts_seen = true;
        } else if trimmed.starts_with(LATEST_PREFIX) {
            lines.push(format!("{LATEST_PREFIX} {latest_at}"));
            latest_seen = true;
        } else {
            lines.push(line.to_string());
        }
    }
    if !attempts_seen || !latest_seen {
        if !lines.is_empty() && lines.last().is_some_and(|l| !l.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push("## Recurrence".to_string());
        lines.push(String::new());
        if !attempts_seen {
            lines.push(format!("{ATTEMPTS_PREFIX} {next_attempts}"));
        }
        if !latest_seen {
            lines.push(format!("{LATEST_PREFIX} {latest_at}"));
        }
    }
    lines.join("\n")
}
