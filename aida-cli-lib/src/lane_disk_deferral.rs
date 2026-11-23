//! A lane launch refused below the disk floor is deferred, not failed.
//!
//! The lane disk preflight runs before any agent starts, on both the
//! standalone and the drain implementer launch. Its refusal is a
//! [`LaneSpaceRefused`], which the pickup treats as a refusal before any agent
//! started: it rolls back the setup (worktree, branch, lease, claim and status
//! write), so the spec keeps its pre-pickup status. The spec's queue rows stay
//! where they are and carry the reason `deferred: disk below floor at <ts>`.
//! The pickup exits with [`EXIT_CODE`], which a drain reads as a deferral that
//! parks nothing.
//!
//! Nothing relaunches the spec. The scheduled disk guard only frees space;
//! the next ordinary admission (a filler, a drain wave, an operator's
//! `aida queue work`) picks the spec up again and repeats every launch check.
// trace:BUG-2684 | ai:claude

use anyhow::Result;

/// Exit code of a pickup deferred by the lane disk preflight.
pub(crate) const EXIT_CODE: i32 = 36;

/// The queue-note prefix that records a deferral; the timestamp follows.
pub(crate) const NOTE_PREFIX: &str = "deferred: disk below floor at ";

/// Separates the deferral from the rest of a queue row's note.
const NOTE_SEPARATOR: &str = " · ";

/// The lane disk preflight's refusal (as opposed to a failed measurement).
#[derive(Debug)]
pub(crate) struct LaneSpaceRefused {
    pub(crate) at: chrono::DateTime<chrono::Utc>,
    pub(crate) message: String,
}

impl LaneSpaceRefused {
    /// The recorded refusal reason: `deferred: disk below floor at <ts>`.
    pub(crate) fn reason(&self) -> String {
        format!(
            "{NOTE_PREFIX}{}",
            self.at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        )
    }
}

impl std::fmt::Display for LaneSpaceRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}; {}", self.message, self.reason())
    }
}

impl std::error::Error for LaneSpaceRefused {}

/// The disk-floor refusal anywhere in `error`'s chain.
pub(crate) fn refusal(error: &anyhow::Error) -> Option<&LaneSpaceRefused> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<LaneSpaceRefused>())
}

/// `prior` with its deferral replaced by `reason`. Any other note text the
/// row carried is kept after it.
pub(crate) fn deferral_note(prior: Option<&str>, reason: &str) -> String {
    let mut note = reason.to_owned();
    for part in prior
        .unwrap_or_default()
        .split(NOTE_SEPARATOR)
        .filter(|part| {
            let text = part.trim();
            !text.is_empty()
                && !text
                    .strip_prefix(NOTE_PREFIX)
                    .is_some_and(|at| chrono::DateTime::parse_from_rfc3339(at).is_ok())
        })
    {
        note.push_str(NOTE_SEPARATOR);
        note.push_str(part);
    }
    note
}

/// Record `reason` on every existing queue row for `requirement_ids`. The
/// rows keep their position, routing and owner, so the spec stays pickable.
/// A requirement with no queue row gets none. Returns the rows annotated.
pub(crate) fn annotate_queue_rows(
    storage: &aida_core::Storage,
    requirement_ids: &[uuid::Uuid],
    reason: &str,
) -> Result<usize> {
    if requirement_ids.is_empty() {
        return Ok(0);
    }
    annotate_selected(storage, storage.queue_users()?, requirement_ids, reason)
}

// Discover queue owners only. The backend reads and transforms the current
// row under the same lock used for queue edits and removals.
// trace:TASK-3155 | ai:codex
fn annotate_selected(
    storage: &aida_core::Storage,
    users: Vec<String>,
    requirement_ids: &[uuid::Uuid],
    reason: &str,
) -> Result<usize> {
    let requirement_ids = requirement_ids.iter().copied().collect();
    let mut annotated = 0;
    for user in users {
        annotated += storage.queue_update_notes(&user, &requirement_ids, &|prior| {
            deferral_note(prior, reason)
        })?;
    }
    Ok(annotated)
}

