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
use crate::punt::{
    self, append_to_ledger, ledger_path, punt_response_path, read_ledger, PuntRecord,
    PuntResolution, PuntResponse,
};

const VALID_MCP_REQUIREMENT_TYPES: &str =
    "functional, non-functional, system, user, bug, epic, story, task, spike, sprint, folder, meta, doc";

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
        .and_then(|s| s.split(|c: char| c == ')' || c == '+').next())
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
fn mcp_claim_path(dir: &Path, spec_id: &str) -> PathBuf {
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
        if tool_name != "" && is_known_tool(tool_name) && !tool_in_profile(tool_name, self.profile)
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
                    }
                ]
            }),
        )
    }

    fn handle_resources_read(&self, id: &Value, params: &Value) -> JsonRpcResponse {
        let uri = params.get("uri").and_then(|v| v.as_str()).unwrap_or("");

        let result = match uri {
            "aida://project/summary" => self.resource_project_summary(),
            "aida://requirements/tree" => self.resource_requirements_tree(),
            _ => Err(format!("Unknown resource: {}", uri)),
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

        let filtered: Vec<&Requirement> = store
            .requirements
            .iter()
            .filter(|r| {
                if let Some(status) = status_filter {
                    if !mcp_filter_eq(&r.status.to_string(), status) {
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
                r.status,
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
            req.status,
            req.priority,
            req.req_type
        );

        if !req.feature.is_empty() {
            output.push_str(&format!("**Feature:** {}\n", req.feature));
        }
        if !req.owner.is_empty() {
            output.push_str(&format!("**Owner:** {}\n", req.owner));
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
                    if let Some(msg) = mcp_status_gate_message(&new_status) {
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
                    if !mcp_filter_eq(&r.status.to_string(), status) {
                        return false;
                    }
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

        let result = walk_union(&store, root_id, &specs, depth);
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
        let since = args
            .get("since")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        let opts = HistoryOpts {
            limit: 100,
            max_commits: 500,
            events_mode: true,
            id_filter: spec_id,
            type_filter: None,
            author_filter: None,
            since,
            until: None,
            status_changes_only: false,
            shipped_only: false,
            comments_only: false,
            oneline: false,
            // MCP consumers expect an event ledger, not the CLI's
            // day-to-day archived-work filter.
            archived_specs: std::collections::HashSet::new(),
            archived_only_specs: None,
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
        let thread_id = args
            .get("thread")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| id.clone());
        let msg = Message {
            id: id.clone(),
            thread_id: thread_id.clone(),
            from,
            to: recipient,
            timestamp: chrono::Utc::now().timestamp_millis(),
            in_reply_to: args
                .get("in_reply_to")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            body: body.to_string(),
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
        let all = crate::mailbox_store::read_local_messages(&self.project_root)
            .map_err(|e| e.to_string())?;
        let messages: Vec<Value> = inbox_for(&agent, &all)
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
                })
            })
            .collect();
        serde_json::to_string_pretty(&json!({
            "agent": agent,
            "count": messages.len(),
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

        let comment = Comment::new("mcp".to_string(), text.to_string());
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
        let filtered: Vec<&PuntRecord> = records
            .iter()
            .filter(|r| match status_filter.as_deref() {
                None => true,
                Some("awaiting") => r.resolution_path == "punted",
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

        Ok(if flipped {
            format!("Punt recorded for {spec} [{category}]; spec flipped to NeedsAttention.")
        } else {
            format!(
                "Punt recorded for {spec} [{category}]. Spec status unchanged \
                 (only an In Progress spec auto-flips to NeedsAttention); flip \
                 manually with `aida edit {spec} --status needs-attention` if needed."
            )
        })
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

        let response = PuntResponse {
            resolution: PuntResolution::Resolved,
            answer: Some(answer.to_string()),
            reasoning: reasoning.to_string(),
            classification,
            escalation_reason: None,
        };

        let path = punt_response_path(&self.project_root, spec);
        let body = serde_json::to_string_pretty(&response).map_err(|e| e.to_string())?;
        // trace:TASK-439 | ai:claude
        write_atomic(&path, body.as_bytes()).map_err(|e| e.to_string())?;

        Ok(format!(
            "Resolution written to {} — the orchestrator will resume the implementer with this answer.",
            path.display()
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

        let response = PuntResponse {
            resolution: PuntResolution::Escalated,
            answer: None,
            reasoning: reasoning.to_string(),
            classification,
            escalation_reason,
        };

        let path = punt_response_path(&self.project_root, spec);
        let body = serde_json::to_string_pretty(&response).map_err(|e| e.to_string())?;
        // trace:TASK-439 | ai:claude
        write_atomic(&path, body.as_bytes()).map_err(|e| e.to_string())?;

        Ok(format!(
            "Escalation written to {} — the orchestrator will park the spec for human triage.",
            path.display()
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
        if let Some(msg) = mcp_status_gate_message(&new_status) {
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
        Ok(format!("Finding {} {}d", id, action.to_ascii_lowercase()))
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

    // trace:EPIC-27
    fn tool_queue_add(&self, args: &Value) -> Result<String, String> {
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

    // trace:EPIC-27
    fn tool_queue_rework(&self, args: &Value) -> Result<String, String> {
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
        self.storage
            .queue_remove(&user_id, &req.id)
            .map_err(|e| e.to_string())?;
        Ok(format!("Removed {} from queue.", display))
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

/// BUG-449: status transitions an MCP caller may NOT make itself, with the
/// message explaining why. `None` = the transition is allowed. Mirrors the
/// `add_requirement` intake gate (TASK-647 / ADR-3) on the update path so
/// `add`-then-`update` isn't a one-line bypass of advisor authority. MCP is
/// never advisor authority (unlike the CLI, which can be the advisor seat or
/// orchestrator-corroborated), so this stays strict regardless of context.
/// trace:BUG-449 | ai:claude
fn mcp_status_gate_message(new_status: &RequirementStatus) -> Option<String> {
    match new_status {
        RequirementStatus::Approved | RequirementStatus::Planned => Some(format!(
            "Cannot set status to {new_status} via MCP: approving or planning a spec is the \
             advisor's triage decision and needs advisor authority. File the spec and let the \
             advisor promote it, or run the CLI as the advisor (AIDA_SESSION_ROLE=advisor)."
        )),
        RequirementStatus::Completed => Some(format!(
            "Cannot set status to {new_status} via MCP: this is set automatically when a \
             (SPEC-ID) commit lands on the default branch (merge-driven auto-bump), not by hand. \
             Mark the work `done` and let the merge promote it."
        )),
        _ => None,
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
        "bug" => Some(RequirementType::Bug),
        "epic" => Some(RequirementType::Epic),
        "story" => Some(RequirementType::Story),
        "task" => Some(RequirementType::Task),
        "spike" => Some(RequirementType::Spike),
        "sprint" => Some(RequirementType::Sprint),
        "folder" => Some(RequirementType::Folder),
        "meta" => Some(RequirementType::Meta),
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
    store
        .requirements
        .iter()
        .map(|r| aida_core::RequirementSummary {
            id: r.id,
            spec_id: r.spec_id.clone(),
            agreed_id: r.agreed_id.clone(),
            title: r.title.clone(),
            description: r.description.clone(),
            status: format!("{}", r.status),
            priority: format!("{}", r.priority),
            owner: r.owner.clone(),
            feature: r.feature.clone(),
            req_type: format!("{}", r.req_type),
            tags: r.tags.iter().cloned().collect(),
            created_at: r.created_at.to_rfc3339(),
            modified_at: r.modified_at.to_rfc3339(),
            archived: r.archived,
            // trace:STORY-441 | ai:claude
            archived_at: r.archived_at.map(|dt| dt.to_rfc3339()),
            yaml_path: String::new(),
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
        | "queue_work" => McpProfile::ReadOnly,

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
            "description": "List requirements from the AIDA database, optionally filtered by status, type, feature category, priority, tags, batch, parent, owner role, or in-flight state. Mirrors the current `aida list` filter surface. Returns a summarized list of matching requirements.",
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
                        "enum": ["functional", "non-functional", "system", "user", "bug", "epic", "story", "task", "spike", "sprint", "folder", "meta", "doc"],
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
                        "description": "Required requirement type. Valid types: functional, non-functional, system, user, bug, epic, story, task, spike, sprint, folder, meta, doc. Normalizes the assigned SPEC-ID prefix (e.g., 'task' becomes 'TASK-N').",
                        "enum": ["functional", "non-functional", "system", "user", "bug", "epic", "story", "task", "spike", "sprint", "folder", "meta", "doc"],
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
            // trace:STORY-82 | ai:claude
            "description": "Update fields of an existing requirement (title, type, status, priority, description, tags, parent). Fields omitted from parameters remain unchanged. Note: advisor-authority transitions (approved/planned) and the merge-driven `completed` status are gated and cannot be set via MCP.",
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
                        "enum": ["functional", "non-functional", "system", "user", "bug", "epic", "story", "task", "spike", "sprint", "folder", "meta", "doc"],
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
            "description": "Case-insensitive keyword search across requirement titles, descriptions, and SPEC-IDs, optionally narrowed by type and/or status. Mirrors `aida search`.",
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
                        "enum": ["functional", "non-functional", "system", "user", "bug", "epic", "story", "task", "spike", "sprint", "folder", "meta", "doc"],
                        "example": "bug"
                    },
                    "status": {
                        "type": "string",
                        "description": "Restrict results to this status.",
                        "enum": ["draft", "approved", "planned", "in-progress", "needs-attention", "done", "completed", "rejected"],
                        "example": "in-progress"
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
                    "from": { "type": "string", "description": "Sender id (default: this server's agent/user identity).", "example": "claude" }
                },
                "required": ["body"]
            },
            "outputSchema": text_envelope_output_schema(
                "a confirmation line `Message sent: <id> (thread <thread-id>)`."
            )
        },
        {
            "name": "read_inbox",
            "description": "Read an agent's mailbox inbox — messages addressed to it plus broadcasts (excluding its own sent), oldest-first — mirroring `aida mailbox inbox`. Returns pretty-printed JSON.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent": { "type": "string", "description": "Whose inbox (default: this server's agent/user identity).", "example": "claude" }
                }
            },
            "outputSchema": text_envelope_output_schema(
                "pretty-printed JSON `{agent, count, messages:[{id,thread_id,from,to,timestamp,in_reply_to,body}]}`."
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
            "description": "Read AIDA's orphan-branch event ledger, mirroring `aida history --events` for MCP consumers. Returns pretty-printed JSON with structured event records.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "spec_id": {
                        "type": "string",
                        "description": "Optional SPEC-ID filter for a single requirement's event history.",
                        "pattern": "^[A-Z]+-\\d+(-\\d+)*$",
                        "example": "TASK-538"
                    },
                    "since": {
                        "type": "string",
                        "description": "Optional lower time bound, passed through to git just like `aida history --since` (RFC3339 or relative expressions supported by git).",
                        "example": "24 hours ago"
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
                        "description": "Detailed human-readable description of the obstacle or design fork.",
                        "example": "The downstream service API has updated its rate limits from 100/min to 10/min, breaking our batch ingest assumption."
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
            "description": "Mark a requirement Done and remove it from the queue in one step. Mirrors `aida queue done`. Flips status to Done (work finished on a branch) — the merge auto-bump later advances Done → Completed. Stamps implementation_info.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Requirement id (UUID or SPEC-ID) to mark done.", "example": "STORY-42" },
                    "user": { "type": "string", "description": "Override the queue user id.", "example": "alice" }
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
            "description": "Remove a requirement from the queue. Mirrors `aida queue remove`. Does not change the spec's status.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Requirement id (UUID or SPEC-ID) to remove.", "example": "STORY-42" },
                    "user": { "type": "string", "description": "Override the queue user id.", "example": "alice" }
                },
                "required": ["id"]
            },
            "outputSchema": text_envelope_output_schema(
                "a confirmation line `Removed <SPEC-ID> from queue.`"
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
        server
            .tool_update_requirement(&json!({ "id": working_id, "status": "in-progress" }))
            .unwrap();
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

        let result = server
            .tool_update_requirement(&json!({
                "id": spec_id,
                // BUG-449: use an implementer-legitimate transition (Planned is
                // advisor-gated via MCP); this test is about field persistence.
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

        // Implementer-legitimate transitions are still allowed. Ordered to also
        // satisfy forbidden_attention_transition (NeedsAttention only from
        // InProgress; leaving it only to Approved/InProgress/Rejected).
        for status in ["in-progress", "needs-attention", "in-progress", "done"] {
            server
                .tool_update_requirement(&json!({ "id": id, "status": status }))
                .unwrap_or_else(|e| panic!("{status} should be allowed via MCP: {e}"));
        }

        // Nothing slipped the gate: the spec never reached an advisor/merge status.
        let store = server.storage.load().unwrap();
        let req = store.get_requirement_by_spec_id(&id).unwrap();
        assert_eq!(req.status, RequirementStatus::Done);
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
        // pre-state) via the ungated update tool.
        srv.tool_update_requirement(&json!({ "id": id, "status": "in-progress" }))
            .unwrap();

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
            .tool_queue_add(&json!({ "id": spec, "for": "implementer", "user": QU }))
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
            .tool_queue_add(&json!({ "id": spec }))
            .expect_err("should refuse terminal without force");
        assert!(err.contains("re-queueing closed work"), "err: {err}");

        // force overrides.
        let ok = server
            .tool_queue_add(&json!({ "id": spec, "force": true }))
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
            .tool_queue_add(&json!({ "id": a, "for": "any", "user": u }))
            .unwrap();
        server
            .tool_queue_add(&json!({ "id": b, "for": "any", "user": u }))
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
            .tool_queue_add(&json!({ "id": &spec, "user": u }))
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
            .tool_queue_rework(&json!({ "id": &spec, "for": "implementer", "reason": "PR review found issues", "user": u }))
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
            .tool_queue_rework(&json!({ "id": &spec }))
            .expect_err("should refuse terminal rework without force");
        assert!(err.contains("re-opening closed work"), "err: {err}");
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
                .tool_queue_add(&json!({ "id": id, "for": "any", "user": u }))
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
}
