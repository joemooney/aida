//! Pure launch policy for agent children: the closed vendor and kind sets,
//! the AIDA environment keys a launch owns, and the posture-bearing flags a
//! caller-supplied argv may not carry. No I/O; `aida-cli-lib`'s `launch/`
//! module constructs vendor processes. The TUI shares the control-env policy
//! for its checked AIDA children.
//!
//! Claude launches project a complete child environment; other vendors await
//! their compatibility cutover. The policy itself performs no I/O.
// trace:STORY-2326 | ai:claude

use std::fmt;

/// `AIDA_SESSION_ROLE`: the seat a child runs as.
pub const ROLE_ENV: &str = "AIDA_SESSION_ROLE";
/// The session grant variable: the child's grant selector (an explicit `""` is a
/// baseline child).
pub const GRANT_ENV: &str = "AIDA_SESSION_GRANT";
/// `AIDA_SESSION_PURPOSE`: describes the launcher's own session.
pub const PURPOSE_ENV: &str = "AIDA_SESSION_PURPOSE";
/// `AIDA_CONTAINED`: set only when the child runs inside the AIDA boundary.
pub const CONTAINED_ENV: &str = "AIDA_CONTAINED";
/// `AIDA_CONTAINMENT_POSTURE`: `aida`, `native` or `none`.
pub const POSTURE_ENV: &str = "AIDA_CONTAINMENT_POSTURE";
/// `AIDA_ROLE_INSTANCE`: the companion marker of a seat instance.
pub const ROLE_INSTANCE_ENV: &str = "AIDA_ROLE_INSTANCE";
/// `AIDA_STANDALONE_IMPLEMENTER`: binds only the child its launcher set it on.
pub const STANDALONE_MARKER_ENV: &str = "AIDA_STANDALONE_IMPLEMENTER";
/// `AIDA_HEADLESS_LANE`: a child lane gets its own lane marker.
pub const LANE_MARKER_ENV: &str = "AIDA_HEADLESS_LANE";
/// `AIDA_AGENT_REGISTRY_TOKEN`: the registry row's write token.
pub const REGISTRY_TOKEN_ENV: &str = "AIDA_AGENT_REGISTRY_TOKEN";
/// `AIDA_HANDLE_LAUNCH_TOKEN`: the handle reservation's launch token.
pub const HANDLE_LAUNCH_TOKEN_ENV: &str = "AIDA_HANDLE_LAUNCH_TOKEN";
/// `AIDA_AGENT_CMD`: the test hook that replaces the vendor program.
pub const AGENT_CMD_ENV: &str = "AIDA_AGENT_CMD";

/// What a launch does with one key it owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owned {
    /// The launch sets it (or explicitly clears it); an ambient value never
    /// reaches the child.
    Decided,
    /// The ambient value passes unchanged, unless the launch sets its own.
    Forwarded,
}

/// Every AIDA key a launch owns, with its disposition. One table, pinned by
/// tests, so the owned set cannot drift between launch paths.
///
/// The two tokens are `Forwarded` on the vendor process only. They must never
/// appear in a tool shell's environment (`codex_tool_env::TOOL_ENV`).
pub const OWNED_KEYS: &[(&str, Owned)] = &[
    (ROLE_ENV, Owned::Decided),
    (GRANT_ENV, Owned::Decided),
    (PURPOSE_ENV, Owned::Decided),
    (CONTAINED_ENV, Owned::Decided),
    (POSTURE_ENV, Owned::Decided),
    (STANDALONE_MARKER_ENV, Owned::Decided),
    (LANE_MARKER_ENV, Owned::Decided),
    (ROLE_INSTANCE_ENV, Owned::Forwarded),
    (REGISTRY_TOKEN_ENV, Owned::Forwarded),
    (HANDLE_LAUNCH_TOKEN_ENV, Owned::Forwarded),
];

