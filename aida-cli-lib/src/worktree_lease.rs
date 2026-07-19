//! TASK-702: pure derivation of an AIDA lease record from Claude Code
//! SubagentStart / SubagentStop hook payloads.
//!
//! Substrate-CAPTURE only: Claude owns harness worktree provisioning. AIDA
//! observes SubagentStart to register an existing worktree lease, then observes
//! SubagentStop to release the lease keyed by the same `agent_id`.
//! trace:TASK-702 | ai:claude

use std::path::PathBuf;

/// The generic lease scope for a harness worktree whose branch carries no
/// recognizable SPEC-ID.
pub(crate) const HARNESS_WORKTREE_SCOPE: &str = "harness-worktree";

/// The subset of the empirically verified SubagentStart/Stop payloads AIDA
/// needs to correlate harness worktree leases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubagentPayload {
    pub agent_id: String,
    pub agent_type: Option<String>,
    pub cwd: PathBuf,
}

/// The deterministic lease fields derived from a SubagentStart payload. The
/// caller supplies `branch` after running `git -C <cwd> rev-parse --abbrev-ref
/// HEAD`; keeping git I/O outside this core leaves it unit-testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreeLeaseSpec {
    /// Stable hook correlation key. The writer also uses this as the lease id.
    pub agent_id: String,
    /// Claude's agent type, retained for role/display context when present.
    pub agent_type: Option<String>,
    /// The lease scope: a SPEC-ID derived from the branch when recognizable,
    /// else the generic [`HARNESS_WORKTREE_SCOPE`].
    pub scope: String,
    /// The branch the worktree is on.
    pub branch: String,
    /// The worktree path to record on the lease.
    pub worktree_path: PathBuf,
}

/// The known AIDA spec-type branch prefixes (`task-688-...` -> `TASK-688`).
const SPEC_TYPES: &[&str] = &[
    "fr", "func", "nfr", "sys", "user", "bug", "epic", "story", "task", "spike", "sprint", "adr",
    "meta", "doc",
];

/// Extract a SPEC-ID (e.g. `TASK-688`) from a branch name like
/// `task-688-aida-release`. Recognizes the standard `<type>-<number>-<slug>`
/// convention; returns `None` for harness-generated names like
/// `worktree-agent-<hex>` or anything without a `<type>-<number>` head.
pub(crate) fn spec_id_from_branch(branch: &str) -> Option<String> {
    let mut parts = branch.split('-');
    let kind = parts.next()?.to_ascii_lowercase();
    let num = parts.next()?;
    if !SPEC_TYPES.contains(&kind.as_str()) {
        return None;
    }
    if num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(format!("{}-{}", kind.to_uppercase(), num))
}

/// Normalize Claude's agent id into a filesystem-safe lease id. Claude agent
/// ids are already stable correlation keys; this maps one to a `.aida/sessions/
/// <id>.toml`-safe filename.
///
/// Distinctness guarantee (TASK-712): the readable prefix lowercases and strips
/// non-alphanumerics, so two DISTINCT agent ids that differ only in punctuation
/// or case (`Agent.1` vs `agent-1`, `AB` vs `ab`) would map to the SAME prefix
/// and collide on one lease file — letting two distinct agents cross-release
/// each other's lease. To prevent that we append a deterministic hash of the
/// ORIGINAL (unmodified) id, so distinct inputs always produce distinct lease
/// ids while the prefix stays human-readable. Determinism matters: the same
/// agent must resolve to the same lease file on start and on stop, so we use a
/// fixed FNV-1a (no random seed) rather than `DefaultHasher`/`RandomState`.
/// trace:TASK-712
pub(crate) fn lease_id_from_agent_id(agent_id: &str) -> String {
    let prefix: String = agent_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let prefix = if prefix.is_empty() {
        "unknownagent".to_string()
    } else {
        prefix
    };
    // FNV-1a 64-bit over the raw bytes — deterministic across runs/machines, so
    // distinct ids (incl. case/punct-only differences) never share a lease file.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in agent_id.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{prefix}-{hash:016x}")
}

/// Derive the lease record to WRITE for a SubagentStart payload. The scope is
/// the branch's SPEC-ID when derivable, else the generic harness scope so the
/// worktree is still tracked.
pub(crate) fn lease_spec_for_start(payload: &SubagentPayload, branch: &str) -> WorktreeLeaseSpec {
    let scope = spec_id_from_branch(branch).unwrap_or_else(|| HARNESS_WORKTREE_SCOPE.to_string());
    WorktreeLeaseSpec {
        agent_id: payload.agent_id.clone(),
        agent_type: payload.agent_type.clone(),
        scope,
        branch: branch.to_string(),
        worktree_path: payload.cwd.clone(),
    }
}

