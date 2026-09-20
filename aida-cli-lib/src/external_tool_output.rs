//! Central registry for unavoidable classifiers over external-tool prose.
//!
//! Prefer structured output at the call site. These strings exist only where
//! the producer does not expose the distinction structurally; keeping them in
//! one registry makes upstream wording drift auditable.

// trace:BUG-1310 | ai:codex

/// GitHub CLI 2.78.x: `gh pr checks` before Actions registers a check suite.
pub(crate) const GH_NO_REGISTERED_CHECKS: &[&str] = &[
    "no checks reported",
    "no checks found",
    "no check runs",
    "no checks have been reported",
];

/// GitHub CLI 2.78.x: merge failures that specifically permit rebase recovery.
pub(crate) const GH_MERGE_CONFLICT: &[&str] = &[
    "not mergeable",
    "merge conflict",
    "merge commit cannot be cleanly created",
    "conflicts must be resolved",
];

/// SQLite 3.x / rusqlite: transient writer-lock diagnostics.
pub(crate) const SQLITE_LOCKED: &[&str] = &["database is locked", "database table is locked"];

/// Linux, macOS, and common toolchain spellings for exhausted disk or memory.
pub(crate) const OS_RESOURCE_EXHAUSTION: &[&str] = &[
    "no space left on device",
    "disk full",
    "out of disk",
    "enospc",
    "out of memory",
    "cannot allocate memory",
    "oom-kill",
    "oomkilled",
];

pub(crate) fn contains_any_case_insensitive(message: &str, needles: &[&str]) -> bool {
    let lower = message.to_ascii_lowercase();
    needles.iter().any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_matches_case_insensitively_and_fails_closed_on_unknown_prose() {
        assert!(contains_any_case_insensitive(
            "DATABASE IS LOCKED",
            SQLITE_LOCKED
        ));
        assert!(!contains_any_case_insensitive(
            "some new, unclassified upstream diagnostic",
            GH_MERGE_CONFLICT
        ));
    }
}