/// Pinned non-authority configuration inherited by vendor children. Fixture,
/// required-live and credential keys are deliberately absent.
// trace:STORY-2326 | ai:codex
pub const AMBIENT_AIDA_KEYS: &[&str] = &[
    "AIDA_ABSENCE_DAYS",
    "AIDA_AGENT_CONTEXT_FILE",
    "AIDA_AGENT_ID",
    "AIDA_AGENT_MODEL",
    "AIDA_AGENT_NAME",
    "AIDA_AGENT_OUTPUT",
    "AIDA_AGENT_TYPE",
    "AIDA_AI_TOOL",
    "AIDA_AUTHOR",
    "AIDA_BIN",
    "AIDA_BUILD_PROFILE",
    "AIDA_CACHE_LOCK_EXPECTED_SECS",
    "AIDA_CACHE_LOCK_STALE_SECS",
    "AIDA_CARGO_SLOTS",
    "AIDA_CARGO_TIMEOUT",
    "AIDA_CODEX_SESSIONS_DIR",
    "AIDA_DEBUG",
    "AIDA_DEBUG_IDENTITY",
    "AIDA_DEV_BIN",
    "AIDA_DEV_HOME_REPO",
    "AIDA_DEV_LINK_DIR",
    "AIDA_DEV_LIVE_DIR",
    "AIDA_DEV_PROFILE",
    "AIDA_DEV_PROFILE_PIN",
    "AIDA_DEV_REPO",
    "AIDA_EDITOR",
    "AIDA_EVENTS_DISABLE",
    "AIDA_EVENTS_MAX_BYTES",
    "AIDA_FIELD_STUDY",
    "AIDA_FIRST_RUN_HINTS",
    "AIDA_FOCUS",
    "AIDA_GLYPHS",
    "AIDA_HANDLE",
    "AIDA_HARNESS_VERSION",
    "AIDA_HEADLESS",
    "AIDA_HEADLESS_RESULT_GRACE_SECS",
    "AIDA_HEADLESS_VENDOR",
    "AIDA_HINTS",
    "AIDA_HISTORY_LIMIT",
    "AIDA_HOME",
    "AIDA_INTENT_MODEL",
    "AIDA_JEV_ENDPOINT",
    "AIDA_JEV_MODEL",
    "AIDA_LOCAL_LLM_ENDPOINT",
    "AIDA_LOCAL_LLM_MODEL",
    "AIDA_MAILBOX_AUTOSYNC",
    "AIDA_MAIL_WAKE",
    "AIDA_MARKDOWN",
    "AIDA_NO_PROGRESS_MINUTES",
    "AIDA_OUTPUT_FORMAT",
    "AIDA_PANES",
    "AIDA_PHASE_CEILING_MINUTES",
    "AIDA_PROJECT_ROOT",
    "AIDA_PUSH_QUIET",
    "AIDA_QUIET",
    "AIDA_REGISTRY_PATH",
    "AIDA_ROTATION_GENERATION",
    "AIDA_ROTATION_HANDOFF",
    "AIDA_ROTATION_PREVIOUS_SESSION",
    "AIDA_ROTATION_SEAT",
    "AIDA_SESSION_ID",
    "AIDA_SESSION_PROJECT",
    "AIDA_SESSION_SCOPE",
    "AIDA_SHELL_WRAPPER",
    "AIDA_STORE",
    "AIDA_STORE_HISTORY_TIMEOUT_SECS",
    "AIDA_STORE_LOCK_TIMEOUT_SECS",
    "AIDA_STORE_PATH",
    "AIDA_TEE_HEADLESS",
    "AIDA_TELEMETRY",
    "AIDA_TRANSPORT_SCAN_DEADLINE_MS",
    "AIDA_TUI_EPIC",
    "AIDA_TUI_LIVENESS_TTL_SECS",
    "AIDA_TUI_REDESIGN",
    "AIDA_TUI_VENDOR",
    "AIDA_USER",
    "AIDA_ZEN",
    "AIDA_ZEN_NO_AI",
    "AIDA_ZEN_PAUSE_ALWAYS",
];

