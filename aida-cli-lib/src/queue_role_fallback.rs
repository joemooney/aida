//! Cross-user visibility for role-routed queue entries.
//!
//! The queue is stored as one YAML file per user id
//! (`registry/queues/<user_id>.yaml`), and every reader historically resolved
//! ONLY the calling shell's own file. A `--for <role>` routing written by user
//! A was therefore invisible to user B who actually wears that role — so an
//! agent advisor seat could not hand work to a human implementer at all.
//!
//! The fix here is **read-side only**. Nothing about storage changes: the
//! stored queue key, the `added_by` stamp, the assignee, and the lease owner
//! are never rewritten. Readers simply ALSO scan the other users' queue files
//! and surface the entries whose `for_role` matches the caller's active role,
//! visibly attributed to the user who routed them.
//!
//! Identity comparison folds through `canonical_user_id` (trim + lowercase) —
//! the one shared helper — so `Joe` and `joe` are the same person and no
//! reader hand-rolls its own case folding.
//
// trace:BUG-774 | ai:claude
// trace:BUG-89 (stored keys untouched) trace:TASK-951 (canonical_user_id fold)

use aida_core::node::canonical_user_id;
use aida_core::QueueEntry;
use anyhow::Result;
use std::collections::HashSet;
use uuid::Uuid;

/// The read surface the fallback needs: enumerate the users holding a queue
/// file, and read one user's entries. Implemented for both queue readers in
/// the CLI (`Storage` and the cache-backed backend) so the statusline / status
/// snapshot fold identity the same way `aida queue list` does.
// trace:BUG-774 | ai:claude
pub(crate) trait QueueFiles {
    fn queue_file_users(&self) -> Result<Vec<String>>;
    fn queue_entries_for(&self, user_id: &str, include_completed: bool) -> Result<Vec<QueueEntry>>;
}

impl QueueFiles for aida_core::Storage {
    fn queue_file_users(&self) -> Result<Vec<String>> {
        self.queue_users()
    }
    fn queue_entries_for(&self, user_id: &str, include_completed: bool) -> Result<Vec<QueueEntry>> {
        self.queue_list(user_id, include_completed)
    }
}

impl QueueFiles for aida_core::CachedGitBackend {
    fn queue_file_users(&self) -> Result<Vec<String>> {
        use aida_core::DatabaseBackend;
        DatabaseBackend::queue_users(self)
    }
    fn queue_entries_for(&self, user_id: &str, include_completed: bool) -> Result<Vec<QueueEntry>> {
        use aida_core::DatabaseBackend;
        DatabaseBackend::queue_list(self, user_id, include_completed)
    }
}

/// Resolve the role whose cross-user routings the caller should see.
///
/// An explicit `--for <role>` is the user's stated interest and wins; the
/// `any` sentinel (which means "unrouted only") disables the fallback
/// entirely; otherwise the active session role is used. `None` = no fallback,
/// so a reader with no role context keeps the historical own-file-only view.
// trace:BUG-774 | ai:claude
pub(crate) fn fallback_role(explicit: Option<&str>, session_role: Option<&str>) -> Option<String> {
    if let Some(raw) = explicit {
        let r = raw.trim();
        if r.is_empty() || r.eq_ignore_ascii_case("any") {
            return None;
        }
        return Some(crate::canonical_role_name(r));
    }
    match session_role.map(str::trim) {
        Some(s) if !s.is_empty() => Some(crate::canonical_role_name(s)),
        _ => None,
    }
}

/// The active session role, as the env var reports it (empty = unset).
// trace:BUG-774 | ai:claude
pub(crate) fn session_role_env() -> Option<String> {
    std::env::var("AIDA_SESSION_ROLE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// `Some(routing_user)` when this entry lives in ANOTHER user's queue file —
/// the attribution a reader shows so a surfaced entry is obviously not the
/// caller's own. Folds identity through `canonical_user_id`.
// trace:BUG-774 trace:TASK-951 | ai:claude
pub(crate) fn routed_by_other_user<'a>(entry: &'a QueueEntry, caller: &str) -> Option<&'a str> {
    if canonical_user_id(&entry.user_id) == canonical_user_id(caller) {
        None
    } else {
        Some(entry.user_id.as_str())
    }
}

/// Pure merge: the caller's own entries, plus every foreign entry routed to
/// `role` whose spec isn't already in the caller's own queue.
///
/// Deduplication is by requirement id — a spec already sitting in your own
/// queue never doubles up because a peer also routed it. Foreign entries are
/// appended after your own (ordered by position, then routing user) so your
/// own ordering is preserved and the borrowed work reads as an addendum.
// trace:BUG-774 | ai:claude
pub(crate) fn merge_role_routed(
    own: Vec<QueueEntry>,
    foreign: Vec<QueueEntry>,
    role: &str,
) -> Vec<QueueEntry> {
    let seen: HashSet<Uuid> = own.iter().map(|e| e.requirement_id).collect();
    let mut extra: Vec<QueueEntry> = Vec::new();
    let mut taken: HashSet<Uuid> = HashSet::new();
    for e in foreign {
        let routed = e
            .for_role
            .as_deref()
            .map(|r| crate::canonical_role_name(r).eq_ignore_ascii_case(role))
            .unwrap_or(false);
        if !routed || seen.contains(&e.requirement_id) || !taken.insert(e.requirement_id) {
            continue;
        }
        extra.push(e);
    }
    extra.sort_by(|a, b| {
        a.position
            .cmp(&b.position)
            .then_with(|| canonical_user_id(&a.user_id).cmp(&canonical_user_id(&b.user_id)))
    });
    let mut out = own;
    out.extend(extra);
    out
}

