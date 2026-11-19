// trace:TASK-3784 | ai:codex
use crate::test_env::EnvVarsGuard;

/// Launch identity a live agent shell exports beyond the per-session
/// evidence a launched child already drops
/// ([`crate::agent_handle::INHERITED_SESSION_ENV`]). Vendor prefix markers
/// (`CLAUDECODE`, `CLAUDE_CODE_*`, `CODEX_*`) stay; a test that depends on the
/// vendor sets `AIDA_AGENT_TYPE`, which outranks them.
// trace:TASK-2349 | ai:claude
const AGENT_LAUNCH_ENV: &[&str] = &[
    "AIDA_AGENT_TYPE",
    "AIDA_AGENT_NAME",
    "AIDA_SESSION_ROLE",
    "AIDA_SESSION_SCOPE",
    "AIDA_SESSION_ID",
];

/// A test that resolves the *current* agent session, isolated from the
/// session (if any) running the suite: its session evidence and launch
/// identity are cleared, then `pairs` applied, under one env-lock
/// acquisition, and the vendor-ancestor process walk sees no client. Without both halves a
/// live session's env or its real process tree and registry binding decide
/// the result, so the test passes in CI and fails in an agent shell.
// trace:TASK-2349 | ai:claude
pub(crate) struct AgentSessionIsolation {
    _ancestry: crate::agent_registry::hermetic_ancestry_seam::Guard,
    _env: EnvVarsGuard,
}

impl AgentSessionIsolation {
    pub(crate) fn apply(pairs: &[(&'static str, Option<&str>)]) -> Self {
        let mut all: Vec<(&'static str, Option<&str>)> = crate::agent_handle::INHERITED_SESSION_ENV
            .iter()
            .chain(AGENT_LAUNCH_ENV)
            .map(|k| (*k, None))
            .collect();
        all.extend_from_slice(pairs);
        Self {
            _ancestry: crate::agent_registry::hermetic_ancestry_seam::enter(),
            _env: EnvVarsGuard::apply(&all),
        }
    }
}