/// Explicitly not inherited: credentials, fixture controls, authority and
/// operation-local signals must be supplied by their owning launch site.
/// This conservative inventory also includes production writes/prefix literals.
// trace:STORY-2326 | ai:codex
pub const DROPPED_AIDA_KEYS: &[&str] = &[
    "AIDA_AGENT_CMD",
    "AIDA_ADD_NO_REMOTE_SYNC",
    "AIDA_ADVISOR_HANDSHAKE",
    "AIDA_ADVISOR_SESSION_UUID",
    "AIDA_AGENT_",
    "AIDA_AGENT_EFFORT",
    "AIDA_AGENT_GATE_",
    "AIDA_AGENT_GATE_APPLIES_TO",
    "AIDA_AGENT_GATE_NAME",
    "AIDA_AGENT_GATE_ROLE",
    "AIDA_ALLOW_ADVISOR_CODE",
    "AIDA_ALLOW_INTERMEDIATE",
    "AIDA_ALLOW_INTERMEDIATE_ONLY",
    "AIDA_ANTIGRAVITY_SETTINGS",
    "AIDA_ASCIINEMA_WRAPPED",
    "AIDA_AUTO_ARCHIVE",
    "AIDA_AUTO_BUMP",
    "AIDA_AUTO_COMPLETE",
    "AIDA_AUTO_COMPLETE_PHASE",
    "AIDA_AUTO_COMPLETE_TOKEN",
    "AIDA_AUTO_COMPLETE_VARIANT",
    "AIDA_AUTO_FOLLOWUPS",
    "AIDA_AUTO_MERGE_GATE",
    "AIDA_AUTO_TEST_PLAN_CAPTURE",
    "AIDA_BG_FETCH",
    "AIDA_BG_FETCH_INTERVAL_SECS",
    "AIDA_BUG_1482_PROJECT",
    "AIDA_BUG_1482_WORKER",
    "AIDA_BUG_1984_HOLDER_DIR",
    "AIDA_BUILD_GIT_DIRTY",
    "AIDA_BUILD_GIT_SHA",
    "AIDA_BUILD_UNIX_TIME",
    "AIDA_BURNDOWN_BG_WAIT_CEILING_MS",
    "AIDA_BURNDOWN_LOCK_HELD",
    "AIDA_BURNDOWN_ORDER",
    "AIDA_CACHE_FALLBACK_DIR",
    "AIDA_CACHE_NO_DETACH",
    "AIDA_CACHE_READ_WAIT_MS",
    "AIDA_CACHE_RETRY_COUNT",
    "AIDA_CACHE_RETRY_MS",
    "AIDA_CACHE_WORKER",
    "AIDA_CALIBRATE",
    "AIDA_CALLER_KIND",
    "AIDA_CANONICAL_IDS",
    "AIDA_CAPTURE_INTERFACE_CHANGES",
    "AIDA_CARGO_NO_SLOT",
    "AIDA_CI_AUTO_FIX",
    "AIDA_CLAUDE_COORDINATOR_WRAPPER",
    "AIDA_COMMIT_STRICT",
    "AIDA_COMPETE_JUDGE",
    "AIDA_CORE_TEST_GUARD_REENTRANCE",
    "AIDA_CORE_TEST_GUARD_REENTRANCE_MULTI",
    "AIDA_CORE_TEST_GUARD_RESTORE",
    "AIDA_DEBUG_AUTOBUMP",
    "AIDA_DEBUG_GH",
    "AIDA_DEBUG_GLAB",
    "AIDA_DEPUTY_JOB",
    "AIDA_DEPUTY_REQUESTER",
    "AIDA_DEPUTY_SHELL_JOB",
    "AIDA_DEPUTY_SHELL_NAME",
    "AIDA_DEPUTY_SPOOL",
    "AIDA_DEPUTY_VIA_WRAPPER",
    "AIDA_DEV_ACTIVATE_REEXEC",
    "AIDA_DEV_ACTIVE",
    "AIDA_DISPATCH_CLAIM_TOKEN",
    "AIDA_DOCTOR_COMPLETED_SINCE",
    "AIDA_DRAIN_BORROW",
    "AIDA_DRAIN_FORCE",
    "AIDA_DRAIN_LOCK_STALE_SECS",
    "AIDA_DRAIN_PIPELINE_DEPTH",
    "AIDA_DRAIN_RESUME_PHASE",
    "AIDA_DRAIN_STOP_FILE",
    "AIDA_DRAIN_TERM_GRACE_SECS",
    "AIDA_DRIVER",
    "AIDA_DRIVE_ROOT",
    "AIDA_EXIT_GRACE_MS",
    "AIDA_EXIT_POLL_MS",
    "AIDA_EXIT_SENTINEL",
    "AIDA_FETCH_FRESHNESS_SECS",
    "AIDA_FORGE_CONFIRM_PAUSE_MS",
    "AIDA_FROM_PR_",
    "AIDA_FROM_PR_HEAD_SHA",
    "AIDA_FROM_PR_NUMBER",
    "AIDA_FROM_PR_REVIEW",
    "AIDA_GH_VERIFY_RETRIES",
    "AIDA_GITHUB_TOKEN",
    "AIDA_GITLAB_LIVE_HOST",
    "AIDA_GITLAB_TOKEN",
    "AIDA_GRADED_REVIEW_CONTEXT",
    "AIDA_GRANT_KEY",
    "AIDA_HISTORY_CACHE",
    "AIDA_HISTORY_INDEX_BUDGET_MS",
    "AIDA_HOLD_SIGNAL_FILE",
    "AIDA_HOOK_EVENT",
    "AIDA_HOOK_PAYLOAD",
    "AIDA_HOST_OVERRIDE",
    "AIDA_IDENTITY_GUARD",
    "AIDA_INIT_CD_FILE",
    "AIDA_INIT_COMMIT_SCAFFOLD",
    "AIDA_INIT_FORGE",
    "AIDA_INIT_LANG",
    "AIDA_INIT_PROJECT_NAME",
    "AIDA_INIT_PROJECT_ROOT",
    "AIDA_INIT_REMOTE_URL",
    "AIDA_INTAKE_APPLY",
    "AIDA_INTAKE_CANDIDATES",
    "AIDA_INTAKE_DISPOSITION_BIAS",
    "AIDA_INTAKE_DO_NOT_APPROVE_CLASSES",
    "AIDA_INTAKE_MAX_APPROVALS",
    "AIDA_INTAKE_ON_APPLY",
    "AIDA_INTAKE_RISK",
    "AIDA_INTAKE_TRIVIAL_PROPOSALS",
    "AIDA_JEV_API_KEY",
    "AIDA_JIRA_TOKEN",
    "AIDA_LIB_TEST_REAL_HOMES",
    "AIDA_LIFECYCLE_TAGS",
    "AIDA_LIST_LIMIT",
    "AIDA_LOCKING",
    "AIDA_LOCK_FAIL_FAST_DEADLINE_MS",
    "AIDA_LOW_ENTROPY_BITS",
    "AIDA_MCP_PROFILE",
    "AIDA_MIRROR_SYNC_ATTEST",
    "AIDA_NO_HUMAN",
    "AIDA_NO_HUMAN_ACKNOWLEDGED",
    "AIDA_NO_HUMAN_MODE",
    "AIDA_ORCHESTRATED_LEASE_RECEIPT",
    "AIDA_OS_WRAP",
    "AIDA_OWNERSHIP_TRACE",
    "AIDA_PERF_GATE_ALL_BINARIES",
    "AIDA_PERMISSION_MODE",
    "AIDA_PHASE",
    "AIDA_PIPELINED_BATCH_CHILD",
    "AIDA_PIPELINED_RESULT_PATH",
    "AIDA_PREFLIGHT_BINARY",
    "AIDA_PREFLIGHT_DEFAULT_BRANCH",
    "AIDA_PROBE_IN",
    "AIDA_PROBE_OUT",
    "AIDA_PROGRESS_ACTIVITY_SECS",
    "AIDA_PR_SHIP_ALLOW_IN_DRIVE",
    "AIDA_PR_SHIP_NO_CI_WAIT",
    "AIDA_PUNT_REQUEST_FILE",
    "AIDA_PUNT_RESPONSE_FILE",
    "AIDA_PUNT_SIGNAL_FILE",
    "AIDA_PUSH_DEFAULT",
    "AIDA_RED_RUN_BUDGET_SECS",
    "AIDA_REGEN_CHECK_RESULTS",
    "AIDA_REGEN_COMMAND_TIMEOUT_SECS",
    "AIDA_REGEN_GUARD_PIPE",
    "AIDA_REGEN_PREFIX",
    "AIDA_REGEN_TRIAL_ROOT",
    "AIDA_RELAY_RESTART",
    "AIDA_RELAY_RUN",
    "AIDA_RELEASE",
    "AIDA_RELEASE_ARTIFACT_TIMEOUT_SECS",
    "AIDA_RELEASE_TAG_FILE",
    "AIDA_REQUIRE_AI_TAG",
    "AIDA_REQUIRE_BWRAP_LIVE",
    "AIDA_REQUIRE_CODEX_CONTAINMENT_LIVE",
    "AIDA_REQUIRE_REQ_FOR_FEAT",
    "AIDA_REQUIRE_USERNS",
    "AIDA_RETRY_ESCALATE_MODEL",
    "AIDA_RETRY_TRANSIENT",
    "AIDA_REVIEW_",
    "AIDA_REVIEW_ROUND",
    "AIDA_REVIEW_VERDICT_FILE",
    "AIDA_REVIEW_WATCH_HEAD",
    "AIDA_ROTATION_HANDOFF_STAMP",
    "AIDA_RUNAWAY_RATE",
    "AIDA_SCCACHE_PIDS",
    "AIDA_SCCACHE_PROC_DIR",
    "AIDA_SCHEDULE_CHILD",
    "AIDA_SCHEDULE_INVOKER",
    "AIDA_SEAT_CLONE_ACCESSOR",
    "AIDA_SEAT_CONTEXT_CEILING",
    "AIDA_SERVER",
    "AIDA_SESSION_",
    "AIDA_SESSION_GRANT_FILE",
    "AIDA_SHIP_CLEANUP_PROBE_SECS",
    "AIDA_SKIP_XPLAT_CHECK",
    "AIDA_SPINNING_AFTER_SECS",
    "AIDA_STALE_LOCKS_DEADLINE_SECS",
    "AIDA_STORE_GC_AGGRESSIVE_PACKS",
    "AIDA_STORE_GC_AUTO",
    "AIDA_STORE_GUARD_FLOOR",
    "AIDA_STORE_HOOKS_ONLY",
    "AIDA_STORE_MUTATION_ENDPOINT",
    "AIDA_STORE_MUTATION_PROFILE",
    "AIDA_STORE_MUTATION_REQUIRED",
    "AIDA_STORE_WRITE_GUARD",
    "AIDA_SUPPRESS_SESSION_NEXT",
    "AIDA_TEAM_REQUIRE_USER",
    "AIDA_TEST_ACTION_READ_CASE",
    "AIDA_TEST_ALLOW_NO_PYTHON3",
    "AIDA_TEST_ANTIGRAVITY_BINARY",
    "AIDA_TEST_ANTIGRAVITY_MCP_ACCEPT",
    "AIDA_TEST_ANTIGRAVITY_MCP_ARGV_OUT",
    "AIDA_TEST_ANTIGRAVITY_MCP_REGISTERED",
    "AIDA_TEST_ANTIGRAVITY_MCP_TTY",
    "AIDA_TEST_CACHE_LOCK_WAIT_MS",
    "AIDA_TEST_CAPTURE",
    "AIDA_TEST_COMPETE_PARENT",
    "AIDA_TEST_COMPETE_VENDOR",
    "AIDA_TEST_CONTROL_ARGV0",
    "AIDA_TEST_CONTROL_REPLACEMENT",
    "AIDA_TEST_FORGE_FALLBACK_DIRS",
    "AIDA_TEST_GC_CONFIG_CHILD",
    "AIDA_TEST_GH_BINARY",
    "AIDA_TEST_GLAB_BINARY",
    "AIDA_TEST_GUARD_NESTED",
    "AIDA_TEST_GUARD_REENTRANCE",
    "AIDA_TEST_GUARD_REENTRANCE_MULTI",
    "AIDA_TEST_GUARD_RESET",
    "AIDA_TEST_GUARD_RESTORE",
    "AIDA_TEST_GUARD_UNSET",
    "AIDA_TEST_HANDLE_FIFO_CHILD",
    "AIDA_TEST_HOME",
    "AIDA_TEST_HOME_NAMESPACE_FIXTURE",
    "AIDA_TEST_HOME_NAMESPACE_SLOT",
    "AIDA_TEST_NOTICE_DEADLINE_MS",
    "AIDA_TEST_PARENT_ARGS",
    "AIDA_TEST_PATH_CHILD",
    "AIDA_TEST_PR_HEAD_SHA",
    "AIDA_TEST_REDO_CHECKPOINT",
    "AIDA_TEST_REDO_ROOT",
    "AIDA_TEST_ROOT",
    "AIDA_TEST_SESSION_END_PAUSE_AT",
    "AIDA_TEST_SESSION_END_PAUSE_DIR",
    "AIDA_TEST_STORE_LOCK_CONTENDED_FILE",
    "AIDA_TEST_TEAM_WRITER_CHILD_DONE",
    "AIDA_TEST_TEAM_WRITER_CHILD_STORE",
    "AIDA_TEST_TUI_CONTROL_ARGV0",
    "AIDA_TEST_UNSHIPPED_SCAN_BUDGET_MS",
    "AIDA_TIMELINE_NO_FORGE",
    "AIDA_TRUST_PID_NAMESPACE",
    "AIDA_TRUST_SEAT_WORKTREES",
    "AIDA_UNKNOWN",
    "AIDA_UPDATE_SNAPSHOTS",
    "AIDA_WAIT_DELAY_MS",
    "AIDA_WITH_PLAN",
    "AIDA_WORKER_CI_ABSOLUTE",
    "AIDA_WORKER_CI_IDLE",
    "AIDA_WORKTREE_CONTEXT",
    "AIDA_WORK_BINDING_FIFO_CHILD",
    "AIDA_WORK_NATIVE_FIXTURE",
    "AIDA_WT_LEASE_FILE",
    "AIDA_WT_PS1_PREFIX",
    "AIDA_ZEN_TOKEN",
    "AIDA_ADMIN_API_KEY",
    "AIDA_AI_PROVIDER",
    "AIDA_ANTHROPIC_BASE_URL",
    "AIDA_CHAT_MODEL",
    "AIDA_DATABASE_URL",
    "AIDA_DEV_MODE",
    "AIDA_EVAL_MODEL",
    "AIDA_OIDC_AUTH_URL",
    "AIDA_OIDC_CLIENT_ID",
    "AIDA_OIDC_CLIENT_SECRET",
    "AIDA_OIDC_ISSUER_URL",
    "AIDA_OIDC_REDIRECT_URL",
    "AIDA_OIDC_SCOPES",
    "AIDA_OIDC_TOKEN_URL",
    "AIDA_OIDC_USERINFO_URL",
    "AIDA_OPENAI_BASE_URL",
    "AIDA_SERVER_API_KEY",
    "AIDA_WEB_ADMIN_USERS",
    "AIDA_WEB_AUTH_MODE",
    "AIDA_WEB_DEFAULT_ROLE",
    "AIDA_WEB_EDITOR_USERS",
    "AIDA_WEB_SESSION_SQLITE_PATH",
    "AIDA_WEB_SESSION_STORE",
    "AIDA_WEB_SESSION_TTL_HOURS",
    "AIDA_WEB_VIEWER_USERS",
    "AIDA_WORKSPACE_ROOT",
];

