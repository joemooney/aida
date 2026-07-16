//! `aida human` command handler — the 'human' role-vector front door (SPIKE-57,
//! Phase 2): the operator "awaiting you" bottleneck view. Extracted verbatim
//! from `main.rs` (SPIKE-78, pure movement — no behavior change). The shared
//! aggregation lives in `crate::handle_list_human`, which BOTH this front door
//! and the STORY-562 `aida list human` view delegate to, so it stays in
//! `main.rs`.

use anyhow::Result;

use crate::{find_project_root, handle_list_human, last_drain};

/// STORY-562: `aida list human` (and `aida list --human`) — the discoverable
/// "what needs me?" view. The DATA already exists: `burndown explain`
/// classifies every open spec into a bucket and flags which `needs_human()`.
/// This surfaces exactly that human-attention subset under the operator's
/// instinct (`list human`, sibling to the `open`/`closed` status aliases),
/// grouped by reason so it reads as a triage list. We REUSE the
/// [`burndown::explain_reasons`] classifier verbatim — no re-derived buckets.
/// `short` prints just the IDs (composes with `aida list --short`).
// trace:STORY-562 | ai:claude
/// `aida human` — the 'human' role-vector front door (SPIKE-57, Phase 2).
///
/// Bare `aida human` surfaces the bottleneck view: every spec classified
/// human-required (the canonical `burndown::human_required` predicate),
/// grouped by WHY. It is the role-vector entry point that converges with the
/// STORY-562 `aida list human` view rather than competing with it — both
/// delegate to the single `handle_list_human` implementation, so the two front
/// doors can never drift.
///
/// Presence (`home`/`away`/`status`) and `--for human` routing are later phases
/// of SPIKE-57; this verb is just the front door + the named predicate today.
// trace:TASK-746 | ai:claude
pub(crate) fn handle_human_command(
    short: bool,
    backend: &aida_core::CachedGitBackend,
) -> Result<()> {
    // STORY-730: the morning-after banner on `aida status` points the operator at
    // `aida human` — running it means they have looked, so acknowledge the last
    // drain outcome and the banner stops nagging. Best-effort. trace:STORY-730
    if let Ok(root) = find_project_root() {
        last_drain::acknowledge(&root);
    }
    handle_list_human(short, backend)
}
