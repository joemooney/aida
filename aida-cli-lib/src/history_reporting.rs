// trace:TASK-3428 | ai:codex
use crate::{events, find_main_worktree_root, history};
use anyhow::Result;

/// STORY-1436: `aida history --kind <KIND>` — one event kind from the local
/// feed (rotated archive included), newest first, with the gate/floor tally
/// for `gate-held`.
// trace:STORY-1436 | ai:claude
pub(crate) fn history_kind_report(
    kind: &str,
    since: Option<&str>,
    until: Option<&str>,
    author: Option<&str>,
    limit: usize,
) -> Result<()> {
    let want = events::normalize_kind_name(kind);
    if !events::EventKind::known_names()
        .iter()
        .any(|n| events::normalize_kind_name(n) == want)
    {
        anyhow::bail!(
            "unknown event kind `{kind}`; known kinds: {}",
            events::EventKind::known_names().join(", ")
        );
    }
    // `--kind` shares `aida history`'s --since/--until flags, so it gets the
    // same time-bound grammar and ordering check as the digest/events views,
    // instead of only the bare-date/RFC3339 forms `events::parse_time_bound`
    // covers.
    // trace:TASK-1502 | ai:claude
    let now = chrono::Utc::now();
    let bound = |v: Option<&str>, flag: &str| -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        v.map(|raw| history::parse_history_bound(raw, flag, now, &chrono::Local))
            .transpose()
    };
    let since_at = bound(since, "--since")?;
    let until_at = bound(until, "--until")?;
    history::validate_window_order(since_at, until_at, &chrono::Local)?;
    let query = events::KindQuery {
        kind: kind.to_string(),
        since: since_at,
        until: until_at,
        who: author.map(str::to_string),
        limit,
    };
    let root = find_main_worktree_root()?;
    let all = events::read_all_with_archive(&root);
    events::kind_report(&all, &query, std::io::stdout().lock())?;
    Ok(())
}

/// STORY-1436: BUG-1535's ambiguous-id refusal fires from dozens of lookup
/// sites; it is recorded ONCE, here, where every refusal surfaces. Only when
/// the error chain carries an `AmbiguousIdError` (no cost otherwise), and
/// only into a project that already has `.aida/` (never creates one).
// trace:STORY-1436 | ai:claude
pub(crate) fn record_ambiguous_id_refusal(err: &anyhow::Error) {
    let Some(ambiguous) = ambiguous_id_in_chain(err) else {
        return;
    };
    let Ok(root) = find_main_worktree_root() else {
        return;
    };
    if !root.join(".aida").is_dir() {
        return;
    }
    events::record_gate_held(
        &root,
        events::GATE_AMBIGUOUS_ID,
        Some(ambiguous.id.to_ascii_uppercase()),
        None,
        &format!(
            "`{}` resolves to {} requirements; refused to pick one",
            ambiguous.id.to_ascii_uppercase(),
            ambiguous.candidates.len()
        ),
    );
}

// trace:STORY-1436 | ai:claude
pub(crate) fn ambiguous_id_in_chain(
    err: &anyhow::Error,
) -> Option<&aida_core::id_collisions::AmbiguousIdError> {
    err.chain()
        .find_map(|e| e.downcast_ref::<aida_core::id_collisions::AmbiguousIdError>())
}

// ─── TASK-965: worktree-tangle gate (substrate-as-bouncer) ─────────────────
//
// AIDA's fan-out incident: a bare agent `git checkout -b`s in the MAIN repo,
// leaving the primary checkout parked on a feature branch, so the next
// `git merge --ff-only origin/main` aborts. We used to defend this with a memory
// RULE only; the substrate-as-bouncer principle says ship a GATE. Two pure
// predicates back the two guards — a spawn-time assertion (`aida agent new`) and
// a stranded-primary alarm (`aida status` / `aida ps`). Kept side-effect-free so
// both are unit-testable without git/leases.