/// Read the caller's own queue file PLUS every entry in the other users'
/// files routed to `role`. With `role == None` this is exactly the historical
/// own-file-only read.
///
/// A sibling file that fails to read (parse error, race) is skipped rather
/// than failing the whole listing — the caller's own queue is the contract,
/// the fallback is additive.
// trace:BUG-774 | ai:claude
pub(crate) fn queue_list_with_role_fallback<S: QueueFiles + ?Sized>(
    src: &S,
    user_id: &str,
    role: Option<&str>,
    include_completed: bool,
) -> Result<Vec<QueueEntry>> {
    let own = src.queue_entries_for(user_id, include_completed)?;
    let Some(role) = role else {
        return Ok(own);
    };
    Ok(merge_role_routed(
        own,
        collect_foreign_entries(src, user_id, include_completed),
        role,
    ))
}

/// Every queue entry held in a user file other than the caller's. Best-effort:
/// unreadable files are skipped.
// trace:BUG-774 | ai:claude
fn collect_foreign_entries<S: QueueFiles + ?Sized>(
    src: &S,
    user_id: &str,
    include_completed: bool,
) -> Vec<QueueEntry> {
    let Ok(users) = src.queue_file_users() else {
        return Vec::new();
    };
    let me = canonical_user_id(user_id);
    let mut out = Vec::new();
    for u in users {
        if canonical_user_id(&u) == me {
            continue;
        }
        if let Ok(entries) = src.queue_entries_for(&u, include_completed) {
            out.extend(entries);
        }
    }
    out
}

/// Who else, if anyone, has this spec queued. Powers the diagnostic that
/// replaces the misleading "the lease may have been lost" guess when a spec
/// is simply sitting in a peer's queue file.
// trace:BUG-774 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForeignQueueHolder {
    pub user: String,
    pub for_role: Option<String>,
}

/// Scan every OTHER user's queue file for `requirement_id`. Terminal entries
/// are included (`include_completed = true`) — the point is to explain where
/// the entry actually lives, not to filter it.
// trace:BUG-774 | ai:claude
pub(crate) fn queued_by_other_users<S: QueueFiles + ?Sized>(
    src: &S,
    user_id: &str,
    requirement_id: &Uuid,
) -> Vec<ForeignQueueHolder> {
    let Ok(users) = src.queue_file_users() else {
        return Vec::new();
    };
    let me = canonical_user_id(user_id);
    let mut out: Vec<ForeignQueueHolder> = Vec::new();
    for u in users {
        if canonical_user_id(&u) == me {
            continue;
        }
        let Ok(entries) = src.queue_entries_for(&u, /* include_completed */ true) else {
            continue;
        };
        for e in entries {
            if e.requirement_id != *requirement_id {
                continue;
            }
            out.push(ForeignQueueHolder {
                user: u.clone(),
                for_role: e.for_role.clone(),
            });
        }
    }
    out.sort_by(|a, b| {
        canonical_user_id(&a.user)
            .cmp(&canonical_user_id(&b.user))
            .then_with(|| a.for_role.cmp(&b.for_role))
    });
    out.dedup();
    out
}

/// Build the honest "it's queued, just not in YOUR file" diagnostic.
///
/// The old message blamed a lost lease for every not-in-my-queue pickup. When
/// the entry demonstrably lives in a peer's queue file, say so and name both
/// recoveries: wear the role it was routed to, or take it into your own queue.
/// Pure so the wording is unit-testable without a store.
// trace:BUG-774 | ai:claude
pub(crate) fn format_queued_by_other_user_error(
    display_id: &str,
    holders: &[ForeignQueueHolder],
    active_role: Option<&str>,
) -> String {
    let mut msg = format!("`{display_id}` isn't in your queue, but it IS queued:\n");
    for h in holders {
        match &h.for_role {
            Some(r) => msg.push_str(&format!("  · queued by @{} for role {}\n", h.user, r)),
            None => msg.push_str(&format!("  · queued by @{} (unrouted)\n", h.user)),
        }
    }
    let routed_role = holders.iter().find_map(|h| h.for_role.clone());
    match (&routed_role, active_role) {
        (Some(r), Some(active)) if !crate::canonical_role_name(active).eq_ignore_ascii_case(r) => {
            msg.push_str(&format!(
                "  Your active role is {active}, so it isn't routed to you.\n"
            ));
            msg.push_str(&format!(
                "  To work it as {r}: `aida role enter {r}` then `aida queue work {display_id}`\n"
            ));
        }
        (Some(r), None) => {
            msg.push_str("  You have no active role, so role-routed work isn't surfaced.\n");
            msg.push_str(&format!(
                "  To work it: `aida role enter {r}` then `aida queue work {display_id}`\n"
            ));
        }
        _ => {}
    }
    let for_role = routed_role.unwrap_or_else(|| "implementer".to_string());
    msg.push_str(&format!(
        "  To take it into your own queue instead: `aida queue add {display_id} --for {for_role}`"
    ));
    msg
}

#[cfg(test)]
#[path = "tests/bug_774_queue_role_fallback_tests.rs"]
mod bug_774_queue_role_fallback_tests;