/// The lease id whose file should be CLEARED for a SubagentStop payload.
pub(crate) fn lease_id_for_stop(payload: &SubagentPayload) -> String {
    lease_id_from_agent_id(&payload.agent_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_id_from_named_branch() {
        assert_eq!(
            spec_id_from_branch("task-688-aida-release"),
            Some("TASK-688".into())
        );
        assert_eq!(
            spec_id_from_branch("bug-466-windows"),
            Some("BUG-466".into())
        );
        assert_eq!(
            spec_id_from_branch("story-510-gitlab-ci"),
            Some("STORY-510".into())
        );
        assert_eq!(spec_id_from_branch("BUG-471-x"), Some("BUG-471".into()));
    }

    #[test]
    fn spec_id_none_for_harness_and_unrecognized_branches() {
        assert_eq!(
            spec_id_from_branch("worktree-agent-a0f3696de475d07c3"),
            None
        );
        assert_eq!(spec_id_from_branch("main"), None);
        assert_eq!(spec_id_from_branch("random-branch"), None);
        assert_eq!(spec_id_from_branch("task-foo-bar"), None);
        assert_eq!(spec_id_from_branch(""), None);
    }

    #[test]
    fn start_derives_spec_scope_from_branch() {
        let p = SubagentPayload {
            agent_id: "agent-123".into(),
            agent_type: Some("general-purpose".into()),
            cwd: PathBuf::from("/repo/.worktrees/task-688"),
        };
        let spec = lease_spec_for_start(&p, "task-688-aida-release");
        assert_eq!(spec.agent_id, "agent-123");
        assert_eq!(spec.agent_type.as_deref(), Some("general-purpose"));
        assert_eq!(spec.scope, "TASK-688");
        assert_eq!(spec.branch, "task-688-aida-release");
        assert_eq!(
            spec.worktree_path,
            PathBuf::from("/repo/.worktrees/task-688")
        );
    }

    #[test]
    fn start_falls_back_to_harness_scope() {
        let p = SubagentPayload {
            agent_id: "agent-abc".into(),
            agent_type: None,
            cwd: PathBuf::from("/repo/.claude/worktrees/agent-abc"),
        };
        let spec = lease_spec_for_start(&p, "worktree-agent-abc");
        assert_eq!(spec.scope, HARNESS_WORKTREE_SCOPE);
        assert_eq!(spec.branch, "worktree-agent-abc");
    }

    #[test]
    fn stop_uses_agent_id_as_release_key() {
        let p = SubagentPayload {
            agent_id: "Agent-ABC-123".into(),
            agent_type: None,
            cwd: PathBuf::from("/repo/.claude/worktrees/agent-abc"),
        };
        // trace:TASK-712 — readable prefix preserved, hash suffix appended.
        let id = lease_id_for_stop(&p);
        assert!(id.starts_with("agentabc123-"), "id: {id}");
        // The stop key must equal the start key for the same agent (round-trip).
        assert_eq!(id, lease_id_from_agent_id("Agent-ABC-123"));
    }

    // trace:TASK-712 — distinct agent ids that differ ONLY in punctuation/case
    // must map to DISTINCT lease ids (no collision → no cross-release).
    #[test]
    fn distinct_agent_ids_get_distinct_lease_ids() {
        let a = lease_id_from_agent_id("Agent.1");
        let b = lease_id_from_agent_id("agent-1");
        let c = lease_id_from_agent_id("AGENT1");
        let d = lease_id_from_agent_id("agent1");
        // All four collapsed to the same prefix under the old scheme.
        assert!(a.starts_with("agent1-"));
        assert!(b.starts_with("agent1-"));
        assert!(c.starts_with("agent1-"));
        assert!(d.starts_with("agent1-"));
        // But every distinct raw id yields a distinct lease id now.
        let ids = [&a, &b, &c, &d];
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                assert_ne!(ids[i], ids[j], "collision: {} vs {}", ids[i], ids[j]);
            }
        }
    }

    // trace:TASK-712 — deterministic across calls (same id → same lease file).
    #[test]
    fn lease_id_is_deterministic() {
        assert_eq!(
            lease_id_from_agent_id("agent-xyz"),
            lease_id_from_agent_id("agent-xyz")
        );
    }

    // trace:TASK-712 — an all-punctuation id still gets a stable, distinct id.
    #[test]
    fn empty_prefix_falls_back_but_stays_distinct() {
        let a = lease_id_from_agent_id("...");
        let b = lease_id_from_agent_id("---");
        assert!(a.starts_with("unknownagent-"));
        assert!(b.starts_with("unknownagent-"));
        assert_ne!(a, b);
    }
}
