// trace:TASK-3428 | ai:codex
use colored::Colorize;
// ── Worktree warm-pool CLI (STORY-714) ──────────────────────────────────────
//
// Dispatched before storage init (via handle_worktree_command): the pool
// operates purely on git + the `.aida/worktree-pool/` registry, no requirement
// store. trace:STORY-714 | ai:claude

// ----------------------------------------------------------------------------
// `aida usage` — inspect locally-recorded CLI invocation telemetry.
// trace:STORY-122 | ai:claude
// ----------------------------------------------------------------------------

// ----------------------------------------------------------------------------
// `aida metrics agent-lift` — dogfood agent-lift report (STORY-477).
// ----------------------------------------------------------------------------

// ----------------------------------------------------------------------------
// `aida push` — push code AND the orphan store in one shot.
// trace:FR-264 | ai:claude
// ----------------------------------------------------------------------------

// trace:BUG-1796 | ai:codex
pub(crate) fn pull_skip_warning(message: &str) -> String {
    format!("{} {message}", "Warning:".yellow().bold())
}

pub(crate) fn pull_code_start_line(branch: &str) -> String {
    format!("{} {} ← origin", "Pulling code".cyan().bold(), branch)
}

pub(crate) fn pull_store_start_line() -> String {
    format!("{} aida-store ← origin", "Pulling store".cyan().bold())
}

// ----------------------------------------------------------------------------
// `aida status` — comprehensive project overview, with extra sections when
// the current project is the aida repo itself.
// trace:EPIC-1-001 | ai:claude
// ----------------------------------------------------------------------------