/// Keys that belong on the vendor process only, never in a tool shell.
pub const VENDOR_PROCESS_ONLY_KEYS: &[&str] = &[REGISTRY_TOKEN_ENV, HANDLE_LAUNCH_TOKEN_ENV];

/// Keys a `--bg` hand-off never carries: the daemon-run session resolves its
/// seat from its lane lease, not from the dispatcher's command.
pub const BACKGROUND_FORBIDDEN_KEYS: &[&str] = &[ROLE_ENV, GRANT_ENV, PURPOSE_ENV];

/// The owned keys a launch removes from the ambient environment before it
/// sets its own.
pub fn decided_keys() -> impl Iterator<Item = &'static str> {
    OWNED_KEYS
        .iter()
        .filter(|(_, how)| *how == Owned::Decided)
        .map(|(key, _)| *key)
}

// trace:STORY-2538 | ai:codex
/// The environment recipient matters: vendor-only tokens cannot reach tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvRecipient {
    VendorProcess,
    ToolShell,
}

/// A pure, explicit projection specification. Vendor profiles declare exact
/// compatibility keys; launch adapters clear inheritance before applying it.
/// Callers supply exact keys, never a prefix wildcard.
/// Owned-key semantics come only from OWNED_KEYS above.
/// No Debug/serialization: key selections can themselves be sensitive.
pub struct EnvPolicy {
    vendor: LaunchVendor,
    kind: LaunchKind,
    recipient: EnvRecipient,
    forwarded: Vec<std::ffi::OsString>,
}

