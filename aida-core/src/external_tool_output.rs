//! Central registry for unavoidable classifiers over external-tool prose.
//!
//! Prefer structured output at the call site. These strings exist only where
//! the producer does not expose the distinction structurally; keeping them in
//! one workspace-wide registry makes upstream wording drift auditable.

// trace:BUG-1310 | ai:codex

/// Return whether `message` contains any lowercase registry token, ignoring
/// ASCII case. Registry tokens must themselves be lowercase.
pub fn contains_any_case_insensitive(message: &str, needles: &[&str]) -> bool {
    let lower = message.to_ascii_lowercase();
    needles.iter().any(|needle| lower.contains(needle))
}

/// GitHub CLI 2.81.0: `gh pr checks` before Actions registers a check suite.
pub const GH_NO_REGISTERED_CHECKS: &[&str] = &[
    "no checks reported",
    "no checks found",
    "no check runs",
    "no checks have been reported",
];
/// GitHub CLI 2.81.0: merge failures that specifically permit rebase recovery.
pub const GH_MERGE_CONFLICT: &[&str] = &[
    "not mergeable",
    "merge conflict",
    "merge commit cannot be cleanly created",
    "conflicts must be resolved",
];
/// GitHub CLI 2.81.0: a branch-protection policy deliberately refused merge.
/// This must be checked before [`GH_MERGE_CONFLICT`] because the surrounding
/// CLI sentence also says "not mergeable".
// trace:BUG-1447 | ai:codex
pub const GH_BRANCH_POLICY_MERGE_REFUSAL: &[&str] = &["the base branch policy prohibits the merge"];
/// GitHub CLI 2.81.0: no pull request exists for the selected branch.
pub const GH_NO_PULL_REQUEST: &[&str] = &["no pull requests found", "no prs found"];
/// GitHub CLI 2.81.0 plus its Go net/TLS stack: transient API errors.
pub const GH_NETWORK_TRANSIENT: &[&str] = &[
    "githubstatus.com",
    "error connecting to api.github.com",
    "connecting to api.github.com",
    "no such host",
    "could not resolve host",
    "name resolution",
    "name or service not known",
    "temporary failure in name resolution",
    "network is unreachable",
    "no route to host",
    "connection refused",
    "connection reset",
    "connection timed out",
    "i/o timeout",
    "request canceled",
    "tls handshake timeout",
    "tls: handshake failure",
    "dial tcp",
];
/// GitLab CLI 1.36.0 plus Go net/http: transient API connectivity errors.
pub const GLAB_NETWORK_TRANSIENT: &[&str] = &[
    "timeout",
    "timed out",
    "connection refused",
    "could not resolve",
    "dial tcp",
    "no such host",
    "network is unreachable",
    "temporary failure",
    "503",
    "502",
    "504",
    "eof",
];
/// GitLab REST v4 JSON `message` values for an optimistic-SHA mismatch.
pub const GITLAB_STALE_HEAD_MESSAGES: &[&str] = &[
    "sha does not match",
    "sha mismatch",
    "head of source branch",
];
/// GitLab REST v4 JSON `message` values for retryable mergeability races.
pub const GITLAB_RETRYABLE_MERGE_MESSAGES: &[&str] = &["method not allowed", "cannot be merged"];
/// GitLab REST v4 HTTP status values for retryable mergeability races.
pub const GITLAB_RETRYABLE_MERGE_STATUSES: &[u64] = &[405, 409];
/// Git 2.43.x: diagnostics identifying index-lock contention.
pub const GIT_INDEX_LOCK: &[&str] = &[
    "index.lock",
    "file exists",
    "unable to create",
    "another git process seems to be running",
];
/// Git 2.43.x: a commit had no staged tree change.
pub const GIT_NOTHING_TO_COMMIT: &[&str] = &["nothing to commit"];
/// Git 2.43.x: a push was rejected because the remote ref moved.
pub const GIT_PUSH_REJECTED: &[&str] = &["non-fast-forward", "rejected", "fetch first"];
/// Git 2.43.x: fetch refused because another worktree owns the branch.
pub const GIT_BRANCH_CHECKED_OUT: &[&str] = &["checked out at", "refusing to fetch into branch"];
/// Cargo/rustc 1.92.0: build-stage failure markers in mixed gate output.
pub const CARGO_BUILD_FAILURE: &[&str] = &["error: could not compile", "error[e", "build failed"];
/// SQLite 3.45.1 / rusqlite: transient writer-lock diagnostics.
pub const SQLITE_LOCKED: &[&str] = &["database is locked", "database table is locked"];
/// Linux, macOS, and common toolchain spellings for exhausted disk or memory.
pub const OS_RESOURCE_EXHAUSTION: &[&str] = &[
    "no space left on device",
    "disk full",
    "out of disk",
    "enospc",
    "out of memory",
    "cannot allocate memory",
    "oom-kill",
    "oomkilled",
];
/// Claude Code 2.1.276 / Anthropic API and common edge proxies: outage markers.
pub const CLAUDE_API_OUTAGE: &[&str] = &[
    "overloaded",
    "upstream connect error",
    "stream timeout",
    "stream disconnected",
];
/// Claude Code 2.1.276: prefix for Anthropic HTTP 5xx API diagnostics.
pub const CLAUDE_API_5XX_PREFIX: &str = "api error: 5";
/// GitHub/GitLab status rollup prose observed via AIDA's forge adapters.
pub const CI_ROLLUP_SUCCESS: &[&str] = &["pass", "green", "success"];
pub const CI_ROLLUP_FAILURE: &[&str] = &["fail", "red", "error", "cancel"];
pub const CI_ROLLUP_PENDING: &[&str] = &["pend", "run", "progress"];
/// systemd 255 `busctl` / Terminator DBus errors when the plugin is absent.
pub const TERMINATOR_PLUGIN_MISSING: &[&str] =
    &["was not provided", "serviceunknown", "not found", "no such"];

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
