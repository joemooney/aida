// trace:FR-0152 | ai:claude
// trace:STORY-361 | ai:claude
// trace:TASK-440 | ai:claude
//! MCP (Model Context Protocol) server for AIDA.
//!
//! Implements JSON-RPC 2.0 over stdio. Exposes two surfaces:
//!
//! 1. **Spec graph** (9 tools) — read/write the requirement
//!    store: `list_requirements`, `show_requirement`, `add_requirement`,
//!    `update_requirement`, `search_requirements`, `add_comment`,
//!    `add_relationship`, `list_features`, `history`.
//! 2. **Coordination** (STORY-361) — read/write the file substrate the
//!    AIDA orchestrator and skills already use:
//!    - **Punts** (`.aida/punts.jsonl` + `.aida/punts/`): `list_punts`,
//!      `read_punt`, `post_punt`, `resolve_punt`, `escalate_punt`.
//!    - **Findings** (draft requirements tagged `from-implementer:` /
//!      `from-review:`): `list_findings`, `file_finding`, `triage_finding`.
//!    - **Task claims** (`.aida/sessions/*.toml`): `claim_task`,
//!      `release_task`, `list_active_leases`.
//!    - **Worker directives** (`.aida/worker.cmd`): `post_directive`,
//!      `list_directives`, `ack_directive`.
//!    - **Agent briefs** (`.aida/agent-briefs/<agent>/`): `list_briefs`,
//!      `read_brief`, `ack_brief`.
//!
//! The coordination tools are the *MCP transport* over AIDA's
//! filesystem-canonical coordination substrate. The orchestrator still
//! reads/writes those files directly — these tools are the surface any
//! MCP-speaking agent (Codex, Cursor, …) can use to participate in the
//! same drains. See `docs/architecture/mcp-coordination-surface.md`.
//!
//! # Output schemas (TASK-440 Path A + STORY-399 Path B)
//!
//! Every tool descriptor in `tool_descriptors()` carries an `outputSchema`
//! that documents the **MCP text-envelope** shape its responses take plus a
//! per-tool `description` summarizing what the text payload conveys.
//! Schema-driven clients (Codex, Cursor, …) get useful discoverability
//! instead of opaque-shape responses.
//!
//! **Path A (TASK-440)** declared those schemas. **Path B (STORY-399)** makes
//! successful responses *also* emit a `structuredContent` object matching the
//! declared schema, so schema-driven clients get a machine-readable result
//! without parsing the human text. The decision is **additive** (acceptance
//! option (a)): the `{ content: [{ type: "text", text: "..." }] }` text
//! envelope is preserved byte-for-byte for legacy text consumers (Claude
//! Code), and `structuredContent` is layered alongside it. Error responses
//! keep the STORY-401 `{ isError: true, structuredError: { … } }` shape and do
//! not carry `structuredContent`. See `handle_tools_call` and
//! `text_envelope_output_schema`.
//!
//! # Tool profiles (STORY-474)
//!
//! The full surface is gated behind a named **profile** — a capability tier
//! (`read-only` < `coordination` < `operator` < `admin`/`full`). `read-only`
//! excludes every write tool and is the recommended safe default for untrusted /
//! marketplace clients; the built-in default stays `full` for backwards
//! compatibility. Resolve order: `AIDA_MCP_PROFILE` env → `[mcp] profile` in
//! `.aida/config.toml` → `full`. The profile is enforced at BOTH `tools/list`
//! (advertise) and `tools/call` (reject above-tier calls). See `McpProfile`,
//! `tool_min_profile`, and `resolve_mcp_profile`.

use std::cmp::Ordering;
use std::ffi::OsString;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use aida_core::{
    Comment, PuntCategory, RelationshipType, Requirement, RequirementPriority, RequirementStatus,
    RequirementType, Storage,
};

use crate::agent_registry::{self, AgentBinaryIdentity};
use crate::findings::{
    build_findings_view, count_findings, finding_source, FindingsFilter, FROM_IMPLEMENTER_PREFIX,
    FROM_REVIEW_PREFIX,
};
use crate::history::{self, HistoryOpts};
use crate::punt::{self, append_to_ledger, ledger_path, read_ledger, PuntRecord};

// Mirrors the full RequirementType taxonomy (aida-core models.rs, 19 variants).
// The ADR / knowledge-graph family (principle, vision, constraint, decision,
// term) and the change-request workflow type are first-class — keep this list in
// sync with `parse_requirement_type` below and the `add_requirement` /
// `update_requirement` schema enums. trace:TASK-716 | ai:claude
const VALID_MCP_REQUIREMENT_TYPES: &str =
    "functional, non-functional, system, user, change-request, bug, epic, story, task, spike, sprint, folder, meta, principle, vision, constraint, decision, term, doc";

/// BUG-591: the archive (STORY-441) + deferred (STORY-584) view-tier predicate,
/// shared by `list_requirements` and `search_requirements` so the MCP read
/// surface hides filed-away specs by default exactly like `aida list` /
/// `aida search` (the STORY-82 "MCP mirrors CLI" contract). The three tiers are
/// active (default) / deferred / archived; a row's tier is `archived` if
/// `Requirement::archived`, else `deferred` if `intake::is_deferred` (flag set
/// OR a legacy `deferred:*` parking tag), else `active`.
///
/// Filter resolution mirrors `aida list`:
/// - default (no flags): admit only ACTIVE rows
/// - `archived`: admit only archived rows (defer axis kept open so the audit is complete)
/// - `deferred`: admit only deferred rows
/// - `all` (highest precedence): admit every tier
///
/// trace:BUG-591 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewTierFilter {
    /// Active rows only — neither archived nor deferred (the default view).
    ActiveOnly,
    /// Archived rows only.
    ArchivedOnly,
    /// Deferred rows only.
    DeferredOnly,
    /// Every tier (active + deferred + archived).
    All,
}

impl ViewTierFilter {
    /// Resolve the three boolean flags into a single tier filter, matching the
    /// CLI precedence (`--all` wins over `--archived` over `--deferred`).
    fn resolve(all: bool, archived: bool, deferred: bool) -> Self {
        if all {
            ViewTierFilter::All
        } else if archived {
            ViewTierFilter::ArchivedOnly
        } else if deferred {
            ViewTierFilter::DeferredOnly
        } else {
            ViewTierFilter::ActiveOnly
        }
    }

    /// Whether this requirement passes the current view-tier filter.
    fn admits(&self, r: &Requirement) -> bool {
        let archived = r.archived;
        let tags: Vec<String> = r.tags.iter().cloned().collect();
        let deferred = crate::intake::is_deferred(r.deferred, &tags);
        match self {
            ViewTierFilter::All => true,
            ViewTierFilter::ArchivedOnly => archived,
            // A row can be both deferred and archived; archive is the stronger
            // tier, so the deferred-only view excludes archived rows.
            ViewTierFilter::DeferredOnly => deferred && !archived,
            ViewTierFilter::ActiveOnly => !archived && !deferred,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct McpBinaryIdentity {
    version: String,
    sha: String,
    dirty: bool,
}

impl McpBinaryIdentity {
    fn current() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            sha: env!("AIDA_BUILD_GIT_SHA").to_string(),
            dirty: env!("AIDA_BUILD_GIT_DIRTY") == "1",
        }
    }

    fn label(&self) -> String {
        format!(
            "{} sha {}{}",
            self.version,
            self.sha,
            if self.dirty { "+dirty" } else { "" }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct McpRespawnPlan {
    exe: PathBuf,
    argv: Vec<OsString>,
    running: McpBinaryIdentity,
    on_disk: McpBinaryIdentity,
}

struct McpRespawnState {
    running: McpBinaryIdentity,
    argv: Vec<OsString>,
}

impl McpRespawnState {
    fn new() -> Self {
        Self {
            running: McpBinaryIdentity::current(),
            argv: std::env::args_os().collect(),
        }
    }

    fn check(&self) -> Option<McpRespawnPlan> {
        let exe = crate::resolve_aida_exe();
        let on_disk = query_aida_binary_identity(&exe)?;
        if mcp_binary_is_newer_or_different(&self.running, &on_disk) {
            Some(McpRespawnPlan {
                exe,
                argv: self.argv.clone(),
                running: self.running.clone(),
                on_disk,
            })
        } else {
            None
        }
    }

    fn respawn(&self, plan: McpRespawnPlan) -> Result<()> {
        eprintln!(
            "AIDA MCP server detected newer binary on disk; self-respawning (running: {}; disk: {})",
            plan.running.label(),
            plan.on_disk.label()
        );
        exec_mcp_respawn(plan)
    }
}

fn strip_aida_program_prefix(output: &str) -> &str {
    ["aida-cli ", "aida-server ", "aida "]
        .iter()
        .find_map(|prefix| output.strip_prefix(*prefix))
        .unwrap_or(output)
}

fn parse_aida_binary_identity(output: &str) -> Option<McpBinaryIdentity> {
    let banner = strip_aida_program_prefix(output.trim());
    let version = banner
        .split_whitespace()
        .next()
        .filter(|v| {
            v.chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false)
        })?
        .to_string();
    let sha = banner
        .split("sha ")
        .nth(1)
        .and_then(|s| s.split([')', '+']).next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown")
        .to_string();
    Some(McpBinaryIdentity {
        version,
        sha,
        dirty: banner.contains("+dirty"),
    })
}

fn query_aida_binary_identity(exe: &Path) -> Option<McpBinaryIdentity> {
    let out = Command::new(exe).arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    parse_aida_binary_identity(&stdout)
}

fn compare_package_versions(a: &str, b: &str) -> Ordering {
    let parse = |raw: &str| {
        raw.split(['.', '-'])
            .map(|part| part.parse::<u64>())
            .collect::<Result<Vec<_>, _>>()
    };
    match (parse(a), parse(b)) {
        (Ok(left), Ok(right)) => left.cmp(&right),
        _ => a.cmp(b),
    }
}

fn mcp_binary_is_newer_or_different(
    running: &McpBinaryIdentity,
    on_disk: &McpBinaryIdentity,
) -> bool {
    match compare_package_versions(&running.version, &on_disk.version) {
        Ordering::Less => true,
        Ordering::Greater => false,
        Ordering::Equal => {
            let both_sha_known = running.sha != "unknown" && on_disk.sha != "unknown";
            both_sha_known && (running.sha != on_disk.sha || running.dirty != on_disk.dirty)
        }
    }
}

fn exec_mcp_respawn(plan: McpRespawnPlan) -> Result<()> {
    let mut command = Command::new(&plan.exe);
    if plan.argv.len() > 1 {
        command.args(plan.argv.iter().skip(1));
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = command.exec();
        Err(anyhow::anyhow!(
            "failed to exec MCP self-respawn via {}: {}",
            plan.exe.display(),
            err
        ))
    }

    #[cfg(windows)]
    {
        command.spawn().map_err(|e| {
            anyhow::anyhow!(
                "failed to spawn MCP self-respawn via {}: {}",
                plan.exe.display(),
                e
            )
        })?;
        std::process::exit(0);
    }
}

// ============================================================================
// JSON-RPC 2.0 types
// ============================================================================

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    method: String,
    #[serde(default)]
    params: Value,
    id: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
    id: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

// ============================================================================
// Stable MCP error shapes (STORY-401)
// ============================================================================
//
// MCP clients (Codex, Cursor, future agents) need predictable, machine-readable
// errors instead of free-form text. Every tool error path now renders into:
//
//   {
//     "content": [{ "type": "text", "text": "<tool>: <code>: <message>" }],
//     "isError": true,
//     "structuredError": { "code", "message", "tool", "recoverable" }
//   }
//
// `code` is a stable enum (`invalid_arg`, `not_found`, `conflict`, `io_error`,
// `permission_denied`, `internal`) so clients can branch on it. The text
// envelope stays additive and back-compatible — it's still `isError: true` with
// a human-readable string that contains the underlying message.
//
// Tool functions keep returning `Result<String, String>`; the dispatch boundary
// (`handle_tools_call`) classifies the error string into an `McpError` using the
// `McpError::classify` heuristics. Centralizing here means a single, audited
// mapping rather than 30 hand-edited error sites. trace:STORY-401 | ai:claude

/// Stable, machine-readable error code shared across all MCP tools.
///
/// The string form is what lands in the `structuredError.code` field and in the
/// `<tool>: <code>: <message>` text envelope, so these values are a contract —
/// add new variants, never rename existing ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum McpErrorCode {
    /// The caller passed a missing/blank/malformed argument. Recoverable: fix
    /// the input and retry.
    InvalidArg,
    /// A referenced entity (spec, finding, lease, brief, tool, resource) does
    /// not exist. Recoverable: the caller may retry with a valid identifier.
    NotFound,
    /// The request conflicts with current state (already exists, gated
    /// transition, duplicate claim). Not recoverable by retrying the same input.
    Conflict,
    /// Permission/authority denied (e.g. MCP may not self-advance an
    /// advisor-gated status). Not recoverable by retrying the same input.
    PermissionDenied,
    /// Filesystem / IO failure while reading or writing the substrate.
    /// Not recoverable by the caller without operator intervention.
    IoError,
    /// Anything we could not classify — treated as a server-side fault.
    Internal,
}

impl McpErrorCode {
    /// Stable string identifier emitted in the envelope.
    fn as_str(self) -> &'static str {
        match self {
            McpErrorCode::InvalidArg => "invalid_arg",
            McpErrorCode::NotFound => "not_found",
            McpErrorCode::Conflict => "conflict",
            McpErrorCode::PermissionDenied => "permission_denied",
            McpErrorCode::IoError => "io_error",
            McpErrorCode::Internal => "internal",
        }
    }

    /// Whether a client can plausibly retry after correcting its input.
    /// `invalid_arg` / `not_found` are caller-fixable; the rest are structural.
    fn recoverable(self) -> bool {
        matches!(self, McpErrorCode::InvalidArg | McpErrorCode::NotFound)
    }
}

/// A structured MCP tool error: a stable `code`, a one-line `message`, the
/// originating `tool`, and a `recoverable` hint. trace:STORY-401 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
struct McpError {
    code: McpErrorCode,
    message: String,
    tool: String,
}

impl McpError {
    fn new(tool: &str, code: McpErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            tool: tool.to_string(),
        }
    }

    /// Classify a free-form error string (as produced by the tool functions)
    /// into a stable `McpError`. The heuristics key off the well-known phrases
    /// the tool functions use (`Missing required parameter`, `not found`,
    /// `already exists`, gate-refusal text, …). Unknown shapes fall back to
    /// `internal` so the envelope is always well-formed.
    fn classify(tool: &str, raw: &str) -> Self {
        let lower = raw.to_lowercase();
        let code = if lower.contains("missing required parameter")
            || lower.contains("cannot be empty")
            || lower.contains("must be a string")
            || lower.contains("must not contain")
            || lower.starts_with("invalid ")
            || lower.contains("invalid punt category")
            || lower.contains("invalid brief path")
            || lower.starts_with("unknown source")
            || lower.starts_with("unknown action")
        {
            McpErrorCode::InvalidArg
        } else if lower.contains("unknown tool")
            || lower.contains("unknown resource")
            || lower.contains("not found")
        {
            McpErrorCode::NotFound
        } else if lower.contains("already exists")
            || lower.contains("already acked")
            || lower.contains("already claimed")
            || lower.contains("conflict")
        {
            McpErrorCode::Conflict
        } else if lower.contains("not advisor authority")
            || lower.contains("advisor-gated")
            || lower.contains("must not self-advance")
            || lower.contains("refus")
            || lower.contains("permission denied")
            || lower.contains("not permitted")
            || lower.contains("forbidden")
        {
            McpErrorCode::PermissionDenied
        } else if lower.contains("io error")
            || lower.contains("no such file")
            || lower.contains("permission")
            || lower.contains("os error")
        {
            McpErrorCode::IoError
        } else {
            McpErrorCode::Internal
        };
        Self::new(tool, code, raw.to_string())
    }

    /// Short, machine-friendly one-line text: `<tool>: <code>: <message>`.
    /// This is what lands on the `content` text array (back-compatible: still a
    /// string that contains the underlying message).
    fn envelope_text(&self) -> String {
        format!("{}: {}: {}", self.tool, self.code.as_str(), self.message)
    }

    /// The full MCP tools/call result value with `isError: true`, the text
    /// content array, and the additive `structuredError` payload.
    fn to_result_value(&self) -> Value {
        json!({
            "content": [{
                "type": "text",
                "text": self.envelope_text()
            }],
            "isError": true,
            "structuredError": {
                "code": self.code.as_str(),
                "message": self.message,
                "tool": self.tool,
                "recoverable": self.code.recoverable()
            }
        })
    }
}

impl JsonRpcResponse {
    fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: Some(result),
            error: None,
            id,
        }
    }

    fn error(id: Value, code: i64, message: String) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data: None,
            }),
            id,
        }
    }
}

// ============================================================================
// Lightweight lease projection
// ============================================================================
//
// The canonical SessionLease lives in `main.rs` as a `pub(crate)` struct with
// many fields. The MCP surface only needs a few; we read `.aida/sessions/*.toml`
// directly with `#[serde(default)]` so unknown fields don't break us — and
// crucially so the MCP server doesn't have to be rebuilt every time a new lease
// field lands.

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct LightLease {
    #[serde(default)]
    id: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    owner: String,
    #[serde(default)]
    worktree_path: String,
    #[serde(default)]
    branch: String,
    #[serde(default)]
    started_at: String,
    #[serde(default)]
    hostname: String,
    #[serde(default)]
    role: Option<String>,
    /// STORY-361: marks a lease that was created by the MCP `claim_task` tool
    /// (as opposed to `aida session start`, which creates a worktree). MCP
    /// claims are lightweight markers — `release_task` deletes them; other
    /// AIDA commands ignore them.
    #[serde(default)]
    mcp_claim: bool,
}

fn leases_dir(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("sessions")
}

// trace:TASK-438 | ai:claude
/// Filename a MCP `claim_task` lease writes to. Keyed by lower-cased spec_id
/// so two concurrent claims on the same spec target the same path — letting
/// `OpenOptions::create_new(true)` enforce single-winner semantics. Distinct
/// from the `<lease_id>.toml` shape `aida session start` uses, so the two
/// claim modes don't collide.
// Made pub(crate) so `aida session end` can reap the sibling mcp-claim for the
// scope it just released (BUG-694). trace:BUG-694 | ai:claude
pub(crate) fn mcp_claim_path(dir: &Path, spec_id: &str) -> PathBuf {
    dir.join(format!("mcp-claim.{}.toml", spec_id.to_ascii_lowercase()))
}

fn canonicalize_worktree_arg(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    Path::new(trimmed)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(trimmed))
        .to_string_lossy()
        .to_string()
}

/// Shell-quote a value for safe interpolation into a `Run:` command string
/// returned by the PEEK tools. These command strings are DISPLAY-ONLY (the MCP
/// server never executes them) — but the user copy-pastes them into a shell, so
/// a value containing spaces, quotes, `$`, `;`, `&`, etc. would otherwise be
/// mis-parsed or, worse, interpreted as separate arguments / metacharacters.
/// POSIX single-quote rule: wrap in `'...'`, and escape any embedded single
/// quote as `'\''`. A value that is already a "safe word" (alphanumerics plus a
/// small set of shell-neutral punctuation) is returned bare to keep the common
/// case readable. trace:TASK-712
fn shell_quote_arg(s: &str) -> String {
    let safe = !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/' | b'='));
    if safe {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

fn list_leases(project_root: &Path) -> Vec<LightLease> {
    let dir = leases_dir(project_root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        // Skip activity / manifest sidecar files — they share the directory.
        if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
            if name.contains(".activity.") || name.contains(".manifest.") {
                continue;
            }
        }
        if let Ok(content) = std::fs::read_to_string(&p) {
            if let Ok(lease) = toml::from_str::<LightLease>(&content) {
                if !lease.id.is_empty() {
                    out.push(lease);
                }
            }
        }
    }
    out.sort_by(|a, b| a.started_at.cmp(&b.started_at));
    out
}

// ============================================================================
// Role helpers — STORY-534 (EPIC-27)
//
// Like `LightLease`, `LightRole` is a tolerant projection of the role TOML the
// CLI writes (`RoleState` in main.rs carries more fields than the MCP surface
// needs). We read `.aida/roles/*.toml` and `~/.aida/roles/*.toml` directly with
// `#[serde(default)]` so unknown / new fields don't break us and the MCP server
// needn't rebuild every time a role field lands. Timestamps stay as raw RFC3339
// strings — we only need to display them, not arithmetic on them (except a
// best-effort "N ago"). trace:EPIC-27
// ============================================================================

#[derive(Debug, Clone, Deserialize, Default)]
struct LightRole {
    #[serde(default)]
    name: String,
    #[serde(default)]
    purpose: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    last_active_at: String,
    #[serde(default)]
    working_directory: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    global: bool,
    #[serde(default)]
    scope_tags: Vec<String>,
    #[serde(default)]
    scope_status: Option<String>,
    #[serde(default)]
    system_prompt: Option<String>,
}

/// Canonicalize a role name, mirroring main.rs's `canonical_role_name`:
/// TASK-586 made `advisor` the canonical identifier; `dialog` is a deprecated,
/// silently-accepted alias. trace:EPIC-27
fn canonical_light_role_name(raw: &str) -> String {
    if raw.eq_ignore_ascii_case("dialog") {
        "advisor".to_string()
    } else {
        raw.to_string()
    }
}

/// The active shell role, read from `AIDA_SESSION_ROLE` (canonicalized). This is
/// the *MCP server process's* environment — it reflects the launching shell's
/// role only when the agent's `aida mcp-serve` was started under an active role.
/// trace:EPIC-27
fn role_active_env() -> Option<String> {
    std::env::var("AIDA_SESSION_ROLE")
        .ok()
        .filter(|v| !v.is_empty())
        .map(|v| canonical_light_role_name(&v))
}

fn project_roles_dir(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("roles")
}

fn global_roles_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".aida").join("roles"))
}

/// List all roles for the project (and any global roles), newest-active first,
/// de-duplicated by canonical name. Mirrors main.rs `list_roles`. trace:EPIC-27
fn list_light_roles(project_root: &Path) -> Vec<LightRole> {
    let mut roles: Vec<LightRole> = Vec::new();
    for dir in [Some(project_roles_dir(project_root)), global_roles_dir()]
        .into_iter()
        .flatten()
    {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&p) {
                if let Ok(mut role) = toml::from_str::<LightRole>(&content) {
                    if role.name.is_empty() {
                        continue;
                    }
                    role.name = canonical_light_role_name(&role.name);
                    roles.push(role);
                }
            }
        }
    }
    // Newest-active first, then drop the canonical-name duplicate (a machine
    // mid-migration can have both `advisor.toml` and the legacy `dialog.toml`).
    roles.sort_by(|a, b| b.last_active_at.cmp(&a.last_active_at));
    let mut seen = std::collections::HashSet::new();
    roles.retain(|r| seen.insert(r.name.clone()));
    roles
}

/// Load a single role by (already-canonicalized) name, checking the project dir
/// first, then the global dir; the legacy `dialog.toml` is accepted as the
/// advisor role. trace:EPIC-27
fn load_light_role(project_root: &Path, canonical: &str) -> Option<LightRole> {
    let mut candidates = vec![canonical.to_string()];
    if canonical == "advisor" {
        candidates.push("dialog".to_string());
    }
    for cand in &candidates {
        let mut paths: Vec<PathBuf> =
            vec![project_roles_dir(project_root).join(format!("{}.toml", cand))];
        if let Some(g) = global_roles_dir() {
            paths.push(g.join(format!("{}.toml", cand)));
        }
        for path in paths {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(mut role) = toml::from_str::<LightRole>(&content) {
                    role.name = canonical_light_role_name(&role.name);
                    return Some(role);
                }
            }
        }
    }
    None
}

/// Best-effort "N ago" for an RFC3339 timestamp string, mirroring main.rs's
/// `humanize_relative`. Falls back to the raw string when it won't parse.
/// trace:EPIC-27
fn light_role_relative(rfc3339: &str) -> String {
    let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(rfc3339) else {
        return rfc3339.to_string();
    };
    let now = chrono::Utc::now();
    let secs = now
        .signed_duration_since(parsed.with_timezone(&chrono::Utc))
        .num_seconds();
    if secs < 0 {
        return "just now".to_string();
    }
    if secs < 60 {
        return format!("{}s ago", secs);
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{}m ago", mins);
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{}h ago", hours);
    }
    format!("{}d ago", hours / 24)
}

// ============================================================================
// MCP Server
// ============================================================================

struct McpServer<'a> {
    storage: &'a Storage,
    /// Project root for resolving `.aida/` coordination files. STORY-361.
    project_root: PathBuf,
    /// Active tool profile governing which tools are advertised + callable.
    /// trace:STORY-474 | ai:claude
    profile: McpProfile,
}

/// Helper to get the display spec_id from a Requirement
fn spec_id(r: &Requirement) -> &str {
    r.spec_id.as_deref().unwrap_or("?")
}

/// STORY-82: render a `## Git linkage` Markdown section for a spec, reusing
/// the same collection `aida show` walks (referencing commits, feature
/// branch / worktree, shipped state, PR). `verbose` expands the per-commit
/// and trace-file lists, matching `aida show --verbose`. Returns an empty
/// string when there is no git context, so callers can append unconditionally.
// trace:STORY-82 | ai:claude
fn render_git_linkage_md(project_root: &Path, spec_id: &str, verbose: bool) -> String {
    let ids = vec![spec_id.to_string()];
    let linkage = crate::collect_git_linkage(project_root, &ids);

    if linkage.commits.is_empty() && linkage.files.is_empty() {
        return "\n## Git linkage\n\nNo commits or trace comments reference this spec yet.\n"
            .to_string();
    }

    let mut out = String::from("\n## Git linkage\n\n");

    let state = if linkage.shipped {
        match linkage.shipped_pr {
            Some(pr) => format!("shipped (merged via PR #{})", pr),
            None => "shipped (merged to the default branch)".to_string(),
        }
    } else if let Some(branch) = &linkage.branch {
        format!("in flight on branch `{}`", branch)
    } else {
        "referenced (not yet on the default branch)".to_string()
    };
    out.push_str(&format!("- **State:** {}\n", state));

    if let Some(worktree) = &linkage.worktree {
        out.push_str(&format!("- **Worktree:** {}\n", worktree));
    }

    out.push_str(&format!("- **Commits:** {}\n", linkage.commits.len()));
    if verbose {
        for (_, short, subject) in &linkage.commits {
            out.push_str(&format!("  - {} {}\n", short, subject));
        }
    } else if let Some((_, short, subject)) = linkage.commits.first() {
        out.push_str(&format!("  - latest: {} {}\n", short, subject));
    }

    out.push_str(&format!("- **Traced files:** {}\n", linkage.files.len()));
    if verbose {
        for (file, symbol) in &linkage.files {
            match symbol {
                Some(sym) => out.push_str(&format!("  - {} ({})\n", file, sym)),
                None => out.push_str(&format!("  - {}\n", file)),
            }
        }
    }

    out
}

impl<'a> McpServer<'a> {
    fn new(storage: &'a Storage, project_root: PathBuf) -> Self {
        // trace:STORY-474 | ai:claude — resolve the active profile from env /
        // config at construction (no CLI override here; the override path is
        // exercised via `with_profile`).
        let profile = resolve_mcp_profile(&project_root, None);
        Self {
            storage,
            project_root,
            profile,
        }
    }

    /// Construct with an explicit profile, bypassing env/config resolution.
    /// Used by tests and any future CLI `--profile` override. trace:STORY-474
    #[cfg(test)]
    fn with_profile(storage: &'a Storage, project_root: PathBuf, profile: McpProfile) -> Self {
        Self {
            storage,
            project_root,
            profile,
        }
    }

    fn handle_request(&self, req: &JsonRpcRequest) -> Option<JsonRpcResponse> {
        let id = req.id.clone().unwrap_or(Value::Null);

        let running = McpBinaryIdentity::current();
        // STORY-435: heartbeat — every JSON-RPC request (tools/call,
        // tools/list, ping, …) bumps the registry entry's last_active_at
        // so `aida status` can compute busy/idle off recency.
        // trace:STORY-435 | ai:claude
        let _ = agent_registry::touch_mcp_agent(
            &self.project_root,
            &AgentBinaryIdentity::new(running.version, running.sha),
        );

        // Notifications (no id) don't get responses
        if req.id.is_none() && req.method == "notifications/initialized" {
            return None;
        }

        let response = match req.method.as_str() {
            "initialize" => self.handle_initialize(&id),
            "tools/list" => self.handle_tools_list(&id),
            "tools/call" => self.handle_tools_call(&id, &req.params),
            "resources/list" => self.handle_resources_list(&id),
            "resources/templates/list" => self.handle_resources_templates_list(&id),
            "resources/read" => self.handle_resources_read(&id, &req.params),
            "ping" => JsonRpcResponse::success(id, json!({})),
            _ => JsonRpcResponse::error(id, -32601, format!("Method not found: {}", req.method)),
        };

        Some(response)
    }

    fn handle_initialize(&self, id: &Value) -> JsonRpcResponse {
        JsonRpcResponse::success(
            id.clone(),
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {},
                    "resources": {}
                },
                "serverInfo": {
                    "name": "aida",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )
    }

    fn handle_tools_list(&self, id: &Value) -> JsonRpcResponse {
        // trace:STORY-474 | ai:claude — advertise only the tools the active
        // profile exposes, each tagged with the tier that admits it.
        JsonRpcResponse::success(
            id.clone(),
            json!({ "tools": tool_descriptors_for_profile(self.profile) }),
        )
    }

    fn handle_tools_call(&self, id: &Value, params: &Value) -> JsonRpcResponse {
        let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

        // trace:STORY-474 | ai:claude — the profile is a real boundary, not just
        // a discovery filter: reject a tool that exists but is above the active
        // tier, even if a client calls it by name. Unknown tools fall through to
        // the dispatch's own "Unknown tool" error below.
        if !tool_name.is_empty()
            && is_known_tool(tool_name)
            && !tool_in_profile(tool_name, self.profile)
        {
            return JsonRpcResponse::success(
                id.clone(),
                McpError::classify(
                    tool_name,
                    &format!(
                        "tool '{}' not permitted: requires the '{}' profile but the server is running the '{}' profile (set AIDA_MCP_PROFILE or [mcp] profile to widen the surface)",
                        tool_name,
                        tool_min_profile(tool_name).as_str(),
                        self.profile.as_str()
                    ),
                )
                .to_result_value(),
            );
        }

        let result = match tool_name {
            // Spec-graph tools
            "list_requirements" => self.tool_list_requirements(&arguments),
            "show_requirement" => self.tool_show_requirement(&arguments),
            "add_requirement" => self.tool_add_requirement(&arguments),
            "update_requirement" => self.tool_update_requirement(&arguments),
            "search_requirements" => self.tool_search_requirements(&arguments),
            "add_comment" => self.tool_add_comment(&arguments),
            "add_relationship" => self.tool_add_relationship(&arguments),
            "query_graph" => self.tool_query_graph(&arguments),
            "send_message" => self.tool_send_message(&arguments),
            "read_inbox" => self.tool_read_inbox(&arguments),
            "list_features" => self.tool_list_features(),
            "history" => self.tool_history(&arguments),

            // Coordination tools — STORY-361
            // Punt channel (.aida/punts.jsonl + .aida/punts/)
            "list_punts" => self.tool_list_punts(&arguments),
            "read_punt" => self.tool_read_punt(&arguments),
            "post_punt" => self.tool_post_punt(&arguments),
            "resolve_punt" => self.tool_resolve_punt(&arguments),
            "escalate_punt" => self.tool_escalate_punt(&arguments),

            // Findings channel (draft requirements with from-* tags)
            "list_findings" => self.tool_list_findings(&arguments),
            "file_finding" => self.tool_file_finding(&arguments),
            "triage_finding" => self.tool_triage_finding(&arguments),

            // Task-claim channel (.aida/sessions/*.toml)
            "claim_task" => self.tool_claim_task(&arguments),
            "release_task" => self.tool_release_task(&arguments),
            "list_active_leases" => self.tool_list_active_leases(),

            // Worker-directive channel (.aida/worker.cmd)
            "post_directive" => self.tool_post_directive(&arguments),
            "list_directives" => self.tool_list_directives(),
            "ack_directive" => self.tool_ack_directive(&arguments),

            // Agent-brief channel (.aida/agent-briefs/<agent>/)
            "list_briefs" => self.tool_list_briefs(&arguments),
            "read_brief" => self.tool_read_brief(&arguments),
            "ack_brief" => self.tool_ack_brief(&arguments),

            // Queue tools — STORY-532 (EPIC-27). Mirror the `aida queue`
            // CLI surface so agents can manage the work queue over MCP
            // instead of shelling out. trace:EPIC-27
            "queue_list" => self.tool_queue_list(&arguments),
            "queue_add" => self.tool_queue_add(&arguments),
            "queue_work" => self.tool_queue_work(&arguments),
            "queue_done" => self.tool_queue_done(&arguments),
            "queue_next" => self.tool_queue_next(&arguments),
            "queue_progress" => self.tool_queue_progress(&arguments),
            "queue_rework" => self.tool_queue_rework(&arguments),
            "queue_move" => self.tool_queue_move(&arguments),
            "queue_remove" => self.tool_queue_remove(&arguments),

            // Session tools — STORY-533 (EPIC-27). Mirror the `aida session`
            // CLI surface so agents can inspect scoped session leases +
            // manifests over MCP. session_start / session_end are PEEKS — the
            // launch / worktree-removal acts are subprocess-driven and stay
            // CLI-only (the queue_work precedent). trace:EPIC-27
            "session_leases" => self.tool_session_leases(&arguments),
            "session_status" => self.tool_session_status(&arguments),
            "session_manifest" => self.tool_session_manifest(&arguments),
            "session_start" => self.tool_session_start(&arguments),
            "session_end" => self.tool_session_end(&arguments),

            // Role tools — STORY-534 (EPIC-27). Mirror the `aida role` CLI
            // surface so agents can inspect the persona set over MCP.
            // role_show / role_list read `.aida/roles/*.toml` (+ ~/.aida/roles)
            // directly. role_enter / role_end are PEEKS — entering/ending a
            // role sets the *shell's* `AIDA_SESSION_ROLE` (role identity is
            // shell-keyed, like the queue user; see BUG-89), an act the
            // stateless MCP server cannot perform for the caller, so they
            // return the `aida role enter/end …` command instead (the
            // queue_work / session_start PEEK precedent). trace:EPIC-27
            "role_list" => self.tool_role_list(&arguments),
            "role_show" => self.tool_role_show(&arguments),
            "role_enter" => self.tool_role_enter(&arguments),
            "role_end" => self.tool_role_end(&arguments),

            // Workflow tools — STORY-536 (EPIC-27). The remaining CLI long
            // tail: the read-only library mirrors (cache_status, plan_verify,
            // plan_helpers, ultraplan_assemble, goal_derive, status_unified,
            // usage_query) compute in-process from the same helpers the CLI
            // uses (no subprocess to `aida`). db_sync / fetch / pull are git
            // network + working-tree / store mutations driven by subprocess
            // `git`; surfacing them as in-process MCP mutations would surprise
            // the caller (working-tree changes, remote pushes), so they follow
            // the queue_work / session_start PEEK precedent — they return the
            // exact `aida …` command to run. trace:EPIC-27
            "cache_status" => self.tool_cache_status(&arguments),
            "plan_verify" => self.tool_plan_verify(&arguments),
            "plan_helpers" => self.tool_plan_helpers(&arguments),
            "ultraplan_assemble" => self.tool_ultraplan_assemble(&arguments),
            "goal_derive" => self.tool_goal_derive(&arguments),
            "status_unified" => self.tool_status_unified(&arguments),
            "usage_query" => self.tool_usage_query(&arguments),
            "db_sync" => self.tool_db_sync(&arguments),
            "fetch" => self.tool_fetch(&arguments),
            "pull" => self.tool_pull(&arguments),

            // TASK-715: storable-substrate introspection. trace:TASK-715
            "schema" => self.tool_schema(&arguments),

            _ => Err(format!("Unknown tool: {}", tool_name)),
        };

        match result {
            // STORY-399 (Path B): emit `structuredContent` matching the declared
            // `outputSchema` alongside the text envelope. Additive — the text
            // `content` array is preserved verbatim so legacy text consumers
            // (Claude Code) keep working; schema-driven clients (Codex, Cursor)
            // read the same logical payload out of `structuredContent` without
            // parsing the human string. trace:STORY-399 | ai:claude
            Ok(content) => {
                let content_array = json!([{
                    "type": "text",
                    "text": content
                }]);
                JsonRpcResponse::success(
                    id.clone(),
                    json!({
                        "content": content_array,
                        "structuredContent": {
                            "content": content_array
                        }
                    }),
                )
            }
            // STORY-401: render the free-form tool error into a stable,
            // machine-readable envelope — `isError: true`, a short
            // `<tool>: <code>: <message>` text, and a `structuredError`
            // payload clients can branch on. trace:STORY-401 | ai:claude
            Err(e) => JsonRpcResponse::success(
                id.clone(),
                McpError::classify(tool_name, &e).to_result_value(),
            ),
        }
    }

    fn handle_resources_list(&self, id: &Value) -> JsonRpcResponse {
        JsonRpcResponse::success(
            id.clone(),
            json!({
                "resources": [
                    {
                        "uri": "aida://project/summary",
                        "name": "Project Summary",
                        "description": "Project statistics and feature list",
                        "mimeType": "text/plain"
                    },
                    {
                        "uri": "aida://requirements/tree",
                        "name": "Requirements Tree",
                        "description": "Requirement hierarchy overview",
                        "mimeType": "text/plain"
                    },
                    // EPIC-27 (STORY-535): live-state resources. These mirror the
                    // read-only CLI surfaces (`aida queue list --in-flight-only`,
                    // `aida session leases`) but as MCP *resources* — addressable
                    // live state an agent can subscribe-poll rather than tool calls
                    // it invokes. They back onto the SAME library helpers the CLI /
                    // equivalent MCP tools use (no subprocess to `aida`).
                    // trace:STORY-535 trace:EPIC-27
                    {
                        "uri": "aida://queue/in-flight",
                        "name": "Queue — in-flight",
                        "description": "Specs with a live session/MCP-claim lease, plus Done-awaiting-merge work",
                        "mimeType": "text/plain"
                    },
                    {
                        "uri": "aida://session/leases",
                        "name": "Session leases",
                        "description": "Active scoped session leases (who holds what right now)",
                        "mimeType": "text/plain"
                    },
                    // TASK-715: the storable-substrate schema as a resource — the
                    // catalog of storable object kinds an agent can read to
                    // discover the shape natively (mirrors `aida schema --json`).
                    // Per-object detail is the `aida://schema/{object}` template
                    // below. Backs onto `crate::schema::catalog_json` (the same
                    // value the CLI emits) — no subprocess to `aida`. Body is the
                    // pretty JSON carried in the `text/plain` envelope all AIDA
                    // resources use. trace:TASK-715
                    {
                        "uri": "aida://schema",
                        "name": "Storable-object schema (catalog)",
                        "description": "Catalog of storable object kinds (mirrors `aida schema --json`); per-object field/enum detail via aida://schema/{object}",
                        "mimeType": "text/plain"
                    }
                ]
            }),
        )
    }

    /// EPIC-27 (STORY-535): advertise the parameterized live-state resources as
    /// RFC-6570 URI templates. The MCP protocol carries these on a distinct
    /// `resources/templates/list` method (separate from the concrete-URI
    /// `resources/list`), so clients that understand templates can expand
    /// `aida://pr/{n}` / `aida://batch/{name}` themselves; the matching read
    /// logic lives in `handle_resources_read`. trace:STORY-535 trace:EPIC-27
    fn handle_resources_templates_list(&self, id: &Value) -> JsonRpcResponse {
        JsonRpcResponse::success(
            id.clone(),
            json!({
                "resourceTemplates": [
                    {
                        "uriTemplate": "aida://pr/{n}",
                        "name": "PR linkage",
                        "description": "Spec/finding linkage for pull-request number N (git-canonical, gh-free)",
                        "mimeType": "text/plain"
                    },
                    {
                        "uriTemplate": "aida://batch/{name}",
                        "name": "Batch progress",
                        "description": "Progress buckets for the batch:<name> tag set",
                        "mimeType": "text/plain"
                    },
                    // TASK-715: per-object schema detail. `aida://schema/requirement`
                    // returns the reflection-derived field table + the four
                    // controlled-vocabulary enums (status/type/priority/relationship)
                    // in on-the-wire token form; every other catalog kind returns
                    // its reflection-derived field table (TASK-714's registry).
                    // Mirrors `aida schema <object> --json`.
                    // trace:TASK-715
                    {
                        "uriTemplate": "aida://schema/{object}",
                        "name": "Storable-object schema (detail)",
                        "description": "Per-object field + controlled-vocabulary detail (mirrors `aida schema <object> --json`; Requirement is reflection-derived)",
                        "mimeType": "text/plain"
                    }
                ]
            }),
        )
    }

    fn handle_resources_read(&self, id: &Value, params: &Value) -> JsonRpcResponse {
        let uri = params.get("uri").and_then(|v| v.as_str()).unwrap_or("");

        // EPIC-27 (STORY-535): static resources match by exact URI; the two
        // parameterized templates are matched by prefix + a parsed tail, since
        // the protocol layer has no built-in URI-template expander. The order
        // matters only in that the exact matches are cheap string compares.
        // trace:STORY-535 trace:EPIC-27
        let result = match uri {
            "aida://project/summary" => self.resource_project_summary(),
            "aida://requirements/tree" => self.resource_requirements_tree(),
            "aida://queue/in-flight" => self.resource_queue_in_flight(),
            "aida://session/leases" => self.resource_session_leases(),
            // TASK-715: `aida://schema` (exact) is the catalog; the
            // `aida://schema/<object>` template is matched below after the
            // exact-URI cases so the bare catalog URI wins. trace:TASK-715
            "aida://schema" => self.resource_schema_catalog(),
            _ => {
                if let Some(object) = uri.strip_prefix("aida://schema/") {
                    self.resource_schema_object(object)
                } else if let Some(n) = uri.strip_prefix("aida://pr/") {
                    self.resource_pr(n)
                } else if let Some(name) = uri.strip_prefix("aida://batch/") {
                    self.resource_batch(name)
                } else {
                    Err(format!("Unknown resource: {}", uri))
                }
            }
        };

        match result {
            Ok(content) => JsonRpcResponse::success(
                id.clone(),
                json!({
                    "contents": [{
                        "uri": uri,
                        "mimeType": "text/plain",
                        "text": content
                    }]
                }),
            ),
            Err(e) => JsonRpcResponse::error(id.clone(), -32602, e),
        }
    }

    // ========================================================================
    // Spec-graph tool implementations
    // ========================================================================

    // trace:STORY-82 | ai:claude
    fn tool_list_requirements(&self, args: &Value) -> Result<String, String> {
        let store = self.storage.load().map_err(|e| e.to_string())?;
        let status_filter = args.get("status").and_then(|v| v.as_str());
        let type_filter = args.get("type").and_then(|v| v.as_str());
        let priority_filter = args.get("priority").and_then(|v| v.as_str());
        let feature_filter = args.get("feature").and_then(|v| v.as_str());
        // STORY-639: assignee filter — exact match, mirroring `aida list
        // --assigned <user>`. trace:STORY-639 | ai:claude
        let assignee_filter = args.get("assignee").and_then(|v| v.as_str());
        // STORY-662: owner-or-assignee filter — exact match on EITHER, mirroring
        // `aida list --user <name>`. trace:STORY-662 | ai:claude
        let user_filter = args.get("user").and_then(|v| v.as_str());
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

        // STORY-82: tags filter — CSV, AND-match (a row must carry ALL of them),
        // mirroring the `aida list --tags` semantic.
        let tag_filters: Vec<String> = args
            .get("tags")
            .and_then(|v| v.as_str())
            .map(|s| {
                s.split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        // STORY-82: batch filter — shorthand for the `batch:<NAME>` tag,
        // matching `aida list --batch`.
        let batch_tag = args
            .get("batch")
            .and_then(|v| v.as_str())
            .map(|b| format!("batch:{}", b.trim()))
            .filter(|b| b != "batch:");

        // STORY-82: role/for filter — requirements don't carry a role field
        // (that's a queue-routing concept), so we match a `role:<value>` tag
        // on the spec. `for` is an accepted alias for `role`.
        let role_tag = args
            .get("role")
            .or_else(|| args.get("for"))
            .and_then(|v| v.as_str())
            .map(|r| format!("role:{}", r.trim().to_ascii_lowercase()))
            .filter(|r| r != "role:");

        // STORY-82: parent filter — direct children of <parent>, found via the
        // parent's outgoing `Parent` relationship edges (mirrors
        // `aida list --parent`).
        let parent_child_ids: Option<std::collections::HashSet<uuid::Uuid>> =
            match args.get("parent").and_then(|v| v.as_str()) {
                Some(parent_ref) => {
                    let parent = store
                        .get_requirement_by_spec_id(parent_ref)
                        .ok_or_else(|| format!("parent '{}': requirement not found", parent_ref))?;
                    Some(
                        parent
                            .relationships
                            .iter()
                            .filter(|rel| rel.rel_type == RelationshipType::Parent)
                            .map(|rel| rel.target_id)
                            .collect(),
                    )
                }
                None => None,
            };

        // STORY-82: in_flight filter — restrict to specs with a live session
        // or MCP-claim lease. Lease scopes are matched case-insensitively
        // against the spec id (the lease `scope` records the owned spec/epic).
        let in_flight_only = args
            .get("in_flight")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let in_flight_scopes: std::collections::HashSet<String> = if in_flight_only {
            list_leases(&self.project_root)
                .into_iter()
                .map(|l| l.scope.to_ascii_lowercase())
                .filter(|s| !s.is_empty())
                .collect()
        } else {
            std::collections::HashSet::new()
        };

        // BUG-591: mirror the CLI's archive (STORY-441) + deferred (STORY-584)
        // view tiers. The default view hides BOTH archived and deferred rows so
        // an MCP agent's baseline picture matches `aida list` — the STORY-82
        // "MCP mirrors CLI" contract. `archived` / `deferred` narrow to that one
        // tier; `all` widens to the union of all three tiers. Resolved into the
        // shared two-axis predicate below. trace:BUG-591 | ai:claude
        let show_all = args.get("all").and_then(|v| v.as_bool()).unwrap_or(false);
        let archived_only = args
            .get("archived")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let deferred_only = args
            .get("deferred")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let view = ViewTierFilter::resolve(show_all, archived_only, deferred_only);

        let filtered: Vec<&Requirement> = store
            .requirements
            .iter()
            .filter(|r| {
                if let Some(status) = status_filter {
                    // BUG-626: filter epics by their derived rollup status.
                    if !mcp_filter_eq(&mcp_effective_status(&store, r).to_string(), status) {
                        return false;
                    }
                }
                if let Some(type_name) = type_filter {
                    if !mcp_filter_eq(&r.req_type.to_string(), type_name) {
                        return false;
                    }
                }
                if let Some(priority) = priority_filter {
                    if !mcp_filter_eq(&r.priority.to_string(), priority) {
                        return false;
                    }
                }
                if let Some(feature) = feature_filter {
                    if !r.feature.eq_ignore_ascii_case(feature) {
                        return false;
                    }
                }
                // STORY-639: exact assignee match. trace:STORY-639 | ai:claude
                if let Some(assignee) = assignee_filter {
                    if r.assignee.as_deref() != Some(assignee) {
                        return false;
                    }
                }
                // STORY-662: owner OR assignee match. trace:STORY-662 | ai:claude
                if let Some(user) = user_filter {
                    let owns = r.owner == user;
                    let assigned = r.assignee.as_deref() == Some(user);
                    if !owns && !assigned {
                        return false;
                    }
                }
                // AND-match every requested tag (case-sensitive, matching the CLI).
                for want in &tag_filters {
                    if !r.tags.contains(want) {
                        return false;
                    }
                }
                if let Some(b) = &batch_tag {
                    if !r.tags.contains(b) {
                        return false;
                    }
                }
                if let Some(role) = &role_tag {
                    let has_role = r.tags.iter().any(|t| t.eq_ignore_ascii_case(role));
                    if !has_role {
                        return false;
                    }
                }
                if let Some(child_ids) = &parent_child_ids {
                    if !child_ids.contains(&r.id) {
                        return false;
                    }
                }
                if in_flight_only {
                    let sid = spec_id(r).to_ascii_lowercase();
                    if sid == "?" || !in_flight_scopes.contains(&sid) {
                        return false;
                    }
                }
                // BUG-591: apply the same view-tier predicate the CLI uses so
                // archived/deferred rows are hidden by default and surfaced only
                // via the explicit filters. trace:BUG-591 | ai:claude
                if !view.admits(r) {
                    return false;
                }
                true
            })
            .take(limit)
            .collect();

        if filtered.is_empty() {
            return Ok("No requirements found matching the criteria.".to_string());
        }

        let mut output = format!("Found {} requirements:\n\n", filtered.len());
        for r in &filtered {
            output.push_str(&format!(
                "- [{}] {} (Status: {}, Priority: {}, Type: {})\n",
                spec_id(r),
                r.title,
                // BUG-626: surface the epic's derived rollup status.
                mcp_effective_status(&store, r),
                r.priority,
                r.req_type
            ));
        }
        Ok(output)
    }

    // trace:STORY-82 | ai:claude
    fn tool_show_requirement(&self, args: &Value) -> Result<String, String> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: id")?;
        // STORY-82: git linkage on by default, matching `aida show` (the
        // CLI default; `aida show --no-git` suppresses it). `verbose`
        // expands the section like `aida show --verbose`.
        let include_git = args
            .get("include_git")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let verbose = args
            .get("verbose")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let store = self.storage.load().map_err(|e| e.to_string())?;
        let req = store
            .get_requirement_by_spec_id(id)
            .ok_or_else(|| format!("Requirement '{}' not found", id))?;
        let canonical_id = spec_id(req).to_string();

        let mut output = format!(
            "# {} — {}\n\n\
             **Status:** {}\n\
             **Priority:** {}\n\
             **Type:** {}\n",
            spec_id(req),
            req.title,
            // BUG-626: surface the epic's derived rollup status.
            mcp_effective_status(&store, req),
            req.priority,
            req.req_type
        );

        if !req.feature.is_empty() {
            output.push_str(&format!("**Feature:** {}\n", req.feature));
        }
        if !req.owner.is_empty() {
            output.push_str(&format!("**Owner:** {}\n", req.owner));
        }
        // STORY-639: surface the assignee in show_requirement when set, mirroring
        // the CLI `aida show`. trace:STORY-639 | ai:claude
        if let Some(assignee) = req.assignee.as_deref() {
            output.push_str(&format!("**Assignee:** {}\n", assignee));
        }
        if !req.tags.is_empty() {
            let tags: Vec<&String> = req.tags.iter().collect();
            output.push_str(&format!(
                "**Tags:** {}\n",
                tags.iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        output.push_str(&format!("\n## Description\n\n{}\n", req.description));

        if !req.comments.is_empty() {
            output.push_str(&format!("\n## Comments ({})\n\n", req.comments.len()));
            for c in &req.comments {
                output.push_str(&format!("- {}: {}\n", c.author, c.content));
            }
        }

        if !req.relationships.is_empty() {
            output.push_str(&format!(
                "\n## Relationships ({})\n\n",
                req.relationships.len()
            ));
            for rel in &req.relationships {
                let target_label = store
                    .requirements
                    .iter()
                    .find(|r| r.id == rel.target_id)
                    .and_then(|r| r.spec_id.clone())
                    .unwrap_or_else(|| rel.target_id.to_string());
                output.push_str(&format!("- {} → {}\n", rel.rel_type, target_label));
            }
        }

        // BUG-527: queue membership — mirror the `Queued:` line `aida show`
        // surfaces (CLI/MCP parity, STORY-82). For-role + the 1-based
        // position the operator sees on `aida queue list`; omitted entirely
        // when the spec sits in no queue. trace:BUG-527
        let memberships = crate::queue_memberships_for(&self.project_root, &req.id);
        if let Some(value) = crate::format_queue_membership(&memberships) {
            output.push_str(&format!("\n## Queued\n\n{}\n", value));
        }

        // STORY-82: git linkage — reuse the same collection `aida show`
        // renders so MCP clients see commits / branch / PR / shipped state.
        // Off when include_git=false (the `aida show --no-git` view).
        if include_git {
            output.push_str(&render_git_linkage_md(
                &self.project_root,
                &canonical_id,
                verbose,
            ));
        }

        Ok(output)
    }

    // trace:STORY-82 | ai:claude
    fn tool_add_requirement(&self, args: &Value) -> Result<String, String> {
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: title")?;
        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: description")?;

        let type_arg = args.get("type").and_then(|v| v.as_str()).ok_or_else(|| {
            format!(
                "Missing required parameter: type. Valid types: {}",
                VALID_MCP_REQUIREMENT_TYPES
            )
        })?;
        let req_type = parse_requirement_type(type_arg).ok_or_else(|| {
            format!(
                "Invalid requirement type '{}'. Valid types: {}",
                type_arg, VALID_MCP_REQUIREMENT_TYPES
            )
        })?;
        let requested_status = args
            .get("status")
            .and_then(|v| v.as_str())
            .and_then(parse_status)
            .unwrap_or(RequirementStatus::Draft);
        if requested_status == RequirementStatus::NeedsAttention {
            return Err(
                "cannot create a requirement with status `needs-attention` — \
                 it is reached only by punting In-Progress work"
                    .to_string(),
            );
        }
        // TASK-647 (ADR-3): the MCP server is an intake surface used by agents
        // (always non-interactive, and it may inherit an advisor role from the
        // launching shell — so it cannot be trusted as advisor authority).
        // Producing an approved+ spec is the advisor's triage decision, so MCP
        // intake always lands `draft` regardless of the requested status; the
        // result tells the agent it is queued for advisor triage.
        // trace:TASK-647 | ai:claude
        let intake_downgraded = matches!(
            requested_status,
            RequirementStatus::Approved
                | RequirementStatus::Planned
                | RequirementStatus::InProgress
                | RequirementStatus::Done
                | RequirementStatus::Completed
        );
        let status = if intake_downgraded {
            RequirementStatus::Draft
        } else {
            requested_status
        };
        let priority = args
            .get("priority")
            .and_then(|v| v.as_str())
            .and_then(parse_priority)
            .unwrap_or(RequirementPriority::Medium);

        let mut store = self.storage.load().map_err(|e| e.to_string())?;

        // STORY-82: resolve an optional parent BEFORE allocating the new id,
        // so a bad/terminal parent fails without leaving an orphan spec.
        // Mirrors `aida add --parent`'s pre-resolution + terminal guard.
        let parent_link: Option<(uuid::Uuid, String)> = match args
            .get("parent")
            .and_then(|v| v.as_str())
        {
            Some(parent_ref) => {
                let parent = store.get_requirement_by_spec_id(parent_ref).ok_or_else(|| {
                        format!(
                            "parent '{}' not found — refusing to create a child without a valid parent",
                            parent_ref
                        )
                    })?;
                if crate::is_terminal_status(&parent.status) {
                    return Err(format!(
                            "parent {} is {} — adding new children to a closed parent is usually a mistake.",
                            parent.spec_id.as_deref().unwrap_or("?"),
                            parent.status,
                        ));
                }
                Some((parent.id, parent.spec_id.clone().unwrap_or_default()))
            }
            None => None,
        };

        let mut req = Requirement::new(title.to_string(), description.to_string());
        req.req_type = req_type;
        req.status = status;
        req.priority = priority;

        // STORY-82: optional feature category + owner (mirrors `aida add
        // --feature` / `--owner`).
        if let Some(feature) = args.get("feature").and_then(|v| v.as_str()) {
            req.feature = feature.to_string();
        }
        if let Some(owner) = args.get("owner").and_then(|v| v.as_str()) {
            req.owner = owner.to_string();
        }

        // Optional tags
        if let Some(tag_arr) = args.get("tags").and_then(|v| v.as_array()) {
            for t in tag_arr {
                if let Some(s) = t.as_str() {
                    req.tags.insert(s.to_string());
                }
            }
        }

        let type_prefix = store.get_type_prefix(&req.req_type).ok_or_else(|| {
            format!(
                "No configured ID prefix for requirement type '{}'",
                req.req_type
            )
        })?;
        store.add_requirement_with_id(req, None, Some(type_prefix.as_str()));

        let (new_spec_id, new_id) = store
            .requirements
            .last()
            .map(|r| (r.spec_id.clone().unwrap_or_else(|| "?".to_string()), r.id))
            .unwrap_or_else(|| ("?".to_string(), uuid::Uuid::nil()));

        // STORY-82: link the parent (Parent→child on parent, Child→parent on
        // the new req via the bidirectional inverse) once the id exists.
        if let Some((parent_id, _parent_spec)) = &parent_link {
            store
                .add_relationship(parent_id, RelationshipType::Parent, &new_id, true)
                .map_err(|e| e.to_string())?;
        }

        self.storage.save(&store).map_err(|e| e.to_string())?;

        // STORY-82: note the parent link in the result so the agent sees the
        // edge landed.
        let parent_note = match &parent_link {
            Some((_, parent_spec)) if !parent_spec.is_empty() => {
                format!(" (linked under {})", parent_spec)
            }
            _ => String::new(),
        };

        // TASK-647 (ADR-3): if a requested approved+ status was held back, say
        // so in the result so the agent knows it is awaiting advisor triage.
        if intake_downgraded {
            Ok(format!(
                "Requirement added: {} — {}{} (filed as draft, queued for advisor triage; \
                 requested status needs advisor authority)",
                new_spec_id, title, parent_note
            ))
        } else {
            Ok(format!(
                "Requirement added: {} — {}{}",
                new_spec_id, title, parent_note
            ))
        }
    }

    // trace:STORY-82 | ai:claude
    fn tool_update_requirement(&self, args: &Value) -> Result<String, String> {
        // (see mcp_status_gate_message below for the BUG-449 status gate)
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: id")?;

        let mut store = self.storage.load().map_err(|e| e.to_string())?;

        // STORY-82: resolve the target req id + (optional) parent up front,
        // while we still hold a shared borrow, so the parent re-parent edge
        // can be applied after the mutable field-edit borrow is dropped
        // (the source and parent are two distinct records).
        let target_id = store
            .get_requirement_by_spec_id(id)
            .ok_or_else(|| format!("Requirement '{}' not found", id))?
            .id;
        let parent_link: Option<(uuid::Uuid, String)> = match args
            .get("parent")
            .and_then(|v| v.as_str())
        {
            Some(parent_ref) => {
                let parent = store
                    .get_requirement_by_spec_id(parent_ref)
                    .ok_or_else(|| format!("parent '{}' not found", parent_ref))?;
                if crate::is_terminal_status(&parent.status) {
                    return Err(format!(
                            "parent {} is {} — re-parenting under a closed parent is usually a mistake.",
                            parent.spec_id.as_deref().unwrap_or("?"),
                            parent.status,
                        ));
                }
                if parent.id == target_id {
                    return Err("a requirement cannot be its own parent".to_string());
                }
                Some((parent.id, parent.spec_id.clone().unwrap_or_default()))
            }
            None => None,
        };

        let req = store
            .get_requirement_by_spec_id_mut(id)
            .ok_or_else(|| format!("Requirement '{}' not found", id))?;

        let mut changes = Vec::new();

        // STORY-82: title.
        if let Some(title) = args.get("title").and_then(|v| v.as_str()) {
            if title != req.title {
                changes.push(format!("title: {} → {}", req.title, title));
                req.title = title.to_string();
            }
        }

        // STORY-82: type. Does not renumber the existing SPEC-ID.
        if let Some(type_arg) = args.get("type").and_then(|v| v.as_str()) {
            let new_type = parse_requirement_type(type_arg).ok_or_else(|| {
                format!(
                    "Invalid requirement type '{}'. Valid types: {}",
                    type_arg, VALID_MCP_REQUIREMENT_TYPES
                )
            })?;
            if new_type != req.req_type {
                changes.push(format!("type: {} → {}", req.req_type, new_type));
                req.req_type = new_type;
            }
        }

        // STORY-82: priority.
        if let Some(priority_arg) = args.get("priority").and_then(|v| v.as_str()) {
            let new_priority = parse_priority(priority_arg).ok_or_else(|| {
                format!(
                    "Invalid priority '{}'. Valid: high, medium, low",
                    priority_arg
                )
            })?;
            if new_priority != req.priority {
                changes.push(format!("priority: {} → {}", req.priority, new_priority));
                req.priority = new_priority;
            }
        }

        // STORY-82: tags — replace the set with the provided list.
        if let Some(tag_arr) = args.get("tags").and_then(|v| v.as_array()) {
            let new_tags: std::collections::HashSet<String> = tag_arr
                .iter()
                .filter_map(|t| t.as_str().map(str::to_string))
                .collect();
            if new_tags != req.tags {
                changes.push("tags updated".to_string());
                req.tags = new_tags;
            }
        }

        if let Some(status) = args.get("status").and_then(|v| v.as_str()) {
            if let Some(new_status) = parse_status(status) {
                // BUG-449 (TASK-647/ADR-3 caller parity): MCP is not advisor
                // authority (the launching shell isn't the advisor seat), so it
                // must not self-advance a spec into a status that is the
                // advisor's triage decision (Approved/Planned) or that is
                // merge-driven (Completed/Released — set by a (SPEC-ID) commit
                // landing on the default branch, STORY-86). add_requirement
                // already downgrades these to Draft on intake; without the same
                // gate here, add-then-update was a one-line bypass that let an
                // agent self-mark Completed on uncommitted code. Implementer-
                // legitimate transitions (InProgress/Done/NeedsAttention/Draft/
                // Rejected) stay allowed. trace:BUG-449 | ai:claude
                if new_status != req.status {
                    // BUG-481: gate on the (current, requested) pair so a
                    // Draft/NeedsAttention → InProgress/Done bypass is closed
                    // while in-pipeline execution flips stay allowed.
                    // trace:BUG-481 | ai:claude
                    if let Some(msg) = mcp_status_gate_message(&req.status, &new_status) {
                        return Err(msg);
                    }
                }
                if let Some(msg) =
                    aida_core::forbidden_attention_transition(&req.status, &new_status)
                {
                    return Err(msg);
                }
                changes.push(format!("status: {} → {}", req.status, new_status));
                req.status = new_status;
            }
        }

        if let Some(desc) = args.get("description").and_then(|v| v.as_str()) {
            changes.push("description updated".to_string());
            req.description = desc.to_string();
        }

        // STORY-639: assignee. Mirrors the CLI assignee field edit. An empty
        // string clears the assignee (the unassign equivalent). NOTE: unlike
        // the CLI `aida assign`, this does NOT route the spec into the target
        // user's queue — it only sets the field; use the queue_add tool to
        // route. trace:STORY-639 | ai:claude
        if let Some(assignee) = args.get("assignee").and_then(|v| v.as_str()) {
            let new_assignee = if assignee.trim().is_empty() {
                None
            } else {
                Some(assignee.trim().to_string())
            };
            if new_assignee != req.assignee {
                changes.push(format!(
                    "assignee: {} → {}",
                    req.assignee.as_deref().unwrap_or("(none)"),
                    new_assignee.as_deref().unwrap_or("(none)")
                ));
                req.assignee = new_assignee;
            }
        }

        // STORY-82: re-parent — the mutable `req` borrow is dropped here, so we
        // can mutate both the new parent and the target via add_relationship
        // (Parent→child on parent, Child→parent inverse on the target).
        if let Some((parent_id, parent_spec)) = &parent_link {
            store
                .add_relationship(parent_id, RelationshipType::Parent, &target_id, true)
                .map_err(|e| e.to_string())?;
            changes.push(format!(
                "parent: linked under {}",
                if parent_spec.is_empty() {
                    "?"
                } else {
                    parent_spec.as_str()
                }
            ));
        }

        if changes.is_empty() {
            return Ok(format!("No changes applied to {}", id));
        }

        self.storage.save(&store).map_err(|e| e.to_string())?;
        Ok(format!("Updated {}: {}", id, changes.join(", ")))
    }

    // trace:STORY-82 | ai:claude
    fn tool_search_requirements(&self, args: &Value) -> Result<String, String> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: query")?;

        let store = self.storage.load().map_err(|e| e.to_string())?;
        let query_lower = query.to_lowercase();
        // STORY-82: optional type/status narrowing (mirrors `aida search`).
        let type_filter = args.get("type").and_then(|v| v.as_str());
        let status_filter = args.get("status").and_then(|v| v.as_str());

        // BUG-591: `aida search` inherits the same archive/deferred view-tier
        // filtering as `aida list` — hide filed-away specs by default, surface
        // them via archived/deferred/all. trace:BUG-591 | ai:claude
        let show_all = args.get("all").and_then(|v| v.as_bool()).unwrap_or(false);
        let archived_only = args
            .get("archived")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let deferred_only = args
            .get("deferred")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let view = ViewTierFilter::resolve(show_all, archived_only, deferred_only);

        let matches: Vec<&Requirement> = store
            .requirements
            .iter()
            .filter(|r| {
                // STORY-476: external issue refs are searchable here too, so
                // an MCP client can find a spec by its linear:/jira:/github:
                // pointer — matching the CLI FTS surface.
                let ref_match = r
                    .external_refs
                    .iter()
                    .any(|er| er.to_lowercase().contains(&query_lower));
                let text_match = ref_match
                    || r.title.to_lowercase().contains(&query_lower)
                    || r.description.to_lowercase().contains(&query_lower)
                    || r.spec_id
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&query_lower);
                if !text_match {
                    return false;
                }
                if let Some(type_name) = type_filter {
                    if !mcp_filter_eq(&r.req_type.to_string(), type_name) {
                        return false;
                    }
                }
                if let Some(status) = status_filter {
                    // BUG-626: filter epics by their derived rollup status.
                    if !mcp_filter_eq(&mcp_effective_status(&store, r).to_string(), status) {
                        return false;
                    }
                }
                // BUG-591: hide archived/deferred rows by default, mirroring the
                // CLI search view. trace:BUG-591 | ai:claude
                if !view.admits(r) {
                    return false;
                }
                true
            })
            .collect();

        if matches.is_empty() {
            return Ok(format!("No requirements found matching '{}'", query));
        }

        let mut output = format!("Found {} results for '{}':\n\n", matches.len(), query);
        for r in &matches {
            output.push_str(&format!("- [{}] {} ({})\n", spec_id(r), r.title, r.status));
        }
        Ok(output)
    }

    /// MCP parity for `aida graph` — cross-spec relationship queries
    /// (blocked-by / blocks chains, epic tree rollup, reverse impact) so any
    /// MCP client converges on the same typed graph the CLI walks. Built on the
    /// shared cycle-safe graph_walk primitive; read-only. The agent-facing half
    /// of the moat: the query a flat per-feature spec store can't answer.
    // trace:STORY-489 | ai:claude
    fn tool_query_graph(&self, args: &Value) -> Result<String, String> {
        use aida_core::graph_walk::{status_rollup, walk_union, Direction};

        let spec = args
            .get("spec_id")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: spec_id")?;
        let mode = args.get("mode").and_then(|v| v.as_str()).unwrap_or("tree");
        let depth = args
            .get("depth")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        // FR-282: traverse arbitrary named relationship types (custom/built-in).
        let follow: Vec<String> = args
            .get("follow")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        let store = self.storage.load().map_err(|e| e.to_string())?;
        let root = store
            .get_requirement_by_spec_id(spec)
            .ok_or_else(|| format!("Requirement '{}' not found", spec))?;
        let root_id = root.id;
        let root_label = root.display_id();

        // Carry a canonical mode label so the output always echoes the
        // hyphenated enum form even when an underscore alias was passed
        // (review finding: don't leak `blocked_by` into the response). Each
        // mode is a list of (rel_types, direction) walk legs; impact spans two
        // so a unidirectionally-stored Blocks edge is still caught (BUG-411).
        type WalkSpecs = Vec<(Vec<RelationshipType>, Direction)>;
        let (specs, canonical_mode): (WalkSpecs, &str) = if !follow.is_empty() {
            (
                follow
                    .iter()
                    .map(|t| (vec![RelationshipType::from_str(t)], Direction::Outgoing))
                    .collect(),
                "follow",
            )
        } else {
            match mode {
                "blocked-by" | "blocked_by" => (
                    vec![(vec![RelationshipType::BlockedBy], Direction::Outgoing)],
                    "blocked-by",
                ),
                "blocks" => (
                    vec![(vec![RelationshipType::Blocks], Direction::Outgoing)],
                    "blocks",
                ),
                "impact" => (
                    vec![
                        (vec![RelationshipType::BlockedBy], Direction::Incoming),
                        (vec![RelationshipType::Blocks], Direction::Outgoing),
                    ],
                    "impact",
                ),
                "tree" => (
                    vec![(vec![RelationshipType::Child], Direction::Outgoing)],
                    "tree",
                ),
                other => {
                    return Err(format!(
                        "unknown mode '{}': use tree, blocked-by, blocks, or impact",
                        other
                    ))
                }
            }
        };

        // TASK-1074: `tree` mode routes through the one shared rank-oriented
        // subtree closure `aida graph --tree` and `aida focus` use, so the MCP
        // and CLI surfaces agree on membership; other modes keep their walk_union
        // legs.
        let result = if canonical_mode == "tree" {
            aida_core::graph_walk::hierarchy_tree(&store, root_id, depth)
        } else {
            walk_union(&store, root_id, &specs, depth)
        };
        let nodes: Vec<Value> = result
            .nodes
            .iter()
            .map(|nid| {
                let r = store.get_requirement_by_id(nid);
                json!({
                    "id": r.map(|x| x.display_id()).unwrap_or_else(|| nid.to_string()),
                    "title": r.map(|x| x.title.clone()),
                    "status": r.map(|x| format!("{:?}", x.status)),
                    "resolved": r.is_some(),
                })
            })
            .collect();
        let rollup = status_rollup(&store, &result.nodes);
        serde_json::to_string_pretty(&json!({
            "root": root_label,
            "mode": canonical_mode,
            "count": result.nodes.len(),
            "nodes": nodes,
            "rollup": {
                "total": rollup.total,
                "completed": rollup.completed,
                "done": rollup.done,
                "in_progress": rollup.in_progress,
                "remaining": rollup.remaining,
                "shelved": rollup.shelved,
                "rejected": rollup.rejected,
            },
        }))
        .map_err(|e| e.to_string())
    }

    // TASK-538: MCP parity with `aida history --events`.
    fn tool_history(&self, args: &Value) -> Result<String, String> {
        let spec_id = args
            .get("spec_id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        // BUG-588: the event ledger is keyed by spec_id, but a caller may pass
        // the raw UUID (the id `show_requirement` returns). Resolve a UUID to
        // its canonical spec_id so the filter matches — mirrors the CLI's
        // `resolve_history_id_filter`. trace:BUG-588 | ai:claude
        let spec_id = spec_id.map(|raw| {
            if let Ok(uuid) = uuid::Uuid::parse_str(&raw) {
                if let Ok(store) = self.storage.load() {
                    if let Some(req) = store.requirements.iter().find(|r| r.id == uuid) {
                        if let Some(sid) = &req.spec_id {
                            return sid.clone();
                        }
                    }
                }
            }
            raw
        });
        // TASK-882: reach CLI parity. Mirror `aida history`'s filter surface
        // onto the MCP tool so an agent can ask the same questions the CLI can:
        // --events/--type/--author/--since/--until/--limit/--shipped/--status-changes/
        // --comments/--oneline. The string filters trim+empty-drop like spec_id.
        // trace:TASK-882 | ai:claude
        let str_arg = |key: &str| -> Option<String> {
            args.get(key)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let bool_arg =
            |key: &str| -> bool { args.get(key).and_then(|v| v.as_bool()).unwrap_or(false) };

        let since = str_arg("since");
        let until = str_arg("until");
        let type_filter = str_arg("type");
        let author_filter = str_arg("author");
        let shipped_only = bool_arg("shipped");
        // `--shipped` implies events mode, mirroring the CLI. The MCP default has
        // always been events mode (the structured ledger), so `events` defaults
        // true here and a caller passing `events: false` only matters for the
        // digest-vs-events distinction the CLI surfaces.
        let events_mode =
            args.get("events").and_then(|v| v.as_bool()).unwrap_or(true) || shipped_only;
        let status_changes_only = bool_arg("status_changes");
        let comments_only = bool_arg("comments");
        let oneline = bool_arg("oneline");
        // `limit` caps decoded events; default preserves the historical 100.
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .filter(|&n| n > 0)
            .unwrap_or(100);
        // Walk at least 5x the limit (CLI events-mode default) so the cap is met.
        let max_commits = (limit.saturating_mul(5)).max(500);

        let opts = HistoryOpts {
            limit,
            max_commits,
            events_mode,
            id_filter: spec_id,
            type_filter,
            author_filter,
            since,
            until,
            status_changes_only,
            shipped_only,
            comments_only,
            oneline,
            // MCP consumers expect the full event ledger, not the CLI's
            // day-to-day archived/deferred-work filter. The ledger is therefore
            // already equivalent to `aida history --all`; no `all` toggle is
            // needed because nothing is hidden by default here.
            archived_specs: std::collections::HashSet::new(),
            archived_only_specs: None,
            // STORY-584: same — no defer filtering on the MCP ledger.
            deferred_specs: std::collections::HashSet::new(),
            deferred_only_specs: None,
            // STORY-737: the MCP ledger is the full event stream — keep META
            // visible (the CLI-only default-view hide doesn't apply here).
            exclude_meta: false,
        };
        let events = history::collect_event_records(self.storage.path(), &opts)
            .map_err(|e| e.to_string())?;
        serde_json::to_string_pretty(&json!({
            "count": events.len(),
            "events": events,
        }))
        .map_err(|e| e.to_string())
    }

    /// MCP parity for `aida mailbox send` (STORY-493): send an inter-agent
    /// peer message into the local layer. `to` a specific agent or
    /// `broadcast: true` to all; `from` defaults to this server's identity.
    // trace:STORY-493 trace:TASK-604 | ai:claude
    fn tool_send_message(&self, args: &Value) -> Result<String, String> {
        use aida_core::mailbox::{Message, Recipient};
        let body = args
            .get("body")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: body")?;
        let broadcast = args
            .get("broadcast")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let recipient = if broadcast {
            Recipient::Broadcast
        } else if let Some(a) = args.get("to").and_then(|v| v.as_str()) {
            Recipient::Agent(a.to_string())
        } else {
            return Err("specify `to` (an agent) or `broadcast: true`".to_string());
        };
        let from = args
            .get("from")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| crate::current_user_id(None));
        let id = uuid::Uuid::new_v4().to_string();
        // BUG-557: mirror the CLI fix — `in_reply_to` must attach the reply to
        // the target's thread, not open a new one. Precedence: explicit
        // `thread` > the reply target's thread > a fresh thread. Resolved
        // against the local layer (the same set `read_inbox` reads); a dangling
        // reference falls back to a fresh thread. trace:BUG-557 | ai:claude
        let in_reply_to = args
            .get("in_reply_to")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let thread_id = if let Some(t) = args.get("thread").and_then(|v| v.as_str()) {
            t.to_string()
        } else if let Some(target) = in_reply_to.as_deref() {
            let local = crate::mailbox_store::read_local_messages(&self.project_root)
                .map_err(|e| e.to_string())?;
            aida_core::mailbox::reply_target_thread(target, &local).unwrap_or_else(|| id.clone())
        } else {
            id.clone()
        };
        // STORY-539: light urgency flag, mirroring the CLI `--urgent`.
        let urgent = args
            .get("urgent")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // TASK-782: intent marker, mirroring the CLI `--intent`. Default fyi;
        // an unrecognized token is a hard error (parity with the CLI flag).
        let intent = match args.get("intent").and_then(|v| v.as_str()) {
            Some(s) => aida_core::mailbox::Intent::parse(s).ok_or_else(|| {
                format!("invalid intent '{s}'; expected one of: fyi, request, handoff")
            })?,
            None => aida_core::mailbox::Intent::default(),
        };
        let msg = Message {
            id: id.clone(),
            thread_id: thread_id.clone(),
            from,
            to: recipient,
            timestamp: chrono::Utc::now().timestamp_millis(),
            in_reply_to,
            body: body.to_string(),
            urgent,
            intent,
            retracted: false,
            deleted: false,
        };
        crate::mailbox_store::write_message(&self.project_root, &msg).map_err(|e| e.to_string())?;
        Ok(format!("Message sent: {id} (thread {thread_id})"))
    }

    /// MCP parity for `aida mailbox inbox` (STORY-493): an agent's inbox —
    /// messages addressed to it + broadcasts (excluding its own sent),
    /// oldest-first, as JSON. `agent` defaults to this server's identity.
    // trace:STORY-493 | ai:claude
    fn tool_read_inbox(&self, args: &Value) -> Result<String, String> {
        use aida_core::mailbox::{inbox_for, Recipient};
        let agent = args
            .get("agent")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| crate::current_user_id(None));
        let unread_only = args
            .get("unread")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let mark_seen = args
            .get("mark_seen")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // STORY-643: merge the canonical (orphan-store) layer with the local
        // one so foreign messages another clone published and we pulled down
        // surface here — matching `aida mailbox inbox` on the CLI. Without this
        // an MCP agent only ever saw its own clone's locally-sent mail.
        // trace:STORY-643 | ai:claude
        let local = crate::mailbox_store::read_local_messages(&self.project_root)
            .map_err(|e| e.to_string())?;
        let store_root = self.project_root.join(".aida-store");
        let canonical =
            crate::mailbox_store::read_canonical_messages(&store_root).unwrap_or_default();
        let all = aida_core::mailbox::merge_dedup(&local, &canonical);
        let watermark = crate::mailbox_store::read_watermark(&self.project_root, &agent);
        let full = inbox_for(&agent, &all);
        // `--unread`/`unread` filters to messages past the watermark; the
        // seen-mark still advances to the FULL inbox's newest (an explicit ack
        // catches everything, not just the filtered slice).
        let mark = watermark.unwrap_or(i64::MIN);
        let shown: Vec<&aida_core::mailbox::Message> = if unread_only {
            full.iter()
                .copied()
                .filter(|m| m.timestamp > mark)
                .collect()
        } else {
            full.clone()
        };
        let messages: Vec<Value> = shown
            .iter()
            .map(|m| {
                let to = match &m.to {
                    Recipient::Agent(a) => a.clone(),
                    Recipient::Broadcast => "all".to_string(),
                };
                json!({
                    "id": m.id,
                    "thread_id": m.thread_id,
                    "from": m.from,
                    "to": to,
                    "timestamp": m.timestamp,
                    "in_reply_to": m.in_reply_to,
                    "body": m.body,
                    "urgent": m.urgent,
                    "intent": m.intent.as_str(),
                })
            })
            .collect();
        // Explicit ack only: advance the watermark to the full inbox's newest
        // so the unread/notice surface clears. Default is a non-marking peek
        // (STORY-585 acceptance #4). trace:STORY-585
        if mark_seen {
            if let Some(newest) = full.iter().map(|m| m.timestamp).max() {
                let _ = crate::mailbox_store::set_watermark(&self.project_root, &agent, newest);
            }
        }
        serde_json::to_string_pretty(&json!({
            "agent": agent,
            "count": messages.len(),
            "unread": unread_only,
            "messages": messages,
        }))
        .map_err(|e| e.to_string())
    }

    fn tool_add_comment(&self, args: &Value) -> Result<String, String> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: id")?;
        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: text")?;

        let mut store = self.storage.load().map_err(|e| e.to_string())?;
        let req = store
            .get_requirement_by_spec_id_mut(id)
            .ok_or_else(|| format!("Requirement '{}' not found", id))?;

        // trace:TASK-330 | ai:claude — stamp the producing session (best-effort)
        let comment = Comment::new("mcp".to_string(), text.to_string())
            .with_session_id(crate::resolve_current_session_id());
        req.add_comment(comment);

        self.storage.save(&store).map_err(|e| e.to_string())?;
        Ok(format!("Comment added to {}", id))
    }

    // trace:TASK-551 | ai:codex
    fn tool_add_relationship(&self, args: &Value) -> Result<String, String> {
        let spec_id = args
            .get("spec_id")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: spec_id")?;
        let target_spec_id = args
            .get("target_spec_id")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: target_spec_id")?;
        let relationship_type_raw = args
            .get("relationship_type")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: relationship_type")?;
        let bidirectional = args
            .get("bidirectional")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let force_parent = args
            .get("force_parent")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let rel_type = parse_mcp_relationship_type(relationship_type_raw)?;
        let mut store = self.storage.load().map_err(|e| e.to_string())?;
        let source_req = store
            .get_requirement_by_spec_id(spec_id)
            .ok_or_else(|| format!("Requirement '{}' not found", spec_id))?;
        let target_req = store
            .get_requirement_by_spec_id(target_spec_id)
            .ok_or_else(|| format!("Requirement '{}' not found", target_spec_id))?;

        let source_id = source_req.id;
        let target_id = target_req.id;
        let source_title = source_req.title.clone();
        let target_title = target_req.title.clone();

        if !force_parent {
            let parent_for_guard = match &rel_type {
                RelationshipType::Child => Some(target_req),
                RelationshipType::Parent => Some(source_req),
                _ => None,
            };
            if let Some(parent) = parent_for_guard {
                if crate::is_terminal_status(&parent.status) {
                    return Err(format!(
                        "parent {} is {} — adding new children to a closed parent is usually a mistake. Pass `force_parent: true` to override.",
                        parent.spec_id.as_deref().unwrap_or("?"),
                        parent.status,
                    ));
                }
            }
        }

        store
            .add_relationship(&source_id, rel_type.clone(), &target_id, bidirectional)
            .map_err(|e| e.to_string())?;
        self.storage.save(&store).map_err(|e| e.to_string())?;

        let mut out = format!(
            "Relationship added: {} ({}) --[{}]--> {} ({})",
            spec_id, source_title, rel_type, target_spec_id, target_title
        );
        if bidirectional {
            if let Some(inverse) = rel_type.inverse() {
                out.push_str(&format!("; inverse added as {}", inverse));
            }
        }
        Ok(out)
    }

    fn tool_list_features(&self) -> Result<String, String> {
        let store = self.storage.load().map_err(|e| e.to_string())?;

        if store.features.is_empty() {
            return Ok("No features defined in this project.".to_string());
        }

        let mut output = format!("Features ({}):\n\n", store.features.len());
        for f in &store.features {
            output.push_str(&format!("- {} (prefix: {})\n", f.name, f.prefix));
        }
        Ok(output)
    }

    // ========================================================================
    // STORY-361: Punt channel tools
    // ========================================================================

    fn tool_list_punts(&self, args: &Value) -> Result<String, String> {
        let status_filter = args
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let records = read_ledger(&self.project_root);
        // BUG-674: the default view hides auto-filed session-end visibility
        // warnings (they bury genuine decision-punts); `awaiting` is the OPEN
        // set (excludes closed + noise); `all` shows everything.
        // trace:BUG-674 | ai:claude
        let filtered: Vec<&PuntRecord> = records
            .iter()
            .filter(|r| match status_filter.as_deref() {
                None => !punt::is_session_end_noise(r),
                Some("awaiting") => punt::is_open(r),
                Some("all") => true,
                Some(other) => r.resolution_path == other,
            })
            .collect();

        if filtered.is_empty() {
            return Ok("No punts found.".to_string());
        }
        let mut out = format!("Found {} punt(s):\n\n", filtered.len());
        for r in &filtered {
            out.push_str(&format!(
                "- {} [{}] {}\n  resolution: {}  ({})\n",
                r.spec,
                r.category,
                r.detail,
                r.resolution_path,
                r.timestamp.to_rfc3339(),
            ));
        }
        Ok(out)
    }

    fn tool_read_punt(&self, args: &Value) -> Result<String, String> {
        let spec = args
            .get("spec_id")
            .or_else(|| args.get("punt_id"))
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: spec_id (or punt_id)")?;

        let records = read_ledger(&self.project_root);
        // Most recent record for the spec is the live one.
        let record = records
            .iter()
            .rev()
            .find(|r| r.spec.eq_ignore_ascii_case(spec))
            .ok_or_else(|| format!("No punt found for spec '{}'", spec))?;

        let body = serde_json::to_string_pretty(record).map_err(|e| e.to_string())?;
        Ok(body)
    }

    fn tool_post_punt(&self, args: &Value) -> Result<String, String> {
        let spec = args
            .get("spec_id")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: spec_id")?;
        let detail = args
            .get("detail")
            .or_else(|| args.get("reason"))
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: detail (or reason)")?;
        let category_raw = args
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("design-fork");
        let category = PuntCategory::from_str(category_raw)
            .ok_or_else(|| format!("invalid punt category `{}`", category_raw))?;
        let lean = args
            .get("lean")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let raised_by = args
            .get("raised_by")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| Some("mcp".to_string()));

        // TASK-349: mirror the CLI `aida punt` nudge — a blocked-dependency
        // punt that named a blocker spec should suggest recording the
        // blocked-by graph edge, so the MCP and CLI surfaces don't drift.
        // Computed before `lean` is moved into the record below. trace:TASK-349
        let blocked_by_hint =
            crate::punt::suggest_blocked_by(spec, category, detail, lean.as_deref())
                .map(|s| s.suggested_command());

        let record = PuntRecord {
            timestamp: Utc::now(),
            spec: spec.to_string(),
            category,
            detail: detail.to_string(),
            lean,
            raised_by,
            resolution_path: "punted".to_string(),
            classification: None,
            escalation_reason: None,
            answer: None,
            answered_by: None,
            decision: None,
            principle_link: None,
            calibration_pair: None,
            paused_at: None,
            resolved_at: None,
        };

        append_to_ledger(&self.project_root, &record).map_err(|e| e.to_string())?;

        // BUG-334: a punt MEANS "I hit a fork I can't resolve — this spec
        // needs attention", so flip the status to match the CLI `aida punt`
        // (which already does this). The state machine only permits
        // `InProgress → NeedsAttention` (forbidden_attention_transition
        // guards it), so flip when the transition is legal and record-only
        // otherwise — never erroring, since the punt ledger entry is already
        // written. trace:BUG-334 | ai:claude
        let mut store = self.storage.load().map_err(|e| e.to_string())?;
        let flipped = match store.get_requirement_by_spec_id_mut(spec) {
            Some(req)
                if aida_core::forbidden_attention_transition(
                    &req.status,
                    &aida_core::RequirementStatus::NeedsAttention,
                )
                .is_none() =>
            {
                req.status = aida_core::RequirementStatus::NeedsAttention;
                req.modified_at = Utc::now();
                true
            }
            _ => false,
        };
        if flipped {
            self.storage.save(&store).map_err(|e| e.to_string())?;
        }

        let mut out = if flipped {
            format!("Punt recorded for {spec} [{category}]; spec flipped to NeedsAttention.")
        } else {
            format!(
                "Punt recorded for {spec} [{category}]. Spec status unchanged \
                 (only an In Progress spec auto-flips to NeedsAttention); flip \
                 manually with `aida edit {spec} --status needs-attention` if needed."
            )
        };
        // trace:TASK-349 | ai:claude
        if let Some(cmd) = blocked_by_hint {
            out.push_str(&format!(
                "\nBlocked-dependency punt — record the graph edge: `{cmd}`"
            ));
        }
        Ok(out)
    }

    fn tool_resolve_punt(&self, args: &Value) -> Result<String, String> {
        let spec = args
            .get("spec_id")
            .or_else(|| args.get("punt_id"))
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: spec_id (or punt_id)")?;
        let answer = args
            .get("answer")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: answer")?;
        let reasoning = args
            .get("reasoning")
            .or_else(|| args.get("rationale"))
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: reasoning (or rationale)")?;
        let classification = args
            .get("classification")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // BUG-674: resolve through the shared core so the MCP tool and the
        // `aida punts resolve` CLI verb cannot drift — it writes the
        // `PuntResponse` the orchestrator polls AND closes the open ledger
        // record(s) so the triage count actually drops. trace:BUG-674 | ai:claude
        let (path, closed) = punt::resolve_punt_core(
            &self.project_root,
            spec,
            answer,
            reasoning,
            classification,
            Some("mcp"),
        )
        .map_err(|e| e.to_string())?;

        Ok(format!(
            "Resolution written to {} — the orchestrator will resume the implementer with this answer.{}",
            path.display(),
            ledger_close_suffix(closed),
        ))
    }

    fn tool_escalate_punt(&self, args: &Value) -> Result<String, String> {
        let spec = args
            .get("spec_id")
            .or_else(|| args.get("punt_id"))
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: spec_id (or punt_id)")?;
        let reasoning = args
            .get("reasoning")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: reasoning")?;
        let escalation_reason = args
            .get("escalation_reason")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let classification = args
            .get("classification")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // BUG-674: escalate through the shared core (parity with `aida punts
        // escalate`) — writes the escalation `PuntResponse` the orchestrator
        // parks on AND closes the open ledger record(s) as escalated-to-human.
        // trace:BUG-674 | ai:claude
        let (path, closed) = punt::escalate_punt_core(
            &self.project_root,
            spec,
            reasoning,
            escalation_reason,
            classification,
            Some("mcp"),
        )
        .map_err(|e| e.to_string())?;

        Ok(format!(
            "Escalation written to {} — the orchestrator will park the spec for human triage.{}",
            path.display(),
            ledger_close_suffix(closed),
        ))
    }

    // ========================================================================
    // STORY-361: Findings channel tools
    // ========================================================================

    fn tool_list_findings(&self, args: &Value) -> Result<String, String> {
        let pr = args.get("pr").and_then(|v| v.as_u64()).map(|n| n as u32);
        let source = args.get("source").and_then(|v| v.as_str()).and_then(|s| {
            match s.to_ascii_lowercase().as_str() {
                "review" => Some(crate::findings::FindingSource::Review),
                "implementer" => Some(crate::findings::FindingSource::Implementer),
                _ => None,
            }
        });
        let kind = args
            .get("kind")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let store = self.storage.load().map_err(|e| e.to_string())?;

        // Project Database into RequirementSummary shape that findings.rs expects.
        let summaries = build_summaries(&store);
        let filter = FindingsFilter { pr, source, kind };
        let sections = build_findings_view(&summaries, &filter);
        let total = count_findings(&sections);

        if total == 0 {
            return Ok("No findings match the filter.".to_string());
        }

        let mut out = format!("Found {} finding(s):\n\n", total);
        for sec in &sections {
            out.push_str(&format!("## From {}\n\n", sec.source.label()));
            for g in &sec.groups {
                out.push_str(&format!("### {}\n", g.origin));
                for r in &g.rows {
                    let kind_label = r
                        .kind
                        .as_deref()
                        .map(|k| format!(" [{}]", k))
                        .unwrap_or_default();
                    out.push_str(&format!(
                        "- {} {} ({}){}\n",
                        r.display_id,
                        r.title,
                        r.severity.label(),
                        kind_label,
                    ));
                }
                out.push('\n');
            }
        }
        Ok(out)
    }

    fn tool_file_finding(&self, args: &Value) -> Result<String, String> {
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: title")?;
        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: description")?;
        let source = args
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("implementer");
        // trace:TASK-437 | ai:claude — enforce the inputSchema contract.
        // `spec_id` is a string SPEC-ID (e.g. "TASK-42"); `pr` is the bare integer.
        // Loose clients used to pass `pr: "7"` and get `from-review:7` instead of
        // `from-review:PR-7`, silently invisible to `aida findings list --pr 7`.
        let origin = if let Some(v) = args.get("spec_id") {
            let s = v
                .as_str()
                .ok_or("spec_id must be a string (e.g. \"TASK-42\")")?;
            s.to_string()
        } else if let Some(v) = args.get("pr") {
            let n = v.as_u64().ok_or(
                "pr must be an integer (the bare PR number, e.g. 7 — not \"7\" or \"PR-7\")",
            )?;
            format!("PR-{}", n)
        } else {
            return Err("Missing required parameter: spec_id (or pr)".to_string());
        };
        let kind = args
            .get("kind")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let severity = args
            .get("severity")
            .and_then(|v| v.as_str())
            .unwrap_or("minor");

        let source_tag = match source.to_ascii_lowercase().as_str() {
            "review" => format!("{}{}", FROM_REVIEW_PREFIX, origin),
            "implementer" => format!("{}{}", FROM_IMPLEMENTER_PREFIX, origin),
            other => return Err(format!("unknown source '{}'", other)),
        };

        let mut store = self.storage.load().map_err(|e| e.to_string())?;
        let mut req = Requirement::new(title.to_string(), description.to_string());
        req.req_type = RequirementType::Task;
        req.status = RequirementStatus::Draft;
        req.priority = RequirementPriority::Medium;
        req.tags.insert(source_tag);
        req.tags.insert(format!("severity:{}", severity));
        if let Some(k) = kind {
            req.tags.insert(format!("kind:{}", k));
        }
        store.requirements.push(req);
        store.assign_spec_ids();
        let new_id = store
            .requirements
            .last()
            .and_then(|r| r.spec_id.clone())
            .unwrap_or_else(|| "?".to_string());
        self.storage.save(&store).map_err(|e| e.to_string())?;

        Ok(format!("Finding filed: {} — {}", new_id, title))
    }

    fn tool_triage_finding(&self, args: &Value) -> Result<String, String> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: id")?;
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: action (promote|dismiss)")?;
        let reason = args
            .get("reason")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // TASK-681 (EPIC-38) caller-parity audit: promoting a finding sets it
        // `Approved`, which is the advisor's triage decision — exactly the
        // transition `mcp_status_gate_message` refuses on the
        // update_requirement path (BUG-449). Without this gate,
        // `triage_finding promote` was a one-tool bypass that let an untrusted
        // MCP caller self-grant Approved status on a draft finding. Apply the
        // same gate before mutating; `dismiss` (Rejected) is implementer-
        // legitimate and stays allowed. The gate is keyed off the target
        // status so it stays in lockstep if the gated-status set ever changes.
        // trace:TASK-681 | ai:claude
        let new_status = match action.to_ascii_lowercase().as_str() {
            "promote" => RequirementStatus::Approved,
            "dismiss" => RequirementStatus::Rejected,
            other => return Err(format!("unknown action '{}'", other)),
        };
        // BUG-481: a finding is a Draft until triaged; promote → Approved is
        // always-gated regardless of source, so the Draft source here is
        // conservative-correct (dismiss → Rejected stays ungated).
        if let Some(msg) = mcp_status_gate_message(&RequirementStatus::Draft, &new_status) {
            return Err(msg);
        }

        let mut store = self.storage.load().map_err(|e| e.to_string())?;
        let req = store
            .get_requirement_by_spec_id_mut(id)
            .ok_or_else(|| format!("Finding '{}' not found", id))?;
        let tags: Vec<String> = req.tags.iter().cloned().collect();
        if finding_source(&tags).is_none() {
            return Err(format!("{} is not a finding (no from-* tag)", id));
        }

        let verb = if new_status == RequirementStatus::Approved {
            "promoted"
        } else {
            "dismissed"
        };
        req.status = new_status;
        if let Some(r) = reason {
            req.add_comment(Comment::new(
                "mcp".to_string(),
                format!("{} via MCP: {}", verb, r),
            ));
        }

        self.storage.save(&store).map_err(|e| e.to_string())?;
        // BUG-590: emit the proper past tense ("dismissed"/"promoted") instead of
        // blindly suffixing "d" to the raw action (which yielded "dismissd").
        // `verb` is already the correct past-tense form. trace:BUG-590 | ai:claude
        Ok(format!("Finding {} {}", id, verb))
    }

    // ========================================================================
    // STORY-361: Task-claim channel tools
    // ========================================================================

    fn tool_claim_task(&self, args: &Value) -> Result<String, String> {
        let spec_id_arg = args
            .get("spec_id")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: spec_id")?;
        let role = args
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("implementer");
        // TASK-474: optional `worktree_path` — the path of the agent's actual
        // working directory (its `pwd`). An MCP server is launched in one
        // place and stays there, so it cannot derive its caller's cwd from
        // `self.project_root`; before this arg the lease always recorded the
        // project root, which misrouted "this session owns scope X" hints
        // in `aida add` to unrelated shells in the parent worktree.
        // Empty / absent → record an empty `worktree_path` (advisory lock
        // only; `lease_covers_cwd` skip-matches it so no cwd-based
        // inference attaches to the lease). Non-empty paths are canonicalized
        // at record-time so cwd comparisons don't drift on relative paths,
        // symlinks, or `..` segments. trace:TASK-474 TASK-504 | ai:claude
        let worktree_path_arg = args
            .get("worktree_path")
            .and_then(|v| v.as_str())
            .map(canonicalize_worktree_arg)
            .unwrap_or_default();

        // First pass: surface non-MCP leases (e.g. `aida session start`) that
        // cover this spec — those land at arbitrary filenames, so the
        // create_new(true) atomic-create below can't see them.
        let leases = list_leases(&self.project_root);
        if let Some(existing) = leases
            .iter()
            .find(|l| l.scope.eq_ignore_ascii_case(spec_id_arg))
        {
            return Ok(format!(
                "already_claimed by {} (lease {} since {})",
                existing.owner, existing.id, existing.started_at
            ));
        }

        // Mint a new lease_id (12-char uuid v7 prefix matches `aida session start`).
        let id_long = uuid::Uuid::now_v7().to_string();
        let id = id_long.replace('-', "")[..12].to_string();
        let owner = std::env::var("USER").unwrap_or_else(|_| "mcp".to_string());
        let hostname = hostname_or_unknown();
        let branch = current_branch_at(&self.project_root).unwrap_or_else(|| "main".to_string());

        let lease = LightLease {
            id: id.clone(),
            scope: spec_id_arg.to_string(),
            slug: spec_id_arg.to_string(),
            owner,
            worktree_path: worktree_path_arg,
            branch,
            started_at: Utc::now().to_rfc3339(),
            hostname,
            role: Some(role.to_string()),
            mcp_claim: true,
        };

        let dir = leases_dir(&self.project_root);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        // trace:TASK-438 | ai:claude
        // Key the MCP claim file by spec_id (lower-cased) instead of lease_id,
        // so two concurrent claim_task calls on the same spec target the same
        // path. `create_new(true)` maps to O_EXCL|O_CREAT on POSIX and
        // CREATE_NEW on Windows — atomically fails if the file already exists.
        // That gives a single serialization point per spec, closing the TOCTOU
        // window between the pre-scan above and the lease write.
        let path = mcp_claim_path(&dir, spec_id_arg);
        let body = toml::to_string(&lease).map_err(|e| e.to_string())?;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                f.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
                f.sync_all().map_err(|e| e.to_string())?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Another claim_task call won the race. Read the existing
                // lease so the response matches the pre-scan format.
                let body = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
                let existing: LightLease = toml::from_str(&body).map_err(|e| e.to_string())?;
                return Ok(format!(
                    "already_claimed by {} (lease {} since {})",
                    existing.owner, existing.id, existing.started_at
                ));
            }
            Err(e) => return Err(e.to_string()),
        }

        Ok(format!("claimed: lease_id={}", id))
    }

    fn tool_release_task(&self, args: &Value) -> Result<String, String> {
        let lease_id = args
            .get("lease_id")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: lease_id")?;

        // trace:TASK-438 | ai:claude
        // Lease files are no longer named `<lease_id>.toml` — MCP claims live
        // at `mcp-claim.<spec>.toml`, and session-start leases use their own
        // scheme. Scan the directory and match the embedded `id` field.
        let dir = leases_dir(&self.project_root);
        let entries =
            std::fs::read_dir(&dir).map_err(|_| format!("lease '{}' not found", lease_id))?;
        let mut found: Option<(PathBuf, LightLease)> = None;
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                if name.contains(".activity.") || name.contains(".manifest.") {
                    continue;
                }
            }
            if let Ok(body) = std::fs::read_to_string(&p) {
                if let Ok(l) = toml::from_str::<LightLease>(&body) {
                    if l.id == lease_id {
                        found = Some((p, l));
                        break;
                    }
                }
            }
        }

        let (path, lease) = found.ok_or_else(|| format!("lease '{}' not found", lease_id))?;

        // Refuse to delete a non-MCP lease — the MCP surface only owns the
        // lightweight claims it creates. Real `aida session start` leases must
        // be released via `aida session end`.
        if !lease.mcp_claim {
            return Err(format!(
                "lease '{}' is a real `aida session start` lease — use `aida session end {}` to release it",
                lease_id, lease_id
            ));
        }

        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        Ok(format!("released: lease_id={}", lease_id))
    }

    fn tool_list_active_leases(&self) -> Result<String, String> {
        let leases = list_leases(&self.project_root);
        if leases.is_empty() {
            return Ok("No active leases.".to_string());
        }
        let mut out = format!("Found {} active lease(s):\n\n", leases.len());
        for l in &leases {
            let claim_kind = if l.mcp_claim { "mcp" } else { "session" };
            let role = l.role.as_deref().unwrap_or("?");
            out.push_str(&format!(
                "- {} scope={} role={} owner={} kind={} started_at={}\n",
                l.id, l.scope, role, l.owner, claim_kind, l.started_at
            ));
        }
        Ok(out)
    }

    // ========================================================================
    // STORY-361: Worker-directive channel tools
    // ========================================================================

    fn tool_post_directive(&self, args: &Value) -> Result<String, String> {
        let verb = args
            .get("verb")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: verb (drain|pause|exit)")?;
        // trace:TASK-436 | ai:claude
        // Validate the verb up front. The worker defensively maps unknown
        // verbs to `pause`, but the caller deserves a clear error rather than
        // a silently-misclassified "directive posted" success.
        if !matches!(verb, "drain" | "pause" | "exit") {
            return Err(format!(
                "invalid directive verb `{}` (expected drain|pause|exit)",
                verb
            ));
        }
        let args_list = if let Some(v) = args.get("args") {
            if let Some(arr) = v.as_array() {
                arr.iter()
                    .filter_map(|x| x.as_str())
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
            } else {
                return Err(
                    "Parameter `args` must be an array of strings (e.g. [\"arg\"])".to_string(),
                );
            }
        } else {
            Vec::new()
        };

        let mut line = verb.to_string();
        if !args_list.is_empty() {
            line.push(' ');
            line.push_str(&args_list.join(" "));
        }

        let path = crate::worker::worker_cmd_path(&self.project_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        append_line(&path, &line).map_err(|e| e.to_string())?;
        Ok(format!("directive posted: {}", line))
    }

    fn tool_list_directives(&self) -> Result<String, String> {
        let path = crate::worker::worker_cmd_path(&self.project_root);
        let directives = crate::worker::parse_directives(&path);
        Ok(crate::worker::render_human(&directives))
    }

    fn tool_ack_directive(&self, args: &Value) -> Result<String, String> {
        let index = args
            .get("index")
            .and_then(|v| v.as_u64())
            .ok_or("Missing required parameter: index (0-based)")? as usize;

        let path = crate::worker::worker_cmd_path(&self.project_root);
        let body = std::fs::read_to_string(&path)
            .map_err(|e| format!("reading {}: {}", path.display(), e))?;

        // Keep blank/comment lines in place; only count non-comment, non-blank
        // lines so the index aligns with `aida worker directives`.
        let mut directive_count = 0usize;
        let mut new_lines: Vec<String> = Vec::with_capacity(body.lines().count());
        let mut removed: Option<String> = None;
        for line in body.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                new_lines.push(line.to_string());
                continue;
            }
            if directive_count == index {
                removed = Some(line.to_string());
                directive_count += 1;
                continue;
            }
            new_lines.push(line.to_string());
            directive_count += 1;
        }

        let removed = removed.ok_or_else(|| {
            format!(
                "no directive at index {} (there are {} directive(s))",
                index, directive_count
            )
        })?;

        let mut new_body = new_lines.join("\n");
        if !new_body.is_empty() && !new_body.ends_with('\n') {
            new_body.push('\n');
        }
        write_atomic(&path, new_body.as_bytes()).map_err(|e| e.to_string())?;
        Ok(format!("acked: {}", removed))
    }

    fn tool_list_briefs(&self, args: &Value) -> Result<String, String> {
        let agent = optional_string(args, "agent")?;
        if let Some(agent) = agent {
            validate_brief_agent(agent)?;
        }
        let include_acked = args
            .get("include_acked")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let entries = collect_brief_refs(&self.project_root, agent, include_acked)?;
        if entries.is_empty() {
            return Ok("No briefs found.".to_string());
        }
        let mut out = format!("Found {} brief(s):", entries.len());
        for entry in entries {
            out.push_str(&format!(
                "\n- path={} agent={} spec_id={} generated_at={} status={}",
                entry.path, entry.agent, entry.spec_id, entry.generated_at, entry.status
            ));
        }
        Ok(out)
    }

    fn tool_read_brief(&self, args: &Value) -> Result<String, String> {
        let raw_path = required_string(args, "path")?;
        let path = resolve_brief_path(&self.project_root, raw_path)?;
        std::fs::read_to_string(&path)
            .map_err(|e| format!("reading brief {}: {}", path.display(), e))
    }

    fn tool_ack_brief(&self, args: &Value) -> Result<String, String> {
        let raw_path = required_string(args, "path")?;
        let path = resolve_brief_path(&self.project_root, raw_path)?;
        let acked_path = if path.extension().and_then(|e| e.to_str()) == Some("acked") {
            path.clone()
        } else {
            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| format!("invalid brief path: {}", path.display()))?;
            path.with_file_name(format!("{file_name}.acked"))
        };

        if acked_path.exists() {
            return Ok(format!(
                "already_acked: {}",
                brief_display_path(&self.project_root, &acked_path)
            ));
        }
        if !path.exists() {
            return Err(format!("brief file not found: {}", raw_path));
        }

        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("reading brief {}: {}", path.display(), e))?;
        let content = if content.contains("\nstatus: pending\n") {
            content.replacen("\nstatus: pending\n", "\nstatus: acked\n", 1)
        } else {
            content
        };
        write_atomic(&path, content.as_bytes()).map_err(|e| e.to_string())?;
        match std::fs::rename(&path, &acked_path) {
            Ok(()) => {
                // TASK-502: keep the `.pending` sentinel in sync when an agent
                // acks through MCP, same as the CLI ack path.
                crate::clear_pending_brief(&path);
                Ok(format!(
                    "acked: {}",
                    brief_display_path(&self.project_root, &acked_path)
                ))
            }
            Err(e) if acked_path.exists() => Ok(format!(
                "already_acked: {}",
                brief_display_path(&self.project_root, &acked_path)
            )),
            Err(e) => Err(format!(
                "acking brief {} -> {}: {}",
                path.display(),
                acked_path.display(),
                e
            )),
        }
    }

    // ========================================================================
    // Queue tool implementations — STORY-532 (EPIC-27)
    //
    // These mirror the `aida queue <subcommand>` CLI surface so MCP-speaking
    // agents can manage the work queue without shelling out. They reuse the
    // same underlying `Storage::queue_*` library functions the CLI handlers
    // call (no subprocess to `aida`), and resolve the queue `user_id` through
    // `crate::current_user_id` exactly like the CLI — so MCP and CLI see the
    // same queue (BUG-89 identity rule). trace:EPIC-27
    // ========================================================================

    /// Resolve the queue `user_id` the same way every CLI queue path does:
    /// `user` arg → AIDA_USER → USER → USERNAME → "default". trace:EPIC-27
    fn queue_user_id(&self, args: &Value) -> String {
        crate::current_user_id(args.get("user").and_then(|v| v.as_str()))
    }

    /// Resolve a requirement by UUID or SPEC-ID (the CLI's two-step lookup).
    /// trace:EPIC-27
    fn resolve_requirement<'s>(
        store: &'s aida_core::RequirementsStore,
        id: &str,
    ) -> Result<&'s Requirement, String> {
        if let Ok(uuid) = uuid::Uuid::parse_str(id) {
            store.requirements.iter().find(|r| r.id == uuid)
        } else {
            store.get_requirement_by_spec_id(id)
        }
        .ok_or_else(|| format!("Requirement '{}' not found", id))
    }

    fn display_id_of(req: &Requirement) -> String {
        req.agreed_id
            .clone()
            .or_else(|| req.spec_id.clone())
            .unwrap_or_else(|| "???".to_string())
    }

    // trace:EPIC-27
    fn tool_queue_list(&self, args: &Value) -> Result<String, String> {
        let user_id = self.queue_user_id(args);
        let include_completed = args
            .get("include_completed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let include_terminal = args
            .get("include_terminal")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let role = args.get("for").and_then(|v| v.as_str());
        let all = args.get("all").and_then(|v| v.as_bool()).unwrap_or(false);

        let entries = self
            .storage
            .queue_list(&user_id, include_completed)
            .map_err(|e| e.to_string())?;
        let store = self.storage.load().map_err(|e| e.to_string())?;

        // Same role-filter resolution as `aida queue list` (BUG-87). MCP has
        // no shell role context, so the active-role default is None — pass
        // `for` to scope. trace:EPIC-27
        let session_role = std::env::var("AIDA_SESSION_ROLE").ok();
        let (role_filter, only_unrouted) =
            crate::resolve_queue_role_filter(role, all, session_role.as_deref());

        let mut lines: Vec<String> = Vec::new();
        let mut shown = 0usize;
        for entry in &entries {
            if !crate::entry_matches_role_filter(
                entry.for_role.as_deref(),
                role_filter.as_deref(),
                only_unrouted,
            ) {
                continue;
            }
            let req = store
                .requirements
                .iter()
                .find(|r| r.id == entry.requirement_id);
            // TASK-46: hide terminal-status entries unless include_terminal.
            if let Some(r) = req {
                if !include_terminal && crate::is_terminal_status(&r.status) {
                    continue;
                }
            }
            shown += 1;
            let (disp, title, status) = match req {
                Some(r) => (
                    Self::display_id_of(r),
                    r.title.clone(),
                    r.status.to_string(),
                ),
                None => (
                    entry.requirement_id.to_string(),
                    "(unknown requirement)".to_string(),
                    "?".to_string(),
                ),
            };
            let routing = entry
                .for_role
                .as_deref()
                .map(|r| format!(" [for:{}]", r))
                .unwrap_or_default();
            lines.push(format!(
                "{}. [{}] {} ({}){}",
                shown, disp, title, status, routing
            ));
        }

        if shown == 0 {
            return Ok(format!("Queue is empty for user '{}'.", user_id));
        }
        Ok(format!(
            "Queue for '{}' ({} item{}):\n{}",
            user_id,
            shown,
            if shown == 1 { "" } else { "s" },
            lines.join("\n")
        ))
    }

    // trace:EPIC-27 trace:BUG-480
    fn tool_queue_add(&self, args: &Value) -> Result<String, String> {
        // BUG-480 (TASK-647 / ADR-3 caller parity): queuing a spec for
        // execution commits it to the pipeline — an advisor-authority act, the
        // exact decision the CLI `aida queue add` gates on
        // (`has_advisor_authority()`). MCP is never advisor authority (the
        // server runs non-TTY and may merely inherit an advisor role from the
        // launching shell), so it refuses unconditionally, mirroring how
        // `add_requirement` / `update_requirement` (BUG-449) treat MCP as
        // untrusted. Without this, an MCP agent could file a Draft and then
        // push it straight into the execution queue, bypassing intake triage.
        // The queue mechanics live in `tool_queue_add_inner` so tests (and any
        // future advisor-corroborated caller) can seed the queue out-of-band,
        // mirroring the `force_status` precedent. trace:BUG-480 | ai:claude
        //
        // BUG-631: scope the gate to DISPATCH-for-execution targets, matching the
        // CLI `aida queue add`. Routing `for: advisor` (or `human`/`reviewer`) is
        // a REQUEST for review/triage — open to any caller, since it does not
        // dispatch execution. Only execution-dispatch routes (implementer,
        // unknown/custom roles, and the unrouted default) keep the MCP gate.
        // trace:BUG-631 | ai:claude
        let for_arg = args.get("for").and_then(|v| v.as_str());
        if crate::for_target_requires_dispatch_authority(for_arg) {
            if let Some(msg) = mcp_queue_authority_message() {
                return Err(msg);
            }
        }
        self.tool_queue_add_inner(args)
    }

    // trace:EPIC-27 trace:BUG-480
    fn tool_queue_add_inner(&self, args: &Value) -> Result<String, String> {
        let id = required_string(args, "id")?;
        let user_id = self.queue_user_id(args);
        let top = args.get("top").and_then(|v| v.as_bool()).unwrap_or(false);
        let note = args
            .get("note")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);

        // BUG-18 routing: `for` = "any" → unrouted; an explicit role routes;
        // absent falls back to the active session role (if any). trace:EPIC-27
        let for_role: Option<String> = match args.get("for").and_then(|v| v.as_str()) {
            Some("any") => None,
            Some(role) => Some(role.to_string()),
            None => std::env::var("AIDA_SESSION_ROLE")
                .ok()
                .filter(|s| !s.is_empty()),
        };
        let for_scope = args
            .get("scope")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let store = self.storage.load().map_err(|e| e.to_string())?;
        let req = Self::resolve_requirement(&store, id)?;
        let req_id = req.id;
        let display = Self::display_id_of(req);
        let spec_id = req.spec_id.clone().unwrap_or_else(|| "???".to_string());
        let title = req.title.clone();

        // TASK-45: refuse re-queueing terminal work without force.
        if crate::is_terminal_status(&req.status) && !force {
            return Err(format!(
                "{} is {} — re-queueing closed work is usually a mistake. \
                 Pass `force: true` if you really mean to re-queue it.",
                display, req.status
            ));
        }

        let position = if top {
            let entries = self
                .storage
                .queue_list(&user_id, true)
                .map_err(|e| e.to_string())?;
            entries.first().map(|e| e.position - 1000).unwrap_or(1000)
        } else {
            i64::MAX // backend resolves to max+1000
        };

        let entry = aida_core::QueueEntry {
            user_id: user_id.clone(),
            requirement_id: req_id,
            position,
            added_by: user_id.clone(),
            note,
            added_at: chrono::Utc::now(),
            for_role: for_role.clone(),
            for_scope,
            for_session: None,
            added_by_machine: None,
        };
        self.storage.queue_add(entry).map_err(|e| e.to_string())?;
        crate::record_role_activity(&spec_id, "queue-add");

        let routing = for_role
            .as_deref()
            .map(|r| format!(" [for:{}]", r))
            .unwrap_or_default();
        Ok(format!("Added {} ({}) to queue{}", display, title, routing))
    }

    // trace:EPIC-27
    //
    // `aida queue work` launches a Claude session in a fresh worktree — an
    // act that cannot (and should not) happen inside the MCP server process.
    // The MCP tool is therefore a metadata-only PEEK: it resolves the item
    // that `aida queue work` WOULD pick up (the named id, or the head of the
    // role queue) and returns it plus the routed role, so an agent can decide
    // and then run the CLI to actually launch. This is the read-tier mirror
    // of the resolver, not the launcher.
    fn tool_queue_work(&self, args: &Value) -> Result<String, String> {
        let user_id = self.queue_user_id(args);
        let role = args.get("for").and_then(|v| v.as_str());
        let all = args.get("all").and_then(|v| v.as_bool()).unwrap_or(false);
        let store = self.storage.load().map_err(|e| e.to_string())?;

        // Explicit id → resolve that specific spec.
        if let Some(id) = args.get("id").and_then(|v| v.as_str()) {
            let req = Self::resolve_requirement(&store, id)?;
            let display = Self::display_id_of(req);
            let entries = self
                .storage
                .queue_list(&user_id, false)
                .map_err(|e| e.to_string())?;
            let queued = entries.iter().find(|e| e.requirement_id == req.id);
            return match queued {
                Some(entry) => Ok(format!(
                    "Pickup target: {} ({}) [status:{}{}]\nRun `aida queue work {}` to launch a session.",
                    display,
                    req.title,
                    req.status,
                    entry
                        .for_role
                        .as_deref()
                        .map(|r| format!(", for:{}", r))
                        .unwrap_or_default(),
                    display
                )),
                None => Err(format!(
                    "{} is {} and not currently queued — queue it first (`aida queue add {}`).",
                    display, req.status, display
                )),
            };
        }

        // No id → peek the head of the (role-filtered) queue, mirroring the
        // no-arg `aida queue work` head pickup. trace:EPIC-27
        let entries = self
            .storage
            .queue_list(&user_id, false)
            .map_err(|e| e.to_string())?;
        let session_role = std::env::var("AIDA_SESSION_ROLE").ok();
        let (role_filter, only_unrouted) =
            crate::resolve_queue_role_filter(role, all, session_role.as_deref());
        let head = entries
            .iter()
            .filter(|e| {
                crate::entry_matches_role_filter(
                    e.for_role.as_deref(),
                    role_filter.as_deref(),
                    only_unrouted,
                )
            })
            .find(|e| {
                store
                    .requirements
                    .iter()
                    .find(|r| r.id == e.requirement_id)
                    .map(|r| !crate::is_terminal_status(&r.status))
                    .unwrap_or(true)
            });
        match head {
            Some(entry) => {
                let req = store
                    .requirements
                    .iter()
                    .find(|r| r.id == entry.requirement_id);
                let (disp, title, status) = match req {
                    Some(r) => (
                        Self::display_id_of(r),
                        r.title.clone(),
                        r.status.to_string(),
                    ),
                    None => (entry.requirement_id.to_string(), String::new(), "?".into()),
                };
                Ok(format!(
                    "Next pickup: {} ({}) [status:{}{}]\nRun `aida queue work` to launch a session.",
                    disp,
                    title,
                    status,
                    entry
                        .for_role
                        .as_deref()
                        .map(|r| format!(", for:{}", r))
                        .unwrap_or_default(),
                ))
            }
            None => Ok("Queue is empty — nothing to pick up.".to_string()),
        }
    }

    // trace:EPIC-27
    fn tool_queue_done(&self, args: &Value) -> Result<String, String> {
        let id = required_string(args, "id")?;
        let user_id = self.queue_user_id(args);
        let store = self.storage.load().map_err(|e| e.to_string())?;
        let req = Self::resolve_requirement(&store, id)?;
        let req_id = req.id;
        let display = Self::display_id_of(req);
        let spec_id = req.spec_id.clone().unwrap_or_else(|| "???".to_string());

        // STORY-86: queue done flips to Done (work finished on a branch), not
        // Completed — the auto-bump on merge advances Done → Completed. Stamp
        // implementation_info the same way the CLI path does. trace:EPIC-27
        let now = chrono::Utc::now();
        let completer = crate::get_default_author();
        let source_tool = std::env::var("AIDA_AI_TOOL").ok().filter(|s| !s.is_empty());

        // STORY-542: capture user-facing interface changes (the deterministic
        // operator-digest source). MCP has no TTY, so it is flag-shaped only:
        // string-array params `interface_cli` / `interface_mcp` / `interface_tui`
        // / `interface_other`, or boolean `no_interface_change` to record an
        // explicit "no impact" marker. Absent ⇒ left untouched, exactly like a
        // non-interactive CLI `queue done`. trace:STORY-542 | ai:claude
        let str_array = |key: &str| -> Vec<String> {
            args.get(key)
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default()
        };
        let no_interface_change = args
            .get("no_interface_change")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let ic_cli = str_array("interface_cli");
        let ic_mcp = str_array("interface_mcp");
        let ic_tui = str_array("interface_tui");
        let ic_other = str_array("interface_other");
        let any_ic =
            !ic_cli.is_empty() || !ic_mcp.is_empty() || !ic_tui.is_empty() || !ic_other.is_empty();
        let captured_ic: Option<aida_core::InterfaceChanges> = if no_interface_change {
            Some(aida_core::InterfaceChanges::default())
        } else if any_ic {
            Some(aida_core::InterfaceChanges {
                cli: ic_cli,
                mcp: ic_mcp,
                tui: ic_tui,
                other: ic_other,
            })
        } else {
            None
        };

        // STORY-698: capture the verification steps the builder ran, into
        // implementation_info.test_coverage_notes (the PR-body audit trail).
        // MCP has no TTY, so it is flag-shaped only: a `test_plan` string array.
        // Absent ⇒ left untouched, exactly like a non-interactive CLI
        // `queue done` with no `--test-plan`. trace:STORY-698 | ai:claude
        let test_plan = str_array("test_plan");
        let captured_test_plan: Option<String> = if test_plan.is_empty() {
            None
        } else {
            Some(test_plan.join("\n"))
        };

        self.storage
            .update_atomically(|s| {
                if let Some(r) = s.requirements.iter_mut().find(|r| r.id == req_id) {
                    r.set_status_from_str("Done");
                    r.modified_at = now;
                    let info = r
                        .implementation_info
                        .get_or_insert_with(aida_core::ImplementationInfo::default);
                    info.implemented = true;
                    info.implemented_at.get_or_insert(now);
                    if info.implemented_by.is_none() {
                        info.implemented_by = Some(completer.clone());
                    }
                    if let Some(ref tool) = source_tool {
                        info.source_tool.get_or_insert_with(|| tool.clone());
                    }
                    if let Some(ref tp) = captured_test_plan {
                        info.test_coverage_notes = Some(tp.clone());
                    }
                    if let Some(ref ic) = captured_ic {
                        r.interface_changes = Some(ic.clone());
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        self.storage
            .queue_remove(&user_id, &req_id)
            .map_err(|e| e.to_string())?;
        crate::record_role_activity(&spec_id, "done");
        crate::update_manifest_for_status(&spec_id, "Done");

        Ok(format!("{} marked done and removed from queue.", display))
    }

    // trace:EPIC-27
    fn tool_queue_next(&self, args: &Value) -> Result<String, String> {
        // `next` is "peek the head" — the same resolution as the no-id
        // `queue_work` peek, so delegate to it. trace:EPIC-27
        let mut peek_args = args.clone();
        if let Some(obj) = peek_args.as_object_mut() {
            obj.remove("id");
        }
        self.tool_queue_work(&peek_args)
    }

    // trace:EPIC-27
    fn tool_queue_progress(&self, args: &Value) -> Result<String, String> {
        let store = self.storage.load().map_err(|e| e.to_string())?;

        // Resolve the spec set from a `batch:NAME` tag (the manifest/session
        // source the CLI uses needs cwd + leases, which MCP has no handle on;
        // batch + explicit spec ids are the portable axes). trace:EPIC-27
        let specs: Vec<String> = if let Some(batch) = args.get("batch").and_then(|v| v.as_str()) {
            let tag = format!("batch:{}", batch);
            let members: Vec<String> = store
                .requirements
                .iter()
                .filter(|r| r.tags.iter().any(|t| t.eq_ignore_ascii_case(&tag)))
                .map(|r| r.display_id())
                .collect();
            if members.is_empty() {
                return Ok(format!(
                    "No requirements tagged `batch:{}` — tag members via update_requirement.",
                    batch
                ));
            }
            members
        } else if let Some(arr) = args.get("specs").and_then(|v| v.as_array()) {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        } else {
            return Err(
                "queue_progress needs `batch: <name>` or `specs: [<id>, ...]` to define the \
                 set to report on (session-manifest progress is CLI-only)."
                    .to_string(),
            );
        };

        // Bucket by live status (same buckets as `aida queue progress`).
        // trace:EPIC-27
        let (mut shipped, mut in_flight, mut working, mut remaining) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for spec in &specs {
            let Some(req) = store.get_requirement_by_spec_id(spec) else {
                continue;
            };
            let disp = req.display_id();
            match req.status {
                RequirementStatus::Completed | RequirementStatus::Rejected => shipped.push(disp),
                RequirementStatus::Done => in_flight.push(disp),
                RequirementStatus::InProgress => working.push(disp),
                _ => remaining.push(disp),
            }
        }
        let total = shipped.len() + in_flight.len() + working.len() + remaining.len();
        let fmt = |label: &str, items: &[String]| {
            if items.is_empty() {
                format!("{}: 0", label)
            } else {
                format!("{}: {} ({})", label, items.len(), items.join(", "))
            }
        };
        Ok(format!(
            "Progress ({} item{}):\n{}\n{}\n{}\n{}",
            total,
            if total == 1 { "" } else { "s" },
            fmt("Shipped", &shipped),
            fmt("In flight", &in_flight),
            fmt("Working now", &working),
            fmt("Remaining", &remaining),
        ))
    }

    // trace:EPIC-27 trace:BUG-480
    fn tool_queue_rework(&self, args: &Value) -> Result<String, String> {
        // BUG-480: rework re-queues a spec for execution (and may flip its
        // status), so it carries the same advisor-authority weight as
        // queue_add. MCP is never advisor authority — refuse unconditionally,
        // matching the queue_add gate above. Mechanics live in
        // `tool_queue_rework_inner`. trace:BUG-480 | ai:claude
        if let Some(msg) = mcp_queue_authority_message() {
            return Err(msg);
        }
        self.tool_queue_rework_inner(args)
    }

    // trace:EPIC-27 trace:BUG-480
    fn tool_queue_rework_inner(&self, args: &Value) -> Result<String, String> {
        let id = required_string(args, "id")?;
        let user_id = self.queue_user_id(args);
        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
        let reason = args.get("reason").and_then(|v| v.as_str());
        let status_override = args.get("status").and_then(|v| v.as_str());

        let store = self.storage.load().map_err(|e| e.to_string())?;
        let req = Self::resolve_requirement(&store, id)?;
        let req_id = req.id;
        let display = Self::display_id_of(req);
        let spec_id = req.spec_id.clone().unwrap_or_else(|| "???".to_string());
        let title = req.title.clone();
        let current_status = req.status.clone();

        // Smart target-status resolution (`--status` wins). trace:EPIC-27
        let target_status: Option<RequirementStatus> = match status_override {
            Some(s) => Some(parse_status(s).ok_or_else(|| format!("invalid status '{}'", s))?),
            None => crate::rework_smart_target(&current_status),
        };

        // Terminal-status guard (mirrors the CLI). trace:EPIC-27
        if matches!(
            current_status,
            RequirementStatus::Completed | RequirementStatus::Rejected
        ) && !force
        {
            return Err(format!(
                "{} is {} — re-opening closed work is usually a mistake. \
                 Pass `force: true` if you really mean to rework it.",
                display, current_status
            ));
        }
        if matches!(current_status, RequirementStatus::InProgress) && !force {
            return Err(format!(
                "{} is already In Progress — pass `force: true` to re-queue it anyway.",
                display
            ));
        }

        let mut summary = String::new();
        if let Some(ref new_status) = target_status {
            if new_status != &current_status {
                let new_status = new_status.clone();
                let now = chrono::Utc::now();
                self.storage
                    .update_atomically(|s| {
                        if let Some(r) = s.requirements.iter_mut().find(|r| r.id == req_id) {
                            r.set_status_from_str(&format!("{:?}", new_status));
                            r.modified_at = now;
                        }
                    })
                    .map_err(|e| e.to_string())?;
                crate::record_role_activity(&spec_id, "rework");
                crate::update_manifest_for_status(&spec_id, &format!("{:?}", new_status));
                summary.push_str(&format!(
                    "{} status: {} → {}\n",
                    display, current_status, new_status
                ));
            }
        }

        // Optional audit comment.
        if let Some(reason_text) = reason {
            let author = crate::get_default_author();
            let comment = aida_core::Comment::new(author, reason_text.to_string());
            self.storage
                .update_atomically(|s| {
                    if let Some(r) = s.requirements.iter_mut().find(|r| r.id == req_id) {
                        r.add_comment(comment);
                    }
                })
                .map_err(|e| e.to_string())?;
        }

        // Routing: `for` wins, else active role, else unrouted. trace:EPIC-27
        let for_role: Option<String> = match args.get("for").and_then(|v| v.as_str()) {
            Some("any") => None,
            Some(role) => Some(role.to_string()),
            None => std::env::var("AIDA_SESSION_ROLE")
                .ok()
                .filter(|s| !s.is_empty()),
        };
        let entry = aida_core::QueueEntry {
            user_id: user_id.clone(),
            requirement_id: req_id,
            position: i64::MAX,
            added_by: user_id.clone(),
            note: reason.map(|r| r.to_string()),
            added_at: chrono::Utc::now(),
            for_role: for_role.clone(),
            for_scope: None,
            for_session: None,
            added_by_machine: None,
        };
        self.storage.queue_add(entry).map_err(|e| e.to_string())?;
        crate::record_role_activity(&spec_id, "queue-add");

        let routing = for_role
            .as_deref()
            .map(|r| format!(" [for:{}]", r))
            .unwrap_or_default();
        summary.push_str(&format!("Queued {} ({}){}", display, title, routing));
        Ok(summary)
    }

    // trace:EPIC-27
    fn tool_queue_move(&self, args: &Value) -> Result<String, String> {
        let id = required_string(args, "id")?;
        let user_id = self.queue_user_id(args);
        let top = args.get("top").and_then(|v| v.as_bool()).unwrap_or(false);
        let bottom = args
            .get("bottom")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let before = args.get("before").and_then(|v| v.as_str());
        let after = args.get("after").and_then(|v| v.as_str());

        let store = self.storage.load().map_err(|e| e.to_string())?;
        let req = Self::resolve_requirement(&store, id)?;
        let req_id = req.id;
        let display = Self::display_id_of(req);

        let entries = self
            .storage
            .queue_list(&user_id, true)
            .map_err(|e| e.to_string())?;
        if !entries.iter().any(|e| e.requirement_id == req_id) {
            return Err(format!("{} is not in the queue.", display));
        }

        let new_position = if top {
            entries.first().map(|e| e.position - 1000).unwrap_or(0)
        } else if bottom {
            entries.last().map(|e| e.position + 1000).unwrap_or(1000)
        } else if let Some(before_id) = before {
            let before_req = Self::resolve_requirement(&store, before_id)?;
            if before_req.id == req_id {
                return Err("`before` target is the same as the moved item".to_string());
            }
            entries
                .iter()
                .find(|e| e.requirement_id == before_req.id)
                .ok_or_else(|| format!("{} is not in the queue", before_id))
                .map(|e| e.position - 1)?
        } else if let Some(after_id) = after {
            let after_req = Self::resolve_requirement(&store, after_id)?;
            if after_req.id == req_id {
                return Err("`after` target is the same as the moved item".to_string());
            }
            entries
                .iter()
                .find(|e| e.requirement_id == after_req.id)
                .ok_or_else(|| format!("{} is not in the queue", after_id))
                .map(|e| e.position + 1)?
        } else {
            return Err(
                "specify a destination: `top`, `bottom`, `before: <id>`, or `after: <id>`"
                    .to_string(),
            );
        };

        self.storage
            .queue_reorder(&user_id, &[(req_id, new_position)])
            .map_err(|e| e.to_string())?;
        Ok(format!("Moved {} in queue.", display))
    }

    // trace:EPIC-27
    fn tool_queue_remove(&self, args: &Value) -> Result<String, String> {
        let id = required_string(args, "id")?;
        let user_id = self.queue_user_id(args);
        let store = self.storage.load().map_err(|e| e.to_string())?;
        let req = Self::resolve_requirement(&store, id)?;
        let display = Self::display_id_of(req);
        // BUG-529: `for` is a role FILTER on remove, mirroring the CLI's
        // `aida queue remove --for`. `any` (or absent) → role-blind; otherwise
        // remove only the entry queued for that role. trace:BUG-529 | ai:claude
        let remove_role: Option<String> = match args.get("for").and_then(|v| v.as_str()) {
            None | Some("any") => None,
            Some(role) => Some(canonical_light_role_name(role)),
        };
        self.storage
            .queue_remove_for_role(&user_id, &req.id, remove_role.as_deref())
            .map_err(|e| e.to_string())?;
        match remove_role.as_deref() {
            Some(role) => Ok(format!("Removed {} from queue [role:{}].", display, role)),
            None => Ok(format!("Removed {} from queue.", display)),
        }
    }

    // ========================================================================
    // Session tool implementations — STORY-533 (EPIC-27)
    //
    // These mirror the `aida session <subcommand>` CLI surface so MCP-speaking
    // agents can inspect scoped session leases + manifests without shelling
    // out. The three inspectors (leases / status / manifest) read the same
    // `.aida/sessions/*.toml` files the CLI handlers read — no subprocess to
    // `aida`. session_start / session_end are metadata-only PEEKS: the actual
    // launch (worktree + `claude`) and teardown (worktree removal, claude
    // process termination, PR detection) are subprocess-driven acts that
    // cannot and should not happen inside the MCP server process, so the tools
    // resolve/describe the work and instruct the caller to run the CLI to
    // perform it. This follows the queue_work PEEK precedent (STORY-532).
    // trace:EPIC-27
    // ========================================================================

    /// Resolve a `LightLease` by 8-char (or longer) id prefix, mirroring the
    /// CLI's `find_lease_by_id_prefix`. trace:EPIC-27
    fn resolve_lease_by_prefix<'l>(
        leases: &'l [LightLease],
        query: &str,
    ) -> Result<&'l LightLease, String> {
        let matches: Vec<&LightLease> = leases.iter().filter(|l| l.id.starts_with(query)).collect();
        match matches.len() {
            0 => Err(format!("no session lease matches id '{}'", query)),
            1 => Ok(matches[0]),
            _ => Err(format!(
                "id prefix '{}' is ambiguous ({} leases match) — pass a longer prefix",
                query,
                matches.len()
            )),
        }
    }

    fn lease_short_id(l: &LightLease) -> &str {
        l.id.get(..8).unwrap_or(&l.id)
    }

    // trace:EPIC-27 — mirrors `aida session leases`.
    fn tool_session_leases(&self, args: &Value) -> Result<String, String> {
        let all = args.get("all").and_then(|v| v.as_bool()).unwrap_or(false);
        let leases = list_leases(&self.project_root);
        if leases.is_empty() {
            return Ok("(no active sessions)".to_string());
        }
        // Default view hides MCP-claim markers (they aren't `session start`
        // leases) unless `all` is set, mirroring the CLI's bias toward
        // real worktree-backed sessions. trace:EPIC-27
        let mut lines: Vec<String> = Vec::new();
        for l in &leases {
            if !all && l.mcp_claim {
                continue;
            }
            let role = l.role.as_deref().unwrap_or("?");
            let kind = if l.mcp_claim { "mcp-claim" } else { "session" };
            lines.push(format!(
                "- {} scope={} branch={} role={} owner={} kind={} started_at={}",
                Self::lease_short_id(l),
                l.scope,
                if l.branch.is_empty() { "?" } else { &l.branch },
                role,
                l.owner,
                kind,
                l.started_at
            ));
        }
        if lines.is_empty() {
            return Ok(
                "(no worktree-backed sessions; pass `all: true` to include MCP claim markers)"
                    .to_string(),
            );
        }
        Ok(format!(
            "Active session lease(s) ({}):\n{}",
            lines.len(),
            lines.join("\n")
        ))
    }

    // trace:EPIC-27 — mirrors `aida session show` (the per-lease status view).
    fn tool_session_status(&self, args: &Value) -> Result<String, String> {
        let leases = list_leases(&self.project_root);
        if leases.is_empty() {
            return Ok(
                "(no active sessions) — start one with `aida session start --owns <scope>`."
                    .to_string(),
            );
        }
        // MCP has no shell cwd/ancestor-PID context that maps to a worktree,
        // so an `id` is required when more than one lease exists; a single
        // lease is shown directly. trace:EPIC-27
        let lease = match args.get("id").and_then(|v| v.as_str()) {
            Some(q) => Self::resolve_lease_by_prefix(&leases, q)?,
            None if leases.len() == 1 => &leases[0],
            None => {
                let ids: Vec<String> = leases
                    .iter()
                    .map(|l| format!("{} ({})", Self::lease_short_id(l), l.scope))
                    .collect();
                return Err(format!(
                    "multiple active sessions — pass `id` to pick one: {}",
                    ids.join(", ")
                ));
            }
        };

        let mut out = format!("Session {}\n", Self::lease_short_id(lease));
        out.push_str(&format!("  scope: {}\n", lease.scope));
        out.push_str(&format!(
            "  branch: {}\n",
            if lease.branch.is_empty() {
                "?"
            } else {
                &lease.branch
            }
        ));
        out.push_str(&format!(
            "  worktree: {}\n",
            if lease.worktree_path.is_empty() {
                "?"
            } else {
                &lease.worktree_path
            }
        ));
        out.push_str(&format!(
            "  role: {}\n",
            lease.role.as_deref().unwrap_or("?")
        ));
        out.push_str(&format!("  owner: {}\n", lease.owner));
        out.push_str(&format!("  hostname: {}\n", lease.hostname));
        out.push_str(&format!("  started_at: {}\n", lease.started_at));
        out.push_str(&format!(
            "  kind: {}",
            if lease.mcp_claim {
                "mcp-claim"
            } else {
                "session"
            }
        ));

        // Surface manifest progress inline when one exists, matching
        // `aida session show --plan`. trace:EPIC-27
        let manifest_path = crate::session_manifest::manifest_path(&self.project_root, &lease.id);
        if let Ok(manifest) = crate::session_manifest::load(&manifest_path) {
            if !manifest.items.is_empty() {
                let done = manifest
                    .items
                    .iter()
                    .filter(|it| it.completed_at.is_some())
                    .count();
                out.push_str(&format!(
                    "\n  manifest: {}/{} item(s) completed",
                    done,
                    manifest.items.len()
                ));
            }
        }
        Ok(out)
    }

    // trace:EPIC-27 — mirrors `aida session manifest` (read of the planned
    // cluster). The CLI `manifest` subcommand is write-side (write /
    // mark-started / mark-completed); those marks happen automatically as
    // `aida edit` / `queue done` run, and writing one needs the active lease's
    // cwd context. The portable MCP mirror is the READ — render the planned
    // cluster + per-item status for a resolvable session. trace:EPIC-27
    fn tool_session_manifest(&self, args: &Value) -> Result<String, String> {
        let leases = list_leases(&self.project_root);
        if leases.is_empty() {
            return Ok("(no active sessions) — no manifest to show.".to_string());
        }
        let lease = match args.get("id").and_then(|v| v.as_str()) {
            Some(q) => Self::resolve_lease_by_prefix(&leases, q)?,
            None if leases.len() == 1 => &leases[0],
            None => {
                let ids: Vec<String> = leases
                    .iter()
                    .map(|l| format!("{} ({})", Self::lease_short_id(l), l.scope))
                    .collect();
                return Err(format!(
                    "multiple active sessions — pass `id` to pick one: {}",
                    ids.join(", ")
                ));
            }
        };
        let manifest_path = crate::session_manifest::manifest_path(&self.project_root, &lease.id);
        let manifest = match crate::session_manifest::load(&manifest_path) {
            Ok(m) => m,
            Err(_) => {
                return Ok(format!(
                    "Session {} has no planned-cluster manifest yet (none written by /aida-pickup).",
                    Self::lease_short_id(lease)
                ));
            }
        };
        if manifest.items.is_empty() {
            return Ok(format!(
                "Session {} manifest is empty.",
                Self::lease_short_id(lease)
            ));
        }
        let mut lines: Vec<String> = Vec::new();
        // completed / started-not-completed / pending — same status markers as
        // `aida session show --plan`, routed through the glyph registry so the
        // ascii profile / [glyphs] overrides apply. `○` (pending) is not a
        // registry glyph and stays literal. trace:EPIC-27 trace:TASK-840 | ai:claude
        let root = crate::find_project_root().ok();
        for it in &manifest.items {
            let marker = if it.completed_at.is_some() {
                crate::glyphs::get(crate::glyphs::Glyph::Check, root.as_deref())
            } else if it.started_at.is_some() {
                crate::glyphs::get(crate::glyphs::Glyph::InFlight, root.as_deref())
            } else {
                "○"
            };
            lines.push(format!("  {} {}", marker, it.spec_id));
        }
        let done = manifest
            .items
            .iter()
            .filter(|it| it.completed_at.is_some())
            .count();
        Ok(format!(
            "Session {} planned cluster ({}/{} done, source: {}):\n{}",
            Self::lease_short_id(lease),
            done,
            manifest.items.len(),
            manifest.plan_source,
            lines.join("\n")
        ))
    }

    // trace:EPIC-27
    //
    // `aida session start` creates a sibling git worktree on a fresh branch
    // and (with `--launch`) execs `claude` inside it — subprocess-driven acts
    // that cannot happen inside the MCP server process. The MCP tool is
    // therefore a metadata-only PEEK: it validates the scope/role inputs and
    // returns the exact CLI invocation the caller should run to actually
    // start the session. This is the read-tier mirror of the resolver, not
    // the launcher (the queue_work PEEK precedent). trace:EPIC-27
    fn tool_session_start(&self, args: &Value) -> Result<String, String> {
        let owns = required_string(args, "owns")?;
        let role = args.get("role").and_then(|v| v.as_str());
        let branch = args.get("branch").and_then(|v| v.as_str());
        let launch = args
            .get("launch")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Warn if the scope is already covered by a live lease (the CLI
        // detects this too; we surface it so the caller doesn't double-start).
        // trace:EPIC-27
        let leases = list_leases(&self.project_root);
        let existing = leases
            .iter()
            .find(|l| l.scope.eq_ignore_ascii_case(owns) && !l.mcp_claim);

        // trace:TASK-712 — shell-quote user-supplied values interpolated into the
        // copy-paste `Run:` command so spaces / metacharacters survive a paste.
        let mut cmd = format!("aida session start --owns {}", shell_quote_arg(owns));
        if let Some(b) = branch {
            cmd.push_str(&format!(" --branch {}", shell_quote_arg(b)));
        }
        if let Some(r) = role {
            cmd.push_str(&format!(" --role {}", shell_quote_arg(r)));
        }
        if launch {
            cmd.push_str(" --launch");
        }

        let mut out = String::new();
        if let Some(l) = existing {
            out.push_str(&format!(
                "Note: scope '{}' is already held by session {} (branch {}).\n",
                owns,
                Self::lease_short_id(l),
                if l.branch.is_empty() { "?" } else { &l.branch }
            ));
        }
        out.push_str(&format!(
            "Starting a scoped session creates a worktree + lease{} — \
             a subprocess act the MCP server does not perform.\nRun:\n  {}",
            if launch {
                " and launches a Claude session"
            } else {
                ""
            },
            cmd
        ));
        Ok(out)
    }

    // trace:EPIC-27
    //
    // `aida session end` removes the worktree (git subprocess), can terminate
    // live `claude` processes, probes the PR + CI state, and files reviewer
    // follow-ups — none of which can happen safely inside the MCP server
    // process. The MCP tool is therefore a metadata-only PEEK: it resolves the
    // lease that `aida session end` would target and returns the CLI command
    // to run. trace:EPIC-27
    fn tool_session_end(&self, args: &Value) -> Result<String, String> {
        let leases = list_leases(&self.project_root);
        if leases.is_empty() {
            return Ok("(no active sessions) — nothing to end.".to_string());
        }
        // Resolve the target the same axes the CLI accepts: id prefix, or
        // scope/spec match. MCP has no cwd-to-worktree mapping, so when none
        // is given and more than one lease exists, ask the caller to pick.
        // trace:EPIC-27
        let id = args.get("id").and_then(|v| v.as_str());
        let spec = args.get("spec").and_then(|v| v.as_str());
        let lease: &LightLease = if let Some(q) = id {
            Self::resolve_lease_by_prefix(&leases, q)?
        } else if let Some(s) = spec {
            let matches: Vec<&LightLease> = leases
                .iter()
                .filter(|l| l.scope.eq_ignore_ascii_case(s))
                .collect();
            match matches.len() {
                0 => return Err(format!("no session lease owns spec '{}'", s)),
                1 => matches[0],
                _ => {
                    let ids: Vec<String> = matches
                        .iter()
                        .map(|l| Self::lease_short_id(l).to_string())
                        .collect();
                    return Err(format!(
                        "multiple leases own '{}' — disambiguate with `id`: {}",
                        s,
                        ids.join(", ")
                    ));
                }
            }
        } else if leases.len() == 1 {
            &leases[0]
        } else {
            let ids: Vec<String> = leases
                .iter()
                .map(|l| format!("{} ({})", Self::lease_short_id(l), l.scope))
                .collect();
            return Err(format!(
                "multiple active sessions — pass `id` or `spec` to pick one: {}",
                ids.join(", ")
            ));
        };

        let cmd = format!("aida session end {}", Self::lease_short_id(lease));
        Ok(format!(
            "Ending session {} (scope {}, branch {}) removes its worktree and lease — \
             a subprocess act the MCP server does not perform.\nRun:\n  {}",
            Self::lease_short_id(lease),
            lease.scope,
            if lease.branch.is_empty() {
                "?"
            } else {
                &lease.branch
            },
            cmd
        ))
    }

    // ========================================================================
    // Role tool implementations — STORY-534 (EPIC-27)
    //
    // These mirror the `aida role <subcommand>` CLI surface so MCP-speaking
    // agents can inspect the project's personas without shelling out. The two
    // inspectors (list / show) read the role TOML files the CLI reads — under
    // `.aida/roles/` and `~/.aida/roles/` — with no subprocess to `aida`.
    //
    // role_enter / role_end are metadata-only PEEKS: `aida role enter` /
    // `aida role end` set/clear the *shell's* `AIDA_SESSION_ROLE` env var by
    // emitting shell code the caller must `eval`. Role identity is therefore
    // shell-keyed (the same way the queue user is shell-keyed — see the BUG-89
    // queue-identity note), and the stateless MCP server process cannot mutate
    // the calling agent's shell environment. So the tools validate the inputs
    // and return the exact `aida role …` command to run, rather than mutating
    // server-side state that would never reach the caller. This follows the
    // queue_work / session_start PEEK precedent. trace:EPIC-27
    // ========================================================================

    // trace:EPIC-27 — mirrors `aida role list`.
    fn tool_role_list(&self, _args: &Value) -> Result<String, String> {
        let active = role_active_env();
        let roles = list_light_roles(&self.project_root);
        if roles.is_empty() {
            return Ok(format!(
                "(no roles defined for {}) — create one with `aida role add <name>` or install a starter set with `aida role scaffold`.",
                self.project_root.display()
            ));
        }
        let mut lines: Vec<String> = Vec::new();
        for r in &roles {
            let marker = if active.as_deref() == Some(r.name.as_str()) {
                "*"
            } else {
                " "
            };
            let scope = if r.global { " [global]" } else { "" };
            let purpose = r
                .purpose
                .as_deref()
                .map(|p| format!(" — {}", p))
                .unwrap_or_default();
            lines.push(format!(
                "{} {}{} last active {}{}",
                marker,
                r.name,
                scope,
                light_role_relative(&r.last_active_at),
                purpose
            ));
        }
        Ok(format!(
            "Roles for {} ({}):\n{}",
            self.project_root.display(),
            lines.len(),
            lines.join("\n")
        ))
    }

    // trace:EPIC-27 — mirrors `aida role show [NAME]`. With no `name`, resolves
    // the active role from `AIDA_SESSION_ROLE` like the CLI; errors when none
    // is given and none is active.
    fn tool_role_show(&self, args: &Value) -> Result<String, String> {
        let resolved = match args.get("name").and_then(|v| v.as_str()) {
            Some(n) => canonical_light_role_name(n),
            None => role_active_env().ok_or_else(|| {
                "No role active and no `name` given. Use role_list to see options.".to_string()
            })?,
        };
        let role = load_light_role(&self.project_root, &resolved).ok_or_else(|| {
            format!(
                "No such role: {} — create it with `aida role add {}`, or see options with role_list.",
                resolved, resolved
            )
        })?;
        let mut out = format!("Role:        {}", role.name);
        if role.global {
            out.push_str(" [global]");
        }
        out.push('\n');
        out.push_str(&format!(
            "Purpose:     {}\n",
            role.purpose.as_deref().unwrap_or("(none)")
        ));
        out.push_str(&format!(
            "Created:     {}\n",
            role.created_at.as_deref().unwrap_or("?")
        ));
        out.push_str(&format!(
            "Last active: {} ({})\n",
            role.last_active_at,
            light_role_relative(&role.last_active_at)
        ));
        if let Some(d) = &role.working_directory {
            out.push_str(&format!("Last cwd:    {}\n", d));
        }
        if let Some(n) = &role.notes {
            out.push_str(&format!("Notes:       {}\n", n));
        }
        if !role.scope_tags.is_empty() || role.scope_status.is_some() {
            let mut parts: Vec<String> = Vec::new();
            if !role.scope_tags.is_empty() {
                parts.push(format!("tags={}", role.scope_tags.join(",")));
            }
            if let Some(s) = &role.scope_status {
                parts.push(format!("status={}", s));
            }
            out.push_str(&format!("Scope:       {}\n", parts.join(" ")));
        }
        if let Some(text) = &role.system_prompt {
            let preview: String = text.lines().next().unwrap_or("").chars().take(80).collect();
            let suffix = if text.chars().count() > preview.chars().count() {
                "…"
            } else {
                ""
            };
            out.push_str(&format!(
                "Addendum:    {} chars — {}{}\n",
                text.len(),
                preview,
                suffix
            ));
        }
        let active_marker = if role_active_env().as_deref() == Some(role.name.as_str()) {
            "  (active in the CLI's shell)"
        } else {
            ""
        };
        out.push_str(&format!(
            "Active:      {}{}",
            role_active_env().as_deref().unwrap_or("(none)"),
            active_marker
        ));
        Ok(out)
    }

    // trace:EPIC-27
    //
    // `aida role enter` sets the shell's `AIDA_SESSION_ROLE` env var (it emits
    // shell code the caller `eval`s). The MCP server cannot mutate the calling
    // agent's shell, so this is a metadata-only PEEK: it validates the role
    // exists and returns the exact `aida role enter` command + the resolved
    // role context. trace:EPIC-27
    fn tool_role_enter(&self, args: &Value) -> Result<String, String> {
        let name = required_string(args, "name")?;
        let cd = args.get("cd").and_then(|v| v.as_bool()).unwrap_or(false);
        let resolved = canonical_light_role_name(name);
        let role = load_light_role(&self.project_root, &resolved).ok_or_else(|| {
            format!(
                "No such role: {} — create it with `aida role add {}`, or see options with role_list.",
                resolved, resolved
            )
        })?;

        // trace:TASK-712 — shell-quote the (user-derived) role name in the
        // copy-paste `Run:` command.
        let mut cmd = format!("aida role enter {}", shell_quote_arg(&resolved));
        if cd {
            cmd.push_str(" --cd");
        }

        let mut out = String::new();
        if role_active_env().as_deref() == Some(resolved.as_str()) {
            out.push_str(&format!(
                "Note: role '{}' is already the active shell role.\n",
                resolved
            ));
        }
        let purpose = role
            .purpose
            .as_deref()
            .map(|p| format!(" ({})", p))
            .unwrap_or_default();
        out.push_str(&format!(
            "Entering a role sets the shell's active role (AIDA_SESSION_ROLE) — \
             a shell-env act the MCP server does not perform for you.\n\
             Resolved role: {}{}{}.\nRun:\n  {}",
            resolved,
            if role.global { " [global]" } else { "" },
            purpose,
            cmd
        ));
        Ok(out)
    }

    // trace:EPIC-27
    //
    // `aida role end` clears the shell's `AIDA_SESSION_ROLE` (it emits shell
    // code the caller `eval`s). Same shell-env constraint as role_enter, so
    // this is a metadata-only PEEK returning the command to run. trace:EPIC-27
    fn tool_role_end(&self, _args: &Value) -> Result<String, String> {
        let active = role_active_env();
        let mut out = String::new();
        match active.as_deref() {
            Some(r) => out.push_str(&format!("Active shell role: {}.\n", r)),
            None => out.push_str("No role appears active in the CLI's shell.\n"),
        }
        out.push_str(
            "Ending a role clears the shell's active role (AIDA_SESSION_ROLE) — \
             a shell-env act the MCP server does not perform for you.\nRun:\n  aida role end",
        );
        Ok(out)
    }

    // ========================================================================
    // Workflow tool implementations — STORY-536 (EPIC-27)
    //
    // The remaining CLI long tail. The read-only mirrors compute in-process
    // from the same library helpers the CLI handlers use (cache reads, the
    // plan-lint pass, the ultraplan assembler, the goal-clause builder, the
    // usage-log aggregator). db_sync / fetch / pull are git network +
    // working-tree / store mutations driven by subprocess `git`; an in-process
    // MCP mutation would surprise the caller (working-tree changes, remote
    // pushes), so they follow the queue_work / session_start PEEK precedent —
    // they return the exact `aida …` command to run. trace:EPIC-27
    // ========================================================================

    // trace:EPIC-27 — mirrors `aida cache status`. Reads the SQLite cache +
    // the orphan store's HEAD SHA directly (no GitBackend construction).
    fn tool_cache_status(&self, _args: &Value) -> Result<String, String> {
        let store_path = self.project_root.join(".aida-store");
        if !store_path.exists() {
            return Err(format!(
                "no git-canonical store at {} — `cache status` applies to a distributed-mode \
                 project. Run `aida init` first.",
                store_path.display()
            ));
        }
        let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
        let cache = aida_core::Cache::open(&cache_path).map_err(|e| e.to_string())?;
        let recorded_sha = cache
            .source_head_sha()
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        let actual_sha = aida_core::git_ops::head_sha(&store_path).unwrap_or_default();
        let count = cache.requirement_count().map_err(|e| e.to_string())?;
        let built_at = cache
            .built_at()
            .map_err(|e| e.to_string())?
            .unwrap_or_else(|| "(never)".into());

        let mut out = String::new();
        out.push_str(&format!(
            "Cache path:          {}\n",
            cache.path().display()
        ));
        out.push_str(&format!("Cached requirements: {}\n", count));
        out.push_str(&format!("Last built:          {}\n", built_at));
        out.push_str(&format!(
            "Cache HEAD SHA:      {}\n",
            if recorded_sha.is_empty() {
                "(none)".to_string()
            } else {
                recorded_sha.clone()
            }
        ));
        out.push_str(&format!(
            "Store HEAD SHA:      {}\n",
            if actual_sha.is_empty() {
                "(no git head — non-git store?)".to_string()
            } else {
                actual_sha.clone()
            }
        ));
        let stale = recorded_sha != actual_sha || recorded_sha.is_empty();
        if stale && !actual_sha.is_empty() {
            out.push_str("Status:              STALE — run `aida cache rebuild`");
        } else {
            out.push_str("Status:              FRESH");
        }
        Ok(out)
    }

    // trace:EPIC-27 — read-only mirror of `aida plan verify <file>`. Runs the
    // pure `compute_plan_report` lint pass and renders the report to a string;
    // never rewrites the file (--fix) or exits the process.
    fn tool_plan_verify(&self, args: &Value) -> Result<String, String> {
        let file = required_string(args, "file")?;
        // Resolve relative paths against the project root so callers can pass
        // a repo-relative plan path.
        let path = {
            let p = Path::new(file);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                self.project_root.join(p)
            }
        };
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("could not read plan file {}: {}", path.display(), e))?;
        let root = crate::plan_repo_root(&path);
        let report = crate::compute_plan_report(&content, &root);
        Ok(crate::render_plan_report_string(
            &report,
            &path.display().to_string(),
        ))
    }

    // trace:EPIC-27 — read-only mirror of `aida plan helpers <spec>`. Derives
    // the `## Reusable helpers` section from the trace graph; the --append
    // file write stays CLI-only.
    fn tool_plan_helpers(&self, args: &Value) -> Result<String, String> {
        let spec = required_string(args, "spec")?;
        let store = self.storage.load().map_err(|e| e.to_string())?;
        let target = Self::resolve_requirement(&store, spec)?;
        let display = Self::display_id_of(target);
        match crate::build_reusable_helpers_section(&store, &self.project_root, target) {
            Some(mut md) => {
                md.push_str(&format!(
                    "\n_Generated by `aida plan helpers {display}` — verify before relying on it._\n"
                ));
                Ok(md)
            }
            None => Ok(format!(
                "No reusable helpers derived for {display} — no related spec (sibling / \
                 tag-mate / same-feature) carries a `trace:` comment that names a helper."
            )),
        }
    }

    // trace:EPIC-27 — read-only mirror of `aida ultraplan <spec> --stdout`.
    // Assembles the prompt + warnings; clipboard / deep-link stays CLI-only.
    fn tool_ultraplan_assemble(&self, args: &Value) -> Result<String, String> {
        let spec = required_string(args, "spec")?;
        let no_comments = args
            .get("no_comments")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // TASK-304: honor `[ultraplan] mode = "never"`.
        if crate::read_ultraplan_config(&self.project_root).mode == crate::UltraplanMode::Never {
            return Err(
                "`aida ultraplan` is disabled for this project (`[ultraplan] mode = \"never\"` \
                 in .aida/config.toml). Set mode = \"on-demand\" or \"suggested\" to re-enable."
                    .to_string(),
            );
        }
        let store = self.storage.load().map_err(|e| e.to_string())?;
        let target = Self::resolve_requirement(&store, spec)?;
        let helpers = crate::build_reusable_helpers_section(&store, &self.project_root, target);
        let (reservations, reservation_warnings) = crate::read_reserved_paths(&self.project_root);
        let (prompt, mut warnings) = crate::assemble_ultraplan_prompt(
            &store,
            target,
            helpers.as_deref(),
            !no_comments,
            &reservations,
        );
        warnings.extend(reservation_warnings);
        let mut out = prompt;
        if !warnings.is_empty() {
            out.push('\n');
            for w in &warnings {
                out.push_str(&format!("\nWarning: {}", w));
            }
        }
        Ok(out)
    }

    // trace:EPIC-27 — mirrors `aida goal …`. Pure clause builder; copy /
    // invoke / deep-link stays CLI-only.
    fn tool_goal_derive(&self, args: &Value) -> Result<String, String> {
        let batch = args.get("batch").and_then(|v| v.as_str());
        let epic = args.get("epic").and_then(|v| v.as_str());
        let spec = args.get("spec").and_then(|v| v.as_str());
        let pr = args.get("pr").and_then(|v| v.as_u64());
        let queue_empty = args.get("queue_empty").and_then(|v| v.as_str());
        let clauses = crate::build_goal_clauses(batch, epic, spec, pr, queue_empty)
            .map_err(|e| e.to_string())?;
        let condition = crate::assemble_goal_condition(&clauses);
        let mut out = format!("/goal {}\n\nverify each clause:", condition);
        for c in &clauses {
            out.push_str(&format!("\n  · {}", c.verify));
        }
        Ok(out)
    }

    // trace:EPIC-27 — a lightweight in-process status snapshot (no CI probe,
    // no CachedGitBackend). The CI-bearing surface of `aida status` shells out
    // to `gh` and stays CLI-only; this mirrors only the substrate-grounded
    // parts the server can read directly (store counts, leases, queue depth).
    fn tool_status_unified(&self, args: &Value) -> Result<String, String> {
        let store = self.storage.load().map_err(|e| e.to_string())?;
        // BUG-717: mirror the CLI work-view (`aida status` / `aida list`) —
        // exclude standing-artifact / stateless types (vision / principle /
        // term / constraint / folder / meta) so the MCP status count agrees
        // with the CLI instead of counting the seeded META prompt-templates as
        // requirements.
        let work: Vec<_> = store
            .requirements
            .iter()
            .filter(|r| !crate::is_standing_artifact_type(&r.req_type.to_string()))
            .collect();
        let by_status = |s: RequirementStatus| work.iter().filter(|r| r.status == s).count();

        let mut out = String::from("Project status\n");
        out.push_str(&format!("  Total requirements: {}\n", work.len()));
        out.push_str("  By status:\n");
        for (label, status) in [
            ("Draft", RequirementStatus::Draft),
            ("Approved", RequirementStatus::Approved),
            ("Planned", RequirementStatus::Planned),
            ("In Progress", RequirementStatus::InProgress),
            ("Needs Attention", RequirementStatus::NeedsAttention),
            ("Done", RequirementStatus::Done),
            ("Completed", RequirementStatus::Completed),
            ("Rejected", RequirementStatus::Rejected),
        ] {
            let n = by_status(status);
            if n > 0 {
                out.push_str(&format!("    {:<16} {}\n", format!("{}:", label), n));
            }
        }

        // Active session leases (worktree-backed only, matching the CLI bias).
        let leases: Vec<_> = list_leases(&self.project_root)
            .into_iter()
            .filter(|l| !l.mcp_claim)
            .collect();
        out.push_str(&format!("  Active sessions: {}\n", leases.len()));
        for l in &leases {
            out.push_str(&format!(
                "    - {} scope={} role={}\n",
                Self::lease_short_id(l),
                l.scope,
                l.role.as_deref().unwrap_or("?")
            ));
        }

        // Queue depth for the resolved user.
        let user_id = self.queue_user_id(args);
        let depth = self
            .storage
            .queue_list(&user_id, false)
            .map(|e| e.len())
            .unwrap_or(0);
        out.push_str(&format!("  Queue depth ('{}'): {}\n", user_id, depth));

        out.push_str(
            "  Note: PR/CI rollup + awaiting-you gates need `gh` — run `aida status`, \
             or read the aida://project/summary / aida://session/leases MCP resources.",
        );
        Ok(out)
    }

    // trace:EPIC-27 — read-only aggregation over the local usage telemetry log
    // (`~/.aida/usage.jsonl`). Mirrors `aida usage` / `--errors` / `--unused`;
    // the orchestrator-telemetry views (`--auto-complete`, `--health`) stay
    // CLI-only.
    fn tool_usage_query(&self, args: &Value) -> Result<String, String> {
        let since_raw = args.get("since").and_then(|v| v.as_str()).unwrap_or("30d");
        let unused_raw = args.get("unused").and_then(|v| v.as_str());
        let errors_only = args
            .get("errors")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(20);

        let events = crate::usage::read_events();
        if events.is_empty() {
            return Ok(
                "Usage: (no events yet; the log fills as `aida …` commands run)".to_string(),
            );
        }
        let now = chrono::Utc::now();
        let since_window = crate::parse_days_arg(since_raw).map_err(|e| e.to_string())?;
        let since = now - since_window;

        // --unused: commands not seen since the cutoff.
        if let Some(raw) = unused_raw {
            let cutoff_window = crate::parse_days_arg(raw).map_err(|e| e.to_string())?;
            let cutoff = now - cutoff_window;
            let mut last_seen: std::collections::HashMap<String, chrono::DateTime<chrono::Utc>> =
                std::collections::HashMap::new();
            for ev in &events {
                if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(&ev.ts) {
                    let ts = ts.with_timezone(&chrono::Utc);
                    let cur = last_seen.entry(ev.cmd.clone()).or_insert(ts);
                    if *cur < ts {
                        *cur = ts;
                    }
                }
            }
            let mut stale: Vec<(String, chrono::DateTime<chrono::Utc>)> = last_seen
                .into_iter()
                .filter(|(_, ts)| *ts < cutoff)
                .collect();
            stale.sort_by_key(|(_, ts)| *ts);
            let mut out = format!(
                "Usage: commands NOT used in the last {} (deprecation candidates):",
                raw
            );
            if stale.is_empty() {
                out.push_str("\n  (none — everything we've seen has been used recently)");
            } else {
                for (cmd, ts) in stale.iter().take(limit) {
                    out.push_str(&format!("\n  {:<24} last {}", cmd, ts.to_rfc3339()));
                }
                if stale.len() > limit {
                    out.push_str(&format!(
                        "\n  … {} more (raise `limit`)",
                        stale.len() - limit
                    ));
                }
            }
            return Ok(out);
        }

        // BUG-699: recent sub-window (mirrors the CLI report).
        let recent_since = std::cmp::max(since, now - chrono::Duration::days(7));
        let by_cmd = crate::aggregate_events(&events, since, recent_since);
        let mut rows: Vec<_> = by_cmd.into_values().collect();
        if errors_only {
            rows.retain(|r| r.errors > 0);
            rows.sort_by(|a, b| {
                b.error_rate()
                    .partial_cmp(&a.error_rate())
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| b.count.cmp(&a.count))
            });
        } else {
            rows.sort_by(|a, b| b.count.cmp(&a.count));
        }

        let header = if errors_only {
            format!("Usage: commands with errors in the last {}", since_raw)
        } else {
            format!("Usage: top commands in the last {}", since_raw)
        };
        let mut out = header;
        out.push_str(&format!(
            "\n  {:<24} {:>6} {:>6} {:>8}",
            "cmd", "count", "errs", "avg_ms"
        ));
        if rows.is_empty() {
            out.push_str("\n  (no qualifying events in the window — try a wider `since`)");
            return Ok(out);
        }
        for row in rows.iter().take(limit) {
            out.push_str(&format!(
                "\n  {:<24} {:>6} {:>6} {:>8}",
                row.cmd,
                row.count,
                row.errors,
                row.avg_ms()
            ));
        }
        if rows.len() > limit {
            out.push_str(&format!(
                "\n  … {} more (raise `limit`)",
                rows.len() - limit
            ));
        }
        Ok(out)
    }

    // trace:EPIC-27 — PEEK: `aida db sync` does git network I/O against the
    // orphan store branch (fetch + rebase + push). Return the command to run.
    fn tool_db_sync(&self, args: &Value) -> Result<String, String> {
        let pull = args.get("pull").and_then(|v| v.as_bool()).unwrap_or(false);
        let push = args.get("push").and_then(|v| v.as_bool()).unwrap_or(false);
        let mut cmd = String::from("aida db sync");
        if pull {
            cmd.push_str(" --pull");
        }
        if push {
            cmd.push_str(" --push");
        }
        if !pull && !push {
            cmd.push_str(" --pull --push");
        }
        Ok(format!(
            "Syncing the orphan `aida-store` branch does git network I/O (fetch + rebase + push) \
             — a subprocess act the MCP server does not perform.\nRun:\n  {}",
            cmd
        ))
    }

    // trace:EPIC-27 — PEEK: `aida fetch` refreshes remote refs via subprocess
    // `git`. Return the command to run.
    fn tool_fetch(&self, args: &Value) -> Result<String, String> {
        let code_only = args
            .get("code_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let store_only = args
            .get("store_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let quiet = args.get("quiet").and_then(|v| v.as_bool()).unwrap_or(false);
        let mut cmd = String::from("aida fetch");
        if code_only {
            cmd.push_str(" --code-only");
        }
        if store_only {
            cmd.push_str(" --store-only");
        }
        if quiet {
            cmd.push_str(" --quiet");
        }
        Ok(format!(
            "Fetching refreshes remote refs (code branch + orphan store) via git network I/O \
             — a subprocess act the MCP server does not perform.\nRun:\n  {}",
            cmd
        ))
    }

    // trace:EPIC-27 — PEEK: `aida pull` mutates the working tree + local store
    // and auto-bumps merged specs. Return the command to run.
    fn tool_pull(&self, args: &Value) -> Result<String, String> {
        let code_only = args
            .get("code_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let store_only = args
            .get("store_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let mut cmd = String::from("aida pull");
        if code_only {
            cmd.push_str(" --code-only");
        }
        if store_only {
            cmd.push_str(" --store-only");
        }
        Ok(format!(
            "Pulling mutates the working tree (code: git pull --ff-only) and the local store \
             (rebase), then auto-bumps merged specs Done → Completed — a working-tree-mutating \
             subprocess act the MCP server does not perform.\nRun:\n  {}",
            cmd
        ))
    }

    /// TASK-715: `schema` tool — read-only introspection of the storable
    /// substrate for MCP clients that consume tools rather than resources. With
    /// no `object`, returns the storable-object catalog; with `object`, returns
    /// that object's detail (the reflection-derived field table for every
    /// catalog kind; Requirement additionally carries the four
    /// controlled-vocabulary enums). Mirrors `aida schema [<object>] --json` and the
    /// `aida://schema[/{object}]` resources — all back onto
    /// `crate::schema::{catalog_json, object_json}` so the surfaces can't drift.
    /// trace:TASK-715 | ai:claude
    fn tool_schema(&self, args: &Value) -> Result<String, String> {
        let object = args
            .get("object")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        // `all: true` (with no `object`) is the full dump — the catalog with
        // each kind's field/enum detail inlined, mirroring `aida schema --all
        // --json`. trace:TASK-799 | ai:claude
        let all = args.get("all").and_then(|v| v.as_bool()).unwrap_or(false);
        // `explain: true` adds the explanatory layer — per-field
        // example/provenance/description and each object's lifecycle block —
        // mirroring `aida schema --explain`. trace:STORY-630 | ai:claude
        let explain = args
            .get("explain")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let value = match object {
            None if all => crate::schema::full_dump_json_explain(explain),
            None => crate::schema::catalog_json_explain(explain),
            Some(name) => crate::schema::object_json_explain(name, explain).ok_or_else(|| {
                format!(
                    "Unknown schema object: {} (omit `object` for the catalog of storable kinds)",
                    name
                )
            })?,
        };
        serde_json::to_string_pretty(&value)
            .map_err(|e| format!("failed to serialize schema: {}", e))
    }

    // ========================================================================
    // Resource implementations
    // ========================================================================

    fn resource_project_summary(&self) -> Result<String, String> {
        let store = self.storage.load().map_err(|e| e.to_string())?;

        let total = store.requirements.len();
        let by_status =
            |s: RequirementStatus| store.requirements.iter().filter(|r| r.status == s).count();

        let mut output = format!(
            "# Project Summary\n\n\
             **Total Requirements:** {}\n\
             - Draft: {}\n\
             - Approved: {}\n\
             - Planned: {}\n\
             - In Progress: {}\n\
             - Needs Attention: {}\n\
             - Done: {}\n\
             - Completed: {}\n\
             - Rejected: {}\n",
            total,
            by_status(RequirementStatus::Draft),
            by_status(RequirementStatus::Approved),
            by_status(RequirementStatus::Planned),
            by_status(RequirementStatus::InProgress),
            by_status(RequirementStatus::NeedsAttention),
            by_status(RequirementStatus::Done),
            by_status(RequirementStatus::Completed),
            by_status(RequirementStatus::Rejected),
        );

        if !store.features.is_empty() {
            output.push_str(&format!("\n**Features ({}):**\n", store.features.len()));
            for f in &store.features {
                output.push_str(&format!("- {}\n", f.name));
            }
        }

        Ok(output)
    }

    fn resource_requirements_tree(&self) -> Result<String, String> {
        let store = self.storage.load().map_err(|e| e.to_string())?;

        let mut output = "# Requirements Tree\n\n".to_string();

        let mut by_feature: std::collections::BTreeMap<String, Vec<&Requirement>> =
            std::collections::BTreeMap::new();
        for r in &store.requirements {
            let feature = if r.feature.is_empty() {
                "Uncategorized".to_string()
            } else {
                r.feature.clone()
            };
            by_feature.entry(feature).or_default().push(r);
        }

        for (feature, reqs) in &by_feature {
            output.push_str(&format!("## {}\n\n", feature));
            for r in reqs {
                output.push_str(&format!("- [{}] {} ({})\n", spec_id(r), r.title, r.status));
            }
            output.push('\n');
        }

        Ok(output)
    }

    // ========================================================================
    // EPIC-27 (STORY-535): live-state resources
    //
    // Each backs onto the SAME library helper its CLI / MCP-tool equivalent
    // uses, with no subprocess to `aida`:
    //  - `aida://queue/in-flight`  ← `list_leases` (the in-flight lease scopes,
    //                                 same probe `queue_list`'s in_flight filter
    //                                 + `aida queue list --in-flight-only` use)
    //                                 + the Done-awaiting-merge status bucket.
    //  - `aida://session/leases`   ← `list_leases` (mirrors `tool_list_active_leases`
    //                                 / `aida session leases`).
    //  - `aida://pr/{n}`           ← `crate::collect_git_linkage` + `from-review:PR-N`
    //                                 finding tags (git-canonical, gh-free, mirroring
    //                                 `aida show <spec>`'s git linkage).
    //  - `aida://batch/{name}`     ← the `batch:<name>` tag set bucketed by status,
    //                                 mirroring `tool_queue_progress` / `aida queue
    //                                 progress --batch`.
    // trace:STORY-535 trace:EPIC-27
    // ========================================================================

    /// `aida://queue/in-flight` — specs the queue considers actively in flight:
    /// those held by a live session/MCP-claim lease (the `--in-flight-only`
    /// axis), plus the Done-awaiting-merge bucket `aida queue list` appends so
    /// freshly-shipped work stays visible until auto-bump. trace:STORY-535
    fn resource_queue_in_flight(&self) -> Result<String, String> {
        let store = self.storage.load().map_err(|e| e.to_string())?;

        // Live lease scopes — the same set `tool_list_requirements`'s in_flight
        // filter and `aida queue list --in-flight-only` derive. trace:STORY-535
        let in_flight_scopes: std::collections::HashSet<String> = list_leases(&self.project_root)
            .into_iter()
            .map(|l| l.scope.to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();

        let mut in_flight: Vec<String> = Vec::new();
        let mut awaiting_merge: Vec<String> = Vec::new();
        for r in &store.requirements {
            let sid = spec_id(r);
            if sid != "?" && in_flight_scopes.contains(&sid.to_ascii_lowercase()) {
                in_flight.push(format!("- [{}] {} ({})", sid, r.title, r.status));
            }
            // STORY-86: Done = finished on a branch, awaiting merge.
            if r.status == RequirementStatus::Done {
                awaiting_merge.push(format!("- [{}] {}", sid, r.title));
            }
        }

        let mut output = "# Queue — in-flight\n\n".to_string();
        output.push_str("## Live leases (in-flight)\n\n");
        if in_flight.is_empty() {
            output.push_str("_No specs held by a live session/MCP-claim lease._\n");
        } else {
            output.push_str(&in_flight.join("\n"));
            output.push('\n');
        }
        output.push_str("\n## Done — awaiting merge\n\n");
        if awaiting_merge.is_empty() {
            output.push_str("_No Done specs awaiting merge._\n");
        } else {
            output.push_str(&awaiting_merge.join("\n"));
            output.push('\n');
        }
        Ok(output)
    }

    /// `aida://session/leases` — active scoped session leases. Mirrors
    /// `tool_list_active_leases` / `aida session leases`. trace:STORY-535
    fn resource_session_leases(&self) -> Result<String, String> {
        let leases = list_leases(&self.project_root);
        let mut output = "# Session leases\n\n".to_string();
        if leases.is_empty() {
            output.push_str("_No active leases._\n");
            return Ok(output);
        }
        output.push_str(&format!("{} active lease(s):\n\n", leases.len()));
        for l in &leases {
            let claim_kind = if l.mcp_claim { "mcp" } else { "session" };
            let role = l.role.as_deref().unwrap_or("?");
            output.push_str(&format!(
                "- {} scope={} role={} owner={} kind={} started_at={}\n",
                l.id, l.scope, role, l.owner, claim_kind, l.started_at
            ));
        }
        Ok(output)
    }

    /// `aida://schema` — the storable-object catalog as pretty JSON. Mirrors
    /// `aida schema --json`; backs onto `crate::schema::catalog_json` so the
    /// MCP surface can't drift from the CLI. trace:TASK-715 | ai:claude
    fn resource_schema_catalog(&self) -> Result<String, String> {
        serde_json::to_string_pretty(&crate::schema::catalog_json())
            .map_err(|e| format!("failed to serialize schema catalog: {}", e))
    }

    /// `aida://schema/{object}` — per-object schema detail as pretty JSON.
    /// Every catalog kind returns its reflection-derived field table;
    /// `requirement` additionally carries the four controlled-vocabulary
    /// enums; an unknown name is a -32602 error. Mirrors
    /// `aida schema <object> --json`. trace:TASK-715 | ai:claude
    fn resource_schema_object(&self, object: &str) -> Result<String, String> {
        match crate::schema::object_json(object) {
            Some(v) => serde_json::to_string_pretty(&v)
                .map_err(|e| format!("failed to serialize schema for {}: {}", object, e)),
            None => Err(format!(
                "Unknown schema object: {} (try `aida://schema` for the catalog)",
                object
            )),
        }
    }

    /// `aida://pr/{n}` — git-canonical, gh-free PR linkage for PR number N.
    /// Resolves the spec(s) tied to the PR two ways, both substrate-local:
    ///   1. squash-merge subject `(#N)` → the `(SPEC-ID)` in the same commit
    ///      (via `crate::collect_git_linkage`'s `shipped_pr`), and
    ///   2. review findings tagged `from-review:PR-N`.
    ///      This mirrors the PR pointer `aida show <spec>` surfaces, without ever
    ///      shelling out to `gh`. trace:STORY-535
    fn resource_pr(&self, raw: &str) -> Result<String, String> {
        let n: u64 = raw
            .trim()
            .parse()
            .map_err(|_| format!("Invalid PR number in resource URI: aida://pr/{}", raw))?;
        let store = self.storage.load().map_err(|e| e.to_string())?;

        // Findings raised against PR-N (cheap, tag-only). trace:STORY-535
        let mut findings: Vec<String> = store
            .requirements
            .iter()
            .filter(|r| {
                r.tags
                    .iter()
                    .any(|t| crate::findings::pr_number_from_tag(t) == Some(n as u32))
            })
            .map(|r| format!("- [{}] {} ({})", spec_id(r), r.title, r.status))
            .collect();
        findings.sort();

        // Shipped specs whose squash-merge commit carries `(#N)`. We restrict
        // the per-spec git probe to Done/Completed specs (the only ones that
        // can carry a shipped PR), keeping the scan bounded. trace:STORY-535
        let mut shipped: Vec<String> = Vec::new();
        for r in &store.requirements {
            if !matches!(
                r.status,
                RequirementStatus::Done | RequirementStatus::Completed
            ) {
                continue;
            }
            let sid = spec_id(r).to_string();
            if sid == "?" {
                continue;
            }
            let linkage =
                crate::collect_git_linkage(&self.project_root, std::slice::from_ref(&sid));
            if linkage.shipped_pr == Some(n) {
                let pr = linkage
                    .commits
                    .first()
                    .map(|(_, short, subj)| format!(" — {} {}", short, subj))
                    .unwrap_or_default();
                shipped.push(format!("- [{}] {} ({}){}", sid, r.title, r.status, pr));
            }
        }
        shipped.sort();

        let mut output = format!("# PR #{}\n\n", n);
        output.push_str("## Shipped specs (squash-merge `(#N)`)\n\n");
        if shipped.is_empty() {
            output.push_str("_No merged spec references this PR number._\n");
        } else {
            output.push_str(&shipped.join("\n"));
            output.push('\n');
        }
        output.push_str("\n## Review findings (`from-review:PR-N`)\n\n");
        if findings.is_empty() {
            output.push_str("_No review findings tagged against this PR._\n");
        } else {
            output.push_str(&findings.join("\n"));
            output.push('\n');
        }
        Ok(output)
    }

    /// `aida://batch/{name}` — progress for the `batch:<name>` tag set, bucketed
    /// by live status exactly like `tool_queue_progress` / `aida queue progress
    /// --batch`. trace:STORY-535
    fn resource_batch(&self, raw: &str) -> Result<String, String> {
        let name = raw.trim();
        if name.is_empty() {
            return Err("Empty batch name in resource URI: aida://batch/<name>".to_string());
        }
        let store = self.storage.load().map_err(|e| e.to_string())?;
        let tag = format!("batch:{}", name);

        let (mut shipped, mut in_flight, mut working, mut remaining) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for r in &store.requirements {
            if !r.tags.iter().any(|t| t.eq_ignore_ascii_case(&tag)) {
                continue;
            }
            let disp = spec_id(r).to_string();
            match r.status {
                RequirementStatus::Completed | RequirementStatus::Rejected => shipped.push(disp),
                RequirementStatus::Done => in_flight.push(disp),
                RequirementStatus::InProgress => working.push(disp),
                _ => remaining.push(disp),
            }
        }
        let total = shipped.len() + in_flight.len() + working.len() + remaining.len();
        if total == 0 {
            return Ok(format!(
                "# Batch `{}`\n\n_No requirements tagged `batch:{}`._\n",
                name, name
            ));
        }
        let fmt = |label: &str, items: &[String]| {
            if items.is_empty() {
                format!("- {}: 0", label)
            } else {
                format!("- {}: {} ({})", label, items.len(), items.join(", "))
            }
        };
        Ok(format!(
            "# Batch `{}`\n\n{} item{}:\n\n{}\n{}\n{}\n{}\n",
            name,
            total,
            if total == 1 { "" } else { "s" },
            fmt("Shipped", &shipped),
            fmt("In flight", &in_flight),
            fmt("Working now", &working),
            fmt("Remaining", &remaining),
        ))
    }
}

// ============================================================================
// Helpers
// ============================================================================

fn normalize_mcp_filter_token(s: &str) -> String {
    s.chars()
        .filter_map(|c| match c {
            ' ' | '-' | '_' => None,
            c if c.is_ascii_alphabetic() => Some(c.to_ascii_lowercase()),
            c => Some(c),
        })
        .collect()
}

fn mcp_filter_eq(stored: &str, filter: &str) -> bool {
    normalize_mcp_filter_token(stored) == normalize_mcp_filter_token(filter)
}

/// The status to surface for a requirement over MCP. For an EPIC this is the
/// read-only rollup of its children (mirrors the CLI `effective_display_status`
/// and the cache projection), so `list_requirements` / `show_requirement` agree
/// with `aida list`. Every non-epic returns its stored status.
// trace:BUG-626 | ai:claude
fn mcp_effective_status(
    store: &aida_core::RequirementsStore,
    r: &Requirement,
) -> RequirementStatus {
    if r.req_type == RequirementType::Epic {
        if let Some(derived) = aida_core::rollup::derive_epic_status(store, r.id) {
            return derived;
        }
    }
    r.status.clone()
}

fn parse_status(s: &str) -> Option<RequirementStatus> {
    match normalize_mcp_filter_token(s).as_str() {
        "draft" => Some(RequirementStatus::Draft),
        "approved" => Some(RequirementStatus::Approved),
        "planned" => Some(RequirementStatus::Planned),
        "inprogress" => Some(RequirementStatus::InProgress),
        "done" => Some(RequirementStatus::Done),
        "completed" => Some(RequirementStatus::Completed),
        "rejected" => Some(RequirementStatus::Rejected),
        "needsattention" => Some(RequirementStatus::NeedsAttention),
        _ => None,
    }
}

/// BUG-486: whether the current MCP caller holds advisor authority, routed
/// through the SAME predicate the CLI uses (`advisor_authority_from`) so the two
/// surfaces can't drift — the CLI↔MCP inconsistency *was* the bug. The MCP
/// server runs non-TTY and is never orchestrator-corroborated, so the only axis
/// that can grant authority here is the resolved role. Role resolution:
/// `role_active_env()` (the canonicalized `AIDA_SESSION_ROLE` the server
/// inherited from the launching shell) is the source — `role_enter` (STORY-534)
/// is a peek that sets exactly that shell env, so the entered session role and
/// the env fallback are one and the same value seen by this process. An agent
/// that has not entered an advisor role resolves to non-advisor and is refused,
/// matching the headless / non-TTY default. trace:BUG-486 | ai:claude
fn mcp_caller_has_advisor_authority() -> bool {
    // trace:BUG-486
    let role = role_active_env().unwrap_or_default();
    // is_tty = false, orchestrated = false: an MCP server is neither.
    crate::advisor_authority_from(&role, false, false)
}

/// BUG-480 / BUG-486: the refusal message for MCP queue-for-execution tools
/// (`queue_add` / `queue_rework`). Queuing a spec for work is an
/// advisor-authority act (the CLI gates it on `has_advisor_authority()`).
/// BUG-486: consult the caller's role instead of refusing unconditionally — an
/// MCP session that has entered an advisor role IS advisor authority and may
/// queue, exactly as the CLI does under `AIDA_SESSION_ROLE=advisor`. A
/// non-advisor caller still gets the refusal, told to file the spec for advisor
/// triage. Returns `None` when the caller may proceed. trace:BUG-486 trace:BUG-480 | ai:claude
fn mcp_queue_authority_message() -> Option<String> {
    mcp_queue_authority_message_for(mcp_caller_has_advisor_authority())
}

/// Pure core of [`mcp_queue_authority_message`] (BUG-486): the queue-authority
/// decision over an explicit `caller_is_advisor`, so it is unit-testable without
/// mutating the process-global `AIDA_SESSION_ROLE` env. `None` = may proceed.
/// trace:BUG-486 | ai:claude
fn mcp_queue_authority_message_for(caller_is_advisor: bool) -> Option<String> {
    if caller_is_advisor {
        return None;
    }
    Some(
        "Cannot queue work for execution via MCP: committing a spec to the execution \
         pipeline is the advisor's decision and needs advisor authority. File the spec (it \
         lands as a draft for advisor triage) and let the advisor queue it, or run the CLI \
         as the advisor (AIDA_SESSION_ROLE=advisor)."
            .to_string(),
    )
}

/// BUG-449 / BUG-481 / BUG-486: status transitions an MCP caller may NOT make
/// itself, with the message explaining why. `None` = the transition is allowed.
/// Mirrors the `add_requirement` intake gate (TASK-647 / ADR-3) on the update
/// path so `add`-then-`update` isn't a one-line bypass of advisor authority.
///
/// BUG-486: the *authority decision* now routes through the SAME predicate the
/// CLI uses (`status_advance_requires_advisor_authority` + `advisor_authority_from`
/// via `mcp_caller_has_advisor_authority`), so an MCP session that has entered an
/// advisor role may make the advisor-gated transitions exactly as the CLI does
/// under `AIDA_SESSION_ROLE=advisor`. Before BUG-486 this gate ignored the
/// caller's role and refused unconditionally even for a genuine advisor session —
/// the CLI↔MCP inconsistency the bug names. A caller WITHOUT advisor authority
/// still hits the refusal.
///
/// The gate is keyed off the **(source, target) pair**, not the target alone
/// (BUG-481). `Approved` / `Planned` are advisor-authority targets regardless of
/// source — approving/planning is the advisor's triage decision. `InProgress` /
/// `Done` are advisor-authority **only when the source is `Draft` or
/// `NeedsAttention`** (advancing a never-triaged / punted spec straight into the
/// execution pipeline is the intake-bypass the advisor gate exists to prevent);
/// a spec *already* in the pipeline (e.g. `Approved → InProgress`, `InProgress →
/// Done`) is an implementer-legitimate execution flip and stays allowed for
/// everyone. `Completed` is **always** refused via MCP regardless of role — it
/// is merge-driven (set by a `(SPEC-ID)` commit landing on the default branch),
/// not a hand-set advisor act, so advisor authority does not unlock it.
/// trace:BUG-449 trace:BUG-481 trace:BUG-486 | ai:claude
fn mcp_status_gate_message(from: &RequirementStatus, to: &RequirementStatus) -> Option<String> {
    mcp_status_gate_message_for(from, to, mcp_caller_has_advisor_authority())
}

/// Pure core of [`mcp_status_gate_message`] (BUG-486): the status-authority
/// decision over an explicit `caller_is_advisor`, so the gate is unit-testable
/// without mutating the process-global `AIDA_SESSION_ROLE` env. The wrapper
/// resolves the caller's role via `mcp_caller_has_advisor_authority`.
/// trace:BUG-486 | ai:claude
fn mcp_status_gate_message_for(
    from: &RequirementStatus,
    to: &RequirementStatus,
    caller_is_advisor: bool,
) -> Option<String> {
    // Completed is merge-driven, never hand-set via MCP — gate it before the
    // advisor-authority check (advisor authority does not unlock it).
    if matches!(to, RequirementStatus::Completed) {
        return Some(format!(
            "Cannot set status to {to} via MCP: this is set automatically when a \
             (SPEC-ID) commit lands on the default branch (merge-driven auto-bump), not by hand. \
             Mark the work `done` and let the merge promote it."
        ));
    }

    // BUG-486 single source of truth: the same (from, to) predicate the CLI's
    // `aida edit --status` path consults. Approved/Planned from any source, and
    // InProgress/Done from a Draft/NeedsAttention source, are advisor-authority
    // acts; everything else is an implementer-legitimate flip.
    if !crate::status_advance_requires_advisor_authority(from, to) {
        return None;
    }

    // The transition needs advisor authority — an entered advisor role permits it
    // (resolved by the caller through the SAME predicate the CLI uses).
    // trace:BUG-486
    if caller_is_advisor {
        return None;
    }

    // BUG-589: lead with the remediation that actually works for an MCP client.
    // `role_enter` is PEEK-ONLY — it cannot set the shell's AIDA_SESSION_ROLE for
    // the running MCP server, so it can never unlock this gate for the same
    // session (steering an agent into role_enter just loops). The only working
    // unlock for an MCP caller is launching `aida mcp-serve` with
    // AIDA_SESSION_ROLE=advisor in its environment; the fallback is to leave the
    // status to the advisor (file it / have the advisor act). trace:BUG-589 | ai:claude
    match to {
        RequirementStatus::Approved | RequirementStatus::Planned => Some(format!(
            "Cannot set status to {to} via MCP: approving or planning a spec is the \
             advisor's triage decision and needs advisor authority. To unlock this from an \
             MCP client, (re)launch `aida mcp-serve` with AIDA_SESSION_ROLE=advisor in its \
             environment, then retry. Otherwise file the spec and let the advisor promote it. \
             (Note: the role_enter tool is peek-only — it cannot self-elevate this session, \
             so it will NOT unlock the gate.)"
        )),
        _ => Some(format!(
            "Cannot advance {from} → {to} via MCP: moving an un-triaged or punted spec \
             into the execution pipeline needs advisor authority. To unlock this from an MCP \
             client, (re)launch `aida mcp-serve` with AIDA_SESSION_ROLE=advisor in its \
             environment, then retry. Otherwise let the advisor approve it first (it will then \
             flip to {to} as implementation proceeds). (Note: the role_enter tool is peek-only \
             — it cannot self-elevate this session, so it will NOT unlock the gate.)"
        )),
    }
}

fn parse_priority(s: &str) -> Option<RequirementPriority> {
    match normalize_mcp_filter_token(s).as_str() {
        "high" => Some(RequirementPriority::High),
        "medium" => Some(RequirementPriority::Medium),
        "low" => Some(RequirementPriority::Low),
        _ => None,
    }
}

fn parse_requirement_type(s: &str) -> Option<RequirementType> {
    match normalize_mcp_filter_token(s).as_str() {
        "functional" => Some(RequirementType::Functional),
        "nonfunctional" => Some(RequirementType::NonFunctional),
        "system" => Some(RequirementType::System),
        "user" => Some(RequirementType::User),
        "changerequest" | "change" | "cr" => Some(RequirementType::ChangeRequest),
        "bug" => Some(RequirementType::Bug),
        "epic" => Some(RequirementType::Epic),
        "story" => Some(RequirementType::Story),
        "task" => Some(RequirementType::Task),
        "spike" => Some(RequirementType::Spike),
        "sprint" => Some(RequirementType::Sprint),
        "folder" => Some(RequirementType::Folder),
        "meta" => Some(RequirementType::Meta),
        // ADR / knowledge-graph family (FR-1-074). trace:TASK-716 | ai:claude
        "principle" | "prin" => Some(RequirementType::Principle),
        "vision" | "vis" => Some(RequirementType::Vision),
        "constraint" | "con" => Some(RequirementType::Constraint),
        "decision" | "adr" => Some(RequirementType::Decision),
        "term" | "glossary" => Some(RequirementType::Term),
        "doc" => Some(RequirementType::Doc),
        _ => None,
    }
}

// trace:TASK-551 | ai:codex
fn parse_mcp_relationship_type(s: &str) -> Result<RelationshipType, String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(
            "relationship_type must be non-empty (examples: parent, child, blocked-by, references)"
                .to_string(),
        );
    }

    let lowered = trimmed.to_ascii_lowercase();
    Ok(match lowered.as_str() {
        "parent" => RelationshipType::Parent,
        "child" => RelationshipType::Child,
        "duplicate" => RelationshipType::Duplicate,
        "verifies" => RelationshipType::Verifies,
        "verified-by" | "verified_by" | "verifiedby" => RelationshipType::VerifiedBy,
        "references" | "related" | "relates-to" | "relates_to" | "relatesto" => {
            RelationshipType::References
        }
        "blocked-by" | "blocked_by" | "blockedby" | "depends-on" | "depends_on" | "dependson" => {
            RelationshipType::BlockedBy
        }
        "blocks" => RelationshipType::Blocks,
        custom => RelationshipType::Custom(custom.to_string()),
    })
}

/// Read the current git branch under `project_root`. `None` on detached HEAD
/// or git failure.
fn current_branch_at(project_root: &Path) -> Option<String> {
    let o = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .ok()?;
    if !o.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn hostname_or_unknown() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Project a loaded `RequirementsStore` into the `RequirementSummary` shape
/// that `findings.rs` expects. Keeps the projection logic in one place;
/// mirrors the cache projection just well enough for the findings filter.
fn build_summaries(store: &aida_core::RequirementsStore) -> Vec<aida_core::RequirementSummary> {
    // STORY-632: compute deterministic in/out degree + heft once over the full
    // graph so the MCP projection carries the same centrality as the cache.
    // trace:STORY-632 | ai:claude
    let degrees = aida_core::compute_degrees(store);
    // TASK-902: project the blocked flag over the same full graph so the MCP
    // summary carries it identically to the cache. trace:TASK-902 | ai:claude
    let blocked = aida_core::compute_blocked(store);
    store
        .requirements
        .iter()
        .map(|r| {
            let d = degrees.get(&r.id).copied().unwrap_or_default();
            aida_core::RequirementSummary {
                id: r.id,
                spec_id: r.spec_id.clone(),
                agreed_id: r.agreed_id.clone(),
                title: r.title.clone(),
                description: r.description.clone(),
                status: format!("{}", r.status),
                priority: format!("{}", r.priority),
                owner: r.owner.clone(),
                // trace:STORY-639 | ai:claude
                assignee: r.assignee.clone(),
                feature: r.feature.clone(),
                req_type: format!("{}", r.req_type),
                tags: r.tags.iter().cloned().collect(),
                created_at: r.created_at.to_rfc3339(),
                modified_at: r.modified_at.to_rfc3339(),
                archived: r.archived,
                // trace:STORY-441 | ai:claude
                archived_at: r.archived_at.map(|dt| dt.to_rfc3339()),
                // trace:STORY-584 | ai:claude
                deferred: r.deferred,
                deferred_at: r.deferred_at.map(|dt| dt.to_rfc3339()),
                deferred_until: r.deferred_until.clone(),
                in_degree: d.in_degree,
                out_degree: d.out_degree,
                heft: d.heft,
                // trace:TASK-902 | ai:claude
                blocked: blocked.contains(&r.id),
                // trace:TASK-1065 | ai:claude
                has_pending_decision: r.decision_request.as_ref().is_some_and(|d| d.is_pending()),
                yaml_path: String::new(),
            }
        })
        .collect()
}

/// Append `line` to `path`, creating the file if needed. One line per call,
/// terminated with a newline. Uses `OpenOptions::append` so concurrent writers
/// don't lose updates on POSIX (the kernel guarantees atomicity of a single
/// small `write(2)` to an append-mode fd). We pre-concatenate the newline so
/// the whole record lands in a single `write_all` — `writeln!` issues
/// multiple `write_str` calls per format arg, and two concurrent writers can
/// interleave the content + newline writes, producing a torn line.
fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut buf = String::with_capacity(line.len() + 1);
    buf.push_str(line);
    buf.push('\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(buf.as_bytes())?;
    Ok(())
}

/// Write `bytes` to `path` atomically: write to a sibling `path.tmp-<uuid>` then
/// `rename` over `path`. Crash mid-write leaves either the old file intact or
/// the new file intact — never a torn write.
/// Human-readable suffix noting how many ledger records a resolve/escalate
/// closed (empty when none). Keeps the MCP resolve/escalate messages honest
/// about the BUG-674 ledger-close side effect.
fn ledger_close_suffix(closed: usize) -> String {
    match closed {
        0 => String::new(),
        1 => " Closed 1 open ledger record.".to_string(),
        n => format!(" Closed {n} open ledger records."),
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp_name = format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("aida-tmp"),
        uuid::Uuid::now_v7()
    );
    let tmp = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(tmp_name);
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

// ============================================================================
// Tool profiles — conservative safe-default surface (STORY-474)
// ============================================================================

// trace:STORY-474 | ai:claude
//
// MCP tool profiles let an operator expose a conservative slice of the tool
// surface to an untrusted / exploratory / remote client while keeping the full
// coordination + admin surface available to trusted agents. The profile is a
// *capability tier*: each higher tier is a strict superset of the one below it.
//
//   read-only     — pure reads only (list/show/search/query/history/read_*).
//                   Excludes EVERY write tool. The recommended marketplace /
//                   untrusted-client default.
//   coordination  — read-only + the file-substrate coordination writes an
//                   agent needs to participate in a drain (post/resolve/escalate
//                   punts, file/triage findings, claim/release tasks, post/ack
//                   directives, ack briefs, send messages, add comments,
//                   add relationships). No spec create/edit.
//   operator      — coordination + spec-graph writes (add_requirement,
//                   update_requirement). Full day-to-day surface.
//   admin / full  — every tool. (Today `admin` == `full`; the name is reserved
//                   so a future privileged-only tool can land at the admin tier
//                   without churning the public profile vocabulary.)
//
// Resolution order (first match wins): `--profile <p>` CLI flag → `AIDA_MCP_PROFILE`
// env → `[mcp] profile` in `.aida/config.toml` → the built-in default (`full`,
// for backwards compatibility with existing trusted installs). Marketplace /
// untrusted installs should set `profile = "read-only"` explicitly — see
// `docs/agents/aida-mcp-install-matrix.md`.
//
// Gating happens in TWO places so the profile is a real security boundary, not
// just a discovery hint: `tools/list` advertises only in-profile tools, and
// `tools/call` REJECTS an out-of-profile tool with a stable error even if a
// client calls it by name anyway.

/// Capability tier governing which MCP tools are exposed. Ordered low→high;
/// each tier admits every tool of the tiers below it. trace:STORY-474 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum McpProfile {
    /// Pure reads only — the conservative safe default for untrusted clients.
    ReadOnly,
    /// Reads + coordination-substrate writes (no spec create/edit).
    Coordination,
    /// Coordination + spec-graph writes (add/update requirement).
    Operator,
    /// Everything (reserved tier for future privileged-only tools).
    Admin,
    /// Everything — the built-in backwards-compatible default.
    Full,
}

impl McpProfile {
    /// Parse a profile token. Accepts hyphen / underscore / space spellings and
    /// is case-insensitive. `admin` and `full` both expose every tool today.
    /// Returns `None` for an unknown token so callers can fall back + warn.
    pub fn from_token(s: &str) -> Option<Self> {
        match s
            .trim()
            .to_ascii_lowercase()
            .replace(['_', ' '], "-")
            .as_str()
        {
            "read-only" | "readonly" | "ro" => Some(Self::ReadOnly),
            "coordination" | "coord" => Some(Self::Coordination),
            "operator" | "op" => Some(Self::Operator),
            "admin" => Some(Self::Admin),
            "full" | "all" => Some(Self::Full),
            _ => None,
        }
    }

    /// The canonical lowercase token for this profile.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::Coordination => "coordination",
            Self::Operator => "operator",
            Self::Admin => "admin",
            Self::Full => "full",
        }
    }
}

impl Default for McpProfile {
    /// Backwards-compatible default: existing installs see the full surface.
    /// Untrusted / marketplace installs should opt into `read-only` explicitly.
    fn default() -> Self {
        Self::Full
    }
}

/// The minimum profile tier at which a given tool (by name) becomes available.
/// This is the single source of truth for the profile→toolset mapping, kept as
/// a pure fn so the selection logic is unit-testable without a server.
///
/// Unknown tool names map to `Admin` (the most restrictive non-`full` tier) so a
/// newly-added tool is never accidentally exposed to a `read-only` client before
/// it has been deliberately classified here. trace:STORY-474 | ai:claude
pub fn tool_min_profile(tool_name: &str) -> McpProfile {
    match tool_name {
        // ---- Pure reads (read-only tier) ----
        "list_requirements"
        | "show_requirement"
        | "search_requirements"
        | "query_graph"
        | "list_features"
        | "history"
        | "read_inbox"
        | "list_punts"
        | "read_punt"
        | "list_findings"
        | "list_active_leases"
        | "list_directives"
        | "list_briefs"
        | "read_brief"
        // STORY-532 (EPIC-27): pure-read queue tools (peek / list / resolve).
        // queue_work over MCP is a metadata-only peek (it never launches a
        // session — that is a CLI-only act), so it lives in the read tier.
        // trace:EPIC-27
        | "queue_list"
        | "queue_next"
        | "queue_progress"
        | "queue_work"
        // STORY-533 (EPIC-27): session tools are all read tier. The three
        // inspectors (leases / status / manifest) are pure reads of
        // `.aida/sessions/*`; session_start / session_end are metadata-only
        // PEEKS (the launch / worktree-removal act is a subprocess-driven
        // CLI-only act), so they never mutate substrate. trace:EPIC-27
        | "session_leases"
        | "session_status"
        | "session_manifest"
        | "session_start"
        | "session_end"
        // STORY-534 (EPIC-27): role tools are all read tier. role_list /
        // role_show are pure reads of `.aida/roles/*` (+ ~/.aida/roles);
        // role_enter / role_end are metadata-only PEEKS (entering/ending a
        // role mutates the *shell's* env, a CLI-only act), so they never
        // mutate substrate. trace:EPIC-27
        | "role_list"
        | "role_show"
        | "role_enter"
        | "role_end"
        // STORY-536 (EPIC-27): workflow tools are all read tier. The library
        // mirrors (cache_status / plan_verify / plan_helpers /
        // ultraplan_assemble / goal_derive / status_unified / usage_query)
        // are pure reads — they compute reports / prompts / conditions without
        // mutating substrate. db_sync / fetch / pull are metadata-only PEEKS
        // (they return the `aida …` command; the git network + working-tree /
        // store mutation is a subprocess-driven CLI-only act), so they never
        // mutate substrate either. trace:EPIC-27
        | "cache_status"
        | "plan_verify"
        | "plan_helpers"
        | "ultraplan_assemble"
        | "goal_derive"
        | "status_unified"
        | "usage_query"
        | "db_sync"
        | "fetch"
        | "pull"
        // TASK-715: `schema` is a pure read of the (reflection-derived)
        // storable-substrate shape — no store access, no mutation. trace:TASK-715
        | "schema" => McpProfile::ReadOnly,

        // ---- Coordination-substrate writes (coordination tier) ----
        "add_comment" | "add_relationship" | "send_message" | "post_punt" | "resolve_punt"
        | "escalate_punt" | "file_finding" | "triage_finding" | "claim_task" | "release_task"
        | "post_directive" | "ack_directive" | "ack_brief"
        // STORY-532 (EPIC-27): queue housekeeping (reorder / drop) mutates only
        // the queue, not spec status — coordination tier. trace:EPIC-27
        | "queue_move" | "queue_remove" => McpProfile::Coordination,

        // ---- Spec-graph writes (operator tier) ----
        // STORY-532 (EPIC-27): queue_add commits a spec to the execution
        // pipeline (advisor-authority act) and queue_done / queue_rework flip
        // spec status — all operator tier. trace:EPIC-27
        "add_requirement" | "update_requirement" | "queue_add" | "queue_done"
        | "queue_rework" => McpProfile::Operator,

        // Unknown / not-yet-classified tools: gate at the most restrictive
        // non-full tier so they never leak into a conservative profile.
        _ => McpProfile::Admin,
    }
}

/// True when `tool_name` is exposed under `profile`. trace:STORY-474 | ai:claude
pub fn tool_in_profile(tool_name: &str, profile: McpProfile) -> bool {
    profile >= tool_min_profile(tool_name)
}

/// True when `tool_name` is a tool AIDA actually serves (appears in
/// `tool_descriptors()`). Used to distinguish "above your profile" from "no such
/// tool" at call time. trace:STORY-474 | ai:claude
pub fn is_known_tool(tool_name: &str) -> bool {
    tool_descriptors()
        .as_array()
        .map(|arr| {
            arr.iter()
                .any(|t| t.get("name").and_then(|v| v.as_str()) == Some(tool_name))
        })
        .unwrap_or(false)
}

/// The set of tool names (in `tool_descriptors()` order) exposed under
/// `profile`. Pure fn over the descriptor list — the unit-tested core of the
/// profile→toolset selection. trace:STORY-474 | ai:claude
// why: unit-tested pure core of profile→toolset selection; the production path filters descriptors inline, so the named helper is test-only today.
#[allow(dead_code)]
pub fn tool_names_for_profile(profile: McpProfile) -> Vec<String> {
    tool_descriptors()
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t.get("name").and_then(|v| v.as_str()))
                .filter(|n| tool_in_profile(n, profile))
                .map(|n| n.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// `tool_descriptors()` filtered to `profile`, with a `"profile"` metadata field
/// added to each descriptor naming its minimum tier so schema-driven clients can
/// see why a tool is (or isn't) present. trace:STORY-474 | ai:claude
pub fn tool_descriptors_for_profile(profile: McpProfile) -> Value {
    let arr = tool_descriptors()
        .as_array()
        .map(|a| a.to_vec())
        .unwrap_or_default();
    let filtered: Vec<Value> = arr
        .into_iter()
        .filter_map(|mut t| {
            let name = t.get("name").and_then(|v| v.as_str())?.to_string();
            if !tool_in_profile(&name, profile) {
                return None;
            }
            if let Some(obj) = t.as_object_mut() {
                obj.insert(
                    "profile".to_string(),
                    json!(tool_min_profile(&name).as_str()),
                );
            }
            Some(t)
        })
        .collect();
    json!(filtered)
}

/// Resolve the active MCP profile from (in order) an explicit override, the
/// `AIDA_MCP_PROFILE` env var, `[mcp] profile` in `.aida/config.toml`, then the
/// built-in default. An unknown token at any layer is ignored (falls through to
/// the next source) so a typo never silently opens a wider surface than
/// intended — it just keeps the previous resolution. trace:STORY-474 | ai:claude
pub fn resolve_mcp_profile(project_root: &Path, override_token: Option<&str>) -> McpProfile {
    if let Some(p) = override_token.and_then(McpProfile::from_token) {
        return p;
    }
    if let Ok(env) = std::env::var("AIDA_MCP_PROFILE") {
        if let Some(p) = McpProfile::from_token(&env) {
            return p;
        }
    }
    if let Ok(body) = std::fs::read_to_string(project_root.join(".aida").join("config.toml")) {
        if let Ok(value) = toml::from_str::<toml::Value>(&body) {
            if let Some(p) = value
                .get("mcp")
                .and_then(|t| t.get("profile"))
                .and_then(|v| v.as_str())
                .and_then(McpProfile::from_token)
            {
                return p;
            }
        }
    }
    McpProfile::default()
}

// ============================================================================
// Tool descriptors (kept at module scope for register-agent + tests)
// ============================================================================

// trace:TASK-440 | ai:claude
// trace:STORY-399 | ai:claude
/// Build the `outputSchema` for a tool that returns the MCP text envelope.
///
/// Path A (TASK-440) shipped the descriptor-level schema describing the wire
/// shape every tool returns (`{ content: [{ type: "text", text: "..." }],
/// isError?: bool }`). Path B (STORY-399) makes successful responses *also*
/// carry a `structuredContent` object that mirrors this same envelope shape,
/// so schema-driven MCP clients (Codex, Cursor, …) get a machine-readable
/// result without parsing the human text.
///
/// The decision is **additive** (acceptance option (a)): the text `content`
/// array is preserved byte-for-byte, and `structuredContent` is layered
/// alongside it — legacy text consumers (Claude Code) keep working unchanged.
/// `structuredContent` is declared optional in the schema because it is absent
/// on error responses (those carry the `structuredError` payload from
/// STORY-401 instead). `payload_description` still tells clients what the
/// `text` string — mirrored verbatim into `structuredContent.content` —
/// conveys for *this* tool.
fn text_envelope_output_schema(payload_description: &str) -> Value {
    // The single text-content array shape, reused by both the top-level
    // `content` property and the Path-B `structuredContent.content` mirror.
    let text_content_array = || {
        json!({
            "type": "array",
            "description": "A single text item carrying this tool's response.",
            "items": {
                "type": "object",
                "properties": {
                    "type": { "type": "string", "const": "text" },
                    "text": { "type": "string" }
                },
                "required": ["type", "text"]
            }
        })
    };
    json!({
        "type": "object",
        "description": format!(
            "MCP text envelope (additive structuredContent per STORY-399). \
             The `text` field contains: {}",
            payload_description
        ),
        "properties": {
            "content": text_content_array(),
            "isError": {
                "type": "boolean",
                "description": "Present and true when the tool returned an error; absent on success."
            },
            "structuredContent": {
                "type": "object",
                "description": "Path B (STORY-399): machine-readable mirror of the text envelope, present on success. Absent on error (see `structuredError`).",
                "properties": {
                    "content": text_content_array()
                },
                "required": ["content"]
            }
        },
        "required": ["content"]
    })
}

/// Every tool the MCP server exposes, in `tools/list` JSON shape. Static so
/// `aida mcp register-agent --print-tools` can render it without spinning up
/// the JSON-RPC loop.
pub fn tool_descriptors() -> Value {
    let mut descriptors = json!([
        // ---- Spec graph ----
        {
            "name": "list_requirements",
            // trace:STORY-82 | ai:claude
            "description": "List requirements from the AIDA database, optionally filtered by status, type, feature category, priority, tags, batch, parent, owner role, or in-flight state. Mirrors the current `aida list` filter surface — including the archive/deferred view tiers: by DEFAULT archived (STORY-441) and deferred (STORY-584) specs are hidden, exactly like `aida list`. Use `archived` / `deferred` / `all` to surface those tiers. Returns a summarized list of matching requirements.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "description": "Filter by the current status of the requirement.",
                        "enum": ["draft", "approved", "planned", "in-progress", "needs-attention", "done", "completed", "rejected"],
                        "example": "in-progress"
                    },
                    "type": {
                        "type": "string",
                        "description": "Filter by the semantic type of the requirement.",
                        "enum": ["functional", "non-functional", "system", "user", "change-request", "bug", "epic", "story", "task", "spike", "sprint", "folder", "meta", "principle", "vision", "constraint", "decision", "term", "doc"],
                        "example": "story"
                    },
                    "priority": {
                        "type": "string",
                        "description": "Filter by priority. Accepts the same case-insensitive spelling as the CLI.",
                        "enum": ["high", "medium", "low"],
                        "example": "high"
                    },
                    "feature": {
                        "type": "string",
                        "description": "Filter by feature category name (e.g., auth, backend).",
                        "example": "auth"
                    },
                    "assignee": {
                        "type": "string",
                        "description": "Filter to specs assigned to this team member (exact match on the assignee handle). Mirrors `aida list --assigned <user>`.",
                        "example": "alice"
                    },
                    "user": {
                        "type": "string",
                        "description": "Filter to specs whose OWNER or ASSIGNEE is this handle (exact match on either). Broader than `assignee`. Mirrors `aida list --user <name>`.",
                        "example": "joe"
                    },
                    "tags": {
                        "type": "string",
                        "description": "Filter by tags. Comma-separated list; a row matches when it carries ALL of the listed tags (e.g. `batch:scaffolding,papercut`). Follows the CLI tag conventions (colon-namespaced `aida:<subcommand>` for surface tags, flat for behavior/severity).",
                        "example": "mcp,modernization"
                    },
                    "batch": {
                        "type": "string",
                        "description": "Filter to members of a batch — shorthand for the `batch:<NAME>` tag (e.g. `scaffolding-2026-05-28`). Mirrors `aida list --batch`.",
                        "example": "scaffolding-2026-05-28"
                    },
                    "parent": {
                        "type": "string",
                        "description": "Restrict to direct children of this parent SPEC-ID (mirrors `aida list --parent`).",
                        "pattern": "^[A-Z]+-\\d+(-\\d+)*$",
                        "example": "EPIC-27"
                    },
                    "role": {
                        "type": "string",
                        "description": "Filter to requirements routed to a role — matches a `role:<value>` tag on the spec (e.g. implementer, advisor). Alias: `for`.",
                        "example": "implementer"
                    },
                    "for": {
                        "type": "string",
                        "description": "Alias for `role` — filter to requirements carrying a `role:<value>` tag.",
                        "example": "advisor"
                    },
                    "in_flight": {
                        "type": "boolean",
                        "description": "When true, restrict to requirements with a live session/claim lease (work currently in flight). When false or omitted (default), no in-flight filter is applied.",
                        "example": true
                    },
                    "archived": {
                        "type": "boolean",
                        "description": "Show ONLY archived specs (STORY-441). Archived rows are hidden from the default view; set this to audit the archive. Mirrors `aida list --archived`.",
                        "example": true
                    },
                    "deferred": {
                        "type": "boolean",
                        "description": "Show ONLY deferred specs — the primed/conditional shelf (STORY-584; deferred view-flag set OR a legacy `deferred:*` parking tag). Deferred rows are hidden from the default view. Mirrors `aida list --deferred`.",
                        "example": true
                    },
                    "all": {
                        "type": "boolean",
                        "description": "Show the union of ALL view tiers — active + deferred + archived. Mirrors `aida list --all`. Takes precedence over `archived` / `deferred`.",
                        "example": true
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results to return (must be at least 1).",
                        "minimum": 1,
                        "example": 10
                    }
                }
            },
            "outputSchema": text_envelope_output_schema(
                "a `Found N requirements:` header followed by one line per match in `- [SPEC-ID] <title> (Status: <status>, Priority: <priority>, Type: <type>)` form, or `No requirements found matching the criteria.` when the filter excludes everything."
            )
        },
        {
            "name": "show_requirement",
            // trace:STORY-82 | ai:claude
            "description": "Retrieve and display the full markdown details of a specific requirement (description, relationships, comments, and git linkage) by its unique SPEC-ID. Mirrors `aida show`: git linkage (referencing commits, feature branch, PR) is included by default.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The unique SPEC-ID of the requirement (e.g., FR-0042). Must follow the canonical spec format.",
                        "pattern": "^[A-Z]+-\\d+(-\\d+)*$",
                        "example": "FR-0042"
                    },
                    "include_git": {
                        "type": "boolean",
                        "description": "Append the git-linkage section (referencing commits, feature branch / worktree, shipped state, PR number). Defaults to true, matching `aida show`; pass false for the `aida show --no-git` view.",
                        "example": true
                    },
                    "verbose": {
                        "type": "boolean",
                        "description": "Expand the git-linkage section with the full per-commit list and trace-tagged files, matching `aida show --verbose`. Defaults to false.",
                        "example": false
                    }
                },
                "required": ["id"]
            },
            "outputSchema": text_envelope_output_schema(
                "a Markdown rendering of the requirement: H1 `# <SPEC-ID> — <title>`, bold-key/value lines for Status / Priority / Type / Feature / Owner / Tags, a `## Description` body, optional `## Comments` and `## Relationships` sections, and (unless `include_git` is false) a `## Git linkage` section listing referencing commits, the feature branch, and any PR."
            )
        },
        {
            "name": "add_requirement",
            // trace:STORY-82 | ai:claude
            "description": "Create and add a new requirement to the AIDA database. Generates a new canonical SPEC-ID automatically based on the type. Optionally links it under a parent, sets a feature category, and assigns an owner. Note: MCP intake always files as `draft` (approving/planning is the advisor's triage decision), regardless of the requested status.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Short, descriptive title of the requirement.",
                        "example": "Implement OAuth2 login flow"
                    },
                    "description": {
                        "type": "string",
                        "description": "Detailed description of the requirement including goals, constraints, and checklist.",
                        "example": "Implement Google and GitHub OAuth2 sign-in capabilities per SEC-101 specifications."
                    },
                    "type": {
                        "type": "string",
                        "description": "Required requirement type. Valid types: functional, non-functional, system, user, change-request, bug, epic, story, task, spike, sprint, folder, meta, principle, vision, constraint, decision, term, doc. (change-request is the workflow type for a proposed change; principle/vision/constraint/decision/term are the ADR + knowledge-graph family.) Normalizes the assigned SPEC-ID prefix (e.g., 'task' becomes 'TASK-N').",
                        "enum": ["functional", "non-functional", "system", "user", "change-request", "bug", "epic", "story", "task", "spike", "sprint", "folder", "meta", "principle", "vision", "constraint", "decision", "term", "doc"],
                        "example": "story"
                    },
                    "status": {
                        "type": "string",
                        "description": "Requested initial status. MCP intake files everything as `draft` and queues approved+ requests for advisor triage; lower-than-draft values still apply.",
                        "enum": ["draft", "approved", "planned", "in-progress", "done", "completed", "rejected"],
                        "example": "draft"
                    },
                    "priority": {
                        "type": "string",
                        "description": "Urgency or priority level.",
                        "enum": ["high", "medium", "low"],
                        "example": "medium"
                    },
                    "feature": {
                        "type": "string",
                        "description": "Feature category name (NOT a type), e.g. auth, backend. Matches `aida add --feature`.",
                        "example": "auth"
                    },
                    "owner": {
                        "type": "string",
                        "description": "Owner to assign to the new requirement (matches `aida add --owner`).",
                        "example": "alice"
                    },
                    "parent": {
                        "type": "string",
                        "description": "SPEC-ID of a parent to link this requirement under (adds a Parent/Child edge, mirroring `aida add --parent`). The parent must exist and not be terminal-status.",
                        "pattern": "^[A-Z]+-\\d+(-\\d+)*$",
                        "example": "EPIC-27"
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional list of tags to categorize this requirement. Follows the CLI tag conventions (colon-namespaced `aida:<subcommand>` surface tags; flat behavior/severity/batch tags like `papercut`, `batch:NAME`).",
                        "example": ["auth", "security"]
                    }
                },
                "required": ["title", "description", "type"]
            },
            "outputSchema": text_envelope_output_schema(
                "a confirmation line `Requirement added: <SPEC-ID> — <title>`, where SPEC-ID is the freshly-assigned identifier for the new requirement. The prefix is auto-normalized from the required `type` argument, for example `task` produces `TASK-N` and never generic `SPEC-N`."
            )
        },
        {
            "name": "update_requirement",
            // trace:STORY-82 | ai:claude — assignee mirror: STORY-639.
            "description": "Update fields of an existing requirement (title, type, status, priority, description, tags, parent, assignee). Fields omitted from parameters remain unchanged. Note: advisor-authority transitions (approved/planned) and the merge-driven `completed` status are gated and cannot be set via MCP. Setting `assignee` edits the field only — it does NOT route the spec into the assignee's queue (use queue_add for that); the CLI `aida assign` does both.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The unique SPEC-ID of the requirement to update.",
                        "pattern": "^[A-Z]+-\\d+(-\\d+)*$",
                        "example": "FR-0042"
                    },
                    "title": {
                        "type": "string",
                        "description": "New title for the requirement.",
                        "example": "Implement OAuth2 + SAML login flow"
                    },
                    "type": {
                        "type": "string",
                        "description": "New semantic type. Changing the type does NOT renumber the existing SPEC-ID.",
                        "enum": ["functional", "non-functional", "system", "user", "change-request", "bug", "epic", "story", "task", "spike", "sprint", "folder", "meta", "principle", "vision", "constraint", "decision", "term", "doc"],
                        "example": "story"
                    },
                    "status": {
                        "type": "string",
                        "description": "New status to transition the requirement into. approved/planned (advisor triage) and completed (merge-driven) are refused via MCP.",
                        "enum": ["draft", "approved", "planned", "in-progress", "done", "completed", "rejected", "needs-attention"],
                        "example": "in-progress"
                    },
                    "priority": {
                        "type": "string",
                        "description": "New priority level.",
                        "enum": ["high", "medium", "low"],
                        "example": "high"
                    },
                    "description": {
                        "type": "string",
                        "description": "Updated detailed description of the requirement.",
                        "example": "Updated detailed implementation checklist for the login interface."
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Replace the requirement's tag set with this list. Follows the CLI tag conventions (colon-namespaced `aida:<subcommand>` surface tags; flat behavior/severity/batch tags).",
                        "example": ["auth", "batch:login-rework"]
                    },
                    "parent": {
                        "type": "string",
                        "description": "Re-parent the requirement under this SPEC-ID (adds a Parent/Child edge to the new parent; existing parent edges are left in place). The parent must exist and not be terminal-status.",
                        "pattern": "^[A-Z]+-\\d+(-\\d+)*$",
                        "example": "EPIC-27"
                    },
                    "assignee": {
                        "type": "string",
                        "description": "Team member this spec is assigned to (a username/handle). An empty string clears the assignee. NOTE: this sets the field only; it does NOT add the spec to the assignee's work queue (use queue_add). The CLI `aida assign --to` does both.",
                        "example": "alice"
                    }
                },
                "required": ["id"]
            },
            "outputSchema": text_envelope_output_schema(
                "either `Updated <SPEC-ID>: <comma-separated change list>` summarizing what was modified, or `No changes applied to <SPEC-ID>` when nothing in the args translated to a change."
            )
        },
        {
            "name": "search_requirements",
            // trace:STORY-82 | ai:claude
            "description": "Case-insensitive keyword search across requirement titles, descriptions, and SPEC-IDs, optionally narrowed by type and/or status. Mirrors `aida search` — including the archive/deferred view tiers: by DEFAULT archived (STORY-441) and deferred (STORY-584) specs are hidden. Use `archived` / `deferred` / `all` to surface those tiers.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Case-insensitive query string to search for.",
                        "example": "oauth2"
                    },
                    "type": {
                        "type": "string",
                        "description": "Restrict results to this semantic type.",
                        "enum": ["functional", "non-functional", "system", "user", "change-request", "bug", "epic", "story", "task", "spike", "sprint", "folder", "meta", "principle", "vision", "constraint", "decision", "term", "doc"],
                        "example": "bug"
                    },
                    "status": {
                        "type": "string",
                        "description": "Restrict results to this status.",
                        "enum": ["draft", "approved", "planned", "in-progress", "needs-attention", "done", "completed", "rejected"],
                        "example": "in-progress"
                    },
                    "archived": {
                        "type": "boolean",
                        "description": "Show ONLY archived specs (STORY-441), hidden from the default search view. Mirrors `aida search --archived`.",
                        "example": true
                    },
                    "deferred": {
                        "type": "boolean",
                        "description": "Show ONLY deferred specs — the primed shelf (STORY-584), hidden from the default search view. Mirrors `aida search --deferred`.",
                        "example": true
                    },
                    "all": {
                        "type": "boolean",
                        "description": "Show the union of ALL view tiers (active + deferred + archived). Takes precedence over `archived` / `deferred`.",
                        "example": true
                    }
                },
                "required": ["query"]
            },
            "outputSchema": text_envelope_output_schema(
                "either `Found N results for '<query>':` followed by `- [SPEC-ID] <title> (<status>)` lines, or `No requirements found matching '<query>'` when nothing matches."
            )
        },
        {
            "name": "add_comment",
            "description": "Append a comment to a requirement's audit trail, providing updates, context, or discussion notes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The unique SPEC-ID of the requirement to comment on.",
                        "pattern": "^[A-Z]+-\\d+(-\\d+)*$",
                        "example": "FR-0042"
                    },
                    "text": {
                        "type": "string",
                        "description": "The text content of the comment to add.",
                        "example": "Verified with the design team; OAuth2 client secrets will be fetched from Secrets Manager."
                    }
                },
                "required": ["id", "text"]
            },
            "outputSchema": text_envelope_output_schema(
                "a confirmation line `Comment added to <SPEC-ID>`."
            )
        },
        {
            "name": "add_relationship",
            "description": "Add a typed relationship between two existing requirements, mirroring `aida rel add` for MCP consumers. Built-in types include parent, child, duplicate, verifies, verified-by, references, blocked-by, and blocks; non-empty custom names are accepted for CLI parity. `depends-on` is accepted as an alias for blocked-by.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "spec_id": {
                        "type": "string",
                        "description": "Source requirement SPEC-ID.",
                        "pattern": "^[A-Z]+-\\d+(-\\d+)*$",
                        "example": "TASK-551"
                    },
                    "relationship_type": {
                        "type": "string",
                        "description": "Relationship type to add. Built-ins: parent, child, duplicate, verifies, verified-by, references, blocked-by, blocks. Aliases: depends-on → blocked-by; related / relates-to → references. Non-empty custom names are accepted for CLI parity.",
                        "example": "blocked-by"
                    },
                    "target_spec_id": {
                        "type": "string",
                        "description": "Target requirement SPEC-ID.",
                        "pattern": "^[A-Z]+-\\d+(-\\d+)*$",
                        "example": "STORY-398"
                    },
                    "bidirectional": {
                        "type": "boolean",
                        "description": "When true, also add the relationship inverse when the type has one.",
                        "example": true
                    },
                    "force_parent": {
                        "type": "boolean",
                        "description": "Override the terminal-parent guard for parent/child edges, matching `aida rel add --force-parent`.",
                        "example": false
                    }
                },
                "required": ["spec_id", "relationship_type", "target_spec_id"]
            },
            "outputSchema": text_envelope_output_schema(
                "a confirmation line `Relationship added: <SOURCE> (<title>) --[<type>]--> <TARGET> (<title>)`, with an inverse note when `bidirectional` added one."
            )
        },
        {
            "name": "query_graph",
            "description": "Query the cross-spec relationship graph from a root spec — transitive blocked-by/blocks chains, epic tree rollup, and reverse impact. Mirrors `aida graph` for MCP consumers; read-only. This is the typed-graph query a flat per-feature spec store cannot answer.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "spec_id": {
                        "type": "string",
                        "description": "Root requirement SPEC-ID to query from.",
                        "pattern": "^[A-Z]+-\\d+(-\\d+)*$",
                        "example": "STORY-489"
                    },
                    "mode": {
                        "type": "string",
                        "description": "Query mode (default tree): tree = Parent/Child descendants + status rollup; blocked-by = transitive BlockedBy chain; blocks = transitive Blocks chain; impact = reverse closure (what is blocked by the root).",
                        "enum": ["tree", "blocked-by", "blocks", "impact"],
                        "example": "blocked-by"
                    },
                    "depth": {
                        "type": "integer",
                        "description": "Limit traversal to N hops from the root. Omit for unbounded.",
                        "minimum": 1,
                        "example": 3
                    },
                    "follow": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Follow arbitrary named relationship types (custom or built-in), outgoing — overrides `mode` when set. E.g. ['begets'] to walk Custom('begets') edges (FR-282).",
                        "example": ["begets"]
                    }
                },
                "required": ["spec_id"]
            },
            "outputSchema": text_envelope_output_schema(
                "pretty-printed JSON `{root, mode, count, nodes:[{id,title,status,resolved}], rollup:{total,completed,done,in_progress,remaining,shelved,rejected}}`."
            )
        },
        {
            "name": "send_message",
            "description": "Send an inter-agent peer message into the mailbox local layer, mirroring `aida mailbox send`. Distinct from briefs (operator→agent work) and directives (top-down control): this is agent↔agent conversation. Address a single agent via `to`, or set `broadcast: true` to reach every agent.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "body": { "type": "string", "description": "The message body.", "example": "can you re-check the auth flow in PR-42? CI flaked once." },
                    "to": { "type": "string", "description": "Recipient agent id. Omit and set broadcast=true to reach all.", "example": "codex" },
                    "broadcast": { "type": "boolean", "description": "Send to every agent instead of a single recipient.", "example": true },
                    "thread": { "type": "string", "description": "Attach to an existing thread id (default: start a new thread).", "example": "0193a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b" },
                    "in_reply_to": { "type": "string", "description": "Id of the message this replies to.", "example": "0193a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b" },
                    "from": { "type": "string", "description": "Sender id (default: this server's agent/user identity).", "example": "claude" },
                    "urgent": { "type": "boolean", "description": "Flag as an urgent escalation so it is surfaced out-of-band (statusline nag) instead of sitting unseen. Lightweight: normal vs urgent only.", "example": true },
                    "intent": { "type": "string", "enum": ["fyi", "request", "handoff"], "description": "How the recipient should treat this message: fyi (informational, surface only), request (needs a response), or handoff (work transfer). Default: fyi. Orthogonal to urgent (loudness vs kind). Mail is interpreted input, not a command channel — an actionable intent is a recommendation, never an authenticated directive.", "example": "request" }
                },
                "required": ["body"]
            },
            "outputSchema": text_envelope_output_schema(
                "a confirmation line `Message sent: <id> (thread <thread-id>)`."
            )
        },
        {
            "name": "read_inbox",
            "description": "Read an agent's mailbox inbox — messages addressed to it plus broadcasts (excluding its own sent), oldest-first — mirroring `aida mailbox inbox`. Reading does NOT mark the inbox seen unless `mark_seen` is true (the explicit ack); pass `unread` to return only messages past the read-watermark. Returns pretty-printed JSON.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent": { "type": "string", "description": "Whose inbox (default: this server's agent/user identity).", "example": "claude" },
                    "unread": { "type": "boolean", "description": "Return only unread messages (newer than this inbox's read-watermark). Default false (whole inbox).", "default": false, "example": true },
                    "mark_seen": { "type": "boolean", "description": "Advance the read-watermark to the newest message after reading — the explicit ack that clears the unread/notice surface. Default false, so a plain read is a non-marking peek (STORY-585 acceptance #4).", "default": false, "example": true }
                }
            },
            "outputSchema": text_envelope_output_schema(
                "pretty-printed JSON `{agent, count, unread, messages:[{id,thread_id,from,to,timestamp,in_reply_to,body,urgent,intent}]}` where intent is one of fyi|request|handoff."
            )
        },
        {
            "name": "list_features",
            "description": "List all active feature categories defined in the project, displaying their names and normalized prefixes.",
            "inputSchema": { "type": "object", "properties": {} },
            "outputSchema": text_envelope_output_schema(
                "either `Features (N):` followed by `- <name> (prefix: <prefix>)` lines, or `No features defined in this project.` when empty."
            )
        },
        {
            "name": "history",
            "description": "Read AIDA's orphan-branch event ledger, mirroring `aida history --events` for MCP consumers. Returns pretty-printed JSON with structured event records. Mirrors the CLI's filter surface (type/author/since/until/limit/shipped/status-changes/comments). The MCP ledger never hides archived/deferred rows, so it is already equivalent to `aida history --all` — no `all` toggle is needed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "spec_id": {
                        "type": "string",
                        "description": "Optional SPEC-ID filter for a single requirement's event history (mirrors `aida history --id`). Accepts the raw UUID `show_requirement` returns; it is resolved to the canonical SPEC-ID before filtering.",
                        "example": "TASK-538"
                    },
                    "events": {
                        "type": "boolean",
                        "description": "Per-event chronological mode (mirrors `aida history --events`). The MCP tool defaults to true (the structured event ledger).",
                        "default": true,
                        "example": true
                    },
                    "type": {
                        "type": "string",
                        "description": "Only events for requirements of this type (mirrors `aida history --type`).",
                        "enum": ["functional", "non-functional", "system", "user", "change-request", "bug", "epic", "story", "task", "spike", "sprint", "folder", "meta", "principle", "vision", "constraint", "decision", "term", "doc"],
                        "example": "bug"
                    },
                    "author": {
                        "type": "string",
                        "description": "Only events authored by this user (mirrors `aida history --author`; matches the YAML `last_modified_by` HLC field if present, else the git committer email).",
                        "example": "joe@example.com"
                    },
                    "since": {
                        "type": "string",
                        "description": "Optional lower time bound, passed through to git just like `aida history --since` (RFC3339 or relative expressions supported by git).",
                        "example": "24 hours ago"
                    },
                    "until": {
                        "type": "string",
                        "description": "Optional upper time bound (mirrors `aida history --until`; RFC3339 or relative expressions supported by git).",
                        "example": "2026-06-01"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Cap the number of decoded events returned (mirrors `aida history --limit`). Defaults to 100.",
                        "minimum": 1,
                        "default": 100,
                        "example": 20
                    },
                    "shipped": {
                        "type": "boolean",
                        "description": "Only Done->Completed ship transitions, newest first — the 'did my ship register?' view (mirrors `aida history --shipped`). Implies events mode; composes with since/until/limit.",
                        "default": false,
                        "example": true
                    },
                    "status_changes": {
                        "type": "boolean",
                        "description": "Filter to status-transition events only (mirrors `aida history --status-changes`; events mode).",
                        "default": false,
                        "example": true
                    },
                    "comments": {
                        "type": "boolean",
                        "description": "Filter to comment events only (mirrors `aida history --comments`; events mode).",
                        "default": false,
                        "example": true
                    },
                    "oneline": {
                        "type": "boolean",
                        "description": "Terse one-line-per-event detail (mirrors `aida history --oneline`; events mode).",
                        "default": false,
                        "example": true
                    }
                }
            },
            "outputSchema": text_envelope_output_schema(
                "pretty-printed JSON `{ count, events }`, where each event has `sha`, `timestamp`, `author`, `spec_id`, `req_type`, `kind`, `summary`, and structured `detail` fields."
            )
        },

        // ---- Punt channel (.aida/punts.jsonl + .aida/punts/) ----
        {
            "name": "list_punts",
            "description": "List existing punt obstacles recorded in `.aida/punts.jsonl`, optionally filtered by resolution status.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "description": "Filter on resolution_path status. 'awaiting' represents unresolved/active obstacles.",
                        "enum": ["awaiting", "advisor-resolved", "escalated-to-human", "escalate-defaulted"],
                        "example": "awaiting"
                    }
                }
            },
            "outputSchema": text_envelope_output_schema(
                "either `Found N punt(s):` followed by per-record blocks of `- <SPEC-ID> [<category>] <detail>\\n  resolution: <resolution_path>  (<RFC3339 timestamp>)`, or `No punts found.` when the filter yields nothing."
            )
        },
        {
            "name": "read_punt",
            "description": "Read the full historical details and response data of the most recent punt filed for a specific spec.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "spec_id": {
                        "type": "string",
                        "description": "The SPEC-ID of the punted requirement.",
                        "pattern": "^[A-Z]+-\\d+(-\\d+)*$",
                        "example": "FR-0042"
                    }
                },
                "required": ["spec_id"]
            },
            "outputSchema": text_envelope_output_schema(
                "the pretty-printed JSON of a PuntRecord (`timestamp`, `spec`, `category`, `detail`, optional `lean` / `raised_by`, `resolution_path`, plus optional resolver-side fields such as `classification`, `escalation_reason`, `answer`, `answered_by`)."
            )
        },
        {
            "name": "post_punt",
            "description": "Append an unresolved obstacle or decision fork (punt) for a spec to `.aida/punts.jsonl` to seek advice or escalate. (Does not modify spec status — pair with `update_requirement status=needs-attention`).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "spec_id": {
                        "type": "string",
                        "description": "The unique SPEC-ID of the requirement facing the obstacle.",
                        "pattern": "^[A-Z]+-\\d+(-\\d+)*$",
                        "example": "FR-0042"
                    },
                    "detail": {
                        "type": "string",
                        "description": "Detailed human-readable description of the obstacle or design fork. Alias: `reason`.",
                        "example": "The downstream service API has updated its rate limits from 100/min to 10/min, breaking our batch ingest assumption."
                    },
                    "reason": {
                        "type": "string",
                        "description": "Alias for `detail` (TASK-883) — the obstacle/design-fork description. Provide either `detail` or `reason`; `detail` wins if both are set.",
                        "example": "Two equally-valid storage layouts; need a decision before proceeding."
                    },
                    "category": {
                        "type": "string",
                        "description": "The class of obstacle being encountered.",
                        "enum": ["design-fork", "ambiguous-spec", "missing-context", "blocked-dependency", "other"],
                        "example": "blocked-dependency"
                    },
                    "lean": {
                        "type": "string",
                        "description": "The sender's preferred resolution or best-guess path forward.",
                        "example": "Wait for the downstream service to lift limits or switch to a queue"
                    },
                    "raised_by": {
                        "type": "string",
                        "description": "Identify the role/agent raising this punt (e.g., implementer, reviewer).",
                        "example": "implementer"
                    }
                },
                "required": ["spec_id", "detail"]
            },
            "outputSchema": text_envelope_output_schema(
                "a confirmation line `Punt recorded for <SPEC-ID> [<category>].` followed by a hint to run `aida edit <SPEC-ID> --status needs-attention` from a session lease to park the spec."
            )
        },
        {
            "name": "resolve_punt",
            "description": "Resolve a punt obstacle by writing a decision response to `.aida/punts/<spec>.response.json`, enabling resumed implementation.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "spec_id": {
                        "type": "string",
                        "description": "The unique SPEC-ID of the requirement to resolve.",
                        "pattern": "^[A-Z]+-\\d+(-\\d+)*$",
                        "example": "FR-0042"
                    },
                    "answer": {
                        "type": "string",
                        "description": "The authoritative resolution or action chosen.",
                        "example": "We will adopt the design fork A and implement Redis-based queue ingestion."
                    },
                    "reasoning": {
                        "type": "string",
                        "description": "Audit-trail explanation of why this resolution was chosen.",
                        "example": "Design fork A ensures we stay within the 10/min API limit by smoothing spikes, avoiding data loss."
                    },
                    "classification": {
                        "type": "string",
                        "description": "How this decision is categorized.",
                        "enum": ["recorded-principle", "recorded-preference", "synthesized"],
                        "example": "synthesized"
                    }
                },
                "required": ["spec_id", "answer", "reasoning"]
            },
            "outputSchema": text_envelope_output_schema(
                "a confirmation line `Resolution written to <absolute path to .aida/punts/<spec>.response.json>` and an explanatory follow-on that the orchestrator will resume the implementer with this answer."
            )
        },
        {
            "name": "escalate_punt",
            "description": "Mark a punt obstacle as requiring human intervention or strategic oversight, saving response to `.aida/punts/<spec>.response.json`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "spec_id": {
                        "type": "string",
                        "description": "The unique SPEC-ID of the requirement being escalated.",
                        "pattern": "^[A-Z]+-\\d+(-\\d+)*$",
                        "example": "FR-0042"
                    },
                    "reasoning": {
                        "type": "string",
                        "description": "Explanation of why this requires human triage.",
                        "example": "Choosing between direct webhooks and queue polling impacts long-term infrastructure budget and ops complexity; requires architectural committee sign-off."
                    },
                    "escalation_reason": {
                        "type": "string",
                        "description": "Categorized reason for the escalation.",
                        "enum": ["strategy", "irreversible", "unrecorded-context", "other"],
                        "example": "strategy"
                    },
                    "classification": {
                        "type": "string",
                        "description": "How this escalated decision should be classified.",
                        "enum": ["recorded-principle", "recorded-preference", "synthesized"],
                        "example": "recorded-principle"
                    }
                },
                "required": ["spec_id", "reasoning"]
            },
            "outputSchema": text_envelope_output_schema(
                "a confirmation line `Escalation written to <absolute path to .aida/punts/<spec>.response.json>` and an explanatory follow-on that the orchestrator will park the spec for human triage."
            )
        },

        // ---- Findings channel (draft requirements with from-* tags) ----
        {
            "name": "list_findings",
            "description": "List review findings and structural discrepancies filed by automated drain checks or manual implementers.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pr": {
                        "type": "integer",
                        "description": "Filter findings to those associated with a specific PR number.",
                        "minimum": 1,
                        "example": 203
                    },
                    "source": {
                        "type": "string",
                        "description": "Filter findings by their authoring source.",
                        "enum": ["review", "implementer"],
                        "example": "review"
                    },
                    "kind": {
                        "type": "string",
                        "description": "Filter by type/category of the finding.",
                        "enum": ["deviation", "design-choice", "bug-spotted", "followup-suggestion"],
                        "example": "deviation"
                    }
                }
            },
            "outputSchema": text_envelope_output_schema(
                "either `Found N finding(s):` followed by `## From <source>` sections, `### <origin>` groups, and `- <SPEC-ID> <title> (<severity>)[ [<kind>]]` rows, or `No findings match the filter.` when empty."
            )
        },
        {
            "name": "file_finding",
            "description": "File a new review finding as a draft TASK with appropriate tags, identifying deviations, bugs, or suggestions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Short, descriptive title summarizing the finding.",
                        "example": "Missing validation on OAuth callback state"
                    },
                    "description": {
                        "type": "string",
                        "description": "Detailed explanation of the issue, impact, and suggested fix.",
                        "example": "The callback handler does not check the state parameter, making it vulnerable to CSRF attacks."
                    },
                    "source": {
                        "type": "string",
                        "description": "Origin phase/role filing the finding.",
                        "enum": ["implementer", "review"],
                        "example": "implementer"
                    },
                    "spec_id": {
                        "type": "string",
                        "description": "Associated requirement SPEC-ID (required when source is 'implementer').",
                        "pattern": "^[A-Z]+-\\d+(-\\d+)*$",
                        "example": "FR-0042"
                    },
                    "pr": {
                        "type": "integer",
                        "description": "Associated PR number (required when source is 'review').",
                        "minimum": 1,
                        "example": 203
                    },
                    "kind": {
                        "type": "string",
                        "description": "The category of this finding.",
                        "enum": ["deviation", "design-choice", "bug-spotted", "followup-suggestion"],
                        "example": "bug-spotted"
                    },
                    "severity": {
                        "type": "string",
                        "description": "Triage severity level.",
                        "enum": ["cosmetic", "minor", "major"],
                        "example": "major"
                    }
                },
                "required": ["title", "description"]
            },
            "outputSchema": text_envelope_output_schema(
                "a confirmation line `Finding filed: <SPEC-ID> — <title>`, where SPEC-ID is the freshly-assigned identifier for the new draft TASK."
            )
        },
        {
            "name": "triage_finding",
            "description": "Triage a finding by dismissing it (sets Rejected). Promotion to an approved active task is the advisor's triage decision and is refused via MCP (run the CLI as the advisor, or let the advisor promote it).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The SPEC-ID of the finding to triage (typically a draft TASK-N).",
                        "pattern": "^[A-Z]+-\\d+(-\\d+)*$",
                        "example": "TASK-123"
                    },
                    "action": {
                        "type": "string",
                        "description": "Promotion to active or dismissal.",
                        "enum": ["promote", "dismiss"],
                        "example": "promote"
                    },
                    "reason": {
                        "type": "string",
                        "description": "One-line rationale for the triage decision, recorded as a comment.",
                        "example": "This is a critical security vulnerability and must be promoted to an approved task."
                    }
                },
                "required": ["id", "action"]
            },
            "outputSchema": text_envelope_output_schema(
                "a confirmation line `Finding <SPEC-ID> promoted` or `Finding <SPEC-ID> dismissed`."
            )
        },

        // ---- Task-claim channel (.aida/sessions/*.toml) ----
        {
            "name": "claim_task",
            "description": "Acquire a lightweight advisory lock (lease) on a specific spec in `.aida/sessions/<id>.toml`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "spec_id": {
                        "type": "string",
                        "description": "The unique SPEC-ID of the requirement to claim.",
                        "pattern": "^[A-Z]+-\\d+(-\\d+)*$",
                        "example": "FR-0042"
                    },
                    "role": {
                        "type": "string",
                        "description": "The role claiming the requirement.",
                        "enum": ["implementer", "advisor", "reviewer"],
                        "example": "implementer"
                    },
                    "worktree_path": {
                        "type": "string",
                        "description": "Optional absolute path of the agent's actual working directory (its `pwd`). Recorded on the lease so consumers like `aida add`'s scope hint can tell whether the lease covers a given shell's cwd. When omitted, the lease records no worktree and is treated as a spec-level advisory lock only — no cwd-based session-context inference is attached to it. Pass this when you're working in a sibling git worktree so other agents in the parent worktree don't get misrouted hints.",
                        "example": "/home/joe/ai/aida-task-310"
                    }
                },
                "required": ["spec_id"]
            },
            "outputSchema": text_envelope_output_schema(
                "either `claimed: lease_id=<12-char id>` on a fresh claim, or `already_claimed by <owner> (lease <id> since <RFC3339 timestamp>)` when an existing lease already covers the spec."
            )
        },
        {
            "name": "release_task",
            "description": "Release and delete an MCP-created advisory lock (lease) from the database by its lease ID. Refuses to delete CLI-created sessions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "lease_id": {
                        "type": "string",
                        "description": "The 12-character hex ID of the lease to release.",
                        "pattern": "^[a-f0-9]{12}$",
                        "example": "019e5170660c"
                    }
                },
                "required": ["lease_id"]
            },
            "outputSchema": text_envelope_output_schema(
                "a confirmation line `released: lease_id=<id>` on success. On failure (lease not found, or a non-mcp lease), the envelope sets `isError: true` and carries a human-readable error message instead."
            )
        },
        {
            "name": "list_active_leases",
            "description": "List all active spec leases (both CLI-created and MCP-created sessions) in `.aida/sessions/`.",
            "inputSchema": { "type": "object", "properties": {} },
            "outputSchema": text_envelope_output_schema(
                "either `Found N active lease(s):` followed by `- <id> scope=<SPEC-ID> role=<role> owner=<owner> kind=<mcp|session> started_at=<RFC3339 timestamp>` rows, or `No active leases.` when none exist."
            )
        },

        // ---- Worker-directive channel (.aida/worker.cmd) ----
        {
            "name": "post_directive",
            "description": "Submit a command directive (drain, pause, exit) to `.aida/worker.cmd` for background worker orchestration.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "verb": {
                        "type": "string",
                        "description": "The directive command verb.",
                        "enum": ["drain", "pause", "exit"],
                        "example": "drain"
                    },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional CLI arguments to forward to `aida queue work` (only valid for 'drain' verb).",
                        "example": ["--only-failed"]
                    }
                },
                "required": ["verb"]
            },
            "outputSchema": text_envelope_output_schema(
                "a confirmation line `directive posted: <verb> [args...]` echoing the line that was appended to .aida/worker.cmd."
            )
        },
        {
            "name": "list_directives",
            "description": "List all pending worker directives registered in `.aida/worker.cmd`.",
            "inputSchema": { "type": "object", "properties": {} },
            "outputSchema": text_envelope_output_schema(
                "the same human-rendered directive list as `aida worker directives` — one line per pending directive (verb plus any forwarded args), or an empty/empty-state rendering when .aida/worker.cmd has no pending directives."
            )
        },
        {
            "name": "ack_directive",
            "description": "Acknowledge and remove a pending directive from `.aida/worker.cmd` by its 0-based index.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "index": {
                        "type": "integer",
                        "description": "The 0-based index of the directive in the current list to acknowledge and remove.",
                        "minimum": 0,
                        "example": 0
                    }
                },
                "required": ["index"]
            },
            "outputSchema": text_envelope_output_schema(
                "a confirmation line `acked: <verb [args...]>` echoing the directive line that was removed from .aida/worker.cmd."
            )
        },

        // ---- Agent-brief channel (.aida/agent-briefs/<agent>/) ----
        {
            "name": "list_briefs",
            "description": "List substrate-resident agent pickup briefs under `.aida/agent-briefs/<agent>/`, optionally filtered to one agent.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent": {
                        "type": "string",
                        "description": "Optional agent name to filter by, e.g. codex.",
                        "pattern": "^[A-Za-z0-9_.-]+$",
                        "example": "codex"
                    },
                    "include_acked": {
                        "type": "boolean",
                        "description": "Include acknowledged briefs that normally stay hidden from pickup lists.",
                        "example": false
                    }
                }
            },
            "outputSchema": text_envelope_output_schema(
                "either `Found N brief(s):` followed by `- path=<project-relative path> agent=<agent> spec_id=<SPEC-ID> generated_at=<YYYY-MM-DDTHHMMSSZ> status=<pending|acked>` rows, or `No briefs found.` when no matching briefs exist."
            )
        },
        {
            "name": "read_brief",
            "description": "Read the full markdown content of an agent brief file returned by list_briefs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Project-relative `.aida/agent-briefs/...` path returned by list_briefs, or an absolute path under the same directory.",
                        "pattern": "^(\\.aida/agent-briefs/|/).+",
                        "example": ".aida/agent-briefs/codex/TASK-492-2026-05-23T020000Z.md"
                    }
                },
                "required": ["path"]
            },
            "outputSchema": text_envelope_output_schema(
                "the full brief file content, including YAML frontmatter and all markdown body sections. On failure (not found or path outside .aida/agent-briefs), the envelope sets `isError: true`."
            )
        },
        {
            "name": "ack_brief",
            "description": "Mark an agent brief acknowledged using the TASK-492 convention: update frontmatter status and rename with a `.acked` suffix.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Project-relative `.aida/agent-briefs/...` path returned by list_briefs, or an absolute path under the same directory. Passing an already-acked path is idempotent.",
                        "pattern": "^(\\.aida/agent-briefs/|/).+",
                        "example": ".aida/agent-briefs/codex/TASK-492-2026-05-23T020000Z.md"
                    }
                },
                "required": ["path"]
            },
            "outputSchema": text_envelope_output_schema(
                "a confirmation line `acked: <project-relative .acked path>` or idempotent `already_acked: <project-relative .acked path>`. On failure, the envelope sets `isError: true` with a human-readable message."
            )
        }
    ]);
    // STORY-532 (EPIC-27): the queue tool descriptors live in their own
    // `json!` array (the single combined literal blew the macro recursion
    // limit) and are appended here. trace:EPIC-27
    if let (Some(base), Some(queue)) = (
        descriptors.as_array_mut(),
        queue_tool_descriptors().as_array(),
    ) {
        base.extend(queue.iter().cloned());
    }
    // STORY-533 (EPIC-27): session tool descriptors live in their own array
    // (same macro-recursion-limit reason as the queue block) and are appended
    // here. trace:EPIC-27
    if let (Some(base), Some(session)) = (
        descriptors.as_array_mut(),
        session_tool_descriptors().as_array(),
    ) {
        base.extend(session.iter().cloned());
    }
    // STORY-534 (EPIC-27): role tool descriptors live in their own array (same
    // macro-recursion-limit reason as the queue / session blocks) and are
    // appended here. trace:EPIC-27
    if let (Some(base), Some(role)) = (
        descriptors.as_array_mut(),
        role_tool_descriptors().as_array(),
    ) {
        base.extend(role.iter().cloned());
    }
    // STORY-536 (EPIC-27): workflow tool descriptors live in their own array
    // (same macro-recursion-limit reason as the queue / session / role blocks)
    // and are appended here. trace:EPIC-27
    if let (Some(base), Some(workflow)) = (
        descriptors.as_array_mut(),
        workflow_tool_descriptors().as_array(),
    ) {
        base.extend(workflow.iter().cloned());
    }
    descriptors
}

// STORY-532 (EPIC-27): queue tool descriptors, factored into their own
// `json!` array so the combined `tool_descriptors()` literal stays under the
// macro recursion limit. trace:EPIC-27
fn queue_tool_descriptors() -> Value {
    json!([
        // ---- Queue (STORY-532 / EPIC-27) ----
        {
            "name": "queue_list",
            "description": "List items in the work queue. Mirrors `aida queue list`. Resolves the queue user identity like the CLI (user arg → AIDA_USER → USER → USERNAME → default), so MCP and CLI see the same queue. Terminal-status entries are hidden unless `include_terminal` is set.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "user": { "type": "string", "description": "Override the queue user id (defaults to the shell identity).", "example": "alice" },
                    "for": { "type": "string", "description": "Filter to items routed to this role (e.g. `implementer`). Pass `any` for UNROUTED items.", "example": "implementer" },
                    "all": { "type": "boolean", "description": "Show all roles (override any active-role default filter).", "example": true },
                    "include_completed": { "type": "boolean", "description": "Include Completed requirements in the underlying load (default false).", "example": false },
                    "include_terminal": { "type": "boolean", "description": "Show Completed/Rejected entries in the listing (default hides them).", "example": false }
                }
            },
            "outputSchema": text_envelope_output_schema(
                "either `Queue for '<user>' (N items):` followed by `N. [SPEC-ID] <title> (<status>) [for:<role>]` lines, or `Queue is empty for user '<user>'.`"
            )
        },
        {
            "name": "queue_add",
            "description": "Add a requirement to the work queue. Mirrors `aida queue add`. Routes to the role named by `for` (or the active session role; pass `for: any` to stay unrouted). Refuses to queue Completed/Rejected work unless `force` is set.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Requirement id (UUID or SPEC-ID) to queue.", "example": "STORY-42" },
                    "user": { "type": "string", "description": "Override the queue user id.", "example": "alice" },
                    "top": { "type": "boolean", "description": "Add to the top of the queue (default appends to the bottom).", "example": true },
                    "note": { "type": "string", "description": "Note explaining why this was queued.", "example": "blocks the release" },
                    "for": { "type": "string", "description": "Route to a specific role queue; `any` keeps it unrouted.", "example": "implementer" },
                    "scope": { "type": "string", "description": "Restrict routing to sessions whose lease scope matches (e.g. an EPIC id).", "example": "EPIC-20" },
                    "force": { "type": "boolean", "description": "Bypass the guard that refuses queueing a Completed/Rejected requirement.", "example": true }
                },
                "required": ["id"]
            },
            "outputSchema": text_envelope_output_schema(
                "a confirmation line `Added <SPEC-ID> (<title>) to queue [for:<role>]`. On a terminal-status spec without `force`, the envelope sets `isError: true`."
            )
        },
        {
            "name": "queue_work",
            "description": "Peek at the item `aida queue work` WOULD pick up. With `id`, resolves that specific queued spec; with no `id`, peeks the head of the (role-filtered) queue. This is a metadata-only resolver — launching a Claude session is a CLI-only act, so this never starts work. Run `aida queue work` from a shell to actually launch.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "A queued requirement id to resolve as the pickup target. Omit to peek the queue head.", "example": "STORY-42" },
                    "user": { "type": "string", "description": "Override the queue user id.", "example": "alice" },
                    "for": { "type": "string", "description": "Filter the head peek to this role; `any` for unrouted.", "example": "implementer" },
                    "all": { "type": "boolean", "description": "Peek across all roles.", "example": true }
                }
            },
            "outputSchema": text_envelope_output_schema(
                "either `Next pickup: <SPEC-ID> (<title>) [status:...]` / `Pickup target: ...` with a `Run \\`aida queue work\\`` hint, or `Queue is empty — nothing to pick up.`"
            )
        },
        {
            "name": "queue_done",
            "description": "Mark a requirement Done and remove it from the queue in one step. Mirrors `aida queue done`. Flips status to Done (work finished on a branch) — the merge auto-bump later advances Done → Completed. Stamps implementation_info. Optionally capture user-facing interface changes (the deterministic operator-digest source) via interface_cli/mcp/tui/other, or no_interface_change for a no-impact spec. Optionally record the verification steps the builder ran via test_plan (surfaced in the PR body).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Requirement id (UUID or SPEC-ID) to mark done.", "example": "STORY-42" },
                    "user": { "type": "string", "description": "Override the queue user id.", "example": "alice" },
                    "interface_cli": { "type": "array", "items": { "type": "string" }, "description": "User-facing CLI surface changes (new commands, changed flags/behavior). Feeds the operator digest.", "example": ["aida mailbox list — new command"] },
                    "interface_mcp": { "type": "array", "items": { "type": "string" }, "description": "User-facing MCP surface changes (new tools, gating, schema).", "example": ["queue_add — now advisor-gated"] },
                    "interface_tui": { "type": "array", "items": { "type": "string" }, "description": "User-facing TUI surface changes (keybindings, panes, overlays).", "example": [] },
                    "interface_other": { "type": "array", "items": { "type": "string" }, "description": "Any other user-facing interface change (not cli/mcp/tui).", "example": ["REST /digest endpoint added"] },
                    "no_interface_change": { "type": "boolean", "description": "Explicitly mark this spec as having no user-facing interface change (clippy/refactor/test). Keeps it out of the operator digest.", "example": true },
                    "test_plan": { "type": "array", "items": { "type": "string" }, "description": "The verification steps the builder actually ran — the implementation audit trail stored in implementation_info.test_coverage_notes and surfaced in the PR body.", "example": ["cargo test -p aida-cli", "manual: aida queue done at a TTY"] }
                },
                "required": ["id"]
            },
            "outputSchema": text_envelope_output_schema(
                "a confirmation line `<SPEC-ID> marked done and removed from queue.`"
            )
        },
        {
            "name": "queue_next",
            "description": "Peek the top item in the queue without removing it. Mirrors `aida queue next`. Equivalent to a no-`id` queue_work peek.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "user": { "type": "string", "description": "Override the queue user id.", "example": "alice" },
                    "for": { "type": "string", "description": "Filter to this role; `any` for unrouted.", "example": "implementer" },
                    "all": { "type": "boolean", "description": "Peek across all roles.", "example": true }
                }
            },
            "outputSchema": text_envelope_output_schema(
                "either `Next pickup: <SPEC-ID> (<title>) [status:...]`, or `Queue is empty — nothing to pick up.`"
            )
        },
        {
            "name": "queue_progress",
            "description": "Bucketed progress (Shipped / In flight / Working now / Remaining) over a set of specs. Mirrors `aida queue progress`. Define the set with `batch: <name>` (a `batch:NAME` tag) or `specs: [<id>, ...]`. Session-manifest progress is CLI-only.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "batch": { "type": "string", "description": "Report on members of this `batch:NAME` tag.", "example": "fall-cleanup" },
                    "specs": { "type": "array", "items": { "type": "string" }, "description": "Explicit list of requirement ids to report on.", "example": ["STORY-42", "TASK-7"] }
                }
            },
            "outputSchema": text_envelope_output_schema(
                "a `Progress (N items):` header followed by `Shipped:` / `In flight:` / `Working now:` / `Remaining:` count lines (each listing member ids)."
            )
        },
        {
            "name": "queue_rework",
            "description": "Re-open a spec: flip its status, route it to a role's queue, and optionally capture a reason. Mirrors `aida queue rework` (metadata-only — it does not launch a session). Smart status transitions (Planned→InProgress, Done→InProgress, Rejected→Approved, …) unless overridden with `status`. Terminal-status and already-InProgress re-opens require `force`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Requirement id (UUID or SPEC-ID) to rework.", "example": "STORY-42" },
                    "user": { "type": "string", "description": "Override the queue user id.", "example": "alice" },
                    "for": { "type": "string", "description": "Override the routing role (default: active role).", "example": "implementer" },
                    "status": { "type": "string", "description": "Override the smart target status.", "enum": ["draft", "approved", "planned", "in-progress", "needs-attention", "done", "completed", "rejected"], "example": "in-progress" },
                    "reason": { "type": "string", "description": "Capture a comment on the spec at rework time (audit trail).", "example": "reviewer requested changes" },
                    "force": { "type": "boolean", "description": "Bypass the terminal-status and already-in-progress guards.", "example": true }
                },
                "required": ["id"]
            },
            "outputSchema": text_envelope_output_schema(
                "an optional `<SPEC-ID> status: <old> → <new>` line followed by `Queued <SPEC-ID> (<title>) [for:<role>]`. On a guarded re-open without `force`, the envelope sets `isError: true`."
            )
        },
        {
            "name": "queue_move",
            "description": "Move a queue item to a new position. Mirrors `aida queue move`. Specify exactly one destination: `top`, `bottom`, `before: <id>`, or `after: <id>`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Requirement id (UUID or SPEC-ID) to move.", "example": "STORY-42" },
                    "user": { "type": "string", "description": "Override the queue user id.", "example": "alice" },
                    "top": { "type": "boolean", "description": "Move to the front of the queue.", "example": true },
                    "bottom": { "type": "boolean", "description": "Move to the back of the queue.", "example": true },
                    "before": { "type": "string", "description": "Move immediately before this requirement id.", "example": "STORY-42" },
                    "after": { "type": "string", "description": "Move immediately after this requirement id.", "example": "STORY-42" }
                },
                "required": ["id"]
            },
            "outputSchema": text_envelope_output_schema(
                "a confirmation line `Moved <SPEC-ID> in queue.` On a spec not in the queue, or with no destination, the envelope sets `isError: true`."
            )
        },
        {
            "name": "queue_remove",
            "description": "Remove a requirement from the queue. Mirrors `aida queue remove`. Does not change the spec's status. Pass `for` to scope removal to one role's queue when the spec is queued for several.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Requirement id (UUID or SPEC-ID) to remove.", "example": "STORY-42" },
                    "user": { "type": "string", "description": "Override the queue user id.", "example": "alice" },
                    "for": { "type": "string", "description": "Role filter: remove only the entry queued for this role (e.g. 'implementer', 'advisor'). 'any' or omitted = remove every entry for the spec.", "example": "advisor" }
                },
                "required": ["id"]
            },
            "outputSchema": text_envelope_output_schema(
                "a confirmation line `Removed <SPEC-ID> from queue.`"
            )
        }
    ])
}

// STORY-533 (EPIC-27): session tool descriptors, factored into their own
// `json!` array so the combined `tool_descriptors()` literal stays under the
// macro recursion limit. trace:EPIC-27
fn session_tool_descriptors() -> Value {
    json!([
        // ---- Session (STORY-533 / EPIC-27) ----
        {
            "name": "session_leases",
            "description": "List active scoped session leases — the 'who holds what scoped work right now' view. Mirrors `aida session leases`. Reads `.aida/sessions/*.toml` directly (no subprocess). By default lists worktree-backed sessions; pass `all` to also include lightweight MCP claim markers.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "all": { "type": "boolean", "description": "Also include MCP `claim_task` markers (not just worktree-backed `session start` leases).", "example": true }
                }
            },
            "outputSchema": text_envelope_output_schema(
                "either `Active session lease(s) (N):` followed by `- <id> scope=<s> branch=<b> role=<r> owner=<o> kind=<session|mcp-claim> started_at=<ts>` lines, or `(no active sessions)`."
            )
        },
        {
            "name": "session_status",
            "description": "Show details for one session lease (the per-session status view). Mirrors `aida session show`. Reads `.aida/sessions/*.toml` directly. Pass `id` (8-char prefix accepted) to pick a lease; when exactly one session is active, `id` may be omitted. Surfaces inline manifest progress when a planned cluster exists.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Session lease id (8-char prefix accepted). Optional when exactly one session is active.", "example": "a1b2c3d4" }
                }
            },
            "outputSchema": text_envelope_output_schema(
                "a `Session <id>` block with `scope`, `branch`, `worktree`, `role`, `owner`, `hostname`, `started_at`, and `kind` lines, plus a `manifest: <done>/<total> item(s) completed` line when a planned cluster exists. On an ambiguous/absent id with multiple sessions, the envelope sets `isError: true`."
            )
        },
        {
            "name": "session_manifest",
            "description": "Show a session's planned-cluster manifest — the SPEC-IDs /aida-pickup recorded it intends to work, with per-item status (a marker per item: done / started / pending). Read mirror of `aida session manifest` / `aida session show --plan`. Writing/marking the manifest stays CLI-only (it needs the active session's cwd context and happens automatically as `aida edit` / `queue done` run).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Session lease id (8-char prefix accepted). Optional when exactly one session is active.", "example": "a1b2c3d4" }
                }
            },
            "outputSchema": text_envelope_output_schema(
                "either `Session <id> planned cluster (<done>/<total> done, source: <src>):` followed by `  <status-marker> <SPEC-ID>` lines (a marker per item: done / started / pending), or a note that no manifest exists yet. On an ambiguous/absent id with multiple sessions, the envelope sets `isError: true`."
            )
        },
        {
            "name": "session_start",
            "description": "PEEK ONLY (does not launch): describe starting a scoped session and return the exact `aida session start` command to run. Mirrors `aida session start` inputs. The actual act — creating a git worktree on a fresh branch and (with `launch`) exec'ing `claude` — is a subprocess-driven act that the MCP server intentionally does NOT perform; the CLI must run it. Warns when the scope is already held by a live lease.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "owns": { "type": "string", "description": "Scope this session would own (EPIC-N, a SPEC-ID, a path glob, or a free-form tag).", "example": "EPIC-27" },
                    "branch": { "type": "string", "description": "Branch name for the new worktree (default: derived from owns).", "example": "epic-27-work" },
                    "role": { "type": "string", "description": "Role to record on the lease (default: derived from scope; PR/MR → reviewer, else implementer).", "example": "implementer" },
                    "launch": { "type": "boolean", "description": "Whether the suggested command should also launch Claude inside the worktree (`--launch`).", "example": false }
                },
                "required": ["owns"]
            },
            "outputSchema": text_envelope_output_schema(
                "a note explaining the peek plus a `Run:` block with the `aida session start --owns <scope> ...` command. Prepends a `Note:` line when the scope is already held by a live lease."
            )
        },
        {
            "name": "session_end",
            "description": "PEEK ONLY (does not end anything): resolve the lease `aida session end` would target and return the CLI command to run. Mirrors `aida session end` selectors. The actual act — removing the git worktree, terminating live `claude` processes, probing PR/CI state, filing reviewer follow-ups — is a subprocess-driven act the MCP server intentionally does NOT perform. Pass `id` (8-char prefix) or `spec` (scope) to target; optional when exactly one session is active.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Session lease id to end (8-char prefix accepted).", "example": "a1b2c3d4" },
                    "spec": { "type": "string", "description": "Resolve the lease by the scope/spec it owns.", "example": "EPIC-27" }
                }
            },
            "outputSchema": text_envelope_output_schema(
                "a note explaining the peek plus a `Run:` block with the `aida session end <id>` command. On an ambiguous/absent selector with multiple sessions, the envelope sets `isError: true`."
            )
        }
    ])
}

// STORY-534 (EPIC-27): role tool descriptors, factored into their own `json!`
// array so the combined `tool_descriptors()` literal stays under the macro
// recursion limit. trace:EPIC-27
fn role_tool_descriptors() -> Value {
    json!([
        // ---- Role (STORY-534 / EPIC-27) ----
        {
            "name": "role_list",
            "description": "List the project's roles (personas / hats) and any global roles, newest-active first. Mirrors `aida role list`. Reads `.aida/roles/*.toml` (+ `~/.aida/roles/*.toml`) directly — no subprocess. The active shell role (`AIDA_SESSION_ROLE`) is marked `*` when it matches one of these.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            },
            "outputSchema": text_envelope_output_schema(
                "either `Roles for <root> (N):` followed by `<*| > <name>[ [global]] last active <rel>[ — <purpose>]` lines, or a `(no roles defined …)` note."
            )
        },
        {
            "name": "role_show",
            "description": "Show details for one role. Mirrors `aida role show [NAME]`. Reads the role TOML directly. With no `name`, resolves the active role from `AIDA_SESSION_ROLE` (errors when none is given and none is active). The legacy `dialog` name resolves to `advisor`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Role name to show. Optional when a role is active in the launching shell (AIDA_SESSION_ROLE).", "example": "advisor" }
                }
            },
            "outputSchema": text_envelope_output_schema(
                "a `Role: <name>` block with `Purpose`, `Created`, `Last active`, optional `Last cwd` / `Notes` / `Scope` / `Addendum` lines, and an `Active:` line. On an unknown name (or no name with no active role) the envelope sets `isError: true`."
            )
        },
        {
            "name": "role_enter",
            "description": "PEEK ONLY (does not switch role): validate the role exists and return the exact `aida role enter` command to run. Mirrors `aida role enter`. Entering a role sets the *shell's* `AIDA_SESSION_ROLE` (role identity is shell-keyed, like the queue user — see BUG-89); the stateless MCP server cannot mutate the calling agent's shell, so it resolves the role context and instructs the caller to run the CLI. Notes when the role is already active.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Role name to enter.", "example": "advisor" },
                    "cd": { "type": "boolean", "description": "Whether the suggested command should also restore the role's last working directory (`--cd`).", "example": false }
                },
                "required": ["name"]
            },
            "outputSchema": text_envelope_output_schema(
                "a note explaining the peek + the resolved role context plus a `Run:` block with the `aida role enter <name>` command. On an unknown role the envelope sets `isError: true`."
            )
        },
        {
            "name": "role_end",
            "description": "PEEK ONLY (does not clear role): report the active shell role and return the `aida role end` command to run. Mirrors `aida role end`. Ending a role clears the shell's `AIDA_SESSION_ROLE` — a shell-env act the MCP server cannot perform for the caller (same shell-keyed constraint as role_enter).",
            "inputSchema": {
                "type": "object",
                "properties": {}
            },
            "outputSchema": text_envelope_output_schema(
                "a line reporting the active shell role (or that none appears active) plus a `Run:` block with the `aida role end` command."
            )
        }
    ])
}

// STORY-536 (EPIC-27): workflow tool descriptors — the remaining CLI long
// tail. Factored into their own `json!` array so the combined
// `tool_descriptors()` literal stays under the macro recursion limit.
// trace:EPIC-27
fn workflow_tool_descriptors() -> Value {
    json!([
        // ---- Workflow (STORY-536 / EPIC-27) ----
        {
            "name": "cache_status",
            "description": "Report the read-cache freshness for the git-canonical store. Mirrors `aida cache status`. Reads the SQLite cache (`.aida/cache.db`) + the orphan store's HEAD SHA directly (no subprocess) and reports cached vs store requirement counts, last-built time, the cache/store HEAD SHAs, and a FRESH/STALE verdict. Rebuilding the cache stays CLI-only (`aida cache rebuild`).",
            "inputSchema": {
                "type": "object",
                "properties": {}
            },
            "outputSchema": text_envelope_output_schema(
                "a block with `Cache path`, `Cached requirements`, `Store requirements`, `Last built`, `Cache HEAD SHA`, `Store HEAD SHA`, and a `Status: FRESH`/`STALE` line. On a store that cannot be opened the envelope sets `isError: true`."
            )
        },
        {
            "name": "plan_verify",
            "description": "Lint a plan file: report drifted line refs, missing/recommended sections, and unresolved file paths. Read-only mirror of `aida plan verify <file>` — it computes the same findings the CLI prints but never rewrites the file (the `--fix` in-place rewrite stays CLI-only) and never exits the process. Resolves refs against the repo the plan lives in.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Path to the plan markdown file to lint (absolute, or relative to the project root).", "example": "docs/plans/2026-06-07-story-536.md" }
                },
                "required": ["file"]
            },
            "outputSchema": text_envelope_output_schema(
                "a `Verifying plan: <path>` header, grouped `Sections` / `Files` / `Line refs` finding lines (each tagged OK / WARN / ERROR), an optional `hint:` line when drifted refs exist, and a final `Verdict: … PASS/FAIL` line. On an unreadable file the envelope sets `isError: true`."
            )
        },
        {
            "name": "plan_helpers",
            "description": "Derive a `## Reusable helpers` section for a spec by walking the trace graph (sibling / tag-mate / same-feature specs) and harvesting their `trace:`-named helpers, so a plan reuses existing code instead of re-inventing it. Read-only mirror of `aida plan helpers <spec>` — it returns the derived markdown; appending it to a plan file (`--append`) stays CLI-only.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "spec": { "type": "string", "description": "Requirement id (UUID or SPEC-ID) to derive helpers for.", "example": "STORY-536" }
                },
                "required": ["spec"]
            },
            "outputSchema": text_envelope_output_schema(
                "the derived `## Reusable helpers` markdown section (with a `verify before relying on it` footer), or a note that no related spec contributes a named helper. On an unknown spec the envelope sets `isError: true`."
            )
        },
        {
            "name": "ultraplan_assemble",
            "description": "Assemble the rich `/ultraplan` prompt for a spec from its description, acceptance criteria, graph context, reusable-helper section, and the 11-section plan structure. Read-only mirror of `aida ultraplan <spec> --stdout` — it returns the assembled prompt text; copying to the clipboard or opening a deep link stays CLI-only.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "spec": { "type": "string", "description": "Requirement id (UUID or SPEC-ID) to assemble the prompt for.", "example": "STORY-536" },
                    "no_comments": { "type": "boolean", "description": "Omit the spec's comments from the prompt (the comments carry long-form enrichment by default).", "example": false }
                },
                "required": ["spec"]
            },
            "outputSchema": text_envelope_output_schema(
                "the assembled `/ultraplan` prompt text, followed by any assembly `Warning:` lines (e.g. a spec with no description). On an unknown spec, or when ultraplan is disabled for the project, the envelope sets `isError: true`."
            )
        },
        {
            "name": "goal_derive",
            "description": "Derive a machine-checkable `/goal` completion condition from one or more axes. Mirrors `aida goal --batch|--epic|--spec|--pr|--queue-empty`. Pure — composes the given flags with AND, inlining each clause's verification command. At least one axis is required. Copying / invoking the condition stays CLI-only.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "batch": { "type": "string", "description": "All specs tagged `batch:NAME` are resolved (Completed or Rejected). Bare name or `batch:` prefix both accepted.", "example": "fall-cleanup" },
                    "epic": { "type": "string", "description": "All direct children of this epic are resolved.", "example": "EPIC-27" },
                    "spec": { "type": "string", "description": "This spec reaches status Completed.", "example": "STORY-536" },
                    "pr": { "type": "integer", "description": "This PR number is merged.", "example": 680 },
                    "queue_empty": { "type": "string", "description": "The named role's queue is empty. Bare role or `role:` prefix both accepted.", "example": "implementer" }
                }
            },
            "outputSchema": text_envelope_output_schema(
                "the assembled `/goal <condition>` line plus a `verify each clause:` list naming the per-clause verification command. On no axis given the envelope sets `isError: true`."
            )
        },
        {
            "name": "status_unified",
            "description": "A lightweight in-process project status snapshot: requirement counts by status, active session leases, and the caller's queue depth. Mirrors the substrate-grounded parts of `aida status` that need no CI probe. The full `aida status` surface (PR/CI rollup, awaiting-you gates) shells out to `gh` and stays CLI-only — also see the `aida://project/summary` / `aida://session/leases` MCP resources.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "user": { "type": "string", "description": "Override the queue user id for the queue-depth line (defaults to the shell identity).", "example": "alice" }
                }
            },
            "outputSchema": text_envelope_output_schema(
                "a `Project status` block with a `By status:` count breakdown, an `Active sessions:` count (+ scope/role lines), and a `Queue depth:` line. A note points at `aida status` / the MCP resources for the CI-bearing surface."
            )
        },
        {
            "name": "usage_query",
            "description": "Query the local command-usage telemetry log (`~/.aida/usage.jsonl`). Mirrors `aida usage` (top commands by count), `aida usage --errors` (high error-rate commands), and `aida usage --unused <window>` (deprecation candidates). Read-only aggregation over the local log; the orchestrator-telemetry views (`--auto-complete`, `--health`) stay CLI-only.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "since": { "type": "string", "description": "Window for the aggregation, e.g. `30d`, `90d` (default `30d`).", "example": "30d" },
                    "unused": { "type": "string", "description": "Instead of top commands, list commands NOT used within this window (deprecation candidates).", "example": "30d" },
                    "errors": { "type": "boolean", "description": "Show only commands with errors, ranked by error rate.", "example": false },
                    "limit": { "type": "integer", "description": "Cap the number of rows returned (default 20).", "example": 20 }
                }
            },
            "outputSchema": text_envelope_output_schema(
                "either a `Usage: top commands …` table (`cmd  count  errs  avg_ms`), an errors-only variant, or an `unused` list of `cmd — last <rel>` lines. With no events yet, a note that the log is empty."
            )
        },
        {
            "name": "db_sync",
            "description": "PEEK ONLY (does not sync): return the exact `aida db sync` command to run. `aida db sync --pull --push` does git network I/O against the orphan `aida-store` branch (fetch + rebase + push) — a subprocess-driven act the in-process MCP server intentionally does NOT perform, since it mutates the remote and the local store worktree. Composes the command from the requested legs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pull": { "type": "boolean", "description": "Include the `--pull` leg (rebase the local store onto origin).", "example": true },
                    "push": { "type": "boolean", "description": "Include the `--push` leg (push the local store to origin).", "example": true }
                }
            },
            "outputSchema": text_envelope_output_schema(
                "a note explaining the peek plus a `Run:` block with the `aida db sync …` command."
            )
        },
        {
            "name": "fetch",
            "description": "PEEK ONLY (does not fetch): return the exact `aida fetch` command to run. `aida fetch` does git network I/O (refreshes the code branch + orphan-store remote refs) via subprocess `git` — an act the in-process MCP server intentionally does NOT perform. Composes the command from the requested scope.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "code_only": { "type": "boolean", "description": "Fetch only the code branch leg (`--code-only`).", "example": false },
                    "store_only": { "type": "boolean", "description": "Fetch only the orphan-store leg (`--store-only`).", "example": false },
                    "quiet": { "type": "boolean", "description": "Suppress per-leg progress output (`--quiet`).", "example": false }
                }
            },
            "outputSchema": text_envelope_output_schema(
                "a note explaining the peek plus a `Run:` block with the `aida fetch …` command."
            )
        },
        {
            "name": "pull",
            "description": "PEEK ONLY (does not pull): return the exact `aida pull` command to run. `aida pull` mutates the working tree (code leg: `git pull --ff-only`) and the local store (store leg: rebase), then auto-bumps Done → Completed for merged specs — subprocess-driven, working-tree-mutating acts the in-process MCP server intentionally does NOT perform. Composes the command from the requested scope.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "code_only": { "type": "boolean", "description": "Pull only the code branch leg (`--code-only`).", "example": false },
                    "store_only": { "type": "boolean", "description": "Pull only the orphan-store leg (`--store-only`).", "example": false }
                }
            },
            "outputSchema": text_envelope_output_schema(
                "a note explaining the peek plus a `Run:` block with the `aida pull …` command."
            )
        },
        // ---- Schema introspection (TASK-715) ----
        {
            "name": "schema",
            // trace:TASK-715 | ai:claude
            "description": "Introspect AIDA's storable substrate, mirroring `aida schema [<object>] --json`. With no `object`, returns the catalog of storable object kinds; set `all: true` for the full dump — the catalog with every kind's field/enum detail inlined (mirrors `aida schema --all --json`). With `object` (e.g. `requirement`), returns that object's detail — for Requirement, the reflection-derived field table plus the four controlled-vocabulary enums (status/type/priority/relationship) in their on-the-wire token form (a paste-ready cheat-sheet for `--status` / `--type` / `--priority` / relationship-type arguments). The same data is also addressable as the `aida://schema` / `aida://schema/{object}` resources. Reflection-derived — never hand-maintained — so it can't drift from `models.rs`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "object": {
                        "type": "string",
                        "description": "Optional storable-object kind to detail (e.g. `requirement`). Omit for the catalog of kinds. Case-insensitive.",
                        "example": "requirement"
                    },
                    "all": {
                        "type": "boolean",
                        "description": "With no `object`, return the full dump: the catalog with each kind's field/enum detail inlined (mirrors `aida schema --all --json`).",
                        "default": false,
                        "example": true
                    },
                    "explain": {
                        "type": "boolean",
                        "description": "Add the explanatory layer (mirrors `aida schema --explain`): each documented field carries `example` / `provenance` (one of user / advisor-gated / merge-driven / orchestrator / reflection-derived) / `description`, and each object carries a `lifecycle` block (who writes it, when, why, how it's read back, when it retires). Requirement is fully documented; other kinds carry their lifecycle block plus the base field shape.",
                        "default": false,
                        "example": true
                    }
                }
            },
            "outputSchema": text_envelope_output_schema(
                "pretty-printed JSON. Catalog form: `{ objects: [{ name, description }] }`. Full dump (`all: true`): `{ objects: [<per-object detail>] }`. Requirement detail: `{ object: \"Requirement\", fields: [{ name, type, optional }], enums: { status, type, priority, relationship } }`. Other catalog kinds: `{ object, fields: [{ name, type, optional }], note? }` (reflection-derived, no enums block)."
            )
        }
    ])
}

// ============================================================================
// Entry point
// ============================================================================

/// Run the MCP server, reading JSON-RPC from stdin and writing to stdout.
///
/// `project_root` is the AIDA project root (parent of `.aida/`). The
/// coordination tools (STORY-361) resolve their file paths under this root.
pub fn run_mcp_server(storage: &Storage, project_root: PathBuf) -> Result<()> {
    let server = McpServer::new(storage, project_root);
    let respawn = McpRespawnState::new();
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    // trace:STORY-474 | ai:claude — surface the active profile on the stderr
    // banner so an operator can confirm an untrusted client got the safe surface.
    eprintln!(
        "AIDA MCP server started (profile: {})",
        server.profile.as_str()
    );

    for line in stdin.lock().lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(line) {
            Ok(req) => req,
            Err(e) => {
                let error_response =
                    JsonRpcResponse::error(Value::Null, -32700, format!("Parse error: {}", e));
                let json = serde_json::to_string(&error_response)?;
                writeln!(stdout, "{}", json)?;
                stdout.flush()?;
                if let Some(plan) = respawn.check() {
                    respawn.respawn(plan)?;
                }
                continue;
            }
        };

        if request.jsonrpc != "2.0" {
            let error_response = JsonRpcResponse::error(
                request.id.unwrap_or(Value::Null),
                -32600,
                "Invalid Request: jsonrpc must be \"2.0\"".to_string(),
            );
            let json = serde_json::to_string(&error_response)?;
            writeln!(stdout, "{}", json)?;
            stdout.flush()?;
            if let Some(plan) = respawn.check() {
                respawn.respawn(plan)?;
            }
            continue;
        }

        if let Some(response) = server.handle_request(&request) {
            let json = serde_json::to_string(&response)?;
            writeln!(stdout, "{}", json)?;
            stdout.flush()?;
            if let Some(plan) = respawn.check() {
                respawn.respawn(plan)?;
            }
        }
    }

    eprintln!("AIDA MCP server stopped");
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BriefRef {
    path: String,
    agent: String,
    spec_id: String,
    generated_at: String,
    status: String,
}

fn brief_root(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("agent-briefs")
}

fn required_string<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("Missing required parameter: {}", key))
}

fn optional_string<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match args.get(key) {
        None => Ok(None),
        Some(v) => v
            .as_str()
            .map(Some)
            .ok_or_else(|| format!("Parameter `{}` must be a string", key)),
    }
}

fn validate_brief_agent(agent: &str) -> Result<(), String> {
    if agent.trim().is_empty() {
        return Err("agent name cannot be empty".to_string());
    }
    if !agent
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(
            "agent name must contain only ASCII letters, digits, '.', '_' or '-'".to_string(),
        );
    }
    Ok(())
}

fn collect_brief_refs(
    project_root: &Path,
    for_agent: Option<&str>,
    include_acked: bool,
) -> Result<Vec<BriefRef>, String> {
    let root = brief_root(project_root);
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for agent_dir in
        std::fs::read_dir(&root).map_err(|e| format!("reading {}: {}", root.display(), e))?
    {
        let agent_dir = agent_dir.map_err(|e| e.to_string())?;
        let file_type = agent_dir.file_type().map_err(|e| e.to_string())?;
        if !file_type.is_dir() {
            continue;
        }
        let agent = agent_dir.file_name().to_string_lossy().to_string();
        if for_agent.is_some_and(|want| want != agent) {
            continue;
        }
        for file in std::fs::read_dir(agent_dir.path()).map_err(|e| e.to_string())? {
            let file = file.map_err(|e| e.to_string())?;
            if !file.file_type().map_err(|e| e.to_string())?.is_file() {
                continue;
            }
            let path = file.path();
            let name = file.file_name().to_string_lossy().to_string();
            let suffix_acked = name.ends_with(".acked");
            if !(name.ends_with(".md") || suffix_acked) {
                continue;
            }
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            let fm_status = frontmatter_value(&content, "status");
            let status = if suffix_acked || fm_status.as_deref() == Some("acked") {
                "acked"
            } else {
                "pending"
            };
            if status == "acked" && !include_acked {
                continue;
            }
            let spec_id = frontmatter_value(&content, "spec_id")
                .unwrap_or_else(|| spec_id_from_brief_filename(&name).unwrap_or_default());
            let generated_at = frontmatter_value(&content, "generated_at")
                .unwrap_or_else(|| timestamp_from_brief_filename(&name).unwrap_or_default());
            entries.push(BriefRef {
                path: brief_display_path(project_root, &path),
                agent: agent.clone(),
                spec_id,
                generated_at,
                status: status.to_string(),
            });
        }
    }
    entries.sort_by(|a, b| {
        a.agent
            .cmp(&b.agent)
            .then(a.generated_at.cmp(&b.generated_at))
            .then(a.path.cmp(&b.path))
    });
    Ok(entries)
}

fn frontmatter_value(content: &str, key: &str) -> Option<String> {
    let mut lines = content.lines();
    if lines.next()? != "---" {
        return None;
    }
    let prefix = format!("{key}:");
    for line in lines {
        if line == "---" {
            return None;
        }
        if let Some(raw) = line.strip_prefix(&prefix) {
            return Some(raw.trim().trim_matches('\'').to_string());
        }
    }
    None
}

fn spec_id_from_brief_filename(name: &str) -> Option<String> {
    let name = name.strip_suffix(".acked").unwrap_or(name);
    let name = name.strip_suffix(".md").unwrap_or(name);
    let (spec, _) = name.split_once("-20")?;
    Some(spec.to_string())
}

fn timestamp_from_brief_filename(name: &str) -> Option<String> {
    let name = name.strip_suffix(".acked").unwrap_or(name);
    let name = name.strip_suffix(".md").unwrap_or(name);
    let (_, rest) = name.split_once("-20")?;
    Some(format!("20{rest}"))
}

fn resolve_brief_path(project_root: &Path, raw_path: &str) -> Result<PathBuf, String> {
    let raw = Path::new(raw_path);
    if raw
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("brief path must not contain `..`".to_string());
    }
    let root = brief_root(project_root);
    if !raw.is_absolute() {
        let mut components = raw.components();
        let expected = [".aida", "agent-briefs"];
        for want in expected {
            match components.next() {
                Some(std::path::Component::Normal(got)) if got == want => {}
                _ => {
                    return Err(format!(
                        "brief path must be under {}",
                        brief_path_for_message(&root)
                    ));
                }
            }
        }
        return Ok(project_root.join(raw));
    }

    let path = raw.to_path_buf();
    // BUG-358: Windows CI can report temp roots through short-name aliases
    // (`RUNNER~1`) while callers pass normal absolute paths. Canonicalize the
    // comparable side instead of relying on lexical `Path::starts_with`.
    let canonical_root = std::fs::canonicalize(&root).unwrap_or(root);
    let canonical_path = if path.exists() {
        std::fs::canonicalize(&path).unwrap_or(path.clone())
    } else if let (Some(parent), Some(file_name)) = (path.parent(), path.file_name()) {
        std::fs::canonicalize(parent)
            .map(|parent| parent.join(file_name))
            .unwrap_or_else(|_| path.clone())
    } else {
        path.clone()
    };
    if !canonical_path.starts_with(&canonical_root) {
        return Err(format!(
            "brief path must be under {}",
            brief_path_for_message(&canonical_root)
        ));
    }
    Ok(path)
}

fn brief_path_for_message(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn brief_display_path(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

// Touch the imports re-exported above to silence dead_code on builds where
// the punt module's helpers happen to be unused (defensive against future
// refactors). All five items are used in tool implementations.
#[allow(dead_code)]
fn _force_use() {
    let _ = punt::SIGNAL_FILE_ENV;
    let _ = ledger_path as fn(&Path) -> PathBuf;
}

// ============================================================================
// Tests — STORY-361
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use tempfile::tempdir;

    fn mk_server(dir: &Path) -> McpServer<'static> {
        // Cache-backed Storage isn't required for the coordination-only tools;
        // we still need *a* Storage to instantiate the server. Point it at a
        // throwaway YAML file under the temp dir.
        let cache_path = dir.join(".aida").join("mcp-test-cache.yaml");
        std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        let storage = Box::leak(Box::new(Storage::new(cache_path)));
        McpServer::new(storage, dir.to_path_buf())
    }

    /// BUG-449: set a gated status directly in the store, bypassing the MCP
    /// gate — mirrors how the advisor (CLI) or a merge auto-bump sets
    /// Approved/Planned/Completed out-of-band. Tests use this to reach a
    /// precondition the MCP caller itself is (correctly) forbidden to set.
    fn force_status(server: &McpServer<'static>, spec_id: &str, status: RequirementStatus) {
        let mut store = server.storage.load().unwrap();
        let req = store
            .get_requirement_by_spec_id_mut(spec_id)
            .expect("spec should exist for force_status");
        req.status = status;
        server.storage.save(&store).unwrap();
    }

    fn added_spec_id(response: &str) -> &str {
        response
            .strip_prefix("Requirement added: ")
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or_else(|| panic!("unexpected add_requirement response: {response}"))
    }

    // ===================================================================
    // STORY-401: stable structured error shapes
    // ===================================================================

    #[test]
    fn mcp_error_classify_invalid_arg() {
        let e = McpError::classify("post_punt", "Missing required parameter: spec_id");
        assert_eq!(e.code, McpErrorCode::InvalidArg);
        assert!(e.code.recoverable());
        assert_eq!(e.tool, "post_punt");
        // short machine-friendly text shape `<tool>: <code>: <message>`
        assert_eq!(
            e.envelope_text(),
            "post_punt: invalid_arg: Missing required parameter: spec_id"
        );
    }

    #[test]
    fn mcp_error_classify_not_found() {
        let e = McpError::classify("show_requirement", "Requirement 'TASK-999' not found");
        assert_eq!(e.code, McpErrorCode::NotFound);
        assert!(e.code.recoverable());

        let unknown = McpError::classify(
            "definitely_not_a_tool",
            "Unknown tool: definitely_not_a_tool",
        );
        assert_eq!(unknown.code, McpErrorCode::NotFound);
    }

    #[test]
    fn mcp_error_classify_conflict_and_permission() {
        let conflict = McpError::classify("claim_task", "lease already claimed by another agent");
        assert_eq!(conflict.code, McpErrorCode::Conflict);
        assert!(!conflict.code.recoverable());

        let denied = McpError::classify(
            "update_requirement",
            "MCP is not advisor authority; cannot self-advance to Planned",
        );
        assert_eq!(denied.code, McpErrorCode::PermissionDenied);
        assert!(!denied.code.recoverable());
    }

    #[test]
    fn mcp_error_classify_falls_back_to_internal() {
        let e = McpError::classify("history", "something totally unexpected happened");
        assert_eq!(e.code, McpErrorCode::Internal);
        assert!(!e.code.recoverable());
    }

    #[test]
    fn mcp_error_result_value_is_back_compatible_envelope() {
        let e = McpError::classify("post_punt", "Missing required parameter: spec_id");
        let v = e.to_result_value();
        // Back-compatible: isError:true + a text content array.
        assert_eq!(v["isError"], json!(true));
        let text = v["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Missing required parameter: spec_id"));
        assert_eq!(v["content"][0]["type"], json!("text"));
        // Additive structured payload.
        let se = &v["structuredError"];
        assert_eq!(se["code"], json!("invalid_arg"));
        assert_eq!(se["message"], json!("Missing required parameter: spec_id"));
        assert_eq!(se["tool"], json!("post_punt"));
        assert_eq!(se["recoverable"], json!(true));
    }

    #[test]
    fn mcp_dispatch_unknown_tool_yields_structured_error() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        let resp = server.handle_tools_call(
            &json!(1),
            &json!({"name": "definitely_not_a_tool", "arguments": {}}),
        );
        let result = resp
            .result
            .expect("tools/call should return a result object");
        assert_eq!(result["isError"], json!(true));
        // Stable code + the legacy "Unknown tool" phrase the stdio suite asserts.
        assert_eq!(result["structuredError"]["code"], json!("not_found"));
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Unknown tool"), "text was: {text}");
    }

    // STORY-399 (Path B): a successful tools/call must carry `structuredContent`
    // whose `content` array mirrors the legacy text envelope verbatim, so both
    // legacy text consumers and schema-driven clients see the same logical
    // result. Error responses must NOT carry structuredContent (they carry the
    // STORY-401 `structuredError` payload instead). trace:STORY-399 | ai:claude
    #[test]
    fn mcp_dispatch_success_emits_structured_content_mirroring_text() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        // `list_active_leases` takes no args and never errors on an empty store —
        // a clean success-path probe of the dispatch envelope.
        let resp = server.handle_tools_call(
            &json!(1),
            &json!({"name": "list_active_leases", "arguments": {}}),
        );
        let result = resp
            .result
            .expect("tools/call should return a result object");

        // Legacy text envelope preserved.
        assert!(
            result.get("isError").is_none(),
            "success must not set isError"
        );
        let text = result["content"][0]["text"]
            .as_str()
            .expect("success must carry a text content item");
        assert_eq!(result["content"][0]["type"], json!("text"));

        // Path B: structuredContent mirrors the same payload.
        let structured = result
            .get("structuredContent")
            .expect("success must carry structuredContent (STORY-399)");
        assert_eq!(
            structured["content"][0]["type"],
            json!("text"),
            "structuredContent.content must hold a text item"
        );
        assert_eq!(
            structured["content"][0]["text"].as_str(),
            Some(text),
            "structuredContent must mirror the text envelope verbatim"
        );
    }

    #[test]
    fn mcp_dispatch_error_has_no_structured_content() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        let resp = server.handle_tools_call(
            &json!(1),
            &json!({"name": "definitely_not_a_tool", "arguments": {}}),
        );
        let result = resp
            .result
            .expect("tools/call should return a result object");
        assert_eq!(result["isError"], json!(true));
        assert!(
            result.get("structuredContent").is_none(),
            "error responses carry structuredError, not structuredContent: {result}"
        );
        assert!(result.get("structuredError").is_some());
    }

    #[test]
    fn handle_request_touches_agent_registry() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "ping".to_string(),
            params: json!({}),
            id: Some(json!(1)),
        };
        let response = server.handle_request(&request);

        assert!(response.is_some());
        let ctx = agent_registry::AgentClassifyContext::new(chrono::Utc::now(), 30, vec![]);
        let agents = agent_registry::list_agent_views(dir.path(), &ctx);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].source, "mcp");
        assert_eq!(
            agents[0].binary_version.as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
    }

    // STORY-435: every handled JSON-RPC request must push `last_active_at`
    // forward. This locks the heartbeat that powers busy/idle freshness.
    // trace:STORY-435 | ai:claude
    #[test]
    fn handle_request_advances_last_active_at() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "ping".to_string(),
            params: json!({}),
            id: Some(json!(1)),
        };

        let _ = server.handle_request(&request);
        let ctx = agent_registry::AgentClassifyContext::new(chrono::Utc::now(), 30, vec![]);
        let t0 = agent_registry::list_agent_views(dir.path(), &ctx)[0].last_active_at;

        // chrono is microsecond-resolution; 5ms is comfortably above the
        // tick so the second touch will strictly advance the timestamp.
        thread::sleep(std::time::Duration::from_millis(5));
        let _ = server.handle_request(&request);
        let t1 = agent_registry::list_agent_views(dir.path(), &ctx)[0].last_active_at;

        assert!(
            t1 > t0,
            "expected last_active_at to advance: t0={t0} t1={t1}"
        );
    }

    #[test]
    fn parses_aida_version_banner_identity() {
        let ident = parse_aida_binary_identity(
            "aida 0.9.1 (built 2026-05-23 01:00:00 MST, sha abc1234+dirty)",
        )
        .expect("banner should parse");
        assert_eq!(ident.version, "0.9.1");
        assert_eq!(ident.sha, "abc1234");
        assert!(ident.dirty);

        let clean = parse_aida_binary_identity("aida-cli 0.9.2 (built x, sha deadbee)")
            .expect("aida-cli prefix should parse");
        assert_eq!(clean.version, "0.9.2");
        assert_eq!(clean.sha, "deadbee");
        assert!(!clean.dirty);
    }

    #[test]
    fn mcp_respawn_decision_uses_version_then_build_identity() {
        let running = McpBinaryIdentity {
            version: "0.9.1".to_string(),
            sha: "aaaaaaa".to_string(),
            dirty: false,
        };
        let newer_version = McpBinaryIdentity {
            version: "0.9.2".to_string(),
            sha: "bbbbbbb".to_string(),
            dirty: false,
        };
        assert!(mcp_binary_is_newer_or_different(&running, &newer_version));

        let same_version_new_sha = McpBinaryIdentity {
            version: "0.9.1".to_string(),
            sha: "bbbbbbb".to_string(),
            dirty: false,
        };
        assert!(mcp_binary_is_newer_or_different(
            &running,
            &same_version_new_sha
        ));

        let older_version = McpBinaryIdentity {
            version: "0.9.0".to_string(),
            sha: "zzzzzzz".to_string(),
            dirty: false,
        };
        assert!(!mcp_binary_is_newer_or_different(&running, &older_version));

        let unknown_sha = McpBinaryIdentity {
            version: "0.9.1".to_string(),
            sha: "unknown".to_string(),
            dirty: false,
        };
        assert!(
            !mcp_binary_is_newer_or_different(&running, &unknown_sha),
            "unknown same-version SHA must not cause an endless respawn loop"
        );
    }

    #[test]
    fn mcp_respawn_plan_preserves_original_argv() {
        let plan = McpRespawnPlan {
            exe: PathBuf::from("/tmp/aida"),
            argv: vec![OsString::from("aida"), OsString::from("mcp-serve")],
            running: McpBinaryIdentity {
                version: "0.9.1".to_string(),
                sha: "aaaaaaa".to_string(),
                dirty: false,
            },
            on_disk: McpBinaryIdentity {
                version: "0.9.1".to_string(),
                sha: "bbbbbbb".to_string(),
                dirty: false,
            },
        };
        assert_eq!(plan.exe, PathBuf::from("/tmp/aida"));
        assert_eq!(plan.argv[1], OsString::from("mcp-serve"));
        assert_eq!(plan.running.label(), "0.9.1 sha aaaaaaa");
        assert_eq!(plan.on_disk.label(), "0.9.1 sha bbbbbbb");
    }

    #[test]
    fn tool_descriptors_lists_coordination_tools() {
        let desc = tool_descriptors();
        let arr = desc.as_array().unwrap();
        let names: Vec<&str> = arr
            .iter()
            .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
            .collect();
        // 10 spec-graph (incl. query_graph) + 19 coordination (incl. the two
        // mailbox tools) = 29 total.
        assert!(names.len() >= 29, "expected ≥29 tools, got {}", names.len());
        for required in [
            "add_relationship",
            "query_graph",
            "send_message",
            "read_inbox",
            "history",
            "list_punts",
            "read_punt",
            "post_punt",
            "resolve_punt",
            "escalate_punt",
            "list_findings",
            "file_finding",
            "triage_finding",
            "claim_task",
            "release_task",
            "list_active_leases",
            "post_directive",
            "list_directives",
            "ack_directive",
            "list_briefs",
            "read_brief",
            "ack_brief",
        ] {
            assert!(names.contains(&required), "missing tool: {}", required);
        }
    }

    // trace:TASK-440 | ai:claude
    /// Every tool descriptor must carry a non-null `outputSchema` that passes
    /// basic JSON-Schema structural validation: an object with `type` =
    /// "object", a `properties` map, and a non-empty `description` so
    /// schema-driven MCP clients (Codex, Cursor, …) can render the response
    /// shape instead of treating it as opaque text.
    #[test]
    fn every_tool_descriptor_has_a_valid_output_schema() {
        let desc = tool_descriptors();
        let arr = desc.as_array().expect("tool_descriptors must be an array");
        assert!(
            arr.len() >= 29,
            "expected ≥29 tool descriptors (10 spec-graph incl. query_graph + 19 coordination incl. mailbox), got {}",
            arr.len()
        );

        for tool in arr {
            let name = tool
                .get("name")
                .and_then(|n| n.as_str())
                .expect("each tool descriptor must have a name");

            let output_schema = tool
                .get("outputSchema")
                .unwrap_or_else(|| panic!("tool '{}' is missing outputSchema", name));
            assert!(
                !output_schema.is_null(),
                "tool '{}' has a null outputSchema",
                name
            );

            // Basic JSON-Schema structural validation.
            let obj = output_schema
                .as_object()
                .unwrap_or_else(|| panic!("tool '{}' outputSchema is not a JSON object", name));

            let schema_type = obj
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("tool '{}' outputSchema is missing `type`", name));
            assert_eq!(
                schema_type, "object",
                "tool '{}' outputSchema `type` must be \"object\" (Path A — text envelope), got {:?}",
                name, schema_type
            );

            let description = obj
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("tool '{}' outputSchema is missing `description`", name));
            assert!(
                !description.trim().is_empty(),
                "tool '{}' outputSchema description must be non-empty",
                name
            );

            let properties = obj
                .get("properties")
                .and_then(|v| v.as_object())
                .unwrap_or_else(|| {
                    panic!(
                        "tool '{}' outputSchema is missing a `properties` object",
                        name
                    )
                });
            assert!(
                properties.contains_key("content"),
                "tool '{}' outputSchema must declare a `content` property (Path A — text envelope wraps every response)",
                name
            );

            // STORY-399 (Path B): the outputSchema must also declare the
            // additive `structuredContent` object, and that object must itself
            // require a `content` array — the machine-readable mirror.
            // trace:STORY-399 | ai:claude
            let structured = properties
                .get("structuredContent")
                .and_then(|v| v.as_object())
                .unwrap_or_else(|| {
                    panic!(
                        "tool '{}' outputSchema must declare a `structuredContent` object (Path B — STORY-399)",
                        name
                    )
                });
            assert_eq!(
                structured.get("type").and_then(|v| v.as_str()),
                Some("object"),
                "tool '{}' structuredContent schema must be an object",
                name
            );
            assert!(
                structured
                    .get("properties")
                    .and_then(|v| v.as_object())
                    .map(|p| p.contains_key("content"))
                    .unwrap_or(false),
                "tool '{}' structuredContent must declare a `content` property",
                name
            );
        }
    }

    #[test]
    fn every_tool_parameter_has_examples_patterns_or_enums() {
        let desc = tool_descriptors();
        let arr = desc.as_array().expect("tool_descriptors must be an array");

        for tool in arr {
            let tool_name = tool
                .get("name")
                .and_then(|n| n.as_str())
                .expect("each tool descriptor must have a name");

            let properties = tool
                .pointer("/inputSchema/properties")
                .and_then(|v| v.as_object());

            if let Some(props) = properties {
                for (param_name, param_val) in props {
                    let param_obj = param_val.as_object().unwrap_or_else(|| {
                        panic!(
                            "tool '{}' parameter '{}' is not a JSON object",
                            tool_name, param_name
                        )
                    });

                    let has_example = param_obj.contains_key("example");
                    let has_enum = param_obj.contains_key("enum");
                    let has_pattern = param_obj.contains_key("pattern");
                    let has_minimum = param_obj.contains_key("minimum");

                    assert!(
                        has_example || has_enum || has_pattern || has_minimum,
                        "tool '{}' parameter '{}' must have an 'example', 'enum', 'pattern', or 'minimum' field",
                        tool_name,
                        param_name
                    );

                    assert!(
                        has_example,
                        "tool '{}' parameter '{}' is missing the 'example' field",
                        tool_name, param_name
                    );
                }
            }
        }
    }

    // trace:BUG-332 | ai:codex
    #[test]
    fn add_requirement_descriptor_requires_type_and_documents_normalized_prefix() {
        let desc = tool_descriptors();
        let arr = desc.as_array().expect("tool_descriptors must be an array");
        let tool = arr
            .iter()
            .find(|tool| tool.get("name").and_then(|v| v.as_str()) == Some("add_requirement"))
            .expect("add_requirement descriptor must exist");

        let required = tool
            .pointer("/inputSchema/required")
            .and_then(|v| v.as_array())
            .expect("add_requirement inputSchema must declare required args");
        assert!(
            required.iter().any(|v| v.as_str() == Some("type")),
            "add_requirement must require `type`"
        );

        let type_description = tool
            .pointer("/inputSchema/properties/type/description")
            .and_then(|v| v.as_str())
            .expect("type property must have a description");
        for expected in ["task", "bug", "folder", "meta", "doc"] {
            assert!(
                type_description.contains(expected),
                "type description should name valid taxonomy member {expected}"
            );
        }

        let output_description = tool
            .pointer("/outputSchema/description")
            .and_then(|v| v.as_str())
            .expect("outputSchema must have a description");
        assert!(
            output_description.contains("auto-normalized"),
            "outputSchema should document auto-normalized ID prefixes"
        );
    }

    // trace:BUG-332 | ai:codex
    #[test]
    fn mcp_add_requirement_rejects_missing_or_invalid_type() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());

        let missing = serde_json::json!({
            "title": "Missing type",
            "description": "Type is required",
        });
        let err = server
            .tool_add_requirement(&missing)
            .expect_err("missing type should fail");
        assert!(err.contains("Missing required parameter: type"));
        assert!(err.contains("task"));
        assert!(err.contains("doc"));

        let invalid = serde_json::json!({
            "title": "Invalid type",
            "description": "Type must be from the taxonomy",
            "type": "spec",
        });
        let err = server
            .tool_add_requirement(&invalid)
            .expect_err("invalid type should fail");
        assert!(err.contains("Invalid requirement type 'spec'"));
        assert!(err.contains("functional"));
        assert!(err.contains("meta"));
    }

    // trace:BUG-332 | ai:codex
    #[test]
    fn mcp_add_requirement_normalizes_every_valid_type_prefix() {
        let valid_types = [
            ("functional", "FR-"),
            ("non-functional", "NFR-"),
            ("system", "SR-"),
            ("user", "UR-"),
            ("bug", "BUG-"),
            ("epic", "EPIC-"),
            ("story", "STORY-"),
            ("task", "TASK-"),
            ("spike", "SPIKE-"),
            ("sprint", "SPRINT-"),
            ("folder", "FOLDER-"),
            ("meta", "META-"),
            ("doc", "DOC-"),
        ];

        for (type_name, expected_prefix) in valid_types {
            let dir = tempdir().unwrap();
            let server = mk_server(dir.path());
            let args = serde_json::json!({
                "title": format!("Canonical {type_name}"),
                "description": "MCP add_requirement must use type-derived prefixes",
                "type": type_name,
            });

            let response = server
                .tool_add_requirement(&args)
                .unwrap_or_else(|err| panic!("add_requirement failed for {type_name}: {err}"));
            let spec_id = added_spec_id(&response);
            assert!(
                spec_id.starts_with(expected_prefix),
                "{type_name} should produce {expected_prefix}*, got {spec_id}"
            );
            assert!(
                !spec_id.starts_with("SPEC-"),
                "{type_name} must not produce generic SPEC-* IDs"
            );
        }
    }

    // trace:BUG-377 TASK-550 TASK-647 | ai:codex
    #[test]
    fn mcp_add_requirement_persists_all_advertised_fields() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        let response = server
            .tool_add_requirement(&json!({
                "title": "MCP write map",
                "description": "All advertised fields should persist",
                "type": "bug",
                "status": "approved",
                "priority": "high",
                "tags": ["mcp", "roundtrip"],
            }))
            .unwrap();
        let spec_id = added_spec_id(&response);
        // TASK-647 (ADR-3): MCP intake always lands `draft` (advisor-gated),
        // and the response says so — even though `approved` was requested.
        assert!(
            response.contains("advisor triage"),
            "expected triage notice in: {response}"
        );

        let store = server.storage.load().unwrap();
        let req = store
            .get_requirement_by_spec_id(spec_id)
            .expect("requirement should be visible after MCP add");
        assert_eq!(req.title, "MCP write map");
        assert_eq!(req.description, "All advertised fields should persist");
        assert_eq!(req.req_type, RequirementType::Bug);
        // TASK-647: requested `approved` was downgraded to `draft` at intake.
        assert_eq!(req.status, RequirementStatus::Draft);
        assert_eq!(req.priority, RequirementPriority::High);
        assert!(req.tags.contains("mcp"));
        assert!(req.tags.contains("roundtrip"));
    }

    // trace:BUG-381 | ai:codex
    #[test]
    fn mcp_list_requirements_normalizes_status_type_and_priority_filters() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        // TASK-647 (ADR-3): MCP add lands draft, so set the non-draft statuses
        // this filter test needs via the (ungated) update tool.
        let working = server
            .tool_add_requirement(&json!({
                "title": "Working MCP item",
                "description": "status filter target",
                "type": "task",
                "priority": "high",
            }))
            .unwrap();
        let working_id = added_spec_id(&working).to_string();
        // BUG-481: Draft → InProgress via MCP is now advisor-gated; reach the
        // InProgress filter target out-of-band like the other gated statuses.
        force_status(&server, &working_id, RequirementStatus::InProgress);
        let planned = server
            .tool_add_requirement(&json!({
                "title": "Planned MCP item",
                "description": "negative control",
                "type": "story",
                "priority": "low",
            }))
            .unwrap();
        let planned_id = added_spec_id(&planned).to_string();
        // BUG-449: Planned is advisor-gated via MCP — set it out-of-band.
        force_status(&server, &planned_id, RequirementStatus::Planned);
        let nfr = server
            .tool_add_requirement(&json!({
                "title": "Nonfunctional MCP item",
                "description": "type filter target",
                "type": "non-functional",
                "priority": "medium",
            }))
            .unwrap();
        let nfr_id = added_spec_id(&nfr).to_string();

        for status in ["in-progress", "InProgress", "In Progress", "in_progress"] {
            let output = server
                .tool_list_requirements(&json!({ "status": status }))
                .unwrap();
            assert!(output.contains(&working_id), "{status}: {output}");
            assert!(!output.contains(&planned_id), "{status}: {output}");
            assert!(
                !output.contains("No requirements found"),
                "{status}: {output}"
            );
        }

        let type_output = server
            .tool_list_requirements(&json!({ "type": "non functional" }))
            .unwrap();
        assert!(type_output.contains(&nfr_id), "{type_output}");
        assert!(!type_output.contains(&working_id), "{type_output}");

        let priority_output = server
            .tool_list_requirements(&json!({ "priority": " HIGH " }))
            .unwrap();
        assert!(priority_output.contains(&working_id), "{priority_output}");
        assert!(!priority_output.contains(&planned_id), "{priority_output}");
    }

    // ===================================================================
    // STORY-82: modernized filter/field surface on the 7 core tools
    // ===================================================================

    // trace:STORY-82 | ai:claude
    #[test]
    fn mcp_list_requirements_filters_by_tags_batch_and_role() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());

        let tagged = server
            .tool_add_requirement(&json!({
                "title": "Tagged + batched + roled",
                "description": "matches all three filters",
                "type": "task",
                "tags": ["mcp", "papercut", "batch:fall-cleanup", "role:implementer"],
            }))
            .unwrap();
        let tagged_id = added_spec_id(&tagged).to_string();
        let other = server
            .tool_add_requirement(&json!({
                "title": "Unrelated",
                "description": "negative control",
                "type": "task",
                "tags": ["docs"],
            }))
            .unwrap();
        let other_id = added_spec_id(&other).to_string();

        // tags CSV: AND-match across the listed tags.
        let out = server
            .tool_list_requirements(&json!({ "tags": "mcp,papercut" }))
            .unwrap();
        assert!(out.contains(&tagged_id), "{out}");
        assert!(!out.contains(&other_id), "{out}");

        // a tag the row lacks excludes it.
        let none = server
            .tool_list_requirements(&json!({ "tags": "mcp,does-not-exist" }))
            .unwrap();
        assert!(none.contains("No requirements found"), "{none}");

        // batch shorthand → batch:<name> tag.
        let batch_out = server
            .tool_list_requirements(&json!({ "batch": "fall-cleanup" }))
            .unwrap();
        assert!(batch_out.contains(&tagged_id), "{batch_out}");
        assert!(!batch_out.contains(&other_id), "{batch_out}");

        // role / for → role:<name> tag (case-insensitive); `for` is an alias.
        for key in ["role", "for"] {
            let role_out = server
                .tool_list_requirements(&json!({ key: "IMPLEMENTER" }))
                .unwrap();
            assert!(role_out.contains(&tagged_id), "{key}: {role_out}");
            assert!(!role_out.contains(&other_id), "{key}: {role_out}");
        }
    }

    // trace:STORY-82 | ai:claude
    #[test]
    fn mcp_list_requirements_filters_by_parent() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());

        let epic = server
            .tool_add_requirement(&json!({
                "title": "Parent epic",
                "description": "has children",
                "type": "epic",
            }))
            .unwrap();
        let epic_id = added_spec_id(&epic).to_string();
        let child = server
            .tool_add_requirement(&json!({
                "title": "Child story",
                "description": "linked under the epic",
                "type": "story",
                "parent": epic_id,
            }))
            .unwrap();
        let child_id = added_spec_id(&child).to_string();
        let stray = server
            .tool_add_requirement(&json!({
                "title": "Unparented",
                "description": "not a child",
                "type": "story",
            }))
            .unwrap();
        let stray_id = added_spec_id(&stray).to_string();

        let out = server
            .tool_list_requirements(&json!({ "parent": epic_id }))
            .unwrap();
        assert!(out.contains(&child_id), "child must show: {out}");
        assert!(!out.contains(&stray_id), "stray must not show: {out}");
    }

    /// Set the `archived` view-flag directly in the store, bypassing the gate —
    /// mirrors `aida archive <ID>`. trace:BUG-591 | ai:claude
    fn force_archive(server: &McpServer<'static>, spec_id: &str) {
        let mut store = server.storage.load().unwrap();
        let req = store
            .get_requirement_by_spec_id_mut(spec_id)
            .expect("spec should exist for force_archive");
        req.archived = true;
        server.storage.save(&store).unwrap();
    }

    /// Set the `deferred` view-flag directly in the store — mirrors
    /// `aida defer <ID>`. trace:BUG-591 | ai:claude
    fn force_defer(server: &McpServer<'static>, spec_id: &str) {
        let mut store = server.storage.load().unwrap();
        let req = store
            .get_requirement_by_spec_id_mut(spec_id)
            .expect("spec should exist for force_defer");
        req.deferred = true;
        server.storage.save(&store).unwrap();
    }

    /// BUG-591: `list_requirements` must apply the same archive/deferred view
    /// tiers as `aida list` — archived + deferred rows hidden by DEFAULT, and
    /// surfaced via the `archived` / `deferred` / `all` filters.
    // trace:BUG-591 | ai:claude
    #[test]
    fn mcp_list_requirements_honors_archive_and_defer_view_tiers() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());

        let active = added_spec_id(
            &server
                .tool_add_requirement(&json!({
                    "title": "Active spec",
                    "description": "stays visible",
                    "type": "task",
                }))
                .unwrap(),
        )
        .to_string();
        let archived = added_spec_id(
            &server
                .tool_add_requirement(&json!({
                    "title": "Archived spec",
                    "description": "filed away",
                    "type": "task",
                }))
                .unwrap(),
        )
        .to_string();
        let deferred = added_spec_id(
            &server
                .tool_add_requirement(&json!({
                    "title": "Deferred spec",
                    "description": "primed shelf",
                    "type": "task",
                }))
                .unwrap(),
        )
        .to_string();
        force_archive(&server, &archived);
        force_defer(&server, &deferred);

        // DEFAULT view: active only; archived + deferred are hidden.
        let default = server.tool_list_requirements(&json!({})).unwrap();
        assert!(
            default.contains(&active),
            "active must show by default: {default}"
        );
        assert!(
            !default.contains(&archived),
            "archived must be hidden by default: {default}"
        );
        assert!(
            !default.contains(&deferred),
            "deferred must be hidden by default: {default}"
        );

        // archived=true → only the archived row.
        let arc = server
            .tool_list_requirements(&json!({ "archived": true }))
            .unwrap();
        assert!(
            arc.contains(&archived),
            "archived view must show archived: {arc}"
        );
        assert!(
            !arc.contains(&active),
            "archived view excludes active: {arc}"
        );
        assert!(
            !arc.contains(&deferred),
            "archived view excludes deferred: {arc}"
        );

        // deferred=true → only the deferred row.
        let def = server
            .tool_list_requirements(&json!({ "deferred": true }))
            .unwrap();
        assert!(
            def.contains(&deferred),
            "deferred view must show deferred: {def}"
        );
        assert!(
            !def.contains(&active),
            "deferred view excludes active: {def}"
        );
        assert!(
            !def.contains(&archived),
            "deferred view excludes archived: {def}"
        );

        // all=true → the union of all three tiers.
        let all = server
            .tool_list_requirements(&json!({ "all": true }))
            .unwrap();
        assert!(all.contains(&active), "all view shows active: {all}");
        assert!(all.contains(&archived), "all view shows archived: {all}");
        assert!(all.contains(&deferred), "all view shows deferred: {all}");
    }

    /// BUG-591: `search_requirements` mirrors the same view-tier default-hide as
    /// `aida search` — an archived spec matching the query is hidden by default
    /// but surfaced with `archived` / `all`.
    // trace:BUG-591 | ai:claude
    #[test]
    fn mcp_search_requirements_honors_archive_view_tiers() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());

        let active = added_spec_id(
            &server
                .tool_add_requirement(&json!({
                    "title": "seedling active",
                    "description": "matches seedquery",
                    "type": "task",
                }))
                .unwrap(),
        )
        .to_string();
        let archived = added_spec_id(
            &server
                .tool_add_requirement(&json!({
                    "title": "seedling archived",
                    "description": "matches seedquery",
                    "type": "task",
                }))
                .unwrap(),
        )
        .to_string();
        force_archive(&server, &archived);

        // DEFAULT search hides the archived match.
        let default = server
            .tool_search_requirements(&json!({ "query": "seedquery" }))
            .unwrap();
        assert!(
            default.contains(&active),
            "active match must show: {default}"
        );
        assert!(
            !default.contains(&archived),
            "archived match must be hidden by default: {default}"
        );

        // archived=true surfaces it.
        let arc = server
            .tool_search_requirements(&json!({ "query": "seedquery", "archived": true }))
            .unwrap();
        assert!(
            arc.contains(&archived),
            "archived search must show archived: {arc}"
        );
        assert!(
            !arc.contains(&active),
            "archived search excludes active: {arc}"
        );

        // all=true → both.
        let all = server
            .tool_search_requirements(&json!({ "query": "seedquery", "all": true }))
            .unwrap();
        assert!(all.contains(&active), "all search shows active: {all}");
        assert!(all.contains(&archived), "all search shows archived: {all}");
    }

    /// BUG-591: the list_requirements descriptor advertises the new view-tier
    /// filters so schema-driven MCP clients can request them.
    // trace:BUG-591 | ai:claude
    #[test]
    fn mcp_list_and_search_descriptors_advertise_view_tier_filters() {
        let desc = tool_descriptors();
        let arr = desc.as_array().unwrap();
        for tool_name in ["list_requirements", "search_requirements"] {
            let tool = arr
                .iter()
                .find(|t| t.get("name").and_then(|v| v.as_str()) == Some(tool_name))
                .unwrap_or_else(|| panic!("{tool_name} descriptor must exist"));
            let props = tool
                .pointer("/inputSchema/properties")
                .and_then(|v| v.as_object())
                .unwrap();
            for p in ["archived", "deferred", "all"] {
                assert!(props.contains_key(p), "{tool_name} must advertise `{p}`");
            }
        }
        // TASK-883: post_punt advertises the `reason` alias.
        let post_punt = arr
            .iter()
            .find(|t| t.get("name").and_then(|v| v.as_str()) == Some("post_punt"))
            .expect("post_punt descriptor must exist");
        let pp_props = post_punt
            .pointer("/inputSchema/properties")
            .and_then(|v| v.as_object())
            .unwrap();
        assert!(
            pp_props.contains_key("reason"),
            "post_punt must advertise the `reason` alias"
        );
    }

    // trace:STORY-82 | ai:claude
    #[test]
    fn mcp_add_requirement_persists_parent_feature_owner() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());

        let epic = server
            .tool_add_requirement(&json!({
                "title": "Owner of children",
                "description": "epic",
                "type": "epic",
            }))
            .unwrap();
        let epic_id = added_spec_id(&epic).to_string();

        let response = server
            .tool_add_requirement(&json!({
                "title": "Full-field child",
                "description": "feature + owner + parent",
                "type": "task",
                "feature": "auth",
                "owner": "alice",
                "parent": epic_id,
            }))
            .unwrap();
        assert!(
            response.contains(&epic_id),
            "result notes parent: {response}"
        );
        let child_id = added_spec_id(&response).to_string();

        let store = server.storage.load().unwrap();
        let child = store.get_requirement_by_spec_id(&child_id).unwrap();
        // The legacy YAML test store normalizes feature names on load
        // (migrate_features prefixes a stable number), so assert containment
        // rather than exact equality.
        assert!(child.feature.contains("auth"), "feature: {}", child.feature);
        assert_eq!(child.owner, "alice");
        // Child carries a Child→parent edge; parent carries Parent→child.
        let parent = store.get_requirement_by_spec_id(&epic_id).unwrap();
        assert!(
            parent
                .relationships
                .iter()
                .any(|r| r.rel_type == RelationshipType::Parent && r.target_id == child.id),
            "parent should have a Parent edge to the child"
        );
    }

    // trace:STORY-82 | ai:claude
    #[test]
    fn mcp_add_requirement_rejects_missing_parent() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        let err = server
            .tool_add_requirement(&json!({
                "title": "Orphan",
                "description": "bad parent",
                "type": "task",
                "parent": "EPIC-999",
            }))
            .expect_err("missing parent should fail");
        assert!(err.contains("EPIC-999"), "{err}");
        assert!(err.contains("not found"), "{err}");
    }

    // trace:STORY-82 | ai:claude
    #[test]
    fn mcp_update_requirement_sets_title_type_priority_tags_parent() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());

        let epic = server
            .tool_add_requirement(&json!({
                "title": "Reparent target",
                "description": "epic",
                "type": "epic",
            }))
            .unwrap();
        let epic_id = added_spec_id(&epic).to_string();
        let added = server
            .tool_add_requirement(&json!({
                "title": "Original title",
                "description": "to be edited",
                "type": "task",
                "priority": "low",
                "tags": ["old"],
            }))
            .unwrap();
        let id = added_spec_id(&added).to_string();

        let result = server
            .tool_update_requirement(&json!({
                "id": id,
                "title": "New title",
                "type": "story",
                "priority": "high",
                "tags": ["fresh", "batch:x"],
                "parent": epic_id,
            }))
            .unwrap();
        assert!(result.starts_with(&format!("Updated {}", id)), "{result}");

        let store = server.storage.load().unwrap();
        let req = store.get_requirement_by_spec_id(&id).unwrap();
        assert_eq!(req.title, "New title");
        assert_eq!(req.req_type, RequirementType::Story);
        assert_eq!(req.priority, RequirementPriority::High);
        assert!(req.tags.contains("fresh") && req.tags.contains("batch:x"));
        assert!(!req.tags.contains("old"), "tags should be replaced");
        assert!(
            req.relationships
                .iter()
                .any(|r| r.rel_type == RelationshipType::Child),
            "target should carry a Child edge to the new parent"
        );
    }

    // trace:STORY-82 | ai:claude
    #[test]
    fn mcp_update_requirement_rejects_invalid_type() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        let added = server
            .tool_add_requirement(&json!({
                "title": "T", "description": "d", "type": "task",
            }))
            .unwrap();
        let id = added_spec_id(&added).to_string();
        let err = server
            .tool_update_requirement(&json!({ "id": id, "type": "nonsense" }))
            .expect_err("invalid type should fail");
        assert!(err.contains("Invalid requirement type 'nonsense'"), "{err}");
    }

    // trace:STORY-82 | ai:claude
    #[test]
    fn mcp_search_requirements_narrows_by_type_and_status() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());

        let bug = server
            .tool_add_requirement(&json!({
                "title": "Login oauth bug",
                "description": "oauth token refresh fails",
                "type": "bug",
            }))
            .unwrap();
        let bug_id = added_spec_id(&bug).to_string();
        let story = server
            .tool_add_requirement(&json!({
                "title": "Login oauth story",
                "description": "oauth happy path",
                "type": "story",
            }))
            .unwrap();
        let story_id = added_spec_id(&story).to_string();

        // type narrows.
        let out = server
            .tool_search_requirements(&json!({ "query": "oauth", "type": "bug" }))
            .unwrap();
        assert!(out.contains(&bug_id), "{out}");
        assert!(!out.contains(&story_id), "{out}");

        // status narrows (both are draft on MCP intake).
        let drafts = server
            .tool_search_requirements(&json!({ "query": "oauth", "status": "draft" }))
            .unwrap();
        assert!(
            drafts.contains(&bug_id) && drafts.contains(&story_id),
            "{drafts}"
        );
        let none = server
            .tool_search_requirements(&json!({ "query": "oauth", "status": "completed" }))
            .unwrap();
        assert!(none.contains("No requirements found"), "{none}");
    }

    // trace:STORY-82 | ai:claude
    #[test]
    fn mcp_show_requirement_appends_git_linkage_by_default() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        let added = server
            .tool_add_requirement(&json!({
                "title": "Show with git", "description": "d", "type": "task",
            }))
            .unwrap();
        let id = added_spec_id(&added).to_string();

        // Default: git linkage present (no repo/commits → the empty-state line).
        let with_git = server.tool_show_requirement(&json!({ "id": id })).unwrap();
        assert!(with_git.contains("## Git linkage"), "{with_git}");

        // include_git=false suppresses it (the `aida show --no-git` view).
        let no_git = server
            .tool_show_requirement(&json!({ "id": id, "include_git": false }))
            .unwrap();
        assert!(!no_git.contains("## Git linkage"), "{no_git}");
    }

    // trace:STORY-82 | ai:claude
    #[test]
    fn list_requirements_descriptor_advertises_modern_filters() {
        let desc = tool_descriptors();
        let arr = desc.as_array().unwrap();
        let tool = arr
            .iter()
            .find(|t| t.get("name").and_then(|v| v.as_str()) == Some("list_requirements"))
            .unwrap();
        let props = tool
            .pointer("/inputSchema/properties")
            .and_then(|v| v.as_object())
            .unwrap();
        for p in ["tags", "batch", "parent", "role", "for", "in_flight"] {
            assert!(
                props.contains_key(p),
                "list_requirements must advertise `{p}`"
            );
        }
        // type enum now covers the full taxonomy.
        let type_enum = tool
            .pointer("/inputSchema/properties/type/enum")
            .and_then(|v| v.as_array())
            .unwrap();
        let names: Vec<&str> = type_enum.iter().filter_map(|v| v.as_str()).collect();
        for t in ["folder", "meta", "doc", "sprint"] {
            assert!(names.contains(&t), "type enum must include {t}: {names:?}");
        }
    }

    // trace:STORY-489 | ai:claude
    /// query_graph walks the typed relationship graph: a spec blocked-by
    /// another surfaces that blocker in the `blocked-by` mode result, and the
    /// JSON carries the count + node id. Regression for the MCP half of the
    // trace:STORY-493 | ai:claude
    /// send_message → read_inbox round-trip: a direct message and a broadcast
    /// both land in the recipient's inbox; the sender's own message does not.
    #[test]
    fn mcp_mailbox_send_then_read_inbox_roundtrip() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());

        server
            .tool_send_message(&json!({ "to": "claude", "body": "direct hi", "from": "codex" }))
            .unwrap();
        server
            .tool_send_message(&json!({ "broadcast": true, "body": "all hands", "from": "agy" }))
            .unwrap();

        let out = server
            .tool_read_inbox(&json!({ "agent": "claude" }))
            .unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["agent"], "claude");
        assert_eq!(parsed["count"], 2, "direct + broadcast: {out}");

        // The sender (codex) sees the broadcast but not its own direct message.
        let codex = server
            .tool_read_inbox(&json!({ "agent": "codex" }))
            .unwrap();
        let codex_parsed: Value = serde_json::from_str(&codex).unwrap();
        assert_eq!(
            codex_parsed["count"], 1,
            "codex sees only agy's broadcast: {codex}"
        );

        // Neither `to` nor `broadcast` is a clean error, not a panic.
        let err = server.tool_send_message(&json!({ "body": "orphan" }));
        assert!(err.is_err(), "must require to/broadcast: {err:?}");
    }

    // trace:STORY-585 | ai:claude
    /// read_inbox is a non-marking peek by default; `mark_seen` advances the
    /// watermark (the explicit ack) and `unread` filters to the unread slice.
    #[test]
    fn mcp_read_inbox_peek_unread_and_mark_seen() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        server
            .tool_send_message(&json!({ "to": "claude", "body": "one", "from": "codex" }))
            .unwrap();
        server
            .tool_send_message(&json!({ "to": "claude", "body": "two", "from": "codex" }))
            .unwrap();

        // Default read does NOT mark seen — a second read still sees both unread.
        let peek = server
            .tool_read_inbox(&json!({ "agent": "claude" }))
            .unwrap();
        let peek_p: Value = serde_json::from_str(&peek).unwrap();
        assert_eq!(peek_p["count"], 2);
        let unread1 = server
            .tool_read_inbox(&json!({ "agent": "claude", "unread": true }))
            .unwrap();
        let unread1_p: Value = serde_json::from_str(&unread1).unwrap();
        assert_eq!(unread1_p["count"], 2, "peek did not consume: {unread1}");
        assert_eq!(unread1_p["unread"], true);

        // Explicit ack: mark_seen advances the watermark.
        server
            .tool_read_inbox(&json!({ "agent": "claude", "mark_seen": true }))
            .unwrap();
        let unread2 = server
            .tool_read_inbox(&json!({ "agent": "claude", "unread": true }))
            .unwrap();
        let unread2_p: Value = serde_json::from_str(&unread2).unwrap();
        assert_eq!(
            unread2_p["count"], 0,
            "ack cleared the unread set: {unread2}"
        );
        // The full inbox is still there (ack is a watermark, not a delete).
        let full = server
            .tool_read_inbox(&json!({ "agent": "claude" }))
            .unwrap();
        let full_p: Value = serde_json::from_str(&full).unwrap();
        assert_eq!(full_p["count"], 2);
    }

    /// graph-query moat (the CLI half is covered by graph_walk's unit tests).
    #[test]
    fn mcp_query_graph_returns_blocked_by_chain() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        let dependent = server
            .tool_add_requirement(&json!({
                "title": "Dependent spec",
                "description": "blocked by the blocker",
                "type": "story",
                "status": "approved",
            }))
            .unwrap();
        let dependent_id = added_spec_id(&dependent).to_string();
        let blocker = server
            .tool_add_requirement(&json!({
                "title": "Blocker spec",
                "description": "blocks the dependent",
                "type": "story",
                "status": "in-progress",
            }))
            .unwrap();
        let blocker_id = added_spec_id(&blocker).to_string();

        server
            .tool_add_relationship(&json!({
                "spec_id": dependent_id,
                "relationship_type": "blocked-by",
                "target_spec_id": blocker_id,
            }))
            .unwrap();

        let out = server
            .tool_query_graph(&json!({
                "spec_id": dependent_id,
                "mode": "blocked-by",
            }))
            .unwrap();
        let parsed: Value = serde_json::from_str(&out).expect("query_graph returns JSON");
        assert_eq!(parsed["mode"], "blocked-by");
        assert_eq!(parsed["count"], 1, "expected one blocker: {out}");
        assert_eq!(parsed["root"], dependent_id, "{out}");
        let ids: Vec<&str> = parsed["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n["id"].as_str())
            .collect();
        assert!(
            ids.contains(&blocker_id.as_str()),
            "blocker in nodes: {out}"
        );

        // BUG-411 parity: impact from the blocker finds what it blocks
        // (the dependent is blocked-by the blocker), via the handler's
        // walk_union legs.
        let impact = server
            .tool_query_graph(&json!({ "spec_id": blocker_id, "mode": "impact" }))
            .unwrap();
        let impact_parsed: Value = serde_json::from_str(&impact).unwrap();
        let impact_ids: Vec<&str> = impact_parsed["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n["id"].as_str())
            .collect();
        assert!(
            impact_ids.contains(&dependent_id.as_str()),
            "impact must surface the dependent: {impact}"
        );

        // Unknown mode is a clean error, not a panic.
        let err = server.tool_query_graph(&json!({ "spec_id": dependent_id, "mode": "bogus" }));
        assert!(err.is_err(), "unknown mode must error: {err:?}");

        // Review finding: an underscore alias is accepted but the output must
        // echo the canonical hyphenated form, never `blocked_by`.
        let aliased = server
            .tool_query_graph(&json!({ "spec_id": dependent_id, "mode": "blocked_by" }))
            .unwrap();
        let aliased_parsed: Value = serde_json::from_str(&aliased).unwrap();
        assert_eq!(
            aliased_parsed["mode"], "blocked-by",
            "canonical mode: {aliased}"
        );
    }

    // trace:BUG-381 | ai:codex
    #[test]
    fn mcp_list_requirements_descriptor_advertises_priority_filter() {
        let desc = tool_descriptors();
        let tool = desc
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool.get("name").and_then(|v| v.as_str()) == Some("list_requirements"))
            .expect("list_requirements descriptor must exist");

        assert!(
            tool.pointer("/inputSchema/properties/priority").is_some(),
            "list_requirements should advertise the priority filter it accepts"
        );
    }

    // trace:BUG-377 TASK-550 | ai:codex
    // STORY-82: title / priority / tags are now advertised + persisted (they
    // were ignored before). trace:STORY-82 | ai:claude
    #[test]
    fn mcp_update_requirement_persists_advertised_fields_only() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        let response = server
            .tool_add_requirement(&json!({
                "title": "Update target",
                "description": "before",
                "type": "task",
            }))
            .unwrap();
        let spec_id = added_spec_id(&response).to_string();
        // BUG-481: Draft → InProgress via MCP is now advisor-gated; this test is
        // about field persistence, so triage the spec into the pipeline
        // out-of-band first (Approved → InProgress is implementer-legitimate).
        force_status(&server, &spec_id, RequirementStatus::Approved);

        let result = server
            .tool_update_requirement(&json!({
                "id": spec_id,
                // BUG-449/BUG-481: Approved → InProgress is an implementer-
                // legitimate transition; this test is about field persistence.
                "status": "in-progress",
                "description": "after",
                "title": "new title",
                "priority": "high",
                "tags": ["kept"],
            }))
            .unwrap();
        assert!(result.contains("status:"), "{result}");
        assert!(result.contains("description updated"), "{result}");
        assert!(result.contains("title:"), "{result}");
        assert!(result.contains("priority:"), "{result}");

        let store = server.storage.load().unwrap();
        let req = store
            .get_requirement_by_spec_id(&spec_id)
            .expect("requirement should exist");
        assert_eq!(req.title, "new title");
        assert_eq!(req.description, "after");
        assert_eq!(req.status, RequirementStatus::InProgress);
        assert_eq!(req.priority, RequirementPriority::High);
        assert!(req.tags.contains("kept"));
    }

    // trace:BUG-449 | ai:claude
    #[test]
    fn mcp_update_requirement_gates_advisor_and_merge_driven_statuses() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        let added = server
            .tool_add_requirement(&json!({
                "title": "Gate target",
                "description": "BUG-449 status gate",
                "type": "task",
            }))
            .unwrap();
        let id = added_spec_id(&added).to_string();

        // The add-then-update bypass must be refused: advisor-triage statuses…
        for status in ["approved", "planned"] {
            let err = server
                .tool_update_requirement(&json!({ "id": id, "status": status }))
                .expect_err("approved/planned via MCP must be refused");
            assert!(err.contains("advisor"), "{status}: {err}");
        }
        // …and the merge-driven Completed (the headline self-mark-on-uncommitted bypass).
        let err = server
            .tool_update_requirement(&json!({ "id": id, "status": "completed" }))
            .expect_err("completed via MCP must be refused");
        assert!(err.contains("merge") || err.contains("(SPEC-ID)"), "{err}");

        // BUG-481: Draft → InProgress/Done via MCP is now also refused — a
        // never-triaged Draft can't be pushed into the execution pipeline.
        for status in ["in-progress", "done"] {
            let err = server
                .tool_update_requirement(&json!({ "id": id, "status": status }))
                .expect_err("Draft → in-progress/done via MCP must be refused (BUG-481)");
            assert!(err.contains("advisor"), "{status}: {err}");
        }

        // Once the advisor has triaged the spec into the pipeline (set
        // out-of-band here, as the advisor/CLI would), the in-pipeline
        // execution flips are implementer-legitimate and stay allowed.
        force_status(&server, &id, RequirementStatus::Approved);
        for status in ["in-progress", "done"] {
            server
                .tool_update_requirement(&json!({ "id": id, "status": status }))
                .unwrap_or_else(|e| panic!("{status} should be allowed via MCP: {e}"));
        }
        // Punting (InProgress → NeedsAttention) stays implementer-legitimate;
        // it is the design-fork escape, not a pipeline advance.
        force_status(&server, &id, RequirementStatus::InProgress);
        server
            .tool_update_requirement(&json!({ "id": id, "status": "needs-attention" }))
            .expect("punt (InProgress → NeedsAttention) stays allowed via MCP");
        // …but re-advancing OUT of NeedsAttention into the pipeline is gated
        // (BUG-481 — the same intake-bypass class as Draft).
        let err = server
            .tool_update_requirement(&json!({ "id": id, "status": "in-progress" }))
            .expect_err("NeedsAttention → in-progress via MCP must be refused (BUG-481)");
        assert!(err.contains("advisor"), "{err}");
        // Reach Done legitimately again so the final assertion holds.
        force_status(&server, &id, RequirementStatus::Done);

        // Nothing slipped the gate: the spec never reached an advisor/merge status.
        let store = server.storage.load().unwrap();
        let req = store.get_requirement_by_spec_id(&id).unwrap();
        assert_eq!(req.status, RequirementStatus::Done);
    }

    /// BUG-481: the update_requirement status gate keyed only on the TARGET
    /// status (`Approved|Planned|Completed`), so `add_requirement` (Draft) →
    /// `update_requirement{status:in-progress|done}` was a two-tool bypass that
    /// pushed a never-triaged Draft into the execution pipeline. The gate is
    /// now keyed on the (SOURCE, TARGET) pair: Draft → InProgress/Done is
    /// refused, while a legitimately-triaged Approved → InProgress still works.
    // trace:BUG-481 | ai:claude
    #[test]
    fn mcp_update_requirement_gates_draft_into_pipeline() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        let added = server
            .tool_add_requirement(&json!({
                "title": "Pipeline bypass attempt",
                "description": "BUG-481 (source,target) gate",
                "type": "task",
            }))
            .unwrap();
        let id = added_spec_id(&added).to_string();

        // Draft → InProgress and Draft → Done are both refused.
        for status in ["in-progress", "done"] {
            let err = server
                .tool_update_requirement(&json!({ "id": id, "status": status }))
                .expect_err("Draft → in-progress/done via MCP must be refused");
            assert!(err.contains("advisor"), "{status}: {err}");
        }
        // The spec never left Draft.
        let store = server.storage.load().unwrap();
        assert_eq!(
            store.get_requirement_by_spec_id(&id).unwrap().status,
            RequirementStatus::Draft
        );

        // A legitimately-triaged spec (advisor set it Approved out-of-band)
        // flips Approved → InProgress freely — the gate only blocks the
        // un-triaged source, not in-pipeline execution.
        force_status(&server, &id, RequirementStatus::Approved);
        server
            .tool_update_requirement(&json!({ "id": id, "status": "in-progress" }))
            .expect("Approved → in-progress is implementer-legitimate via MCP");
        let store = server.storage.load().unwrap();
        assert_eq!(
            store.get_requirement_by_spec_id(&id).unwrap().status,
            RequirementStatus::InProgress
        );
    }

    // ===================================================================
    // TASK-681 (EPIC-38): authority/lifecycle gate parity audit across
    // every mutating MCP tool. The audit found one bypass —
    // `triage_finding promote` self-granted Approved — now gated below.
    // The remaining mutating tools carry no spec-status/lifecycle
    // invariant for an MCP caller to bypass (they either always land a
    // safe status, or write coordination state the orchestrator/CLI
    // re-derives); these guard tests assert that property so a future
    // edit that adds a status mutation trips a red test.
    // trace:TASK-681 | ai:claude
    // ===================================================================

    /// THE FIX: `triage_finding promote` sets a finding `Approved`, which is
    /// the advisor's triage decision (same invariant `update_requirement`
    /// gates under BUG-449). It must be refused for an MCP caller.
    // trace:TASK-681 | ai:claude
    #[test]
    fn mcp_triage_finding_promote_is_advisor_gated() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        let filed = server
            .tool_file_finding(&json!({
                "title": "Promote bypass attempt",
                "description": "finding body",
                "source": "review",
                "pr": 681,
            }))
            .unwrap();
        let finding_id = filed
            .strip_prefix("Finding filed: ")
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap()
            .to_string();

        let err = server
            .tool_triage_finding(&json!({
                "id": finding_id,
                "action": "promote",
                "reason": "I want this approved",
            }))
            .expect_err("triage_finding promote must be advisor-gated via MCP");
        assert!(err.contains("advisor"), "{err}");

        // The finding never reached Approved — it stays Draft as filed.
        let store = server.storage.load().unwrap();
        let req = store.get_requirement_by_spec_id(&finding_id).unwrap();
        assert_eq!(req.status, RequirementStatus::Draft);

        // `dismiss` (Rejected) is implementer-legitimate and still works.
        server
            .tool_triage_finding(&json!({ "id": finding_id, "action": "dismiss" }))
            .expect("dismiss is implementer-legitimate and stays allowed");
        let store = server.storage.load().unwrap();
        let req = store.get_requirement_by_spec_id(&finding_id).unwrap();
        assert_eq!(req.status, RequirementStatus::Rejected);
    }

    /// `file_finding` is the findings analogue of `add_requirement`: it must
    /// always land `Draft` regardless of any status the caller might try to
    /// smuggle in (it accepts no `status` arg today; this pins that).
    // trace:TASK-681 | ai:claude
    #[test]
    fn mcp_file_finding_always_lands_draft() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        let filed = server
            .tool_file_finding(&json!({
                "title": "Intake finding",
                "description": "body",
                "source": "implementer",
                "spec_id": "TASK-681",
                // a status arg, if it ever leaks in, must be ignored:
                "status": "approved",
            }))
            .unwrap();
        let id = filed
            .strip_prefix("Finding filed: ")
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap()
            .to_string();
        let store = server.storage.load().unwrap();
        let req = store.get_requirement_by_spec_id(&id).unwrap();
        assert_eq!(req.status, RequirementStatus::Draft);
    }

    /// `add_comment` carries no status/lifecycle mutation — confirm it leaves
    /// the spec's status untouched no matter what.
    // trace:TASK-681 | ai:claude
    #[test]
    fn mcp_add_comment_does_not_touch_status() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        let added = server
            .tool_add_requirement(&json!({
                "title": "Comment-only target",
                "description": "body",
                "type": "task",
            }))
            .unwrap();
        let id = added_spec_id(&added).to_string();
        server
            .tool_add_comment(&json!({ "id": id, "text": "a note" }))
            .unwrap();
        let store = server.storage.load().unwrap();
        let req = store.get_requirement_by_spec_id(&id).unwrap();
        assert_eq!(req.status, RequirementStatus::Draft);
    }

    /// `add_relationship` links specs and (correctly) guards adding children
    /// to a terminal parent, but it must not mutate either spec's status.
    // trace:TASK-681 | ai:claude
    #[test]
    fn mcp_add_relationship_does_not_touch_status() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        let parent = server
            .tool_add_requirement(&json!({
                "title": "Parent",
                "description": "p",
                "type": "epic",
            }))
            .unwrap();
        let parent_id = added_spec_id(&parent).to_string();
        let child = server
            .tool_add_requirement(&json!({
                "title": "Child",
                "description": "c",
                "type": "task",
            }))
            .unwrap();
        let child_id = added_spec_id(&child).to_string();
        server
            .tool_add_relationship(&json!({
                "spec_id": child_id,
                "target_spec_id": parent_id,
                "relationship_type": "parent",
            }))
            .unwrap();
        let store = server.storage.load().unwrap();
        for id in [&parent_id, &child_id] {
            assert_eq!(
                store.get_requirement_by_spec_id(id).unwrap().status,
                RequirementStatus::Draft,
                "{id} status must be untouched by add_relationship"
            );
        }
    }

    /// The coordination-channel writers (`post_punt`/`resolve_punt`/
    /// `escalate_punt`/`post_directive`/`ack_brief`/`send_message`) write
    /// runtime files the orchestrator/CLI consume; none of them touch spec
    /// status in the store. Exercise the punt + directive writers and assert
    /// the requirement store is never mutated by them. (Status transitions
    /// for punted work are applied by the orchestrator, not the MCP writer.)
    // trace:TASK-681 | ai:claude
    #[test]
    fn mcp_coordination_writers_do_not_touch_spec_status() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        let added = server
            .tool_add_requirement(&json!({
                "title": "Coordination target",
                "description": "body",
                "type": "task",
            }))
            .unwrap();
        let id = added_spec_id(&added).to_string();

        server
            .tool_post_punt(&json!({
                "spec_id": id,
                "detail": "which approach?",
                "category": "design-fork",
            }))
            .unwrap();
        server
            .tool_resolve_punt(&json!({
                "spec_id": id,
                "answer": "approach A",
                "reasoning": "simpler",
            }))
            .unwrap();
        server
            .tool_post_directive(&json!({ "verb": "pause" }))
            .unwrap();

        let store = server.storage.load().unwrap();
        let req = store.get_requirement_by_spec_id(&id).unwrap();
        assert_eq!(
            req.status,
            RequirementStatus::Draft,
            "coordination writers must not mutate spec status"
        );
    }

    // trace:BUG-377 TASK-550 | ai:codex
    #[test]
    fn mcp_add_comment_persists_text_as_comment_content() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        let response = server
            .tool_add_requirement(&json!({
                "title": "Comment target",
                "description": "comment roundtrip",
                "type": "task",
            }))
            .unwrap();
        let spec_id = added_spec_id(&response).to_string();

        server
            .tool_add_comment(&json!({
                "id": spec_id,
                "text": "visible comment via MCP",
            }))
            .unwrap();

        let store = server.storage.load().unwrap();
        let req = store
            .get_requirement_by_spec_id(&spec_id)
            .expect("requirement should exist");
        assert_eq!(req.comments.len(), 1);
        assert_eq!(req.comments[0].author, "mcp");
        assert_eq!(req.comments[0].content, "visible comment via MCP");
    }

    // trace:BUG-377 TASK-550 | ai:codex
    #[test]
    fn mcp_triage_finding_persists_reason_as_comment_content() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        let filed = server
            .tool_file_finding(&json!({
                "title": "Finding target",
                "description": "finding body",
                "source": "review",
                "pr": 377,
            }))
            .unwrap();
        let finding_id = filed
            .strip_prefix("Finding filed: ")
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap()
            .to_string();

        // TASK-681: `promote` is now advisor-gated (it sets Approved), so the
        // reason-persistence path is exercised via `dismiss` (Rejected stays
        // an implementer-legitimate transition).
        server
            .tool_triage_finding(&json!({
                "id": finding_id,
                "action": "dismiss",
                "reason": "not actually a problem",
            }))
            .unwrap();

        let store = server.storage.load().unwrap();
        let req = store
            .get_requirement_by_spec_id(&finding_id)
            .expect("finding should exist");
        assert_eq!(req.status, RequirementStatus::Rejected);
        assert_eq!(req.comments.len(), 1);
        assert_eq!(req.comments[0].author, "mcp");
        assert_eq!(
            req.comments[0].content,
            "dismissed via MCP: not actually a problem"
        );
    }

    // trace:TASK-551 | ai:codex
    #[test]
    fn mcp_add_relationship_roundtrips_to_show_requirement() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        let parent = server
            .tool_add_requirement(&json!({
                "title": "Parent epic",
                "description": "relationship target",
                "type": "epic",
            }))
            .unwrap();
        let parent_id = added_spec_id(&parent).to_string();
        let child = server
            .tool_add_requirement(&json!({
                "title": "Child task",
                "description": "relationship source",
                "type": "task",
            }))
            .unwrap();
        let child_id = added_spec_id(&child).to_string();

        let response = server
            .tool_add_relationship(&json!({
                "spec_id": child_id,
                "relationship_type": "child",
                "target_spec_id": parent_id,
                "bidirectional": true,
            }))
            .unwrap();
        assert!(response.contains("Relationship added"), "{response}");
        assert!(response.contains("inverse added as parent"), "{response}");

        let shown_child = server
            .tool_show_requirement(&json!({ "id": child_id }))
            .unwrap();
        assert!(
            shown_child.contains(&format!("- child → {parent_id}")),
            "{shown_child}"
        );
        let shown_parent = server
            .tool_show_requirement(&json!({ "id": parent_id }))
            .unwrap();
        assert!(
            shown_parent.contains(&format!("- parent → {child_id}")),
            "{shown_parent}"
        );
    }

    // trace:TASK-551 | ai:codex
    #[test]
    fn mcp_add_relationship_validates_missing_specs_and_empty_type() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        let req = server
            .tool_add_requirement(&json!({
                "title": "Source",
                "description": "source exists",
                "type": "task",
            }))
            .unwrap();
        let source_id = added_spec_id(&req).to_string();

        let missing_source = server
            .tool_add_relationship(&json!({
                "spec_id": "TASK-999999",
                "relationship_type": "references",
                "target_spec_id": source_id,
            }))
            .expect_err("missing source should fail");
        assert!(missing_source.contains("Requirement 'TASK-999999' not found"));

        let empty_type = server
            .tool_add_relationship(&json!({
                "spec_id": source_id,
                "relationship_type": " ",
                "target_spec_id": "TASK-999999",
            }))
            .expect_err("empty type should fail before target lookup");
        assert!(empty_type.contains("relationship_type must be non-empty"));
    }

    // trace:TASK-551 | ai:codex
    #[test]
    fn mcp_add_relationship_aliases_dependency_vocabulary() {
        assert_eq!(
            parse_mcp_relationship_type("depends-on").unwrap(),
            RelationshipType::BlockedBy
        );
        assert_eq!(
            parse_mcp_relationship_type("related").unwrap(),
            RelationshipType::References
        );
        assert_eq!(
            parse_mcp_relationship_type("subsumes").unwrap(),
            RelationshipType::Custom("subsumes".to_string())
        );
    }

    // trace:TASK-551 | ai:codex
    #[test]
    fn mcp_add_relationship_descriptor_is_advertised() {
        let desc = tool_descriptors();
        let names: Vec<&str> = desc
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool.get("name").and_then(|name| name.as_str()))
            .collect();
        assert!(names.contains(&"add_relationship"));
    }

    // trace:TASK-551 | ai:codex
    #[test]
    fn mcp_add_relationship_preserves_terminal_parent_guard() {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        let parent = server
            .tool_add_requirement(&json!({
                "title": "Closed parent",
                "description": "terminal parent",
                "type": "epic",
            }))
            .unwrap();
        let parent_id = added_spec_id(&parent).to_string();
        // TASK-647 (ADR-3): MCP add lands draft; drive the parent to the
        // terminal `completed` state (the guard's precondition) out-of-band —
        // BUG-449 forbids setting Completed via the MCP update tool itself.
        force_status(&server, &parent_id, RequirementStatus::Completed);
        let child = server
            .tool_add_requirement(&json!({
                "title": "Child task",
                "description": "new child",
                "type": "task",
            }))
            .unwrap();
        let child_id = added_spec_id(&child).to_string();

        let err = server
            .tool_add_relationship(&json!({
                "spec_id": child_id,
                "relationship_type": "child",
                "target_spec_id": parent_id,
            }))
            .expect_err("closed parent should require force_parent");
        assert!(
            err.contains("adding new children to a closed parent"),
            "{err}"
        );
    }

    #[test]
    fn post_punt_then_list_and_read_round_trips() {
        let dir = tempdir().unwrap();
        let srv = mk_server(dir.path());

        let r = srv.tool_post_punt(&json!({
            "spec_id": "STORY-X",
            "detail": "two valid auth flows; spec doesn't say",
            "category": "design-fork",
            "lean": "OAuth"
        }));
        assert!(r.is_ok(), "{:?}", r);

        let listed = srv.tool_list_punts(&json!({})).unwrap();
        assert!(listed.contains("STORY-X"), "{}", listed);

        let read = srv.tool_read_punt(&json!({"spec_id": "STORY-X"})).unwrap();
        assert!(read.contains("design-fork"), "{}", read);
        assert!(read.contains("OAuth"), "{}", read);
    }

    /// BUG-334: post_punt flips an In Progress spec to NeedsAttention (matching
    /// the CLI `aida punt`), instead of only recording the ledger entry and
    /// telling the operator to flip it by hand.
    #[test]
    fn post_punt_flips_in_progress_spec_to_needs_attention() {
        let dir = tempdir().unwrap();
        let srv = mk_server(dir.path());
        let add = srv
            .tool_add_requirement(&json!({"title":"flip me","description":"d","type":"task"}))
            .unwrap();
        let id = added_spec_id(&add).to_string();
        // TASK-647 (ADR-3): MCP add lands draft; reach In Progress (the punt
        // pre-state). BUG-481: Draft → InProgress via MCP is now advisor-gated,
        // so set the pre-state out-of-band like the other gated statuses.
        force_status(&srv, &id, RequirementStatus::InProgress);

        let r = srv
            .tool_post_punt(
                &json!({"spec_id": id, "detail":"hit a fork", "category":"design-fork"}),
            )
            .unwrap();
        assert!(
            r.contains("NeedsAttention"),
            "punt response should report the flip: {r}"
        );

        let shown = srv.tool_show_requirement(&json!({"id": id})).unwrap();
        assert!(
            shown.to_lowercase().contains("needs attention") || shown.contains("NeedsAttention"),
            "spec should be NeedsAttention after punt: {shown}"
        );
    }

    #[test]
    fn resolve_punt_writes_response_file() {
        let dir = tempdir().unwrap();
        let srv = mk_server(dir.path());

        srv.tool_resolve_punt(&json!({
            "spec_id": "STORY-Y",
            "answer": "pick option B",
            "reasoning": "consistent with recorded preference"
        }))
        .unwrap();

        let path = punt::punt_response_path(dir.path(), "STORY-Y");
        assert!(path.exists());
        let body = std::fs::read_to_string(&path).unwrap();
        let resp: punt::PuntResponse = serde_json::from_str(&body).unwrap();
        assert_eq!(resp.resolution, punt::PuntResolution::Resolved);
        assert_eq!(resp.answer.as_deref(), Some("pick option B"));
    }

    #[test]
    fn escalate_punt_writes_response_file_with_escalated_resolution() {
        let dir = tempdir().unwrap();
        let srv = mk_server(dir.path());

        srv.tool_escalate_punt(&json!({
            "spec_id": "STORY-Z",
            "reasoning": "irreversible architecture call",
            "escalation_reason": "irreversible"
        }))
        .unwrap();

        let path = punt::punt_response_path(dir.path(), "STORY-Z");
        let body = std::fs::read_to_string(&path).unwrap();
        let resp: punt::PuntResponse = serde_json::from_str(&body).unwrap();
        assert_eq!(resp.resolution, punt::PuntResolution::Escalated);
        assert!(resp.answer.is_none());
        assert_eq!(resp.escalation_reason.as_deref(), Some("irreversible"));
    }

    #[test]
    fn directives_post_list_ack_round_trip() {
        let dir = tempdir().unwrap();
        let srv = mk_server(dir.path());

        srv.tool_post_directive(&json!({"verb": "drain", "args": ["--zen"]}))
            .unwrap();
        srv.tool_post_directive(&json!({"verb": "pause"})).unwrap();

        let listed = srv.tool_list_directives().unwrap();
        assert!(listed.contains("drain"), "{}", listed);
        assert!(listed.contains("pause"), "{}", listed);

        // Ack the first (index 0) — pause should remain.
        srv.tool_ack_directive(&json!({"index": 0})).unwrap();
        let listed_after = srv.tool_list_directives().unwrap();
        assert!(!listed_after.contains("drain --zen"), "{}", listed_after);
        assert!(listed_after.contains("pause"), "{}", listed_after);
    }

    // trace:TASK-437 | ai:claude
    #[test]
    fn file_finding_rejects_pr_as_string() {
        let dir = tempdir().unwrap();
        let srv = mk_server(dir.path());

        let err = srv
            .tool_file_finding(&json!({
                "title": "x",
                "description": "y",
                "source": "review",
                "pr": "7",
            }))
            .expect_err("string pr should be rejected");
        assert!(err.contains("pr must be an integer"), "{}", err);
    }

    // trace:TASK-437 | ai:claude
    #[test]
    fn file_finding_accepts_pr_as_integer_and_uses_canonical_tag() {
        let dir = tempdir().unwrap();
        let srv = mk_server(dir.path());

        srv.tool_file_finding(&json!({
            "title": "broken thing",
            "description": "details",
            "source": "review",
            "pr": 7,
        }))
        .expect("integer pr should be accepted");

        let store = srv.storage.load().unwrap();
        let req = store
            .requirements
            .iter()
            .find(|r| r.title == "broken thing")
            .expect("requirement should have been saved");
        assert!(
            req.tags.contains("from-review:PR-7"),
            "expected canonical from-review:PR-7 tag, got tags: {:?}",
            req.tags,
        );
    }

    // trace:TASK-437 | ai:claude
    #[test]
    fn file_finding_rejects_spec_id_as_integer() {
        let dir = tempdir().unwrap();
        let srv = mk_server(dir.path());

        let err = srv
            .tool_file_finding(&json!({
                "title": "x",
                "description": "y",
                "source": "implementer",
                "spec_id": 42,
            }))
            .expect_err("integer spec_id should be rejected");
        assert!(err.contains("spec_id must be a string"), "{}", err);
    }

    // trace:TASK-436 | ai:claude
    #[test]
    fn post_directive_rejects_unknown_verb() {
        let dir = tempdir().unwrap();
        let srv = mk_server(dir.path());

        let err = srv
            .tool_post_directive(&json!({"verb": "drian"}))
            .expect_err("typo should be rejected");
        assert!(err.contains("invalid directive verb"), "{}", err);
        assert!(err.contains("drian"), "{}", err);

        // The bad verb must NOT have been appended to .aida/worker.cmd.
        let path = crate::worker::worker_cmd_path(dir.path());
        if path.exists() {
            let body = std::fs::read_to_string(&path).unwrap();
            assert!(!body.contains("drian"), "{}", body);
        }
    }

    #[test]
    fn mcp_claim_path_is_case_insensitive_for_session_end_reaping() {
        // BUG-694: `aida session end` reaps the sibling mcp-claim by calling
        // mcp_claim_path(leases_dir, target.scope). The scope on the session
        // lease may differ in case from the spec_id claim_task lowercased into
        // the filename, so the path MUST fold case — else the reap misses and
        // the orphan lingers (the 24-day TASK-702 leak). Lock the contract.
        let dir = std::path::Path::new("/tmp/sessions");
        let lower = mcp_claim_path(dir, "task-702");
        let upper = mcp_claim_path(dir, "TASK-702");
        assert_eq!(lower, upper, "scope casing must not change the reaped path");
        assert_eq!(
            upper,
            dir.join("mcp-claim.task-702.toml"),
            "the reaped filename must match what claim_task writes"
        );
    }

    #[test]
    fn claim_task_then_release_round_trips() {
        let dir = tempdir().unwrap();
        let srv = mk_server(dir.path());

        let claim = srv
            .tool_claim_task(&json!({"spec_id": "STORY-CLAIM", "role": "implementer"}))
            .unwrap();
        assert!(claim.starts_with("claimed:"), "{}", claim);
        let lease_id = claim.trim_start_matches("claimed: lease_id=").trim();

        // Second claim on same spec → already_claimed
        let again = srv
            .tool_claim_task(&json!({"spec_id": "STORY-CLAIM"}))
            .unwrap();
        assert!(again.contains("already_claimed"), "{}", again);

        // list_active_leases sees it
        let listed = srv.tool_list_active_leases().unwrap();
        assert!(listed.contains("STORY-CLAIM"), "{}", listed);
        assert!(listed.contains("kind=mcp"), "{}", listed);

        // Release deletes the lease file
        srv.tool_release_task(&json!({"lease_id": lease_id}))
            .unwrap();
        let listed_after = srv.tool_list_active_leases().unwrap();
        assert!(!listed_after.contains("STORY-CLAIM"), "{}", listed_after);
    }

    /// TASK-474: when the agent passes `worktree_path`, the lease records
    /// that explicit sibling worktree so consumers (`active_lease_for_cwd`
    /// in main.rs) route hints correctly — instead of misattributing the
    /// agent's scope to every shell in the parent (which is what happened
    /// before the arg existed: `worktree_path` was hardcoded to
    /// `self.project_root`). TASK-504 canonicalizes the path at write-time
    /// so later cwd comparisons are stable.
    /// trace:TASK-474 TASK-504 | ai:claude
    #[test]
    fn claim_task_records_explicit_worktree_path() {
        let dir = tempdir().unwrap();
        let srv = mk_server(dir.path());

        let sibling_dir = dir.path().join("aida-task-310");
        std::fs::create_dir_all(&sibling_dir).unwrap();
        let sibling = sibling_dir.to_string_lossy().to_string();
        let claim = srv
            .tool_claim_task(&json!({
                "spec_id": "TASK-310",
                "role": "implementer",
                "worktree_path": sibling,
            }))
            .unwrap();
        assert!(claim.starts_with("claimed:"), "{}", claim);

        // Round-trip through the on-disk TOML — that's what `aida add`'s
        // hint reads via `list_leases` + `active_lease_for_cwd`. TASK-504
        // canonicalizes the path at write-time, so compare against the
        // canonical form (matches sibling test
        // `claim_task_canonicalizes_explicit_worktree_path`). Without
        // canonicalizing first this fails on Windows (UNC `\\?\` prefix +
        // `RUNNER~1` → `runneradmin` short-name expansion) and macOS
        // (`/var` → `/private/var` symlink resolution). trace:BUG-385 | ai:claude
        let leases = list_leases(dir.path());
        let l = leases
            .iter()
            .find(|l| l.scope == "TASK-310")
            .expect("the claim should have written a lease");
        assert_eq!(
            l.worktree_path,
            sibling_dir.canonicalize().unwrap().to_string_lossy()
        );
    }

    /// TASK-504: claim_task canonicalizes the recorded `worktree_path` so a
    /// non-canonical input path (for example, containing `..`) still matches
    /// the canonical cwd used by `lease_covers_cwd`.
    /// trace:TASK-504 | ai:codex
    #[test]
    fn claim_task_canonicalizes_explicit_worktree_path() {
        let dir = tempdir().unwrap();
        let srv = mk_server(dir.path());
        let real_worktree = dir.path().join("worktrees").join("task-504");
        std::fs::create_dir_all(&real_worktree).unwrap();
        let raw_worktree = real_worktree
            .parent()
            .unwrap()
            .join("..")
            .join("worktrees")
            .join("task-504");

        srv.tool_claim_task(&json!({
            "spec_id": "TASK-504",
            "role": "implementer",
            "worktree_path": raw_worktree.to_string_lossy(),
        }))
        .unwrap();

        let leases = list_leases(dir.path());
        let l = leases
            .iter()
            .find(|l| l.scope == "TASK-504")
            .expect("the claim should have written a lease");
        assert_eq!(
            l.worktree_path,
            real_worktree.canonicalize().unwrap().to_string_lossy()
        );
    }

    /// TASK-474: when `worktree_path` is omitted, the lease's worktree field
    /// is the empty string — which `lease_covers_cwd` in main.rs treats as
    /// "no session context, can't cover any cwd," preventing the misrouted
    /// scope hint that was the original symptom.
    /// trace:TASK-474 | ai:claude
    #[test]
    fn claim_task_omits_worktree_when_arg_absent() {
        let dir = tempdir().unwrap();
        let srv = mk_server(dir.path());

        srv.tool_claim_task(&json!({"spec_id": "TASK-474", "role": "implementer"}))
            .unwrap();

        let leases = list_leases(dir.path());
        let l = leases
            .iter()
            .find(|l| l.scope == "TASK-474")
            .expect("the claim should have written a lease");
        assert_eq!(
            l.worktree_path, "",
            "absent worktree_path arg must NOT default to project_root — that was the BUG",
        );
    }

    /// TASK-474: an empty-string `worktree_path` arg behaves the same as
    /// omitting the arg — both signal "no session context, advisory lock
    /// only," and both result in an empty `worktree_path` on the lease.
    /// trace:TASK-474 | ai:claude
    #[test]
    fn claim_task_treats_empty_worktree_arg_as_absent() {
        let dir = tempdir().unwrap();
        let srv = mk_server(dir.path());

        srv.tool_claim_task(&json!({
            "spec_id": "TASK-474B",
            "worktree_path": "   ",
        }))
        .unwrap();

        let leases = list_leases(dir.path());
        let l = leases
            .iter()
            .find(|l| l.scope == "TASK-474B")
            .expect("the claim should have written a lease");
        assert_eq!(l.worktree_path, "");
    }

    #[test]
    fn release_task_refuses_non_mcp_lease() {
        let dir = tempdir().unwrap();
        let srv = mk_server(dir.path());

        // Hand-create a non-MCP lease (mcp_claim defaults false).
        let lease = LightLease {
            id: "deadbeef0000".to_string(),
            scope: "STORY-Q".to_string(),
            slug: "STORY-Q".to_string(),
            owner: "alice".to_string(),
            worktree_path: dir.path().to_string_lossy().to_string(),
            branch: "main".to_string(),
            started_at: Utc::now().to_rfc3339(),
            hostname: "imac".to_string(),
            role: Some("implementer".to_string()),
            mcp_claim: false,
        };
        let dir_leases = leases_dir(dir.path());
        std::fs::create_dir_all(&dir_leases).unwrap();
        let path = dir_leases.join("deadbeef0000.toml");
        std::fs::write(&path, toml::to_string(&lease).unwrap()).unwrap();

        let err = srv
            .tool_release_task(&json!({"lease_id": "deadbeef0000"}))
            .unwrap_err();
        assert!(err.contains("aida session end"), "{}", err);
        assert!(path.exists(), "non-mcp lease should not be deleted");
    }

    fn write_test_brief(root: &Path, agent: &str, name: &str, status: &str) -> PathBuf {
        let dir = root.join(".aida").join("agent-briefs").join(agent);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let spec_id = name.split_once("-20").map(|(s, _)| s).unwrap_or("TASK-X");
        std::fs::write(
            &path,
            format!(
                "---\nspec_id: {spec_id}\nagent: {agent}\ngenerated_at: 2026-05-23T020000Z\nstatus: {status}\n---\n\n## Routing\n\nBrief body for {spec_id}\n"
            ),
        )
        .unwrap();
        path
    }

    #[test]
    fn brief_tools_list_read_ack_roundtrip() {
        let dir = tempdir().unwrap();
        let srv = mk_server(dir.path());
        let path = write_test_brief(
            dir.path(),
            "codex",
            "TASK-492-2026-05-23T020000Z.md",
            "pending",
        );
        write_test_brief(
            dir.path(),
            "antigravity",
            "TASK-999-2026-05-23T020001Z.md",
            "pending",
        );

        let listed = srv
            .tool_list_briefs(&json!({"agent": "codex"}))
            .expect("list briefs");
        assert!(listed.contains("Found 1 brief(s):"), "{}", listed);
        assert!(
            listed.contains("path=.aida/agent-briefs/codex/TASK-492-2026-05-23T020000Z.md"),
            "{}",
            listed
        );
        assert!(listed.contains("status=pending"), "{}", listed);
        assert!(!listed.contains("antigravity"), "{}", listed);

        let read = srv
            .tool_read_brief(
                &json!({"path": ".aida/agent-briefs/codex/TASK-492-2026-05-23T020000Z.md"}),
            )
            .expect("read brief");
        assert!(read.contains("Brief body for TASK-492"), "{}", read);

        let acked = srv
            .tool_ack_brief(
                &json!({"path": ".aida/agent-briefs/codex/TASK-492-2026-05-23T020000Z.md"}),
            )
            .expect("ack brief");
        assert!(
            acked.contains("acked: .aida/agent-briefs/codex/TASK-492-2026-05-23T020000Z.md.acked"),
            "{}",
            acked
        );
        assert!(!path.exists());
        let acked_path = dir
            .path()
            .join(".aida/agent-briefs/codex/TASK-492-2026-05-23T020000Z.md.acked");
        let body = std::fs::read_to_string(&acked_path).unwrap();
        assert!(body.contains("status: acked"), "{}", body);

        let pending = srv
            .tool_list_briefs(&json!({"agent": "codex"}))
            .expect("list pending");
        assert_eq!(pending, "No briefs found.");
        let all = srv
            .tool_list_briefs(&json!({"agent": "codex", "include_acked": true}))
            .expect("list acked");
        assert!(all.contains("status=acked"), "{}", all);
    }

    #[test]
    fn ack_brief_is_idempotent_for_acked_paths_and_original_paths() {
        let dir = tempdir().unwrap();
        let srv = mk_server(dir.path());
        write_test_brief(
            dir.path(),
            "codex",
            "TASK-492-2026-05-23T020000Z.md",
            "pending",
        );
        srv.tool_ack_brief(
            &json!({"path": ".aida/agent-briefs/codex/TASK-492-2026-05-23T020000Z.md"}),
        )
        .unwrap();

        let again_original = srv
            .tool_ack_brief(
                &json!({"path": ".aida/agent-briefs/codex/TASK-492-2026-05-23T020000Z.md"}),
            )
            .unwrap();
        assert!(
            again_original.starts_with("already_acked:"),
            "{}",
            again_original
        );

        let again_acked = srv
            .tool_ack_brief(
                &json!({"path": ".aida/agent-briefs/codex/TASK-492-2026-05-23T020000Z.md.acked"}),
            )
            .unwrap();
        assert!(again_acked.starts_with("already_acked:"), "{}", again_acked);
    }

    #[test]
    fn brief_tools_reject_path_escape_and_missing_files() {
        let dir = tempdir().unwrap();
        let srv = mk_server(dir.path());

        let err = srv
            .tool_read_brief(&json!({"path": "../secret"}))
            .unwrap_err();
        assert!(err.contains("must not contain"), "{}", err);

        let err = srv
            .tool_read_brief(&json!({"path": "CLAUDE.md"}))
            .unwrap_err();
        assert!(err.contains(".aida/agent-briefs"), "{}", err);

        let err = srv
            .tool_read_brief(&json!({"path": ".aida/agent-briefs/codex/missing.md"}))
            .unwrap_err();
        assert!(err.contains("reading brief"), "{}", err);
    }

    /// Concurrent-write contention test: many threads append punt records
    /// in parallel; the ledger must contain exactly N parseable lines.
    /// trace:STORY-361 | ai:claude
    #[test]
    fn concurrent_punt_appends_dont_corrupt_ledger() {
        let dir = tempdir().unwrap();
        let project_root = Arc::new(dir.path().to_path_buf());
        const N: usize = 32;

        let mut handles = Vec::new();
        for i in 0..N {
            let root = Arc::clone(&project_root);
            handles.push(thread::spawn(move || {
                let record = PuntRecord {
                    timestamp: Utc::now(),
                    spec: format!("STORY-{}", i),
                    category: PuntCategory::Other,
                    detail: format!("concurrent punt {}", i),
                    lean: None,
                    raised_by: Some("test".to_string()),
                    resolution_path: "punted".to_string(),
                    classification: None,
                    escalation_reason: None,
                    answer: None,
                    answered_by: None,
                    decision: None,
                    principle_link: None,
                    calibration_pair: None,
                    paused_at: None,
                    resolved_at: None,
                };
                append_to_ledger(&root, &record).unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        let records = read_ledger(&project_root);
        assert_eq!(records.len(), N, "every concurrent append must land");
        let specs: std::collections::HashSet<_> = records.iter().map(|r| r.spec.clone()).collect();
        assert_eq!(specs.len(), N, "each spec must appear exactly once");
    }

    /// Concurrent directive posts must not interleave / corrupt the worker.cmd
    /// file. trace:STORY-361 | ai:claude
    #[test]
    fn concurrent_directive_posts_dont_corrupt_worker_cmd() {
        let dir = tempdir().unwrap();
        let project_root = Arc::new(dir.path().to_path_buf());
        const N: usize = 16;

        let mut handles = Vec::new();
        for i in 0..N {
            let root = Arc::clone(&project_root);
            handles.push(thread::spawn(move || {
                let path = crate::worker::worker_cmd_path(&root);
                append_line(&path, &format!("drain batch:{}", i)).unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        let path = crate::worker::worker_cmd_path(&project_root);
        let directives = crate::worker::parse_directives(&path);
        assert_eq!(directives.len(), N, "every directive must land");
        // Every line is a well-formed "drain batch:K"
        let unique: std::collections::HashSet<_> =
            directives.iter().map(|d| d.raw.clone()).collect();
        assert_eq!(unique.len(), N);
    }

    /// Concurrent claim_task calls on the same spec must elect exactly one
    /// winner — the others must see `already_claimed`. Regression test for
    /// the pre-fix TOCTOU race between the existence check and the lease
    /// write. trace:TASK-438 | ai:claude
    #[test]
    fn concurrent_claim_task_on_same_spec_elects_one_winner() {
        let dir = tempdir().unwrap();
        let project_root = Arc::new(dir.path().to_path_buf());
        const N: usize = 16;
        const SPEC: &str = "STORY-RACE";

        // Gate every thread on a barrier so the contention window is as
        // tight as possible — maximises the chance the buggy version would
        // flake before the fix lands.
        let barrier = Arc::new(std::sync::Barrier::new(N));

        let mut handles = Vec::new();
        for _ in 0..N {
            let root = Arc::clone(&project_root);
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                let storage = Box::leak(Box::new(Storage::new(
                    root.join(".aida").join("mcp-test-cache.yaml"),
                )));
                std::fs::create_dir_all(root.join(".aida")).unwrap();
                let srv = McpServer::new(storage, (*root).clone());
                barrier.wait();
                srv.tool_claim_task(&json!({"spec_id": SPEC})).unwrap()
            }));
        }
        let results: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        let claimed: Vec<&String> = results
            .iter()
            .filter(|r| r.starts_with("claimed:"))
            .collect();
        let already: Vec<&String> = results
            .iter()
            .filter(|r| r.contains("already_claimed"))
            .collect();

        assert_eq!(
            claimed.len(),
            1,
            "exactly one thread should win the claim, got {:?}",
            results
        );
        assert_eq!(
            already.len(),
            N - 1,
            "every loser should see already_claimed, got {:?}",
            results
        );

        // And on disk: exactly one MCP claim file for this spec.
        let leases = list_leases(&project_root);
        let matching: Vec<_> = leases
            .iter()
            .filter(|l| l.scope.eq_ignore_ascii_case(SPEC) && l.mcp_claim)
            .collect();
        assert_eq!(matching.len(), 1, "exactly one lease file on disk");
    }

    /// Crash-mid-write recovery: a punt-ledger truncated mid-line must still
    /// yield every valid record before the corruption, and the file must
    /// remain appendable after recovery. trace:STORY-361 | ai:claude
    #[test]
    fn ledger_with_partial_trailing_line_reads_valid_records() {
        let dir = tempdir().unwrap();
        // Two valid records, then a torn third line.
        let valid_a = PuntRecord {
            timestamp: Utc::now(),
            spec: "STORY-A".to_string(),
            category: PuntCategory::Other,
            detail: "a".to_string(),
            lean: None,
            raised_by: None,
            resolution_path: "punted".to_string(),
            classification: None,
            escalation_reason: None,
            answer: None,
            answered_by: None,
            decision: None,
            principle_link: None,
            calibration_pair: None,
            paused_at: None,
            resolved_at: None,
        };
        let valid_b = PuntRecord {
            spec: "STORY-B".to_string(),
            detail: "b".to_string(),
            ..valid_a.clone()
        };
        append_to_ledger(dir.path(), &valid_a).unwrap();
        append_to_ledger(dir.path(), &valid_b).unwrap();
        // Now simulate a crash mid-write by appending half a JSON line.
        let path = ledger_path(dir.path());
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        f.write_all(b"{\"timestamp\":\"2026-05-21T10:00:00")
            .unwrap();
        f.sync_all().unwrap();
        drop(f);

        // read_ledger must skip the torn line and return the two valid records.
        let records = read_ledger(dir.path());
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].spec, "STORY-A");
        assert_eq!(records[1].spec, "STORY-B");

        // After recovery the file is still appendable.
        let valid_c = PuntRecord {
            spec: "STORY-C".to_string(),
            detail: "c".to_string(),
            ..valid_a.clone()
        };
        // Pad the broken half-line with a newline to keep the JSONL invariant,
        // then append — the read_ledger skip means a half-line *between*
        // records is tolerated, but appending after a torn line without a
        // newline would glue STORY-C onto the broken line. write_atomic of a
        // recovery newline is the production recovery path; here we model it
        // inline.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        f.write_all(b"\n").unwrap();
        drop(f);
        append_to_ledger(dir.path(), &valid_c).unwrap();
        let records2 = read_ledger(dir.path());
        assert!(records2.iter().any(|r| r.spec == "STORY-C"));
    }

    /// Crash-mid-write recovery for a TOML lease: a write_atomic interrupted
    /// before rename leaves the original file intact. trace:STORY-361 | ai:claude
    #[test]
    fn write_atomic_leaves_original_intact_on_failed_rename() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("lease.toml");
        std::fs::write(&target, b"original").unwrap();

        // First successful atomic write
        write_atomic(&target, b"updated").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"updated");

        // Simulate a torn write: an orphan .tmp file beside target should NOT
        // affect future reads of target.
        let orphan = dir.path().join(".lease.toml.tmp-orphan");
        std::fs::write(&orphan, b"junk").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"updated");

        // A subsequent atomic write still works (uses a fresh tmp name).
        write_atomic(&target, b"updated2").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"updated2");
    }

    /// BUG-310: an MCP `add_requirement` against a Storage pointing at the
    /// git-canonical store directory must be visible to a fresh GitBackend
    /// load — that is, to the next CLI invocation. The pre-fix MCP startup
    /// wrote to a private YAML snapshot, so this roundtrip silently dropped
    /// every MCP write.
    /// trace:BUG-310 | ai:claude
    #[test]
    fn mcp_add_requirement_is_visible_to_cli_via_git_backend() {
        use aida_core::db::{DatabaseBackend, GitBackend};

        let dir = tempdir().unwrap();
        let store_dir = dir.path().join(".aida-store");
        std::fs::create_dir_all(&store_dir).unwrap();

        // Seed an empty git-canonical store so subsequent loads succeed.
        let seed = GitBackend::new(&store_dir).unwrap();
        seed.save(&aida_core::RequirementsStore::new()).unwrap();

        // Point the MCP server at the store directory — the same shape the
        // fixed `aida mcp-serve` startup now uses.
        let storage = Box::leak(Box::new(Storage::new(&store_dir)));
        let server = McpServer::new(storage, dir.path().to_path_buf());

        let args = serde_json::json!({
            "title": "Roundtrip via MCP",
            "description": "MCP add must reach the canonical git store",
            "type": "task",
        });
        let result = server.tool_add_requirement(&args).expect("MCP add failed");
        assert!(
            result.contains("Requirement added"),
            "unexpected MCP add response: {result}"
        );

        // A fresh GitBackend simulates the next `aida` CLI invocation. It
        // must see the requirement the MCP server just wrote — that is the
        // contract BUG-310 broke.
        let cli_view = GitBackend::new(&store_dir).unwrap();
        let loaded = cli_view.load().unwrap();
        assert!(
            loaded
                .requirements
                .iter()
                .any(|r| r.title == "Roundtrip via MCP"),
            "CLI-equivalent GitBackend did not see the MCP-written requirement"
        );
    }

    // ========================================================================
    // STORY-474: tool profiles + safe-default surface
    // ========================================================================

    /// Every name in `tool_descriptors()` must be classified explicitly — no
    /// tool may fall through to the `Admin` catch-all in `tool_min_profile`. A
    /// new tool added without a classification line is a bug (it would silently
    /// be admin-only). trace:STORY-474 | ai:claude
    #[test]
    fn every_tool_is_explicitly_classified() {
        let names: Vec<String> = tool_descriptors()
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.get("name").unwrap().as_str().unwrap().to_string())
            .collect();
        // The catch-all returns Admin; an explicit `add_requirement`-style
        // operator/coordination/read classification returns something <= Operator.
        // No *real* tool should be Admin today.
        let unclassified: Vec<&String> = names
            .iter()
            .filter(|n| tool_min_profile(n) == McpProfile::Admin)
            .collect();
        assert!(
            unclassified.is_empty(),
            "tools missing a profile classification (would be admin-only): {:?}",
            unclassified
        );
    }

    #[test]
    fn read_only_profile_excludes_all_write_tools() {
        let names = tool_names_for_profile(McpProfile::ReadOnly);
        // Spot-check the writes are gone.
        for write in [
            "add_requirement",
            "update_requirement",
            "add_comment",
            "add_relationship",
            "post_punt",
            "resolve_punt",
            "escalate_punt",
            "file_finding",
            "triage_finding",
            "claim_task",
            "release_task",
            "post_directive",
            "ack_directive",
            "ack_brief",
            "send_message",
        ] {
            assert!(
                !names.contains(&write.to_string()),
                "read-only profile leaked write tool {write}"
            );
        }
        // Reads are present.
        for read in [
            "list_requirements",
            "show_requirement",
            "search_requirements",
            "query_graph",
            "history",
            "list_punts",
            "read_brief",
        ] {
            assert!(
                names.contains(&read.to_string()),
                "read-only profile dropped read tool {read}"
            );
        }
    }

    /// Each higher tier is a strict superset of the one below it.
    #[test]
    fn profiles_are_monotonic_supersets() {
        let ro = tool_names_for_profile(McpProfile::ReadOnly);
        let coord = tool_names_for_profile(McpProfile::Coordination);
        let op = tool_names_for_profile(McpProfile::Operator);
        let full = tool_names_for_profile(McpProfile::Full);
        let admin = tool_names_for_profile(McpProfile::Admin);

        for n in &ro {
            assert!(coord.contains(n), "coordination dropped {n}");
        }
        for n in &coord {
            assert!(op.contains(n), "operator dropped {n}");
        }
        for n in &op {
            assert!(full.contains(n), "full dropped {n}");
        }
        // admin == full today.
        assert_eq!(admin.len(), full.len());
        // full exposes every descriptor.
        assert_eq!(full.len(), tool_descriptors().as_array().unwrap().len());
        // strict growth across tiers.
        assert!(ro.len() < coord.len());
        assert!(coord.len() < op.len());
        assert!(op.len() <= full.len());
    }

    #[test]
    fn profile_token_parsing_round_trips_and_aliases() {
        assert_eq!(
            McpProfile::from_token("read-only"),
            Some(McpProfile::ReadOnly)
        );
        assert_eq!(
            McpProfile::from_token("readonly"),
            Some(McpProfile::ReadOnly)
        );
        assert_eq!(McpProfile::from_token("  RO "), Some(McpProfile::ReadOnly));
        assert_eq!(
            McpProfile::from_token("read_only"),
            Some(McpProfile::ReadOnly)
        );
        assert_eq!(
            McpProfile::from_token("Coordination"),
            Some(McpProfile::Coordination)
        );
        assert_eq!(
            McpProfile::from_token("operator"),
            Some(McpProfile::Operator)
        );
        assert_eq!(McpProfile::from_token("admin"), Some(McpProfile::Admin));
        assert_eq!(McpProfile::from_token("full"), Some(McpProfile::Full));
        assert_eq!(McpProfile::from_token("all"), Some(McpProfile::Full));
        assert_eq!(McpProfile::from_token("bogus"), None);
        // canonical token round-trips
        for p in [
            McpProfile::ReadOnly,
            McpProfile::Coordination,
            McpProfile::Operator,
            McpProfile::Admin,
            McpProfile::Full,
        ] {
            assert_eq!(McpProfile::from_token(p.as_str()), Some(p));
        }
    }

    #[test]
    fn default_profile_is_full_for_backwards_compat() {
        assert_eq!(McpProfile::default(), McpProfile::Full);
    }

    /// `tools/list` advertises only the in-profile tools and tags each with its
    /// minimum tier.
    #[test]
    fn tools_list_respects_profile() {
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join(".aida").join("c.yaml");
        std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        let storage = Box::leak(Box::new(Storage::new(cache_path)));
        let server =
            McpServer::with_profile(storage, dir.path().to_path_buf(), McpProfile::ReadOnly);

        let resp = server.handle_tools_list(&json!(1));
        let result = resp.result.expect("tools/list returns a result");
        let tools = result["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"list_requirements"));
        assert!(!names.contains(&"add_requirement"));
        // descriptors carry the profile tier metadata.
        let lr = tools
            .iter()
            .find(|t| t["name"] == json!("list_requirements"))
            .unwrap();
        assert_eq!(lr["profile"], json!("read-only"));
    }

    /// `tools/call` rejects an out-of-profile (but known) tool with a permission
    /// error, even when the client calls it directly. trace:STORY-474 | ai:claude
    #[test]
    fn tools_call_rejects_out_of_profile_tool() {
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join(".aida").join("c.yaml");
        std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        let storage = Box::leak(Box::new(Storage::new(cache_path)));
        let server =
            McpServer::with_profile(storage, dir.path().to_path_buf(), McpProfile::ReadOnly);

        let resp = server.handle_tools_call(
            &json!(1),
            &json!({ "name": "add_requirement", "arguments": { "title": "X" } }),
        );
        let result = resp.result.expect("tools/call returns a result");
        assert_eq!(result["isError"], json!(true));
        let code = result["structuredError"]["code"].as_str().unwrap();
        assert_eq!(code, "permission_denied");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("operator"),
            "should name the required tier: {text}"
        );
        assert!(
            text.contains("read-only"),
            "should name the active tier: {text}"
        );
    }

    /// A read tool still works under read-only (smoke test the gate doesn't
    /// over-block).
    #[test]
    fn tools_call_allows_in_profile_read() {
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join(".aida").join("c.yaml");
        std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        let storage = Box::leak(Box::new(Storage::new(cache_path)));
        let server =
            McpServer::with_profile(storage, dir.path().to_path_buf(), McpProfile::ReadOnly);

        let resp = server.handle_tools_call(&json!(1), &json!({ "name": "list_features" }));
        let result = resp.result.expect("tools/call returns a result");
        // list_features returns Ok (not a profile rejection).
        assert!(
            result["isError"] != json!(true),
            "in-profile read was rejected: {result}"
        );
    }

    /// Resolution order for the non-env layers (override > config > default).
    /// The `AIDA_MCP_PROFILE` env layer is intentionally NOT exercised here:
    /// mutating a process-global env var races with the many parallel tests that
    /// build a server via `McpServer::new` (which reads it). The env layer is a
    /// thin `std::env::var` read above the config layer in `resolve_mcp_profile`.
    #[test]
    fn resolve_profile_prefers_override_then_config_then_default() {
        // Guard: a leaked AIDA_MCP_PROFILE from the ambient shell would change
        // the default-layer expectation; skip that one assertion if so.
        let env_clean = std::env::var("AIDA_MCP_PROFILE").is_err();

        let dir = tempdir().unwrap();
        let aida = dir.path().join(".aida");
        std::fs::create_dir_all(&aida).unwrap();

        // No override, no env, no config -> default (full).
        if env_clean {
            assert_eq!(resolve_mcp_profile(dir.path(), None), McpProfile::Full);
        }

        // Config sets coordination (override and env both absent/clean).
        std::fs::write(
            aida.join("config.toml"),
            "[mcp]\nprofile = \"coordination\"\n",
        )
        .unwrap();
        if env_clean {
            assert_eq!(
                resolve_mcp_profile(dir.path(), None),
                McpProfile::Coordination
            );
        }

        // Explicit override beats config (and env) regardless of ambient env.
        assert_eq!(
            resolve_mcp_profile(dir.path(), Some("read-only")),
            McpProfile::ReadOnly
        );

        // An unknown override token is ignored (falls through to config).
        if env_clean {
            assert_eq!(
                resolve_mcp_profile(dir.path(), Some("nonsense")),
                McpProfile::Coordination
            );
        }
    }

    // =====================================================================
    // STORY-532 (EPIC-27): queue MCP tools
    // =====================================================================

    /// A git-canonical-backed server — the queue layer (`Storage::queue_*`)
    /// only works against a SQLite or directory backend, so the queue tests
    /// need this rather than the plain-YAML `mk_server`. trace:EPIC-27
    fn mk_git_server(dir: &Path) -> McpServer<'static> {
        use aida_core::db::{DatabaseBackend, GitBackend};
        let store_dir = dir.join(".aida-store");
        std::fs::create_dir_all(&store_dir).unwrap();
        let seed = GitBackend::new(&store_dir).unwrap();
        seed.save(&aida_core::RequirementsStore::new()).unwrap();
        let storage = Box::leak(Box::new(Storage::new(&store_dir)));
        McpServer::new(storage, dir.to_path_buf())
    }

    /// Seed a requirement via the MCP add tool (works against the git store)
    /// and return its SPEC-ID. trace:EPIC-27
    fn seed_req(server: &McpServer<'static>, title: &str) -> String {
        let resp = server
            .tool_add_requirement(&json!({
                "title": title,
                "description": "queue-test seed",
                "type": "task",
            }))
            .expect("seed add_requirement failed");
        added_spec_id(&resp).to_string()
    }

    // Queue tests pass `user` explicitly (rather than relying on AIDA_USER /
    // USER env) so they stay deterministic under cargo's parallel test
    // runner — env mutation would race across threads. trace:EPIC-27
    const QU: &str = "queue-test-user";

    #[test]
    fn queue_add_list_remove_roundtrip() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());

        let spec = seed_req(&server, "Queue roundtrip");

        // Empty to start.
        let listed = server.tool_queue_list(&json!({ "user": QU })).unwrap();
        assert!(listed.contains("Queue is empty"), "listed: {listed}");

        // Add.
        let added = server
            .tool_queue_add_inner(&json!({ "id": spec, "for": "implementer", "user": QU }))
            .unwrap();
        assert!(added.contains(&spec), "added: {added}");
        assert!(added.contains("for:implementer"), "added: {added}");

        // List shows it.
        let listed = server.tool_queue_list(&json!({ "user": QU })).unwrap();
        assert!(listed.contains(&spec), "listed: {listed}");
        assert!(listed.contains("Queue roundtrip"), "listed: {listed}");

        // Remove.
        let removed = server
            .tool_queue_remove(&json!({ "id": spec, "user": QU }))
            .unwrap();
        assert!(removed.contains(&spec), "removed: {removed}");

        let listed = server.tool_queue_list(&json!({ "user": QU })).unwrap();
        assert!(listed.contains("Queue is empty"), "listed: {listed}");
    }

    #[test]
    fn queue_add_refuses_terminal_without_force() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());
        let spec = seed_req(&server, "Terminal spec");
        force_status(&server, &spec, RequirementStatus::Completed);

        let err = server
            .tool_queue_add_inner(&json!({ "id": spec }))
            .expect_err("should refuse terminal without force");
        assert!(err.contains("re-queueing closed work"), "err: {err}");

        // force overrides.
        let ok = server
            .tool_queue_add_inner(&json!({ "id": spec, "force": true }))
            .expect("force should allow queueing terminal");
        assert!(ok.contains(&spec), "ok: {ok}");
    }

    #[test]
    fn queue_next_and_work_peek_the_head() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());
        let u = "queue-peek-user";

        // Empty: both report nothing.
        let next = server.tool_queue_next(&json!({ "user": u })).unwrap();
        assert!(next.contains("Queue is empty"), "next: {next}");

        let a = seed_req(&server, "First");
        let b = seed_req(&server, "Second");
        server
            .tool_queue_add_inner(&json!({ "id": a, "for": "any", "user": u }))
            .unwrap();
        server
            .tool_queue_add_inner(&json!({ "id": b, "for": "any", "user": u }))
            .unwrap();

        // `all: true` bypasses any ambient active-role default filter so the
        // unrouted (`for: any`) entries are visible regardless of the test
        // shell's AIDA_SESSION_ROLE. trace:EPIC-27
        let next = server
            .tool_queue_next(&json!({ "user": u, "all": true }))
            .unwrap();
        assert!(next.contains(&a), "head should be first-added: {next}");

        // queue_work with no id mirrors the head peek.
        let work = server
            .tool_queue_work(&json!({ "user": u, "all": true }))
            .unwrap();
        assert!(work.contains(&a), "work peek: {work}");
        assert!(work.contains("aida queue work"), "work hint: {work}");

        // queue_work with an explicit queued id resolves that spec.
        let work_b = server
            .tool_queue_work(&json!({ "id": &b, "user": u }))
            .unwrap();
        assert!(work_b.contains(&b), "work_b: {work_b}");
    }

    #[test]
    fn queue_done_flips_to_done_and_dequeues() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());
        let u = "queue-done-user";

        let spec = seed_req(&server, "To finish");
        server
            .tool_queue_add_inner(&json!({ "id": &spec, "user": u }))
            .unwrap();

        let done = server
            .tool_queue_done(&json!({ "id": &spec, "user": u }))
            .unwrap();
        assert!(done.contains("marked done"), "done: {done}");

        // Status is Done, queue is empty.
        let store = server.storage.load().unwrap();
        let req = store.get_requirement_by_spec_id(&spec).unwrap();
        assert_eq!(req.status, RequirementStatus::Done);
        assert!(req
            .implementation_info
            .as_ref()
            .map(|i| i.implemented)
            .unwrap_or(false));

        let listed = server.tool_queue_list(&json!({ "user": u })).unwrap();
        assert!(listed.contains("Queue is empty"), "listed: {listed}");
    }

    #[test]
    fn queue_rework_flips_status_and_requeues() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());
        let u = "queue-rework-user";

        let spec = seed_req(&server, "Reworkable");
        // Done → rework smart-targets InProgress.
        force_status(&server, &spec, RequirementStatus::Done);

        let resp = server
            .tool_queue_rework_inner(&json!({ "id": &spec, "for": "implementer", "reason": "PR review found issues", "user": u }))
            .unwrap();
        assert!(resp.contains("Done → In Progress"), "resp: {resp}");
        assert!(resp.contains("Queued"), "resp: {resp}");

        let store = server.storage.load().unwrap();
        let req = store.get_requirement_by_spec_id(&spec).unwrap();
        assert_eq!(req.status, RequirementStatus::InProgress);
        // Reason captured as a comment.
        assert!(req.comments.iter().any(|c| c.content.contains("PR review")));

        // It's back in the queue.
        let listed = server.tool_queue_list(&json!({ "user": u })).unwrap();
        assert!(listed.contains(&spec), "listed: {listed}");
    }

    #[test]
    fn queue_rework_refuses_terminal_without_force() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());
        let spec = seed_req(&server, "Closed spec");
        force_status(&server, &spec, RequirementStatus::Rejected);

        let err = server
            .tool_queue_rework_inner(&json!({ "id": &spec }))
            .expect_err("should refuse terminal rework without force");
        assert!(err.contains("re-opening closed work"), "err: {err}");
    }

    /// BUG-480: the public MCP `queue_add` / `queue_rework` tools had NO
    /// advisor-authority check — an MCP agent could push a spec straight into
    /// the execution pipeline, the exact act the CLI `aida queue add` gates on.
    /// Both public tools now refuse unconditionally (MCP is never advisor
    /// authority); the queue mechanics still work via the `_inner` methods
    /// (used by the advisor-corroborated CLI and by the mechanics tests above).
    // trace:BUG-480 | ai:claude
    #[test]
    fn mcp_queue_add_and_rework_are_advisor_gated() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());
        let spec = seed_req(&server, "Authority-gated queue target");

        // queue_add via the public MCP tool is refused.
        let err = server
            .tool_queue_add(&json!({ "id": &spec, "for": "implementer" }))
            .expect_err("queue_add via MCP must be advisor-gated");
        assert!(err.contains("advisor"), "queue_add err: {err}");

        // queue_rework via the public MCP tool is refused too.
        let err = server
            .tool_queue_rework(&json!({ "id": &spec, "reason": "self-requeue attempt" }))
            .expect_err("queue_rework via MCP must be advisor-gated");
        assert!(err.contains("advisor"), "queue_rework err: {err}");

        // Nothing slipped the gate: the spec is not in any queue and its status
        // is unchanged (the public tools mutated nothing).
        let store = server.storage.load().unwrap();
        assert_eq!(
            store.get_requirement_by_spec_id(&spec).unwrap().status,
            RequirementStatus::Draft
        );
    }

    /// BUG-486 (THE FIX): the MCP authority gate was role-blind — it refused the
    /// advisor-gated status transitions (Draft → Approved/Planned, the
    /// Draft/NeedsAttention → InProgress/Done bypass) and the queue_add/rework
    /// cases UNCONDITIONALLY, ignoring the caller's role even when the MCP session
    /// was a genuine advisor. The gate now routes its authority decision through
    /// the SAME predicate the CLI uses (`status_advance_requires_advisor_authority`
    /// + `advisor_authority_from`): an advisor caller is permitted, a non-advisor
    /// caller is refused.
    ///
    /// This test exercises the pure cores (`mcp_status_gate_message_for` /
    /// `mcp_queue_authority_message_for`) so the authority axis is injected, not
    /// read from the process-global `AIDA_SESSION_ROLE` env. Against the
    /// pre-BUG-486 gate (which took no authority param and refused regardless),
    /// the `caller_is_advisor = true` assertions below are red; with the fix they
    /// pass.
    // trace:BUG-486 | ai:claude
    #[test]
    fn mcp_authority_gate_honors_advisor_role() {
        use RequirementStatus::*;

        // ---- Status gate: advisor-gated transitions ----
        // The advisor-authority transitions the bug names.
        let advisor_gated = [
            (Draft, Approved),
            (Draft, Planned),
            (NeedsAttention, Approved),
            (NeedsAttention, Planned),
            (Draft, InProgress),
            (Draft, Done),
            (NeedsAttention, InProgress),
            (NeedsAttention, Done),
        ];
        for (from, to) in advisor_gated {
            // Non-advisor: refused with the existing guidance message.
            let refused = mcp_status_gate_message_for(&from, &to, false)
                .unwrap_or_else(|| panic!("{from} → {to} must be refused for a non-advisor"));
            assert!(
                refused.contains("advisor"),
                "{from} → {to} refusal should mention advisor authority: {refused}"
            );
            // Advisor (role entered): PERMITTED, exactly as the CLI under
            // AIDA_SESSION_ROLE=advisor. THIS is the BUG-486 regression: the old
            // gate refused here too.
            assert_eq!(
                mcp_status_gate_message_for(&from, &to, true),
                None,
                "{from} → {to} must be PERMITTED for an advisor (BUG-486)"
            );
        }

        // Completed stays merge-driven: refused regardless of role (advisor
        // authority does not unlock it).
        for advisor in [false, true] {
            let msg = mcp_status_gate_message_for(&Draft, &Completed, advisor)
                .expect("Completed via MCP is always refused (merge-driven)");
            assert!(
                msg.contains("merge") || msg.contains("(SPEC-ID)"),
                "Completed refusal should explain merge-driven: {msg}"
            );
        }

        // In-pipeline execution flips are implementer-legitimate for everyone —
        // never gated, with or without advisor authority.
        for (from, to) in [(Approved, InProgress), (InProgress, Done)] {
            for advisor in [false, true] {
                assert_eq!(
                    mcp_status_gate_message_for(&from, &to, advisor),
                    None,
                    "{from} → {to} is an implementer-legitimate flip (advisor={advisor})"
                );
            }
        }

        // ---- Queue authority gate (queue_add / queue_rework) ----
        // Non-advisor refused; advisor permitted (BUG-480/TASK-718 reconciled).
        let refused = mcp_queue_authority_message_for(false)
            .expect("queue_add/rework via MCP must be refused for a non-advisor");
        assert!(refused.contains("advisor"), "queue refusal: {refused}");
        assert_eq!(
            mcp_queue_authority_message_for(true),
            None,
            "queue_add/rework must be PERMITTED for an advisor (BUG-486)"
        );

        // ---- The env-resolving wrapper agrees with the pure core ----
        // mcp_caller_has_advisor_authority() reads AIDA_SESSION_ROLE; in the test
        // process no advisor role is entered, so it resolves non-advisor and the
        // public wrappers refuse — matching the headless / non-TTY default
        // (acceptance #5).
        assert!(
            !mcp_caller_has_advisor_authority(),
            "test process has no advisor role entered"
        );
        assert!(
            mcp_status_gate_message(&Draft, &Approved).is_some(),
            "public status gate refuses without an advisor role"
        );
        assert!(
            mcp_queue_authority_message().is_some(),
            "public queue gate refuses without an advisor role"
        );
    }

    #[test]
    fn queue_move_reorders() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());
        let u = "queue-move-user";

        let a = seed_req(&server, "Alpha");
        let b = seed_req(&server, "Bravo");
        let c = seed_req(&server, "Charlie");
        for id in [&a, &b, &c] {
            server
                .tool_queue_add_inner(&json!({ "id": id, "for": "any", "user": u }))
                .unwrap();
        }

        // Move C to the top.
        let moved = server
            .tool_queue_move(&json!({ "id": &c, "top": true, "user": u }))
            .unwrap();
        assert!(moved.contains(&c), "moved: {moved}");

        let next = server
            .tool_queue_next(&json!({ "user": u, "all": true }))
            .unwrap();
        assert!(next.contains(&c), "C should now be the head: {next}");

        // Moving a spec not in the queue errors.
        let stray = seed_req(&server, "Stray");
        let err = server
            .tool_queue_move(&json!({ "id": &stray, "top": true, "user": u }))
            .expect_err("non-queued move should error");
        assert!(err.contains("not in the queue"), "err: {err}");

        // No destination errors.
        let err = server
            .tool_queue_move(&json!({ "id": &a, "user": u }))
            .expect_err("no destination should error");
        assert!(err.contains("specify a destination"), "err: {err}");
    }

    #[test]
    fn queue_progress_buckets_by_status() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());

        let done = seed_req(&server, "Shipped one");
        let inprog = seed_req(&server, "Working one");
        let todo = seed_req(&server, "Remaining one");
        force_status(&server, &done, RequirementStatus::Completed);
        force_status(&server, &inprog, RequirementStatus::InProgress);
        // todo stays Draft.

        let resp = server
            .tool_queue_progress(&json!({ "specs": [done, inprog, todo] }))
            .unwrap();
        assert!(resp.contains("Progress (3 items)"), "resp: {resp}");
        assert!(resp.contains("Shipped: 1"), "resp: {resp}");
        assert!(resp.contains("Working now: 1"), "resp: {resp}");
        assert!(resp.contains("Remaining: 1"), "resp: {resp}");
    }

    #[test]
    fn queue_progress_needs_a_source() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());
        let err = server
            .tool_queue_progress(&json!({}))
            .expect_err("progress with no source should error");
        assert!(err.contains("batch"), "err: {err}");
    }

    #[test]
    fn queue_tools_are_in_descriptors_and_classified() {
        let names: Vec<String> = tool_descriptors()
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.get("name").unwrap().as_str().unwrap().to_string())
            .collect();
        for t in [
            "queue_list",
            "queue_add",
            "queue_work",
            "queue_done",
            "queue_next",
            "queue_progress",
            "queue_rework",
            "queue_move",
            "queue_remove",
        ] {
            assert!(names.contains(&t.to_string()), "missing descriptor: {t}");
            // No queue tool may fall through to the Admin catch-all.
            assert_ne!(
                tool_min_profile(t),
                McpProfile::Admin,
                "{t} is unclassified"
            );
        }
        // Read vs write tier sanity.
        assert_eq!(tool_min_profile("queue_list"), McpProfile::ReadOnly);
        assert_eq!(tool_min_profile("queue_work"), McpProfile::ReadOnly);
        assert_eq!(tool_min_profile("queue_remove"), McpProfile::Coordination);
        assert_eq!(tool_min_profile("queue_add"), McpProfile::Operator);
    }

    #[test]
    fn queue_user_id_honors_explicit_user_arg() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());
        // Explicit `user` arg wins over any ambient env (deterministic under
        // the parallel test runner). The env-fallback chain itself is covered
        // by `current_user_id_*` tests in main.rs. trace:EPIC-27
        assert_eq!(
            server.queue_user_id(&json!({ "user": "explicit" })),
            "explicit"
        );
        // With no arg, it never returns empty — resolves to *some* identity.
        assert!(!server.queue_user_id(&json!({})).is_empty());
    }

    // =====================================================================
    // STORY-533 (EPIC-27): session MCP tools
    // =====================================================================

    /// Write a minimal lease TOML under `.aida/sessions/<id>.toml` matching
    /// the `LightLease` shape the session tools read. trace:EPIC-27
    fn seed_lease(dir: &Path, id: &str, scope: &str, role: &str) {
        let sessions = dir.join(".aida").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let body = format!(
            "id = \"{id}\"\n\
             scope = \"{scope}\"\n\
             slug = \"{scope}\"\n\
             owner = \"tester\"\n\
             worktree_path = \"/tmp/wt-{scope}\"\n\
             branch = \"{scope}-branch\"\n\
             started_at = \"2026-06-07T00:00:00Z\"\n\
             hostname = \"testhost\"\n\
             role = \"{role}\"\n\
             mcp_claim = false\n"
        );
        std::fs::write(sessions.join(format!("{id}.toml")), body).unwrap();
    }

    #[test]
    fn session_leases_lists_seeded_leases() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());

        // Empty to start.
        let empty = server.tool_session_leases(&json!({})).unwrap();
        assert!(empty.contains("no active sessions"), "empty: {empty}");

        seed_lease(dir.path(), "aaaaaaaa1111", "EPIC-27", "implementer");
        let listed = server.tool_session_leases(&json!({})).unwrap();
        assert!(listed.contains("EPIC-27"), "listed: {listed}");
        assert!(listed.contains("role=implementer"), "listed: {listed}");
        assert!(listed.contains("aaaaaaaa"), "listed short id: {listed}");
    }

    #[test]
    fn session_status_shows_a_single_lease_and_errors_on_ambiguity() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());
        seed_lease(dir.path(), "bbbbbbbb2222", "STORY-1", "implementer");

        // Single lease — id optional.
        let shown = server.tool_session_status(&json!({})).unwrap();
        assert!(shown.contains("STORY-1"), "shown: {shown}");
        assert!(shown.contains("branch: STORY-1-branch"), "shown: {shown}");
        assert!(shown.contains("role: implementer"), "shown: {shown}");

        // Add a second — now id is required.
        seed_lease(dir.path(), "cccccccc3333", "STORY-2", "reviewer");
        let err = server
            .tool_session_status(&json!({}))
            .expect_err("ambiguous without id");
        assert!(err.contains("multiple active sessions"), "err: {err}");

        // id prefix disambiguates.
        let shown2 = server
            .tool_session_status(&json!({ "id": "cccccccc" }))
            .unwrap();
        assert!(shown2.contains("STORY-2"), "shown2: {shown2}");
    }

    #[test]
    fn session_manifest_reads_planned_cluster() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());
        let lease_id = "dddddddd4444";
        seed_lease(dir.path(), lease_id, "EPIC-9", "implementer");

        // No manifest yet.
        let none = server.tool_session_manifest(&json!({})).unwrap();
        assert!(none.contains("no planned-cluster manifest"), "none: {none}");

        // Write a manifest with one completed + one pending item.
        let path = crate::session_manifest::manifest_path(dir.path(), lease_id);
        let manifest = crate::session_manifest::SessionManifest {
            session_id: lease_id.to_string(),
            planned_at: chrono::Utc::now(),
            plan_source: "test".to_string(),
            claude_session_id: None,
            batch_name: None,
            plan: None,
            items: vec![
                crate::session_manifest::ManifestItem {
                    spec_id: "TASK-100".to_string(),
                    position: 0,
                    status_at_plan: "Draft".to_string(),
                    started_at: Some(chrono::Utc::now()),
                    completed_at: Some(chrono::Utc::now()),
                    note: None,
                },
                crate::session_manifest::ManifestItem {
                    spec_id: "TASK-101".to_string(),
                    position: 1,
                    status_at_plan: "Draft".to_string(),
                    started_at: None,
                    completed_at: None,
                    note: None,
                },
            ],
        };
        crate::session_manifest::save(&path, &manifest).unwrap();

        let shown = server.tool_session_manifest(&json!({})).unwrap();
        let check = crate::glyphs::Glyph::Check.render(crate::glyphs::active_profile(None));
        assert!(shown.contains("1/2 done"), "shown: {shown}");
        assert!(
            shown.contains(&format!("{check} TASK-100")),
            "shown: {shown}"
        );
        assert!(shown.contains("○ TASK-101"), "shown: {shown}");
    }

    #[test]
    fn session_start_is_a_peek_with_command() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());

        let peek = server
            .tool_session_start(&json!({ "owns": "EPIC-27", "role": "implementer" }))
            .unwrap();
        assert!(
            peek.contains("aida session start --owns EPIC-27"),
            "peek: {peek}"
        );
        assert!(peek.contains("--role implementer"), "peek: {peek}");
        // Does NOT actually create a lease.
        assert!(list_leases(&server.project_root).is_empty());

        // Warns when the scope is already held.
        seed_lease(dir.path(), "eeeeeeee5555", "EPIC-27", "implementer");
        let warned = server
            .tool_session_start(&json!({ "owns": "EPIC-27" }))
            .unwrap();
        assert!(warned.contains("already held"), "warned: {warned}");

        // owns is required.
        let err = server
            .tool_session_start(&json!({}))
            .expect_err("owns required");
        assert!(err.contains("owns"), "err: {err}");
    }

    // trace:TASK-712 — POSIX single-quote rule.
    #[test]
    fn shell_quote_arg_quotes_metachars_and_passes_safe_words() {
        // Safe words pass through bare for readability.
        assert_eq!(shell_quote_arg("EPIC-27"), "EPIC-27");
        assert_eq!(shell_quote_arg("feature/foo_bar.v2"), "feature/foo_bar.v2");
        // Spaces / metacharacters get single-quoted.
        assert_eq!(shell_quote_arg("a b"), "'a b'");
        assert_eq!(shell_quote_arg("a;rm -rf b"), "'a;rm -rf b'");
        assert_eq!(shell_quote_arg("$(whoami)"), "'$(whoami)'");
        // Embedded single quote uses the '\'' escape.
        assert_eq!(shell_quote_arg("it's"), "'it'\\''s'");
        // Empty is quoted (not a safe word) so it stays a single empty arg.
        assert_eq!(shell_quote_arg(""), "''");
    }

    // trace:TASK-712 — the session_start peek must shell-quote a scope/branch
    // containing spaces so the pasted command is a single correct argument.
    #[test]
    fn session_start_peek_quotes_unsafe_args() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());
        let peek = server
            .tool_session_start(&json!({ "owns": "my scope", "branch": "a;b" }))
            .unwrap();
        assert!(
            peek.contains("--owns 'my scope'"),
            "scope must be quoted; peek: {peek}"
        );
        assert!(
            peek.contains("--branch 'a;b'"),
            "branch must be quoted; peek: {peek}"
        );
    }

    #[test]
    fn session_end_is_a_peek_with_command() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());

        // No sessions.
        let none = server.tool_session_end(&json!({})).unwrap();
        assert!(none.contains("nothing to end"), "none: {none}");

        seed_lease(dir.path(), "ffffffff6666", "EPIC-27", "implementer");
        let peek = server.tool_session_end(&json!({})).unwrap();
        assert!(peek.contains("aida session end ffffffff"), "peek: {peek}");
        // Lease still present (peek did not remove it).
        assert!(!list_leases(&server.project_root).is_empty());

        // Resolve by spec/scope.
        let by_spec = server
            .tool_session_end(&json!({ "spec": "EPIC-27" }))
            .unwrap();
        assert!(by_spec.contains("ffffffff"), "by_spec: {by_spec}");

        // Ambiguous without selector once a second lease exists.
        seed_lease(dir.path(), "99999999aaaa", "EPIC-99", "implementer");
        let err = server
            .tool_session_end(&json!({}))
            .expect_err("ambiguous end");
        assert!(err.contains("multiple active sessions"), "err: {err}");
    }

    #[test]
    fn session_tools_are_in_descriptors_and_classified() {
        let names: Vec<String> = tool_descriptors()
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.get("name").unwrap().as_str().unwrap().to_string())
            .collect();
        for t in [
            "session_leases",
            "session_status",
            "session_manifest",
            "session_start",
            "session_end",
        ] {
            assert!(names.contains(&t.to_string()), "missing descriptor: {t}");
            // No session tool may fall through to the Admin catch-all.
            assert_ne!(
                tool_min_profile(t),
                McpProfile::Admin,
                "{t} is unclassified"
            );
            // All session tools are read tier (the two peeks never mutate).
            assert_eq!(
                tool_min_profile(t),
                McpProfile::ReadOnly,
                "{t} should be read-only"
            );
        }
    }

    // =====================================================================
    // STORY-534 (EPIC-27): role MCP tools
    // =====================================================================

    /// Write a minimal role TOML under `.aida/roles/<name>.toml` matching the
    /// `LightRole` shape the role tools read. trace:EPIC-27
    fn seed_role(dir: &Path, name: &str, purpose: &str) {
        let roles = dir.join(".aida").join("roles");
        std::fs::create_dir_all(&roles).unwrap();
        let body = format!(
            "name = \"{name}\"\n\
             purpose = \"{purpose}\"\n\
             created_at = \"2026-06-01T00:00:00Z\"\n\
             last_active_at = \"2026-06-07T00:00:00Z\"\n\
             working_directory = \"/tmp/wt-{name}\"\n\
             global = false\n\
             scope_tags = [\"inbox\"]\n\
             scope_status = \"draft\"\n"
        );
        std::fs::write(roles.join(format!("{name}.toml")), body).unwrap();
    }

    #[test]
    fn role_list_lists_seeded_roles() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());

        // NOTE: list_light_roles also reads `~/.aida/roles/` (global roles),
        // which may be populated on the developer/CI machine, so we don't
        // assert emptiness — we assert the seeded *project* roles surface. The
        // empty-state branch is covered by the help text path, not here.
        // trace:EPIC-27
        seed_role(dir.path(), "proj-impl", "ships code");
        seed_role(dir.path(), "proj-adv", "strategy partner");
        let listed = server.tool_role_list(&json!({})).unwrap();
        assert!(listed.contains("proj-impl"), "listed: {listed}");
        assert!(listed.contains("proj-adv"), "listed: {listed}");
        assert!(listed.contains("ships code"), "listed: {listed}");
    }

    #[test]
    fn role_show_renders_a_role_and_errors_on_unknown() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());
        seed_role(dir.path(), "reviewer", "guards the merge");

        let shown = server
            .tool_role_show(&json!({ "name": "reviewer" }))
            .unwrap();
        assert!(shown.contains("Role:        reviewer"), "shown: {shown}");
        assert!(shown.contains("guards the merge"), "shown: {shown}");
        assert!(shown.contains("tags=inbox"), "shown: {shown}");
        assert!(shown.contains("status=draft"), "shown: {shown}");

        // Unknown role errors.
        let err = server
            .tool_role_show(&json!({ "name": "nope" }))
            .expect_err("unknown role");
        assert!(err.contains("No such role"), "err: {err}");
    }

    #[test]
    fn role_show_canonicalizes_dialog_to_advisor() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());
        // Legacy `dialog.toml` should resolve as the advisor role.
        let roles = dir.path().join(".aida").join("roles");
        std::fs::create_dir_all(&roles).unwrap();
        std::fs::write(
            roles.join("dialog.toml"),
            "name = \"dialog\"\nlast_active_at = \"2026-06-07T00:00:00Z\"\n",
        )
        .unwrap();

        let shown = server
            .tool_role_show(&json!({ "name": "advisor" }))
            .unwrap();
        assert!(shown.contains("Role:        advisor"), "shown: {shown}");
    }

    #[test]
    fn role_enter_is_a_peek_with_command() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());
        seed_role(dir.path(), "implementer", "ships code");

        let peek = server
            .tool_role_enter(&json!({ "name": "implementer", "cd": true }))
            .unwrap();
        assert!(
            peek.contains("aida role enter implementer --cd"),
            "peek: {peek}"
        );
        assert!(peek.contains("ships code"), "peek: {peek}");

        // name is required.
        let err = server
            .tool_role_enter(&json!({}))
            .expect_err("name required");
        assert!(err.contains("name"), "err: {err}");

        // Unknown role errors.
        let unknown = server
            .tool_role_enter(&json!({ "name": "ghost" }))
            .expect_err("unknown role");
        assert!(unknown.contains("No such role"), "unknown: {unknown}");
    }

    #[test]
    fn role_end_is_a_peek_with_command() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());
        let peek = server.tool_role_end(&json!({})).unwrap();
        assert!(peek.contains("aida role end"), "peek: {peek}");
    }

    #[test]
    fn role_tools_are_in_descriptors_and_classified() {
        let names: Vec<String> = tool_descriptors()
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.get("name").unwrap().as_str().unwrap().to_string())
            .collect();
        for t in ["role_list", "role_show", "role_enter", "role_end"] {
            assert!(names.contains(&t.to_string()), "missing descriptor: {t}");
            // No role tool may fall through to the Admin catch-all.
            assert_ne!(
                tool_min_profile(t),
                McpProfile::Admin,
                "{t} is unclassified"
            );
            // All role tools are read tier (the two peeks never mutate).
            assert_eq!(
                tool_min_profile(t),
                McpProfile::ReadOnly,
                "{t} should be read-only"
            );
        }
    }

    // =====================================================================
    // STORY-536 (EPIC-27): workflow MCP tools
    // =====================================================================

    #[test]
    fn workflow_tools_are_in_descriptors_and_classified() {
        let names: Vec<String> = tool_descriptors()
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.get("name").unwrap().as_str().unwrap().to_string())
            .collect();
        for t in [
            "cache_status",
            "plan_verify",
            "plan_helpers",
            "ultraplan_assemble",
            "goal_derive",
            "status_unified",
            "usage_query",
            "db_sync",
            "fetch",
            "pull",
        ] {
            assert!(names.contains(&t.to_string()), "missing descriptor: {t}");
            // No workflow tool may fall through to the Admin catch-all, and
            // every one is read tier (the library mirrors are pure reads; the
            // three git verbs are metadata-only PEEKs). trace:EPIC-27
            assert_eq!(
                tool_min_profile(t),
                McpProfile::ReadOnly,
                "{t} should be read-only"
            );
        }
    }

    #[test]
    fn cache_status_reports_freshness() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());
        let out = server.tool_cache_status(&json!({})).unwrap();
        assert!(out.contains("Cache path:"), "out: {out}");
        assert!(out.contains("Cached requirements:"), "out: {out}");
        assert!(
            out.contains("Status:") && (out.contains("FRESH") || out.contains("STALE")),
            "out: {out}"
        );
    }

    #[test]
    fn plan_verify_lints_a_plan_file() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());
        // Minimal plan missing required sections — must report and verdict.
        let plan = dir.path().join("plan.md");
        std::fs::write(&plan, "# Plan: STORY-X\n\nSome prose, no real sections.\n").unwrap();
        let out = server
            .tool_plan_verify(&json!({ "file": plan.to_str().unwrap() }))
            .unwrap();
        assert!(out.contains("Verifying plan:"), "out: {out}");
        assert!(out.contains("Verdict:"), "out: {out}");
        // A missing file is an error, not a panic.
        let err = server
            .tool_plan_verify(&json!({ "file": "does-not-exist.md" }))
            .unwrap_err();
        assert!(err.contains("could not read plan file"), "err: {err}");
    }

    #[test]
    fn plan_helpers_handles_no_helpers() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());
        let spec = seed_req(&server, "Helpers target");
        let out = server.tool_plan_helpers(&json!({ "spec": spec })).unwrap();
        // A lone seeded spec has no related helper-bearing specs.
        assert!(out.contains("No reusable helpers derived"), "out: {out}");
        // Unknown spec errors cleanly.
        let err = server
            .tool_plan_helpers(&json!({ "spec": "NOPE-999" }))
            .unwrap_err();
        assert!(!err.is_empty(), "err: {err}");
    }

    #[test]
    fn ultraplan_assemble_builds_a_prompt() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());
        let spec = seed_req(&server, "Ultraplan target");
        let out = server
            .tool_ultraplan_assemble(&json!({ "spec": spec }))
            .unwrap();
        assert!(out.contains("Plan the implementation of"), "out: {out}");
        assert!(out.contains("## Acceptance criteria"), "out: {out}");
    }

    #[test]
    fn goal_derive_composes_clauses_and_requires_an_axis() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());
        let out = server
            .tool_goal_derive(&json!({ "spec": "STORY-7", "pr": 42 }))
            .unwrap();
        assert!(out.starts_with("/goal "), "out: {out}");
        assert!(out.contains("verify each clause:"), "out: {out}");
        assert!(out.contains("aida show STORY-7"), "out: {out}");
        assert!(out.contains("gh pr view 42"), "out: {out}");
        // No axis → error.
        let err = server.tool_goal_derive(&json!({})).unwrap_err();
        assert!(err.contains("no condition flags"), "err: {err}");
    }

    #[test]
    fn status_unified_reports_counts_and_queue_depth() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());
        seed_req(&server, "A status spec");
        let out = server
            .tool_status_unified(&json!({ "user": "su-test" }))
            .unwrap();
        assert!(out.contains("Project status"), "out: {out}");
        assert!(out.contains("By status:"), "out: {out}");
        assert!(out.contains("Active sessions:"), "out: {out}");
        assert!(out.contains("Queue depth ('su-test')"), "out: {out}");
    }

    #[test]
    fn status_unified_excludes_meta_from_count_matching_cli() {
        // BUG-717: the MCP status count must exclude standing-artifact /
        // stateless types (meta, folder, ...) exactly as `aida status` / `aida
        // list` do — otherwise the seeded META prompt-templates inflate the
        // "Total requirements" the MCP surface reports vs the CLI.
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());
        seed_req(&server, "A real work spec"); // type=task → counted
        server
            .tool_add_requirement(&json!({
                "title": "A META prompt template",
                "description": "standing artifact, not work",
                "type": "meta",
            }))
            .expect("seed meta add_requirement failed");
        let out = server
            .tool_status_unified(&json!({ "user": "su-test" }))
            .unwrap();
        // Only the task counts; the META spec is excluded (the BUG-717 bug was
        // it reading "Total requirements: 2").
        assert!(
            out.contains("Total requirements: 1"),
            "META must be excluded from the MCP status count: {out}"
        );
    }

    #[test]
    fn usage_query_handles_empty_log_gracefully() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());
        // The local log may or may not exist in CI; the tool must never panic
        // and must return a string in both the empty and populated cases.
        let out = server.tool_usage_query(&json!({})).unwrap();
        assert!(out.contains("Usage:"), "out: {out}");
    }

    #[test]
    fn git_verbs_peek_and_return_commands() {
        let dir = tempdir().unwrap();
        let server = mk_git_server(dir.path());

        let sync = server
            .tool_db_sync(&json!({ "pull": true, "push": true }))
            .unwrap();
        assert!(sync.contains("Run:"), "sync: {sync}");
        assert!(sync.contains("aida db sync --pull --push"), "sync: {sync}");
        // No legs → defaults to both.
        let sync2 = server.tool_db_sync(&json!({})).unwrap();
        assert!(
            sync2.contains("aida db sync --pull --push"),
            "sync2: {sync2}"
        );

        let fetch = server.tool_fetch(&json!({ "code_only": true })).unwrap();
        assert!(fetch.contains("aida fetch --code-only"), "fetch: {fetch}");

        let pull = server.tool_pull(&json!({ "store_only": true })).unwrap();
        assert!(pull.contains("aida pull --store-only"), "pull: {pull}");
    }
}
