//! Shared MCP tool descriptors for the transport and CLI registration.
// trace:TASK-3429 | ai:codex
use serde_json::{json, Value};

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
                "description": "Path B (STORY-399): machine-readable mirror of the text envelope, present on success. Also present after a tolerant read on an error response (see `structuredError`).",
                "properties": {
                    "content": text_content_array(),
                    "cache": {
                        "type": "object",
                        "description": "Read-cache freshness for this tools/call, present when the tolerant cache path was touched.",
                        "properties": {
                            "stale": { "type": "boolean" },
                            "cache_head": { "type": ["string", "null"] },
                            "store_head": { "type": "string" },
                            "built_at": { "type": ["string", "null"] },
                            "refreshing": { "type": "string" }
                        },
                        "required": ["stale"]
                    }
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
// trace:BUG-2017 | ai:codex — published ID inputs accept opaque node namespaces.
pub fn tool_descriptors() -> Value {
    let mut descriptors = json!([
        // ---- Spec graph ----
        {
            "name": "list_requirements",
            // trace:STORY-82 | ai:claude
            "description": "List requirements from the AIDA database, optionally filtered by status, type, feature category, priority, tags, batch, parent, owner role, in-flight state, or numeric weight bounds (min_weight/max_weight). Mirrors the current `aida list` filter surface — including the archive/deferred view tiers: by DEFAULT archived (STORY-441) and deferred (STORY-584) specs are hidden, exactly like `aida list`. Use `archived` / `deferred` / `all` to surface those tiers. Returns a summarized list of matching requirements.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "description": "Filter by the current status of the requirement.",
                        // trace:TASK-1176 | ai:claude — mirrors the CLI status set.
                        "enum": ["open", "closed", "draft", "approved", "planned", "in-progress", "needs-attention", "done", "completed", "rejected", "superseded"],
                        "example": "in-progress"
                    },
                    "type": {
                        "type": "string",
                        "description": "Filter by the semantic type of the requirement.",
                        "enum": ["functional", "non-functional", "system", "user", "change-request", "bug", "epic", "story", "task", "spike", "sprint", "folder", "meta", "principle", "vision", "constraint", "decision", "term", "doc", "faq"],
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
                        "pattern": "^[A-Z]+-(?:[A-Za-z0-9][A-Za-z0-9_-]*-)?[0-9]+$",
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
                    "min_weight": {
                        "type": "number",
                        "description": "Only specs whose numeric weight/score is >= this value; specs with no weight set are excluded. Mirrors `aida list --min-weight`.",
                        "example": 0.5
                    },
                    "max_weight": {
                        "type": "number",
                        "description": "Only specs whose numeric weight/score is <= this value; specs with no weight set are excluded. Mirrors `aida list --max-weight`.",
                        "example": 10
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
                        "pattern": "^[A-Z]+-(?:[A-Za-z0-9][A-Za-z0-9_-]*-)?[0-9]+$",
                        "example": "FR-0042"
                    },
                    "include_origin_prompt": {
                        "type": "boolean",
                        "description": "Explicitly retrieve only the originating prompt artifact, labeled untrusted historical input. Default false; ordinary views never load it.",
                        "example": false
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
                "a Markdown rendering of the requirement: H1 `# <SPEC-ID> — <title>`, bold-key/value lines for Status / Priority / Type / Feature / Owner / Tags, a `## Description` body, optional `## Comments` and `## Relationships` sections, and (unless `include_git` is false) a `## Git linkage` section listing referencing commits, the feature branch, and any PR. With `include_origin_prompt: true`, returns only the labeled historical prompt artifact, or an absent-capture message."
            )
        },
        {
            "name": "add_requirement",
            // trace:STORY-82 | ai:claude
            "description": "Create and add a new requirement to the AIDA database. Generates a new canonical SPEC-ID automatically based on the type. Optionally links it under a parent, sets a feature category, and assigns an owner. Note: MCP intake always files as `draft` (approving/planning is the advisor's triage decision), regardless of the requested status.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "origin_prompt": {
                        "type": ["object", "null"],
                        "description": "Opt-in only: reviewed originating prompt, stored separately in Git history and replicas. Redact secrets before submitting; 64 KiB max; obvious credentials refuse. Omit or pass null to opt out. Git-canonical stores only.",
                        "properties": {
                            "text": {"type": "string", "description": "Reviewed, optionally redacted UTF-8 text."},
                            "redacted": {"type": ["boolean", "null"], "description": "Whether the caller redacted the text; omitted or null defaults to false."}
                        },
                        "required": ["text"],
                        "example": {"text": "Please make the dashboard blue", "redacted": false},
                        "additionalProperties": false
                    },
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
                        "description": "Required requirement type. Valid types: functional, non-functional, system, user, change-request, bug, epic, story, task, spike, sprint, folder, meta, principle, vision, constraint, decision, term, doc, faq. (change-request is the workflow type for a proposed change; principle/vision/constraint/decision/term are the ADR + knowledge-graph family.) Normalizes the assigned SPEC-ID prefix (e.g., 'task' becomes 'TASK-N').",
                        "enum": ["functional", "non-functional", "system", "user", "change-request", "bug", "epic", "story", "task", "spike", "sprint", "folder", "meta", "principle", "vision", "constraint", "decision", "term", "doc", "faq"],
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
                        "pattern": "^[A-Z]+-(?:[A-Za-z0-9][A-Za-z0-9_-]*-)?[0-9]+$",
                        "example": "EPIC-27"
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional list of tags to categorize this requirement. Follows the CLI tag conventions (colon-namespaced `aida:<subcommand>` surface tags; flat behavior/severity/batch tags like `papercut`, `batch:NAME`).",
                        "example": ["auth", "security"]
                    },
                    "weight": {
                        "type": "number",
                        "description": "Optional numeric weight/score — a continuous ranking signal beyond the high/medium/low priority enum. Mirrors `aida add --weight`.",
                        "example": 0.75
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
            "description": "Update fields of an existing requirement (title, type, status, priority, description, tags, parent, assignee, weight). Fields omitted from parameters remain unchanged. Note: advisor-authority transitions (approved/planned) and the merge-driven `completed` status are gated and cannot be set via MCP. Setting `assignee` edits the field only — it does NOT route the spec into the assignee's queue (use queue_add for that); the CLI `aida assign` does both.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The unique SPEC-ID of the requirement to update.",
                        "pattern": "^[A-Z]+-(?:[A-Za-z0-9][A-Za-z0-9_-]*-)?[0-9]+$",
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
                        "enum": ["functional", "non-functional", "system", "user", "change-request", "bug", "epic", "story", "task", "spike", "sprint", "folder", "meta", "principle", "vision", "constraint", "decision", "term", "doc", "faq"],
                        "example": "story"
                    },
                    "status": {
                        "type": "string",
                        "description": "New status to transition the requirement into. approved/planned (advisor triage) and completed (merge-driven) are refused via MCP. `superseded` records an adopted-then-replaced spec — pair it with an `add_relationship` of type `superseded-by` naming the successor.",
                        // trace:TASK-1176 | ai:claude — mirrors the CLI status set.
                        "enum": ["draft", "approved", "planned", "in-progress", "done", "completed", "rejected", "superseded", "needs-attention"],
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
                    "force": {
                        "type": "boolean",
                        "description": "Allow tags replacement to drop structural tags. The CLI alias is --replace-tags.",
                        "example": false
                    },
                    "replace_tags": {
                        "type": "boolean",
                        "description": "Explicit alias for force when intentionally replacing structural tags.",
                        "example": false
                    },
                    "parent": {
                        "type": "string",
                        "description": "Re-parent the requirement under this SPEC-ID (adds a Parent/Child edge to the new parent; existing parent edges are left in place). The parent must exist and not be terminal-status.",
                        "pattern": "^[A-Z]+-(?:[A-Za-z0-9][A-Za-z0-9_-]*-)?[0-9]+$",
                        "example": "EPIC-27"
                    },
                    "assignee": {
                        "type": "string",
                        "description": "Team member this spec is assigned to (a username/handle). An empty string clears the assignee. NOTE: this sets the field only; it does NOT add the spec to the assignee's work queue (use queue_add). The CLI `aida assign --to` does both.",
                        "example": "alice"
                    },
                    "execution_mode": {
                        "type": "string",
                        "description": "The advisor's bless-time classification of HOW this spec runs when dispatched by `aida do` (STORY-776). ADVISOR-AUTHORITY WRITE: refused unless this MCP server carries a validated advisor grant — same gate as the CLI `aida edit --mode`. An empty string clears back to ungroomed.",
                        "enum": ["drain", "drive", "guided", "operator", "decide", ""],
                        "example": "drive"
                    },
                    "origin": {
                        "type": "string",
                        "description": "Where this spec's code lives in a multi-repo shared-store workspace: a repo slug from the workspace manifest, optionally with a component (\"repo\" or \"repo/component\"). Requires a .aida-workspace manifest; the repo slug must name one of its repos. An empty string clears (back to single-repo semantics). Mirrors `aida edit --origin`.",
                        "example": "api/orchestrator"
                    },
                    "weight": {
                        "type": ["number", "null"],
                        "description": "New numeric weight/score — a continuous ranking signal beyond the priority enum. An explicit null clears it. Mirrors `aida edit --weight` (empty string clears on the CLI).",
                        "example": 0.75
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
                        "enum": ["functional", "non-functional", "system", "user", "change-request", "bug", "epic", "story", "task", "spike", "sprint", "folder", "meta", "principle", "vision", "constraint", "decision", "term", "doc", "faq"],
                        "example": "bug"
                    },
                    "status": {
                        "type": "string",
                        "description": "Restrict results to this status.",
                        // trace:TASK-1176 | ai:claude — mirrors the CLI status set.
                        "enum": ["draft", "approved", "planned", "in-progress", "needs-attention", "done", "completed", "rejected", "superseded"],
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
                        "pattern": "^[A-Z]+-(?:[A-Za-z0-9][A-Za-z0-9_-]*-)?[0-9]+$",
                        "example": "FR-0042"
                    },
                    "text": {
                        "type": "string",
                        "description": "The text content of the comment to add.",
                        "example": "Verified with the design team; OAuth2 client secrets will be fetched from Secrets Manager."
                    },
                    "relayed_from": {
                        "type": "string",
                        "description": "The seat whose claim this comment relays (mirrors `aida comment add --relayed-from`). Omit for claims you verified yourself.",
                        "example": "claude-reviewer-1"
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
            "description": "Add a typed relationship between two existing local requirements, mirroring `aida rel add` for MCP consumers. Built-in types include parent, child, duplicate, verifies, verified-by, references, blocked-by, and blocks; non-empty custom names are accepted for CLI parity. `depends-on` is accepted as an alias for blocked-by. Cross-store references such as `other-project#STORY-21` are comment/prose notation today, not resolvable graph targets.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "spec_id": {
                        "type": "string",
                        "description": "Source requirement SPEC-ID.",
                        "pattern": "^[A-Z]+-(?:[A-Za-z0-9][A-Za-z0-9_-]*-)?[0-9]+$",
                        "example": "TASK-551"
                    },
                    "relationship_type": {
                        "type": "string",
                        // trace:TASK-1176 | ai:claude — supersede lineage added.
                        "description": "Relationship type to add. Built-ins: parent, child, duplicate, verifies, verified-by, references, blocked-by, blocks, superseded-by, supersedes. Aliases: depends-on → blocked-by; related / relates-to → references; replaced-by → superseded-by; replaces → supersedes. Non-empty custom names are accepted for CLI parity.",
                        "example": "blocked-by"
                    },
                    "target_spec_id": {
                        "type": "string",
                        "description": "Target requirement SPEC-ID.",
                        "pattern": "^[A-Z]+-(?:[A-Za-z0-9][A-Za-z0-9_-]*-)?[0-9]+$",
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
                        "pattern": "^[A-Z]+-(?:[A-Za-z0-9][A-Za-z0-9_-]*-)?[0-9]+$",
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
            "description": "Send an inter-agent peer message into the mailbox local layer, mirroring `aida mailbox send`. Distinct from briefs (operator→agent work) and directives (top-down control): this is agent↔agent conversation. Address a single agent via `to`, or set `broadcast: true` to reach every agent. MCP callers pass `body` as structured JSON; CLI callers with rich text can use `--body-file` or `--stdin` to avoid shell expansion.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "body": { "type": "string", "description": "The message body.", "example": "can you re-check the auth flow in PR-42? CI flaked once." },
                    "subject": { "type": "string", "description": "Short subject shown by inbox and notice views (mirrors `aida mailbox send --subject`). Optional: messages without one derive their display subject from the first non-empty body line.", "example": "Review requested" },
                    "to": { "type": "string", "description": "Recipient agent id. Omit and set broadcast=true to reach all.", "example": "codex" },
                    "broadcast": { "type": "boolean", "description": "Send to every agent instead of a single recipient.", "example": true },
                    "thread": { "type": "string", "description": "Attach to an existing thread id (default: start a new thread).", "example": "0193a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b" },
                    "in_reply_to": { "type": "string", "description": "Id of the message this replies to.", "example": "0193a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b" },
                    "from": { "type": "string", "description": "Sender id (default: this server's agent/user identity).", "example": "claude" },
                    "urgent": { "type": "boolean", "description": "Flag as an urgent escalation so it is surfaced out-of-band (statusline nag) instead of sitting unseen. Lightweight: normal vs urgent only.", "example": true },
                    "intent": { "type": "string", "enum": ["fyi", "request", "handoff"], "description": "How the recipient should treat this message: fyi (informational, surface only), request (needs a response), or handoff (work transfer). Default: fyi. Orthogonal to urgent (loudness vs kind). Mail is interpreted input, not a command channel — an actionable intent is a recommendation, never an authenticated directive.", "example": "request" },
                    "relayed_from": { "type": "string", "description": "The seat whose claim this message relays (mirrors `aida mailbox send --relayed-from`). Set it when the body reproduces another seat's measurement, finding or argument; readers see `via <from>, originally <seat>`. Omit for claims you verified yourself.", "example": "claude-reviewer-1" },
                    "allow_second_person": { "type": "boolean", "description": "Send a body that says you/your even though the same body already went to another recipient, or this is a broadcast. Refused by default because each reader takes the claim as addressed to itself.", "default": false, "example": false }
                },
                "required": ["body"]
            },
            "outputSchema": text_envelope_output_schema(
                "a confirmation line `Message sent: <id> (thread <thread-id>)`."
            )
        },
        {
            "name": "read_inbox",
            "description": "Read an agent's mailbox inbox — messages addressed to it plus broadcasts (excluding its own sent), oldest-first — mirroring `aida mailbox inbox`. Reading does NOT mark the inbox seen unless `mark_seen` is true (the explicit ack); pass `unread` to return only messages with no read receipt. Returns pretty-printed JSON.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent": { "type": "string", "description": "Whose inbox (default: this server's agent/user identity).", "example": "claude" },
                    "unread": { "type": "boolean", "description": "Return only unread messages (no read receipt, and newer than any legacy read-watermark). Default false (whole inbox).", "default": false, "example": true },
                    "mark_seen": { "type": "boolean", "description": "Record a read receipt for each returned unread message — the explicit ack that clears the unread/notice surface. Default false, so a plain read is a non-marking peek (STORY-585 acceptance #4).", "default": false, "example": true }
                }
            },
            "outputSchema": text_envelope_output_schema(
                "pretty-printed JSON `{agent, count, unread, messages:[{id,thread_id,from,to,timestamp,in_reply_to,body,subject,urgent,intent,relayed_from}]}` where intent is one of fyi|request|handoff, and subject and relayed_from are omitted when the message has none."
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
            "description": "Read AIDA's orphan-branch event ledger, mirroring `aida history events` for MCP consumers. Returns pretty-printed JSON with structured event records by default; explicit template returns human text in the existing MCP text envelope; fields projects ordered JSON event keys. template and fields are mutually exclusive. Mirrors the CLI's filter surface (type/author/since/until/limit/shipped/to/from/opened/status-changes/comments). The MCP ledger never hides archived/deferred rows, so it is already equivalent to `aida history --all` — no `all` toggle is needed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "template": {
                        "type": "string",
                        "example": "{id} {event}",
                        "description": "Opt-in human event text: inline if it contains {, otherwise a scoped template name (user > project > builtin). Fields: commit,date[:strftime],author,id,type,priority,title,kind,event,from,to,comment; escape {{/}}. Title: Added/Deleted/TitleChange new value; priority: Added/PriorityChange new value; absent otherwise. Comment is a CommentsAdded count/summary, never a body. Missing fields are empty. Cannot combine with fields, oneline or explicit events:true. No save/remove API."
                    },
                    "fields": {
                        "type": "string",
                        "example": "id,date,event",
                        "description": "Ordered CSV event fields: commit,date,author,id,type,priority,title,kind,event,from,to,comment. Returns the existing JSON envelope with only these keys in each events row, in caller order; unavailable event-local values are null. Selects the full event feed. Cannot combine with template or oneline."
                    },
                    "spec_id": {
                        "type": "string",
                        "description": "Optional SPEC-ID filter for a single requirement's event history (mirrors `aida history events --id`, i.e. `aida history <SPEC-ID> --full`: the full event trail, not the status-progression view). Accepts the raw UUID `show_requirement` returns; it is resolved to the canonical SPEC-ID before filtering.",
                        "example": "TASK-538"
                    },
                    "events": {
                        "type": "boolean",
                        "description": "Per-event chronological mode (mirrors `aida history events`). The MCP tool defaults to true (the structured event ledger).",
                        "default": true,
                        "example": true
                    },
                    "type": {
                        "type": "string",
                        "description": "Only events for requirements of this type (mirrors `aida history --type`).",
                        "enum": ["functional", "non-functional", "system", "user", "change-request", "bug", "epic", "story", "task", "spike", "sprint", "folder", "meta", "principle", "vision", "constraint", "decision", "term", "doc", "faq"],
                        "example": "bug"
                    },
                    "author": {
                        "type": "string",
                        "description": "Only events authored by this user (mirrors `aida history --author`; matches the YAML `last_modified_by` HLC field if present, else the git committer email).",
                        "example": "joe@example.com"
                    },
                    "since": {
                        "type": "string",
                        "description": "Optional lower time bound (mirrors `aida history --since`). Accepts a relative duration meaning 'that far before now' (`30m`, `5h`, `7d`, `2w`, or `<N> minutes/hours/days/weeks ago`), an ISO date (`2026-05-01`, local midnight), a zone-less ISO datetime (`2026-05-01T10:00`, local time), or a full RFC3339 timestamp (zone honored exactly). Other git date phrases are no longer passed through and are rejected. A `since` that resolves later than `until` is rejected.",
                        "example": "24 hours ago"
                    },
                    "until": {
                        "type": "string",
                        "description": "Optional upper time bound (mirrors `aida history --until`). Same forms as `since`: a relative duration, an ISO date (local midnight), a zone-less ISO datetime (local time), or RFC3339.",
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
                        "description": "Only transitions into Completed (merged to the default branch), from any prior status, newest first — the 'did my ship register?' view (mirrors `aida history --shipped`). Implies events mode; composes with since/until/limit.",
                        "default": false,
                        "example": true
                    },
                    "to": {
                        "type": "string",
                        "description": "Only status transitions into this status (mirrors `aida history --to`). Accepts the spellings `aida edit --status` does (`approved`, `in-progress`, `needs-attention`, any case) plus `accepted` for approved; an unknown status is an error listing the valid set. With `from`, both ends must match; `from` and `to` naming the same status is refused. With `status_changes` it narrows the transitions; with `comments` or `opened` the result is the union. Cannot be combined with `shipped` (which is `to: completed`).",
                        "example": "approved"
                    },
                    "from": {
                        "type": "string",
                        "description": "Only status transitions out of this status (mirrors `aida history --from`). Alone it matches any transition leaving that status; same spellings and combination rules as `to`.",
                        "example": "in-progress"
                    },
                    "opened": {
                        "type": "boolean",
                        "description": "Only spec-creation events: the specs filed in the window, whatever status they were filed at (mirrors `aida history --opened` and its alias `--created`). Combined with `to`/`from`, `status_changes` or `comments`, the result is the union of both kinds of event. Cannot be combined with `shipped`.",
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
                "With explicit template: human text in this same text envelope. With fields: pretty-printed JSON with caller-ordered projected events rows (unavailable event-local values are null) and the same envelope. Neither parameter: unchanged pretty-printed JSON `{ count, events, window_exhausted, source, index_tip }`, where each event has `sha`, `timestamp`, `author`, `spec_id`, `req_type`, `kind`, `summary`, and structured `detail` fields, plus the flat per-event row fields `id` (same as `spec_id`), `ts` (same as `timestamp`), and `from`/`to` (old and new value of a status, priority, title, owner, feature or type transition; null for other kinds). `window_exhausted` is true when the commit walk hit its cap before `count` reached the requested `limit` — there may be older matching events beyond it; a caller who needs them should narrow with `since`/`until`, since the MCP tool has no `max_commits` override of its own. `source` is `history-cache` when the rebuildable history index answered or `git-walk` when the store's git log was read directly (the index was off, still filling, or could not prove it held the whole answer); both return the same events. `index_tip` is the full store commit SHA an index answer reflects, or null for `git-walk`."
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
                        "pattern": "^[A-Z]+-(?:[A-Za-z0-9][A-Za-z0-9_-]*-)?[0-9]+$",
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
                        "pattern": "^[A-Z]+-(?:[A-Za-z0-9][A-Za-z0-9_-]*-)?[0-9]+$",
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
                        "pattern": "^[A-Z]+-(?:[A-Za-z0-9][A-Za-z0-9_-]*-)?[0-9]+$",
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
                        "pattern": "^[A-Z]+-(?:[A-Za-z0-9][A-Za-z0-9_-]*-)?[0-9]+$",
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
                        "pattern": "^[A-Z]+-(?:[A-Za-z0-9][A-Za-z0-9_-]*-)?[0-9]+$",
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
                        "pattern": "^[A-Z]+-(?:[A-Za-z0-9][A-Za-z0-9_-]*-)?[0-9]+$",
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
                        "pattern": "^[A-Z]+-(?:[A-Za-z0-9][A-Za-z0-9_-]*-)?[0-9]+$",
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
            "description": "Mark a requirement Done and remove it from the queue in one step. Mirrors `aida queue done`. Flips status to Done (work finished on a branch) — the merge auto-bump later advances Done → Completed. Refuses a spec that has not passed the approval gate (Draft or NeedsAttention, unless the server holds approval authority) and a closed spec (Rejected, Completed, Superseded). Stamps implementation_info. Optionally capture user-facing interface changes (the deterministic operator-digest source) via interface_cli/mcp/tui/other, or no_interface_change for a no-impact spec. Optionally record the verification steps the builder ran via test_plan (surfaced in the PR body).",
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
            "description": "Re-open a spec: flip its status, route it to a role's queue, and optionally capture a reason. Mirrors `aida queue rework` (metadata-only — it does not launch a session). Smart status transitions (Planned→InProgress, Done→InProgress, Rejected→Approved, …) unless overridden with `status`. By default, preserves the existing queue route or falls back to implementer, and re-queues at the head. Terminal-status and already-InProgress re-opens require `force`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Requirement id (UUID or SPEC-ID) to rework.", "example": "STORY-42" },
                    "user": { "type": "string", "description": "Override the queue user id.", "example": "alice" },
                    "for": { "type": "string", "description": "Override the routing role (default: existing queue route, else implementer).", "example": "implementer" },
                    "tail": { "type": "boolean", "description": "Re-queue at the tail instead of the default head position.", "example": true },
                    // trace:TASK-1176 | ai:claude — mirrors the CLI status set.
                    "status": { "type": "string", "description": "Override the smart target status.", "enum": ["draft", "approved", "planned", "in-progress", "needs-attention", "done", "completed", "rejected", "superseded"], "example": "in-progress" },
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
            "description": "Assemble a rich planner prompt for a spec from its description, acceptance criteria, graph context, reusable-helper section, and the 11-section plan structure. Read-only mirror of `aida ultraplan <spec> --stdout` — it returns the assembled prompt text; copying to the clipboard or opening a deep link stays CLI-only.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "spec": { "type": "string", "description": "Requirement id (UUID or SPEC-ID) to assemble the prompt for.", "example": "STORY-536" },
                    "no_comments": { "type": "boolean", "description": "Omit the spec's comments from the prompt (the comments carry long-form enrichment by default).", "example": false }
                },
                "required": ["spec"]
            },
            "outputSchema": text_envelope_output_schema(
                "the assembled planning prompt text, followed by any assembly `Warning:` lines (e.g. a spec with no description). On an unknown spec, or when ultraplan is disabled for the project, the envelope sets `isError: true`."
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
            "description": "Query the local command-usage telemetry log (`~/.aida/usage.jsonl`). Mirrors `aida usage` (top commands by count), `aida usage errors` (high error-rate commands), and `aida usage unused <window>` (deprecation candidates). STORY-1028 moved CLI mode selectors to subcommands; MCP keeps the existing booleans/strings for schema stability. Read-only aggregation over the local log; the orchestrator-telemetry views (`usage drains`, `usage health`) stay CLI-only.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    // trace:TASK-1509 | ai:claude
                    "since": { "type": "string", "description": "Window start for the aggregation (default `30d`): a relative duration (`30d`, `12h`, `2w`, `24 hours ago`), an ISO date (`2026-05-01`, local midnight), a zone-less ISO datetime (local time), or RFC3339.", "example": "30d" },
                    "unused": { "type": "string", "description": "Instead of top commands, list commands NOT used since this point (deprecation candidates). Same forms as `since`.", "example": "30d" },
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
