// trace:TASK-3428 | ai:codex
use crate::current_user_id;

/// Count queue entries in the user's queue file that pass the role filter.
/// When `role` is `Some`, returns the count of entries with `for_role`
/// matching exactly. When `role` is `None`, returns the total entry
/// count (no role filter).
///
/// Reads `<project>/.aida-store/registry/queues/<user>.yaml` directly —
/// keeps statusline off the heavier Storage::load() path so the sub-50ms
/// budget holds even when the orphan store has hundreds of objects.
/// Returns `None` if the file is missing or unreadable.
/// trace:FR-1-041 | ai:claude
/// TASK-648 (ADR-3): non-archived, non-deferred draft count from the cache,
/// read-only and fast (same SQLite-without-Cache::open pattern as the
/// statusline freshness probe — no migration, no write lock on the prompt hot
/// path). The advisor's statusline shows this as the active draft backlog to
/// triage. Returns 0 when the cache is absent or unreadable rather than
/// erroring the prompt.
// trace:BUG-1171 | ai:codex
pub(crate) fn read_draft_backlog_depth(cache_path: &std::path::Path) -> usize {
    if !cache_path.exists() {
        return 0;
    }
    let conn = match rusqlite::Connection::open_with_flags(
        cache_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    let has_deferred_column = conn
        .prepare("SELECT deferred FROM requirements_cache LIMIT 0")
        .is_ok();
    let sql = if has_deferred_column {
        "SELECT COUNT(*) FROM requirements_cache \
         WHERE archived = 0 AND LOWER(status) = 'draft' AND deferred = 0"
    } else {
        "SELECT COUNT(*) FROM requirements_cache \
         WHERE archived = 0 AND LOWER(status) = 'draft'"
    };
    conn.query_row(sql, [], |row| row.get::<_, i64>(0))
        .map(|n| n.max(0) as usize)
        .unwrap_or(0)
}

/// Count URGENT unread mailbox messages for the current shell user, for the
/// statusline nag. Returns `None` when there is no mailbox to count (stay
/// silent), `Some(0)` when caught up.
///
/// This MUST agree with `aida mailbox inbox`: the statusline count and the
/// read command are two surfaces of one "unread" state, so reading the inbox
/// has to drive the count to 0. Both read the same logical view and identity
/// union (STORY-1843). Finding the repository's product worktrees runs one
/// bounded `git worktree list`, so this is no longer pure file I/O; the
/// passive read deadline caps it, and a source not read in time makes the
/// view partial rather than blocking the statusline.
// trace:STORY-539 trace:BUG-625 | ai:claude
pub(crate) fn read_urgent_unread_count(project_root: &std::path::Path) -> Option<usize> {
    read_urgent_unread_count_for(project_root, &crate::mailbox_view::own_audience())
}

/// [`read_urgent_unread_count`] for a given verified audience. STORY-1843:
/// the count reads the same logical view (every worktree of this repository
/// plus the canonical store) and the same read evidence the inbox marks, so
/// reading the inbox clears it. `None` when there is no visible mail. The
/// statusline segment shows only a positive urgent count; a partial view is
/// surfaced by the status, notice and awaiting surfaces.
// trace:TASK-2345 trace:STORY-1843 | ai:claude
pub(crate) fn read_urgent_unread_count_for(
    project_root: &std::path::Path,
    audience: &aida_core::mailbox_logical::ResolvedAudience,
) -> Option<usize> {
    let view = crate::mailbox_view::read_own_mail(
        project_root,
        &project_root.join(".aida-store"),
        audience,
        &crate::mailbox_view::passive_limits(),
    );
    if view.records().is_empty() {
        return None;
    }
    Some(view.unread().iter().filter(|m| m.urgent).count())
}

pub(crate) fn read_queue_depth(
    project_root: &std::path::Path,
    role: Option<&str>,
) -> Option<usize> {
    // BUG-89 + BUG-675: resolve identity through the SAME two-step path
    // `aida queue list` uses — `current_user_id` first, then the queue-file
    // case-fold (`resolve_queue_user`, TASK-951/TASK-845) — so a shell reporting
    // `Joe` counts the queue keyed under `joe.yaml` instead of reading zero.
    // Reading `<current_user_id>.yaml` directly skipped that fold, so the depth
    // diverged from the queue-list count on a case-only identity mismatch; the
    // BUG-670 band-aid then suppressed the divergent number instead of fixing the
    // resolution. trace:BUG-675 trace:BUG-89 | ai:claude
    let store_path = project_root.join(".aida-store");
    let user = aida_core::db::resolve_queue_user(&store_path, &current_user_id(None));

    let queue_path = store_path
        .join("registry/queues")
        .join(format!("{}.yaml", user));

    // An absent own-queue file is zero own items, NOT "unknown" — a peer may
    // still have routed work to this role, and bailing here would hide it.
    // A file that exists but won't parse stays `None` (a real read failure).
    // trace:BUG-774 | ai:claude
    let entries: Vec<serde_yaml::Value> = match std::fs::read_to_string(&queue_path) {
        Ok(content) if content.trim().is_empty() => Vec::new(),
        Ok(content) => serde_yaml::from_str(&content).ok()?,
        Err(_) => Vec::new(),
    };

    let count = match role {
        Some(want) => entries
            .iter()
            .filter(|e| {
                e.get("for_role")
                    .and_then(serde_yaml::Value::as_str)
                    .map(|r| r == want)
                    .unwrap_or(false)
            })
            .count(),
        None => entries.len(),
    };
    // A `--for <role>` routing written by a PEER lands in that peer's queue
    // file, so a depth read scoped to our own file under-counted the work
    // actually routed to us — the meter disagreed with `aida queue list`.
    // Add the sibling files' entries routed to this role. Read-only; the
    // stored keys are untouched and identity folds through the shared
    // canonical helper. trace:BUG-774 | ai:claude
    let cross_user = match role {
        Some(want) => cross_user_role_routed_depth(&store_path, &user, want),
        None => 0,
    };
    Some(count + cross_user)
}

/// Count entries routed to `role` that live in OTHER users' queue files.
/// Best-effort + allocation-light (the same raw-YAML read the own-file depth
/// uses); an unreadable sibling file contributes zero rather than failing the
/// meter.
// trace:BUG-774 | ai:claude
pub(crate) fn cross_user_role_routed_depth(
    store_path: &std::path::Path,
    self_user: &str,
    role: &str,
) -> usize {
    let dir = store_path.join("registry/queues");
    let Ok(read) = std::fs::read_dir(&dir) else {
        return 0;
    };
    let me = aida_core::node::canonical_user_id(self_user);
    let mut total = 0usize;
    for entry in read.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if aida_core::node::canonical_user_id(stem) == me {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(rows) = serde_yaml::from_str::<Vec<serde_yaml::Value>>(&content) else {
            continue;
        };
        total += rows
            .iter()
            .filter(|e| {
                e.get("for_role")
                    .and_then(serde_yaml::Value::as_str)
                    .map(|r| r == role)
                    .unwrap_or(false)
            })
            .count();
    }
    total
}