/// A refused projection, deliberately omitting keys and values from diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvPolicyRefusal;

impl fmt::Display for EnvPolicyRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("environment projection violates launch policy")
    }
}
impl std::error::Error for EnvPolicyRefusal {}

/// A complete child environment, not an overlay on the parent. A launch
/// adapter must clear inheritance before applying these pairs. This type does
/// not execute commands, validate grants or confer launcher authority.
#[derive(Clone)]
pub struct ChildEnv(std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>);

impl ChildEnv {
    pub fn get(&self, key: impl AsRef<std::ffi::OsStr>) -> Option<&std::ffi::OsStr> {
        self.0.get(key.as_ref()).map(std::ffi::OsString::as_os_str)
    }

    pub fn pairs(&self) -> impl Iterator<Item = (&std::ffi::OsStr, &std::ffi::OsStr)> {
        self.0.iter().map(|(k, v)| (k.as_os_str(), v.as_os_str()))
    }
}

impl EnvPolicy {
    pub fn new(
        vendor: LaunchVendor,
        kind: LaunchKind,
        recipient: EnvRecipient,
        forwarded: impl IntoIterator<Item = std::ffi::OsString>,
    ) -> Result<Self, EnvPolicyRefusal> {
        let policy = Self {
            vendor,
            kind,
            recipient,
            forwarded: forwarded.into_iter().collect(),
        };
        for key in &policy.forwarded {
            policy.check_key(key)?;
            let owned = OWNED_KEYS
                .iter()
                .find(|(k, _)| std::ffi::OsStr::new(k) == key);
            if matches!(owned, Some((_, Owned::Decided))) {
                return Err(EnvPolicyRefusal);
            }
            // Unknown AIDA names cannot expand child authority. Fixture and
            // required-live inputs are never vendor allowlist entries.
            if key.as_encoded_bytes().starts_with(b"AIDA_")
                && owned.is_none()
                && !AMBIENT_AIDA_KEYS.iter().any(|name| key == *name)
            {
                return Err(EnvPolicyRefusal);
            }
        }
        Ok(policy)
    }

