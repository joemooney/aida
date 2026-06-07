//! Inferred "recent remote agent activity" for `aida status`.
//!
//! STORY-452 (Phase-1, Option A — inference only). AIDA's local agent registry
//! (`.aida/agents/*.toml`, STORY-431) and session leases only see processes on
//! THIS machine. Cloud `/ultraplan` sessions and cross-machine sibling agents
//! execute work and push commits but never appear locally, so the operator
//! becomes the manual aggregator of multi-source work.
//!
//! This module closes the cheapest slice of that gap with zero new manual
//! verbs and zero protocol: it reads commit-trailer provenance (`[AI:codex]`,
//! `[AI:antigravity]`, …) on branches that have NO local session lease, and
//! surfaces a read-only "Recent remote activity" section. It is lossy by
//! design (post-hoc, no live-work visibility) — the honest limitations are
//! documented in `docs/aida/multi-source-coordination.md`.
//!
//! The inference itself ([`infer_remote_activity`]) is a pure function over
//! parsed commits so it is fully unit-testable without touching git.

use chrono::{DateTime, Utc};

/// One parsed commit observed on a remote-tracking branch. Built by the
/// git-driven collector in `main.rs`; the inference is pure over a slice of
/// these. `branch` is the short ref name (e.g. `origin/bug-250` → `bug-250`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteCommit {
    pub(crate) branch: String,
    pub(crate) subject: String,
    pub(crate) when: Option<DateTime<Utc>>,
}

/// One row in the rendered "Recent remote activity" section. Represents the
/// most recent agent-attributed commit on a lease-less branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteActivityRow {
    pub(crate) agent_type: String,
    pub(crate) branch: String,
    pub(crate) spec_id: Option<String>,
    pub(crate) subject: String,
    pub(crate) when: Option<DateTime<Utc>>,
}

/// Parse the `[AI:<tool>]` provenance trailer at the head of a commit subject.
/// Returns the normalized agent type (`codex`, `antigravity`, `claude`,
/// `other`) or `None` when the subject has no trailer. A confidence suffix
/// (`[AI:codex:med]`) and mixed authorship (`[AI:antigravity+claude]`) are
/// tolerated — the FIRST tool wins, matching how commit subjects are written.
pub(crate) fn parse_ai_tool(subject: &str) -> Option<String> {
    let subject = subject.trim_start();
    let rest = subject.strip_prefix("[AI:")?;
    let end = rest.find(']')?;
    let inner = &rest[..end];
    // Strip a trailing confidence token (":med" / ":low"); keep the tool list.
    let tools = inner.split(':').next().unwrap_or(inner);
    // Mixed authorship: first tool wins.
    let first = tools.split('+').next().unwrap_or(tools).trim();
    if first.is_empty() {
        return None;
    }
    Some(crate::agent_registry::normalize_agent_type(
        first.to_string(),
    ))
}

/// Pull the trailing `(SPEC-ID)` reference from a commit subject, if any.
/// Reuses the project-wide spec-id extractor so PR/MR/GH/GL forge refs are
/// excluded. Returns the first AIDA spec id found.
fn spec_id_from_subject(subject: &str) -> Option<String> {
    crate::pr_ship::extract_spec_ids_from_text(subject)
        .into_iter()
        .next()
}

/// Infer recent remote agent activity from commit-trailer provenance.
///
/// Keeps the most-recent agent-attributed commit per branch, restricted to
/// branches NOT covered by a local session lease (those are local work, already
/// shown in the "Active agents" section). `lease_branches` is the set of branch
/// names with a live local lease; matching is case-insensitive.
///
/// Commits without an `[AI:...]` trailer are skipped (human/un-attributed
/// commits aren't "agent activity"). The result is sorted newest-first, then by
/// branch for stable ordering of timestamp-less commits, and capped at `limit`.
///
/// trace:STORY-452 | ai:claude
pub(crate) fn infer_remote_activity(
    commits: &[RemoteCommit],
    lease_branches: &[String],
    limit: usize,
) -> Vec<RemoteActivityRow> {
    use std::collections::HashMap;

    let lease_set: Vec<String> = lease_branches
        .iter()
        .map(|b| b.to_ascii_lowercase())
        .collect();

    // Keep the most-recent attributed commit per branch.
    let mut best: HashMap<String, RemoteActivityRow> = HashMap::new();
    for commit in commits {
        if lease_set
            .iter()
            .any(|b| b == &commit.branch.to_ascii_lowercase())
        {
            continue;
        }
        let Some(agent_type) = parse_ai_tool(&commit.subject) else {
            continue;
        };
        let row = RemoteActivityRow {
            agent_type,
            branch: commit.branch.clone(),
            spec_id: spec_id_from_subject(&commit.subject),
            subject: commit.subject.trim().to_string(),
            when: commit.when,
        };
        match best.get(&commit.branch) {
            Some(existing) if newer_or_equal(existing.when, row.when) => {}
            _ => {
                best.insert(commit.branch.clone(), row);
            }
        }
    }

    let mut rows: Vec<RemoteActivityRow> = best.into_values().collect();
    // Newest-first; ties (and timestamp-less commits) fall back to branch name
    // so ordering is deterministic.
    rows.sort_by(|a, b| b.when.cmp(&a.when).then_with(|| a.branch.cmp(&b.branch)));
    rows.truncate(limit);
    rows
}

