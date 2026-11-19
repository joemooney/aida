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
        .map(str::trim)
        .filter(|part| !part.is_empty() && !part.starts_with(NOTE_PREFIX))
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
    let mut annotated = 0;
    for id in requirement_ids {
        for (_, mut entry) in storage.queue_entries_for_requirement(id)? {
            let note = deferral_note(entry.note.as_deref(), reason);
            if entry.note.as_deref() == Some(note.as_str()) {
                continue;
            }
            entry.note = Some(note);
            storage.queue_add(entry)?;
            annotated += 1;
        }
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