/// The pickup's result after a disk-floor refusal rolled its setup back:
/// the reason is recorded on the spec's queue rows, and the error exits
/// [`EXIT_CODE`] so a drain parks nothing.
pub(crate) fn defer(
    storage: &aida_core::Storage,
    scope: &str,
    requirement_ids: &[uuid::Uuid],
    error: anyhow::Error,
) -> anyhow::Error {
    let Some(refused) = refusal(&error) else {
        return error;
    };
    let reason = refused.reason();
    let outcome = match annotate_queue_rows(storage, requirement_ids, &reason) {
        Ok(0) => format!(
            "{scope} has no queue row to keep; run `aida queue work {scope}` again once \
             space is freed"
        ),
        Ok(_) => format!(
            "{scope} stays queued with the reason \"{reason}\"; the next ordinary pickup \
             admits it once the disk guard or `aida worktree gc --apply` frees space"
        ),
        Err(why) => {
            format!("{scope} stays queued, but its queue row could not record the reason: {why:#}")
        }
    };
    anyhow::Error::new(crate::standalone_implementer::StandaloneImplementerExit {
        code: EXIT_CODE,
        message: format!("{error:#}\n{outcome}"),
        vendor_exit: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // trace:TASK-3155 | ai:codex
    #[test]
    fn task_3155_interleaved_queue_edit_and_removal() {
        let root = tempfile::tempdir().unwrap();
        for path in [root.path().to_owned(), root.path().join("queue.db")] {
            check_interleaved_annotation(aida_core::Storage::new(path));
        }
    }

    fn check_interleaved_annotation(storage: aida_core::Storage) {
        let id = uuid::Uuid::new_v4();
        let entry = aida_core::models::QueueEntry {
            user_id: "alice".into(),
            requirement_id: id,
            position: 1000,
            added_by: "alice".into(),
            note: Some(format!("{NOTE_PREFIX}2026-11-19T05:00:00Z")),
            added_at: chrono::Utc::now(),
            for_role: None,
            for_scope: None,
            for_session: None,
            added_by_machine: None,
        };
        storage.queue_add(entry.clone()).unwrap();
        let mut removed = entry.clone();
        removed.user_id = "bob".into();
        storage.queue_add(removed).unwrap();
        let users = storage.queue_users().unwrap();

        let mut edited = entry;
        edited.position = 42;
        edited.added_by = "operator".into();
        if !storage.is_sqlite() {
            edited.for_role = Some("reviewer".into());
        }
        edited.note = Some(format!("{NOTE_PREFIX}2026-11-19T05:00:00Z ·  keep me  "));
        storage.queue_add(edited.clone()).unwrap();
        storage.queue_remove("bob", &id).unwrap();

        let reason = &format!("{NOTE_PREFIX}2026-11-19T06:00:00Z");
        assert_eq!(
            annotate_selected(&storage, users, &[id], reason).unwrap(),
            1
        );
        edited.note = Some(format!("{reason} ·  keep me  "));
        let remaining = storage.queue_entries_for_requirement(&id).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].1, edited);
        let absent = uuid::Uuid::new_v4();
        assert_eq!(annotate_queue_rows(&storage, &[absent], reason).unwrap(), 0);
        assert!(storage
            .queue_entries_for_requirement(&absent)
            .unwrap()
            .is_empty());
    }

    fn refused() -> anyhow::Error {
        anyhow::Error::new(LaneSpaceRefused {
            at: chrono::DateTime::parse_from_rfc3339("2026-11-19T06:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            message: "lane launch refused".into(),
        })
    }

    #[test]
    fn bug_2684_refusal_is_found_through_context_and_names_its_time() {
        let error = refused().context("queue work failed");
        let found = refusal(&error).expect("the refusal survives added context");
        assert_eq!(
            found.reason(),
            "deferred: disk below floor at 2026-11-19T06:00:00Z"
        );
        assert!(format!("{error:#}").contains("deferred: disk below floor at"));
        assert!(refusal(&anyhow::anyhow!("cannot measure lane disk headroom")).is_none());
    }

    #[test]
    fn bug_2684_deferral_note_replaces_only_its_own_reason() {
        let reason = "deferred: disk below floor at 2026-11-19T06:00:00Z";
        assert_eq!(deferral_note(None, reason), reason);
        assert_eq!(deferral_note(Some("  "), reason), reason);
        assert_eq!(
            deferral_note(Some("operator note"), reason),
            format!("{reason} · operator note")
        );
        let earlier = "deferred: disk below floor at 2026-11-19T05:00:00Z · operator note";
        assert_eq!(
            deferral_note(Some(earlier), reason),
            format!("{reason} · operator note")
        );
    }
}