/// True when `existing` is at least as recent as `candidate` (so the candidate
/// should NOT replace it). A present timestamp always beats `None`.
fn newer_or_equal(existing: Option<DateTime<Utc>>, candidate: Option<DateTime<Utc>>) -> bool {
    match (existing, candidate) {
        (Some(e), Some(c)) => e >= c,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn commit(branch: &str, subject: &str, secs_ago: i64) -> RemoteCommit {
        RemoteCommit {
            branch: branch.to_string(),
            subject: subject.to_string(),
            when: Some(Utc::now() - Duration::seconds(secs_ago)),
        }
    }

    #[test]
    fn parse_ai_tool_extracts_normalized_type() {
        assert_eq!(
            parse_ai_tool("[AI:codex] feat: x").as_deref(),
            Some("codex")
        );
        assert_eq!(
            parse_ai_tool("[AI:antigravity] fix: y (BUG-1)").as_deref(),
            Some("antigravity")
        );
        assert_eq!(
            parse_ai_tool("[AI:claude] docs: z").as_deref(),
            Some("claude")
        );
    }

    #[test]
    fn parse_ai_tool_tolerates_confidence_and_mixed_authorship() {
        assert_eq!(
            parse_ai_tool("[AI:codex:med] fix: y").as_deref(),
            Some("codex")
        );
        // Mixed authorship: first tool wins.
        assert_eq!(
            parse_ai_tool("[AI:antigravity+claude] test: t").as_deref(),
            Some("antigravity")
        );
        assert_eq!(
            parse_ai_tool("[AI:codex+claude:med] feat: m").as_deref(),
            Some("codex")
        );
    }

    #[test]
    fn parse_ai_tool_none_without_trailer() {
        assert!(parse_ai_tool("feat: no trailer").is_none());
        assert!(parse_ai_tool("chore: deps").is_none());
        assert!(parse_ai_tool("[AI:] empty").is_none());
    }

    #[test]
    fn unknown_tool_normalizes_to_other() {
        assert_eq!(
            parse_ai_tool("[AI:somebot] feat: x").as_deref(),
            Some("other")
        );
    }

    // STORY-452: the core inference — commits in, remote-activity rows out.
    #[test]
    fn infers_rows_from_agent_trailers() {
        let commits = vec![
            commit(
                "bug-250",
                "[AI:codex] fix(orchestrator): held outcome (BUG-250)",
                30,
            ),
            commit(
                "story-431",
                "[AI:antigravity] feat(agents): registry (STORY-431)",
                120,
            ),
        ];
        let rows = infer_remote_activity(&commits, &[], 10);
        assert_eq!(rows.len(), 2);
        // Newest-first: bug-250 (30s) before story-431 (120s).
        assert_eq!(rows[0].agent_type, "codex");
        assert_eq!(rows[0].branch, "bug-250");
        assert_eq!(rows[0].spec_id.as_deref(), Some("BUG-250"));
        assert_eq!(rows[1].agent_type, "antigravity");
        assert_eq!(rows[1].spec_id.as_deref(), Some("STORY-431"));
    }

    // STORY-452: a branch with a local lease is local work, not remote.
    #[test]
    fn excludes_branches_with_a_local_lease() {
        let commits = vec![
            commit("bug-250", "[AI:codex] fix: x (BUG-250)", 30),
            commit("story-431", "[AI:antigravity] feat: y (STORY-431)", 60),
        ];
        // Case-insensitive lease-branch match.
        let rows = infer_remote_activity(&commits, &["BUG-250".to_string()], 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].branch, "story-431");
    }

    // STORY-452: un-attributed (human) commits are not "agent activity".
    #[test]
    fn skips_commits_without_ai_trailer() {
        let commits = vec![
            commit("feature-x", "feat: human work (TASK-9)", 10),
            commit("feature-y", "[AI:codex] feat: bot work (TASK-10)", 20),
        ];
        let rows = infer_remote_activity(&commits, &[], 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].branch, "feature-y");
    }

    // STORY-452: one row per branch — the most-recent commit wins.
    #[test]
    fn keeps_only_latest_commit_per_branch() {
        let commits = vec![
            commit("bug-250", "[AI:codex] fix: older (BUG-250)", 300),
            commit("bug-250", "[AI:antigravity] fix: newer (BUG-250)", 10),
        ];
        let rows = infer_remote_activity(&commits, &[], 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].agent_type, "antigravity");
        assert!(rows[0].subject.contains("newer"));
    }

    // STORY-452: empty signal yields an empty section (renders nothing).
    #[test]
    fn empty_when_no_remote_signal() {
        assert!(infer_remote_activity(&[], &[], 10).is_empty());
        let only_human = vec![commit("x", "chore: deps", 5)];
        assert!(infer_remote_activity(&only_human, &[], 10).is_empty());
    }

    // STORY-452: the limit caps the rendered rows (newest kept).
    #[test]
    fn respects_limit_keeping_newest() {
        let commits = vec![
            commit("a", "[AI:codex] feat: a (TASK-1)", 300),
            commit("b", "[AI:codex] feat: b (TASK-2)", 200),
            commit("c", "[AI:codex] feat: c (TASK-3)", 100),
        ];
        let rows = infer_remote_activity(&commits, &[], 2);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].branch, "c");
        assert_eq!(rows[1].branch, "b");
    }

    // STORY-452: timestamp-less commits sort deterministically by branch and
    // never crash the inference.
    #[test]
    fn handles_missing_timestamps_deterministically() {
        let commits = vec![
            RemoteCommit {
                branch: "zeta".to_string(),
                subject: "[AI:codex] feat: z".to_string(),
                when: None,
            },
            RemoteCommit {
                branch: "alpha".to_string(),
                subject: "[AI:codex] feat: a".to_string(),
                when: None,
            },
        ];
        let rows = infer_remote_activity(&commits, &[], 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].branch, "alpha");
        assert_eq!(rows[1].branch, "zeta");
    }
}