    pub fn vendor(&self) -> LaunchVendor {
        self.vendor
    }

    fn check_key(&self, key: &std::ffi::OsStr) -> Result<(), EnvPolicyRefusal> {
        let bytes = key.as_encoded_bytes();
        if bytes.is_empty()
            || bytes.contains(&0)
            || bytes.contains(&b'=')
            || (self.kind == LaunchKind::Background
                && BACKGROUND_FORBIDDEN_KEYS.iter().any(|k| key == *k))
            || (self.recipient == EnvRecipient::ToolShell
                && VENDOR_PROCESS_ONLY_KEYS.iter().any(|k| key == *k))
        {
            return Err(EnvPolicyRefusal);
        }
        Ok(())
    }

    /// `decisions` are already validated launcher inputs, never ambient values.
    /// Omitted Decided keys are absent, including when present in the parent.
    /// Forwarded owned keys may also be explicitly replaced or removed.
    /// Declared cargo resources may override only the fixed build/cache keys.
    // trace:STORY-2641 | ai:codex
    pub fn project(
        &self,
        parent: &crate::exec_context::Environment,
        decisions: impl IntoIterator<Item = (std::ffi::OsString, Option<std::ffi::OsString>)>,
    ) -> Result<ChildEnv, EnvPolicyRefusal> {
        let mut child = std::collections::BTreeMap::new();
        for key in &self.forwarded {
            if let Some(value) = parent.get(key) {
                child.insert(key.clone(), value.to_owned());
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for (key, value) in decisions {
            self.check_key(&key)?;
            if !(OWNED_KEYS.iter().any(|(k, _)| key == *k)
                || [
                    "RUSTC_WRAPPER",
                    "CARGO_TARGET_DIR",
                    "SCCACHE_DIR",
                    "SCCACHE_SERVER_UDS",
                    "SCCACHE_SERVER_PORT",
                    "SCCACHE_IDLE_TIMEOUT",
                ]
                .iter()
                .any(|k| key == *k))
                || !seen.insert(key.clone())
            {
                return Err(EnvPolicyRefusal);
            }
            if let Some(value) = value {
                child.insert(key, value);
            } else {
                child.remove(&key);
            }
        }
        if child.values().any(|v| v.as_encoded_bytes().contains(&0)) {
            return Err(EnvPolicyRefusal);
        }
        Ok(ChildEnv(child))
    }
}

// Compatibility path: identity and descriptors have one owner.
// trace:STORY-2539 | ai:codex
pub use crate::vendors::{LaunchVendor, UnknownVendor};

/// What a launch is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LaunchKind {
    /// A new interactive or headless session.
    Fresh,
    /// A resume of a recorded session.
    Resume,
    /// This process becomes the vendor (`exec`).
    Exec,
    /// A `--bg` hand-off to the vendor's own daemon.
    Background,
    /// A vendor probe (version, agents, MCP list): no seat, never a session.
    Query,
    /// An `aida` child that continues this process's own seat.
    Control,
}

impl LaunchKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Resume => "resume",
            Self::Exec => "exec",
            Self::Background => "background",
            Self::Query => "query",
            Self::Control => "control",
        }
    }

    /// Parse a kind name exactly; an unknown name is `None`.
    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw.trim().to_ascii_lowercase().as_str() {
            "fresh" => Self::Fresh,
            "resume" => Self::Resume,
            "exec" => Self::Exec,
            "background" | "bg" => Self::Background,
            "query" => Self::Query,
            "control" => Self::Control,
            _ => return None,
        })
    }
}

/// A posture-bearing flag in caller-supplied argv. Posture comes from the
/// vendor profile, never from a request tail or `default_flags`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostureFlag(pub String);

impl fmt::Display for PostureFlag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "`{}` sets the agent's sandbox or permission posture, which AIDA decides; \
             remove it from the launch arguments",
            self.0
        )
    }
}

impl std::error::Error for PostureFlag {}

/// Whether one argument (with the next one, for `-c <value>`) sets posture.
fn posture_flag_at(args: &[String], at: usize) -> Option<String> {
    let arg = args[at].as_str();
    let name = arg.split_once('=').map_or(arg, |(name, _)| name);
    // Claude: --settings, --permission-mode, --add-dir, --dangerously-*.
    // Codex: --sandbox/-s, --ask-for-approval/-a, --full-auto, --yolo,
    // --dangerously-bypass-approvals-and-sandbox, and --profile (a profile
    // can set sandbox_mode and approval_policy). trace:STORY-2326 | ai:claude
    let named = matches!(
        name,
        "--settings"
            | "--permission-mode"
            | "--sandbox"
            | "-s"
            | "--ask-for-approval"
            | "-a"
            | "--add-dir"
            | "--full-auto"
            | "--yolo"
            | "--profile"
    ) || name.starts_with("--dangerously-");
    if named {
        return Some(arg.to_string());
    }
    // `-c <key>=…` / `--config <key>=…` and the joined `-c<key>=…` spelling,
    // where the (possibly quoted) TOML key sets the sandbox, the approval
    // policy (`approval_policy=never` is `-a never`) or selects a profile.
    let config_value = match name {
        "-c" | "--config" => match arg.split_once('=') {
            Some((_, value)) => Some(value),
            None => args.get(at + 1).map(String::as_str),
        },
        _ => arg
            .strip_prefix("-c")
            .filter(|v| !v.is_empty() && !arg.starts_with("--")),
    };
    config_value
        .filter(|value| {
            let key = value.trim_start().trim_start_matches(['"', '\'']);
            POSTURE_CONFIG_KEYS.iter().any(|k| key.starts_with(k))
        })
        .map(|value| format!("{name} {value}"))
}

/// Codex config key prefixes that set posture: the sandbox (`sandbox_mode`,
/// `sandbox_workspace_write.*`), the approval policy, and `profile` /
/// `profiles.*` (a profile can set either).
const POSTURE_CONFIG_KEYS: &[&str] = &["sandbox", "approval_policy", "profile"];

/// Refuse caller-supplied argv (a request tail or `default_flags`) that
/// carries a posture-bearing flag.
pub fn refuse_posture_flags(args: &[String]) -> Result<(), PostureFlag> {
    for (at, arg) in args.iter().enumerate() {
        if arg == "--" {
            // Everything after `--` is a positional (a prompt), not a flag.
            return Ok(());
        }
        if let Some(flag) = posture_flag_at(args, at) {
            return Err(PostureFlag(flag));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_context_projection_excludes_secrets_and_fixture_inputs() {
        use crate::exec_context::Environment;
        let parent = Environment::from_pairs([
            ("PATH".into(), "/fixture".into()),
            ("GH_TOKEN".into(), "secret".into()),
            (ROLE_ENV.into(), "advisor".into()),
            ("AIDA_TEST_ROOT".into(), "/host".into()),
            ("AIDA_REQUIRE_BWRAP_LIVE".into(), "1".into()),
        ]);
        let policy = EnvPolicy::new(
            LaunchVendor::Codex,
            LaunchKind::Fresh,
            EnvRecipient::VendorProcess,
            ["PATH".into()],
        )
        .unwrap();
        let child = policy
            .project(&parent, [(ROLE_ENV.into(), Some("implementer".into()))])
            .unwrap();
        assert_eq!(child.get("PATH"), Some(std::ffi::OsStr::new("/fixture")));
        assert_eq!(
            child.get(ROLE_ENV),
            Some(std::ffi::OsStr::new("implementer"))
        );
        assert_eq!(child.pairs().count(), 2);
        for forbidden in [
            "AIDA_TEST_ROOT",
            "AIDA_REQUIRE_BWRAP_LIVE",
            "AIDA_REQUIRE_CODEX_CONTAINMENT_LIVE",
            "AIDA_UNKNOWN",
            ROLE_ENV,
        ] {
            assert!(EnvPolicy::new(
                LaunchVendor::Codex,
                LaunchKind::Fresh,
                EnvRecipient::VendorProcess,
                [forbidden.into()]
            )
            .is_err());
        }
        assert!(policy
            .project(&parent, [("GH_TOKEN".into(), Some("secret".into()))])
            .is_err());
        assert!(policy
            .project(&parent, [(ROLE_ENV.into(), None), (ROLE_ENV.into(), None)])
            .is_err());
        let absent = policy.project(&parent, []).unwrap();
        assert!(absent.get(ROLE_ENV).is_none());
    }

    #[test]
    fn exec_context_projection_preserves_owned_background_and_tool_boundaries() {
        use crate::exec_context::Environment;
        for vendor in [LaunchVendor::Claude, LaunchVendor::Codex, LaunchVendor::Agy] {
            for key in BACKGROUND_FORBIDDEN_KEYS {
                let policy = EnvPolicy::new(
                    vendor,
                    LaunchKind::Background,
                    EnvRecipient::VendorProcess,
                    [],
                )
                .unwrap();
                assert!(policy
                    .project(&Environment::default(), [((*key).into(), Some("x".into()))])
                    .is_err());
            }
            for key in VENDOR_PROCESS_ONLY_KEYS {
                assert!(EnvPolicy::new(
                    vendor,
                    LaunchKind::Fresh,
                    EnvRecipient::ToolShell,
                    [(*key).into()]
                )
                .is_err());
                let tools =
                    EnvPolicy::new(vendor, LaunchKind::Fresh, EnvRecipient::ToolShell, []).unwrap();
                assert!(tools
                    .project(&Environment::default(), [((*key).into(), Some("x".into()))])
                    .is_err());
                let policy = EnvPolicy::new(
                    vendor,
                    LaunchKind::Fresh,
                    EnvRecipient::VendorProcess,
                    [(*key).into()],
                )
                .unwrap();
                assert_eq!(policy.vendor(), vendor);
                let parent = Environment::from_pairs([((*key).into(), "token".into())]);
                assert!(policy.project(&parent, []).unwrap().get(key).is_some());
                assert!(policy
                    .project(&parent, [((*key).into(), None)])
                    .unwrap()
                    .get(key)
                    .is_none());
            }
        }
    }

    #[test]
    fn exec_context_projection_rejects_invalid_process_pairs() {
        use crate::exec_context::Environment;
        for key in ["", "A=B", "A\0B"] {
            assert!(EnvPolicy::new(
                LaunchVendor::Claude,
                LaunchKind::Query,
                EnvRecipient::VendorProcess,
                [key.into()]
            )
            .is_err());
        }
        let policy = EnvPolicy::new(
            LaunchVendor::Claude,
            LaunchKind::Query,
            EnvRecipient::VendorProcess,
            ["PATH".into()],
        )
        .unwrap();
        let parent = Environment::from_pairs([("PATH".into(), "a\0b".into())]);
        assert!(policy.project(&parent, []).is_err());
    }

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn vendor_parse_is_closed() {
        assert_eq!(LaunchVendor::parse("Claude"), Ok(LaunchVendor::Claude));
        assert_eq!(LaunchVendor::parse(" codex "), Ok(LaunchVendor::Codex));
        assert_eq!(LaunchVendor::parse("antigravity"), Ok(LaunchVendor::Agy));
        assert_eq!(LaunchVendor::parse("agy"), Ok(LaunchVendor::Agy));
        for refused in ["opencode", "gemini", "", "claude-code", "bash"] {
            assert!(LaunchVendor::parse(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn kind_parse_is_closed() {
        for kind in [
            LaunchKind::Fresh,
            LaunchKind::Resume,
            LaunchKind::Exec,
            LaunchKind::Background,
            LaunchKind::Query,
            LaunchKind::Control,
        ] {
            assert_eq!(LaunchKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(LaunchKind::parse("daemon"), None);
    }

    #[test]
    fn owned_set_is_pinned() {
        let keys: Vec<&str> = OWNED_KEYS.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            keys,
            [
                "AIDA_SESSION_ROLE",
                "AIDA_SESSION_GRANT",
                "AIDA_SESSION_PURPOSE",
                "AIDA_CONTAINED",
                "AIDA_CONTAINMENT_POSTURE",
                "AIDA_STANDALONE_IMPLEMENTER",
                "AIDA_HEADLESS_LANE",
                "AIDA_ROLE_INSTANCE",
                "AIDA_AGENT_REGISTRY_TOKEN",
                "AIDA_HANDLE_LAUNCH_TOKEN",
            ]
        );
        // The companion marker and the two tokens are forwarded, never stripped.
        for forwarded in [
            ROLE_INSTANCE_ENV,
            REGISTRY_TOKEN_ENV,
            HANDLE_LAUNCH_TOKEN_ENV,
        ] {
            assert!(decided_keys().all(|k| k != forwarded), "{forwarded}");
        }
        for key in BACKGROUND_FORBIDDEN_KEYS {
            assert!(decided_keys().any(|k| k == *key), "{key}");
        }
        for key in VENDOR_PROCESS_ONLY_KEYS {
            assert!(keys.contains(key), "{key}");
        }
    }

    #[test]
    fn posture_flags_refused_in_caller_argv() {
        let refused: &[&[&str]] = &[
            &["--settings", "s.json"],
            &["--settings=s.json"],
            &["--permission-mode", "bypassPermissions"],
            &["--permission-mode=plan"],
            &["--dangerously-skip-permissions"],
            &["--dangerously-bypass-approvals-and-sandbox"],
            &["--sandbox", "danger-full-access"],
            &["--add-dir", "/"],
            &["--full-auto"],
            &["--yolo"],
            &["-s", "danger-full-access"],
            &["-s=workspace-write"],
            &["--ask-for-approval", "never"],
            &["--ask-for-approval=never"],
            &["-a", "never"],
            &["-c", "sandbox_mode=danger-full-access"],
            &["-c", "sandbox_workspace_write.writable_roots=[\"/\"]"],
            &["--config", "sandbox_mode=x"],
            &["--config=sandbox_mode=x"],
            &["-csandbox_mode=x"],
            &["--model", "opus", "--settings", "x"],
            // Advisor ruling 5 (round 5): the approval policy as config, a
            // quoted TOML key, and a profile (flag or config).
            &["-c", "approval_policy=never"],
            &["--config=approval_policy=\"never\""],
            &["-c", "\"sandbox_mode\"=danger-full-access"],
            &["-c", "'sandbox_mode'=danger-full-access"],
            &["--profile", "yolo"],
            &["--profile=yolo"],
            &["-c", "profile=yolo"],
            &["-c", "profiles.yolo.sandbox_mode=danger-full-access"],
        ];
        for args in refused {
            assert!(refuse_posture_flags(&argv(args)).is_err(), "{args:?}");
        }
        let allowed: &[&[&str]] = &[
            &[],
            &["--model", "opus"],
            &["-c", "model_reasoning_effort=high"],
            &["--name", "lane"],
            &["fix the --settings parser"],
            &["--", "--settings", "is only a prompt here"],
            &["-p", "prompt"],
            &["-m", "gpt-5"],
            &["--search"],
        ];
        for args in allowed {
            assert!(refuse_posture_flags(&argv(args)).is_ok(), "{args:?}");
        }
    }
}

/// An AIDA control child forwards this process's selector, never a requested
/// role. The receiving CLI validates the selector using its ordinary gate.
/// An absent selector is explicitly removed, so it cannot arrive ambiently.
/// Callers execute the kernel-held running image with `argv[0]` set to `aida`.
// trace:STORY-2326 | ai:codex
pub fn control_env(
    own_grant_selector: Option<&std::ffi::OsStr>,
) -> [(&'static str, Option<std::ffi::OsString>); 2] {
    [
        (ROLE_ENV, None),
        (
            GRANT_ENV,
            own_grant_selector.map(std::ffi::OsStr::to_os_string),
        ),
    ]
}
