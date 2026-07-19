//! `aida schema` — read-only introspection of the storable substrate.
//!
//! Surfaces two things an agent (or a curious operator) otherwise has to
//! reverse-engineer from `models.rs`:
//!   1. the **catalog** of storable object kinds, and
//!   2. the **Requirement** field table + the four controlled-vocabulary
//!      enums in their exact on-the-wire token form, so the output doubles
//!      as a paste-ready cheat-sheet for `--status` / `--type` / `--priority`
//!      / relationship-type arguments.
//!
//! DERIVATION (substrate-as-bouncer): the Requirement field set and the enum
//! variant sets are **derived from the `ts-rs-forge` type reflection** that
//! already backs `aida-generate-types` — never hand-maintained here. We parse
//! the `TS::decl()` output of `Requirement` (the field table) and of the four
//! enums (the variant lists). The wire token for each variant is the
//! kebab-case of its PascalCase reflected name — exactly the canonical form
//! every CLI parser (`parse_status` / `parse_type` / `parse_priority`,
//! `RelationshipType::from_str`) accepts. A drift-guard test
//! (`schema_enums_match_reflection`) pins this so the schema can't silently
//! rot away from `models.rs`.
//!
//! Read-only: this module mutates nothing.
//
// trace:STORY-538 | ai:claude

use aida_core::models::{
    Comment, HistoryEntry, QueueEntry, Relationship, RelationshipType, Requirement,
    RequirementPriority, RequirementStatus, RequirementType,
};
use serde_json::{json, Value};
use ts_rs_forge::TS;

// trace:TASK-714 — the catalog kinds whose canonical struct lives CLI-side.
use crate::findings::FindingRow;
use crate::punt::PuntRecord;
use crate::worker::Directive;
use crate::{BriefListEntry, SessionLease};

/// One row in the storable-object catalog.
struct CatalogEntry {
    /// Object kind name as a human would say it.
    name: &'static str,
    /// One-line description of what it stores / where it lives.
    description: &'static str,
    /// Reflection hook: returns the `TS::decl()` of the canonical Rust struct
    /// that backs this object, so `aida schema <object>` can render a
    /// reflection-derived field table the same way the Requirement view does.
    /// `None` for objects with no single serde-reflectable struct (today: none
    /// — every catalog kind now has a reflected backing type). The closure is
    /// what the drift-guard tests pin: a field added/removed/renamed on the
    /// backing struct changes its `decl()` and ripples through here.
    /// trace:TASK-714 | ai:claude
    decl: fn() -> String,
    /// Optional note rendered under the field table — used where the reflected
    /// struct is a *projection* (a derived in-memory shape) rather than the
    /// exact on-disk record, so the reader isn't misled. trace:TASK-714
    note: Option<&'static str>,
    /// The object-level **lifecycle** block surfaced by `aida schema --explain`:
    /// who writes the record, when, why it exists, how/where it is read back,
    /// and when it is deleted/archived. Hand-curated prose grounded in
    /// `docs/lifecycle.md` + the discipline `lifecycle-vocabulary.md`; not
    /// reflected. Rendered only under `--explain`, so the terse view is
    /// unchanged. trace:STORY-630 | ai:claude
    lifecycle: &'static str,
}

/// The curated catalog of storable object kinds. The one-liner descriptions are
/// hand-written; the per-object *field detail* (`aida schema <object>`) is
/// reflection-derived from the `decl` closure on each entry — never
/// hand-maintained. STORY-538 shipped Requirement detail; TASK-714 extended the
/// reflection registry to every remaining kind. trace:STORY-538 trace:TASK-714 | ai:claude
const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        name: "Requirement",
        description: "The core spec node (epic/story/task/bug/...) — title, status, type, priority, relationships, history.",
        decl: Requirement::decl,
        note: None,
        lifecycle: "Written as one YAML file under `.aida-store/objects/TYPE/000/SPEC-ID.yaml` on the \
             orphan `aida-store` branch — the writer of record. Created by `aida add` (status \
             Draft) and edited by `aida edit` / MCP `update_requirement`; every change also appends \
             a HistoryEntry row in-file. Status climbs Draft → Approved (advisor sign-off) → \
             Planned → In Progress (`aida queue work`) → Done (`aida queue done`, on a branch) → \
             Completed (auto-bumped by `aida pull` once a referencing commit lands on main). Read \
             back via the SQLite cache (`.aida/cache.db`, a rebuildable projection) for \
             `aida list`/`search`, and directly from YAML for the full record. Never deleted — \
             retired by the orthogonal view flags `archived` / `deferred`; the YAML, audit trail, \
             and graph survive.",
    },
    CatalogEntry {
        name: "Finding",
        description: "A shelved phase-failure or advisor observation surfaced for triage (aida findings).",
        decl: FindingRow::decl,
        note: Some(
            "A Finding has no standalone record — it is a draft Requirement carrying a \
             `from-review:` / `from-implementer:` / `from-advisor:` tag. The fields below are \
             the triage-row projection `aida findings list` derives from that tagged spec.",
        ),
        lifecycle: "No standalone file. The orchestrator (or an advisor/implementer/reviewer) files \
             a Finding when it shelves a phase-failure or records an observation: `aida findings \
             add` writes a Draft Requirement tagged `from-review:` / `from-implementer:` / \
             `from-advisor:`, so on disk it is just another spec YAML. Read back as the \
             triage-row projection by `aida findings list`; triaged with `aida findings triage` \
             (promote to real work, or dismiss). Cleared by archiving/rejecting the underlying \
             spec — same retirement path as any Requirement.",
    },
    CatalogEntry {
        name: "Brief",
        description: "A local pickup brief routing work to an agent without scrollback (.aida/agent-briefs/).",
        decl: BriefListEntry::decl,
        note: Some(
            "A brief is a markdown file with YAML frontmatter; the fields below are the listing \
             projection `aida brief list` parses from that frontmatter (spec_id / agent / \
             generated_at / depends_on / status), plus its on-disk path.",
        ),
        lifecycle: "A markdown file with YAML frontmatter under `.aida/agent-briefs/<agent>/` — \
             local per-clone runtime state, not committed. Written by `aida brief <agent> <SPEC>` \
             (or the orchestrator) to route work to an agent without scrollback. Read back by \
             `aida brief list --for-agent` / MCP `list_briefs` + `read_brief` at pickup; the \
             role-context snapshot leads with it. Acknowledged with `aida brief ack` / \
             `ack_brief`, which marks it consumed. Ephemeral — superseded or cleaned up once the \
             work is picked up.",
    },
    CatalogEntry {
        name: "Punt",
        description: "A design-fork an autonomous agent could not safely resolve; parks the spec NeedsAttention.",
        decl: PuntRecord::decl,
        note: Some("One append-only line in `.aida/punts.jsonl`."),
        lifecycle: "One append-only JSONL line in `.aida/punts.jsonl` — local runtime state. \
             Written when an autonomous (`--no-human`) implementer or reviewer hits a decision it \
             cannot safely make and **punts** rather than guess (`aida punt` / `/aida-punt`), which \
             also flips the spec to NeedsAttention and stamps its `attention_reason`. Read back by \
             `aida findings list` / MCP `list_punts` + `read_punt` for human/advisor triage. \
             Resolved with `aida punt resolve` (or by editing the spec out of NeedsAttention); the \
             ledger is the durable history and is never rewritten — it keeps the decision trail \
             even after the spec resumes.",
    },
    CatalogEntry {
        name: "Directive",
        description: "A standing instruction posted to an agent/role via the inter-agent mailbox.",
        decl: Directive::decl,
        note: Some("One parsed line of `.aida/worker.cmd` (verb + args)."),
        lifecycle: "One parsed line (verb + args) of `.aida/worker.cmd` — local runtime state on the \
             inter-agent mailbox substrate. Posted by an operator/advisor via `aida directive post` \
             / MCP `post_directive` to give a running worker a standing instruction. Read back by \
             the worker (and `aida directive list` / MCP `list_directives`) on its poll loop. \
             Acknowledged with `ack_directive` once acted on; consumed/cleared from the queue \
             thereafter.",
    },
    CatalogEntry {
        name: "Comment",
        description: "A threaded note on a Requirement (carries reactions; doc-seed carrier).",
        decl: Comment::decl,
        note: None,
        lifecycle: "Not a standalone file — an element of the parent Requirement's `comments:` array, \
             so it lives inside that spec's YAML on the orphan `aida-store` branch and is written \
             with it. Added by a human or agent via `aida comment add` / MCP `add_comment`; \
             threaded (a comment may reply to another) and the carrier for doc-seeds captured \
             during design discussion. Read back wherever the spec is shown (`aida show`, MCP \
             `show_requirement`). Persists for the life of the spec; never edited in place.",
    },
    CatalogEntry {
        name: "Lease",
        description: "An active claim on a spec/worktree by a session — prevents double-driving.",
        decl: SessionLease::decl,
        note: Some("One session lease file under `.aida/sessions/`."),
        lifecycle: "One lease file under `.aida/sessions/` — local per-clone runtime state. Written \
             when a session starts working a spec/worktree (`aida session start` / `aida queue \
             work` / MCP `session_start`) so a second session can't double-drive the same spec. \
             Read back by `aida session leases` / MCP `list_active_leases` + `session_leases` \
             before pickup or before rejecting/pivoting a spec. Released when the session ends \
             (`aida session end` / `release_task`); a stale lease (dead PID) is reclaimable.",
    },
    CatalogEntry {
        name: "QueueItem",
        description: "A position in a role's work queue (keyed off the shell user identity).",
        decl: QueueEntry::decl,
        note: None,
        lifecycle: "A position in a per-user work queue (local runtime state), keyed off the shell's \
             user identity — `current_user_id()` resolves `--user` → `AIDA_USER` → `USER`, NOT the \
             node/role identity. Added by `aida queue add` / MCP `queue_add` (queue membership = \
             the advisor's sign-off that the work is worth doing). Read back by `aida queue list` / \
             `queue_next` and the statusline depth. Drained by `aida queue work`; removed on `aida \
             queue done` / `queue_remove`, reordered by `queue_move`. A freshly-Done item lingers \
             in the 'awaiting merge' section until the auto-bump.",
    },
    CatalogEntry {
        name: "HistoryEntry",
        description: "An immutable change row inside a Requirement's YAML (the spec-state time series).",
        decl: HistoryEntry::decl,
        note: None,
        lifecycle: "Not a standalone file — an element of the parent Requirement's `history:` array, \
             so it lives inside that spec's YAML on the orphan `aida-store` branch. Appended \
             automatically on every field change (status flip, priority/tag/owner edit, …) by the \
             write path — never written by hand. Immutable once written: each row carries an id \
             (UUID), author, timestamp, and a `changes:` list of {field, old, new} triples. This \
             is the source-of-truth spec-state time series — read back by `aida history --events` \
             / `--id <id>`; burn-down/status-flow analyses walk these arrays directly. Never \
             deleted (it is the audit trail).",
    },
    CatalogEntry {
        name: "Relationship",
        description: "A typed edge between two Requirements (parent/child/blocked-by/blocks/references/...).",
        decl: Relationship::decl,
        note: None,
        lifecycle: "Not a standalone file — an element of the source Requirement's `relationships:` \
             array, so it lives inside that spec's YAML on the orphan `aida-store` branch. Added by \
             `aida add-relationship` / MCP `add_relationship` (or implied at creation via \
             `--parent`). A typed directed edge (parent/child/blocked-by/blocks/verifies/…) to \
             another spec's UUID. Read back by `aida graph` / `aida show` / MCP `query_graph` for \
             transitive blocked-by/blocks chains, epic rollups, and impact closure; the `blocked-by` \
             edge gates autonomous pickability. Persists with the spec; removed when the edge is \
             deleted.",
    },
];

/// True if `name` (case-insensitive) is a kind in the storable-object catalog —
/// lets the dispatcher tell "known object, detail not built yet" from a typo.
/// trace:STORY-538 | ai:claude
pub fn is_catalog_object(name: &str) -> bool {
    CATALOG.iter().any(|e| e.name.eq_ignore_ascii_case(name))
}

/// The storable-object catalog as the JSON value the `aida schema --json`
/// CLI surface and the `aida://schema` MCP resource / `schema` MCP tool all
/// emit. Single source so the MCP surface can't drift from the CLI.
/// trace:TASK-715 | ai:claude
pub fn catalog_json() -> Value {
    catalog_json_inner(false)
}

/// The catalog JSON with the optional explanatory layer (lifecycle per object)
/// — the public surface the MCP `schema` tool calls. trace:STORY-630
pub fn catalog_json_explain(explain: bool) -> Value {
    catalog_json_inner(explain)
}

/// The full-dump JSON with the optional explanatory layer. trace:STORY-630
pub fn full_dump_json_explain(explain: bool) -> Value {
    full_dump_json_inner(explain)
}

/// The per-object detail JSON with the optional explanatory layer.
/// trace:STORY-630
pub fn object_json_explain(name: &str, explain: bool) -> Option<Value> {
    object_json_inner(name, explain)
}

/// The catalog JSON, optionally carrying the `lifecycle` block per object
/// (`--explain`). trace:STORY-630 | ai:claude
fn catalog_json_inner(explain: bool) -> Value {
    let objects: Vec<Value> = CATALOG
        .iter()
        .map(|e| {
            if explain {
                json!({ "name": e.name, "description": e.description, "lifecycle": e.lifecycle })
            } else {
                json!({ "name": e.name, "description": e.description })
            }
        })
        .collect();
    json!({ "objects": objects })
}

/// The **full dump** as the JSON value `aida schema --all --json` and the
/// field-included no-arg `aida schema --json` emit: the catalog with each
/// object's reflection-derived `fields` array (and `note`, and — for
/// Requirement — its controlled-vocabulary `enums`) inlined, in catalog order.
/// A true one-fetch full dump for the CLI manual generator, aida-tutor, and MCP
/// consumers. Reuses the same per-object projection [`object_json`] builds — no
/// reimplementation. With `explain`, adds the explanatory layer (per-field
/// example/provenance/description + per-object lifecycle). trace:TASK-799
/// trace:STORY-630 | ai:claude
fn full_dump_json_inner(explain: bool) -> Value {
    let objects: Vec<Value> = CATALOG
        .iter()
        .map(|e| object_json_inner(e.name, explain).expect("catalog kind has object_json detail"))
        .collect();
    json!({ "objects": objects })
}

/// The per-object detail as the JSON value the `aida schema <object> --json`
/// CLI surface and the `aida://schema/{object}` MCP resource / `schema` MCP
/// tool emit. Every catalog kind renders its reflection-derived field table
/// (TASK-714's registry); `Requirement` additionally carries the four
/// controlled-vocabulary enums. An unknown name returns `None` so the caller
/// can distinguish a typo from a catalog kind. Single source so the MCP
/// surface can't drift from the CLI. trace:TASK-715 | ai:claude
pub fn object_json(name: &str) -> Option<Value> {
    object_json_inner(name, false)
}

/// As [`object_json`] but, when `explain` is set, adds the per-field
/// `example`/`provenance`/`description` (where a curated doc entry exists) and
/// the object's `lifecycle` block. trace:STORY-630 | ai:claude
fn object_json_inner(name: &str, explain: bool) -> Option<Value> {
    if name.eq_ignore_ascii_case("Requirement") {
        return Some(requirement_json_inner(explain));
    }
    let entry = catalog_entry(name)?;
    let fields = parse_struct_fields(&(entry.decl)());
    let field_vals: Vec<Value> = fields
        .iter()
        .map(|f| field_json(f, explain, name))
        .collect();
    let mut obj = serde_json::Map::new();
    obj.insert("object".to_string(), json!(entry.name));
    obj.insert("fields".to_string(), json!(field_vals));
    if let Some(note) = entry.note {
        obj.insert("note".to_string(), json!(note));
    }
    if explain {
        obj.insert("lifecycle".to_string(), json!(entry.lifecycle));
    }
    Some(Value::Object(obj))
}

/// JSON for one reflected field. With `explain`, folds in the curated
/// `example`/`provenance`/`description` when a doc entry exists for that field
/// (Requirement is fully documented today; other kinds carry the base shape
/// until Slice 2). trace:STORY-630 | ai:claude
fn field_json(f: &FieldSchema, explain: bool, object: &str) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("name".to_string(), json!(f.name));
    m.insert("type".to_string(), json!(f.ts_type));
    m.insert("optional".to_string(), json!(f.optional));
    if explain && object.eq_ignore_ascii_case("Requirement") {
        if let Some(doc) = requirement_field_doc(&f.name) {
            m.insert("example".to_string(), json!(doc.example));
            m.insert("provenance".to_string(), json!(doc.provenance.token()));
            m.insert("description".to_string(), json!(doc.description));
        }
    }
    Value::Object(m)
}

/// The reflection-derived `Requirement` field table + the four
/// controlled-vocabulary enums as a JSON value. Shared by `print_requirement`
/// (CLI `--json`) and the MCP schema surface. trace:TASK-715 | ai:claude
/// With `explain`, folds the curated per-field semantics into each field and
/// adds the object lifecycle block. trace:STORY-630 | ai:claude
fn requirement_json_inner(explain: bool) -> Value {
    let fields = requirement_fields();
    let enums = requirement_enums();
    let field_vals: Vec<Value> = fields
        .iter()
        .map(|f| field_json(f, explain, "Requirement"))
        .collect();
    let enum_vals: Value = {
        let mut map = serde_json::Map::new();
        for e in &enums {
            map.insert(e.field.to_string(), json!(e.tokens));
        }
        Value::Object(map)
    };
    let mut out = serde_json::Map::new();
    out.insert("object".to_string(), json!("Requirement"));
    out.insert("fields".to_string(), json!(field_vals));
    out.insert("enums".to_string(), enum_vals);
    if explain {
        let entry = catalog_entry("Requirement").expect("Requirement is a catalog kind");
        out.insert("lifecycle".to_string(), json!(entry.lifecycle));
    }
    Value::Object(out)
}

/// A controlled-vocabulary enum the CLI/MCP accept as argument tokens.
struct EnumSchema {
    /// Field name on the Requirement this enum controls.
    field: &'static str,
    /// On-the-wire tokens, in declaration order.
    tokens: Vec<String>,
}

/// A reflected Requirement field.
struct FieldSchema {
    name: String,
    ts_type: String,
    /// `true` when the field is optional / nullable in the wire shape.
    optional: bool,
}

// ============================================================================
// STORY-630: the explanatory layer.
//
// The field LIST stays reflection-derived (substrate-as-bouncer). The per-field
// prose / example / provenance is necessarily hand-curated. So: reflection owns
// the field set; the curated `FieldDoc` map owns the semantics; and a drift-guard
// test (`explain_docs_match_reflection`) asserts the doc-map covers EXACTLY the
// reflected fields of each documented kind — fail on any undocumented field OR
// orphan entry. Mirrors `schema_enums_match_reflection`. trace:STORY-630
// ============================================================================

/// The closed set of "set-by" provenance tokens a documented field carries.
/// Keeping this an enum (not free text) is what makes provenance a controlled
/// vocabulary the operator can scan. trace:STORY-630 | ai:claude
#[derive(Clone, Copy, PartialEq, Eq)]
enum Provenance {
    /// Written by a person or an agent acting on a person's behalf (`aida add`,
    /// `aida edit`, MCP write tools).
    User,
    /// A transition gated to the advisor seat (e.g. Approved/Planned status).
    AdvisorGated,
    /// Set by the merge → `aida pull` auto-bump, not by hand (e.g. Completed).
    MergeDriven,
    /// Written by the `--auto-complete` orchestrator / autonomous drain
    /// machinery (punts, failure reasons, attention reasons).
    Orchestrator,
    /// Maintained by the engine itself — IDs, timestamps, the history array,
    /// version counters. The user never sets these directly.
    ReflectionDerived,
    /// Generated by an AI synthesis pass and cached on the spec — not
    /// hand-authored ground truth (e.g. `intent`, the AI WHY-comprehension
    /// written by `aida intent`). trace:STORY-631 | ai:claude
    Ai,
}

impl Provenance {
    /// The on-the-wire token (the closed set the spec settled on).
    fn token(self) -> &'static str {
        match self {
            Provenance::User => "user",
            Provenance::AdvisorGated => "advisor-gated",
            Provenance::MergeDriven => "merge-driven",
            Provenance::Orchestrator => "orchestrator",
            Provenance::ReflectionDerived => "reflection-derived",
            Provenance::Ai => "ai",
        }
    }
}

/// The hand-curated semantics for one reflected field: a concrete example
/// value, a provenance token, and a 1-2 line "set by X when Y, because Z"
/// gloss. Keyed by the reflected field name so the drift-guard can pair them.
/// trace:STORY-630 | ai:claude
struct FieldDoc {
    /// Reflected field name this entry documents (must match a `FieldSchema.name`).
    name: &'static str,
    /// A concrete, realistic example value — makes the field paste-ready in a
    /// way the bare reflected type cannot.
    example: &'static str,
    /// Who sets the field (the closed provenance set).
    provenance: Provenance,
    /// 1-2 line gloss of when/why the field is used.
    description: &'static str,
}

/// The per-field semantics for `Requirement` — Slice 1 of STORY-630 documents
/// the core spec node completely. The drift-guard test asserts this list covers
/// exactly the reflected `Requirement` field set (no gaps, no orphans), so a
/// field added to `models.rs` without an entry here fails the build.
/// trace:STORY-630 | ai:claude
const REQUIREMENT_FIELD_DOCS: &[FieldDoc] = &[
    FieldDoc {
        name: "id",
        example: "019ecead-1278-7752-b7e1-0c384e97cb48",
        provenance: Provenance::ReflectionDerived,
        description: "Stable UUID, assigned once at creation. The permanent identity every \
             spec_id/agreed_id resolves back to; never changes.",
    },
    FieldDoc {
        name: "spec_id",
        example: "TASK-837",
        provenance: Provenance::ReflectionDerived,
        description: "Human-friendly node-scoped ID minted at `aida add` from the dispenser. The \
             breadcrumb you put in trace comments and commit trailers.",
    },
    FieldDoc {
        name: "agreed_id",
        example: "FR-423",
        provenance: Provenance::MergeDriven,
        description: "Short agreed ID assigned at the merge gate (`aida db merge-gate`) in \
             distributed mode. Resolves to the same UUID as spec_id; unused in centralized mode.",
    },
    FieldDoc {
        name: "prefix_override",
        example: "SEC",
        provenance: Provenance::User,
        description:
            "Optional uppercase prefix forcing the spec_id family (e.g. SEC for security) \
             instead of deriving it from feature/type. Set at creation.",
    },
    FieldDoc {
        name: "title",
        example: "aida schema --explain: per-field semantics",
        provenance: Provenance::User,
        description: "Short human title. Set at `aida add`, editable via `aida edit --title`.",
    },
    FieldDoc {
        name: "description",
        example: "Add an opt-in explanatory mode to `aida schema` …",
        provenance: Provenance::User,
        description: "The full spec body (acceptance criteria, design notes). Set at creation, \
             edited via `aida edit`. The primary input to grooming and planning.",
    },
    FieldDoc {
        name: "status",
        example: "in-progress",
        provenance: Provenance::AdvisorGated,
        description:
            "Lifecycle state (draft→approved→planned→in-progress→done→completed). Draft is \
             the default; Approved/Planned are advisor-gated; Done is set by `aida queue done`; \
             Completed is merge-driven (auto-bumped by `aida pull`).",
    },
    FieldDoc {
        name: "priority",
        example: "high",
        provenance: Provenance::User,
        description: "high/medium/low. Set at creation (default medium) or via `aida edit \
             --priority`; feeds queue ordering and triage.",
    },
    FieldDoc {
        name: "owner",
        example: "joe",
        provenance: Provenance::User,
        description: "Person/agent responsible for the spec. Set via `aida add --owner` / `aida \
             edit`; empty when unassigned.",
    },
    // trace:STORY-639 | ai:claude
    FieldDoc {
        name: "assignee",
        example: "alice",
        provenance: Provenance::User,
        description: "Team member this spec is assigned to (work-division metadata, distinct from \
             `owner`/creator). Set via `aida assign --to <user>` (which also routes the spec into \
             that user's queue) and cleared by `aida unassign`. Surfaced by `aida list --mine` / \
             `--assigned <user>`; None/omitted when unassigned.",
    },
    FieldDoc {
        name: "feature",
        example: "schema-surface",
        provenance: Provenance::User,
        description:
            "The feature category the spec belongs to (NOT a type). Drives spec_id prefix \
             derivation and grouping; set at creation.",
    },
    FieldDoc {
        name: "created_at",
        example: "2026-06-15T09:30:00Z",
        provenance: Provenance::ReflectionDerived,
        description: "UTC creation timestamp, stamped once by the engine at `aida add`. Immutable.",
    },
    FieldDoc {
        name: "created_by",
        example: "joe",
        provenance: Provenance::ReflectionDerived,
        description: "Identity that created the spec, captured by the engine at creation.",
    },
    FieldDoc {
        name: "modified_at",
        example: "2026-06-15T21:48:00Z",
        provenance: Provenance::ReflectionDerived,
        description: "UTC last-modified timestamp, re-stamped by the engine on every write. For a \
             true change time series read `history:` rather than this single value.",
    },
    FieldDoc {
        name: "req_type",
        example: "story",
        provenance: Provenance::User,
        description:
            "Requirement type (epic/story/task/bug/spike/… 19 variants). Set at `aida add \
             --type`; governs the spec_id family and lifecycle expectations.",
    },
    FieldDoc {
        name: "meta_subtype",
        example: "prompt",
        provenance: Provenance::User,
        description:
            "Subtype for Meta requirements (prompt/skill/command/…). Only meaningful when \
             req_type is meta; None otherwise.",
    },
    FieldDoc {
        name: "dependencies",
        example: "[\"019ec…\", \"019ed…\"]",
        provenance: Provenance::User,
        description: "UUIDs of specs this one depends on (a coarser list than typed \
             relationships). Set via the add/edit paths.",
    },
    FieldDoc {
        name: "tags",
        example: "[\"aida:schema\", \"papercut\"]",
        provenance: Provenance::User,
        description: "Free-form labels (colon-namespaced for subcommand surfaces, flat for \
             behavior/severity). Set via `--tags`; drive filtering, batching, and parking.",
    },
    FieldDoc {
        name: "weight",
        example: "3.0",
        provenance: Provenance::User,
        description:
            "Optional effort estimate (story points). Set via `aida edit`; only shown when \
             present.",
    },
    FieldDoc {
        name: "relationships",
        example: "[{ kind: blocked-by, target: 019ec… }]",
        provenance: Provenance::User,
        description:
            "Typed directed edges to other specs (parent/child/blocked-by/blocks/…). Added \
             via `aida add-relationship`; the blocked-by edge gates autonomous pickability.",
    },
    FieldDoc {
        name: "comments",
        example: "[{ author: joe, content: \"Refinement: …\" }]",
        provenance: Provenance::User,
        description: "Threaded notes on the spec (and the doc-seed carrier). Appended via `aida \
             comment add`; never binding on implementers — refinements belong in the description.",
    },
    FieldDoc {
        name: "history",
        example: "[{ author: joe, changes: [{field: status, …}] }]",
        provenance: Provenance::ReflectionDerived,
        description: "Immutable append-only change rows — the source-of-truth spec-state time \
             series. Written automatically on every field change; read via `aida history`.",
    },
    FieldDoc {
        name: "processing_record",
        example: "[{ outcome: \"merged via #759\", … }]",
        provenance: Provenance::Orchestrator,
        description: "Durable audit trail of what was done + why each time the spec was processed \
             to completion, promoted from the brief/review verdict. Parallel to history:.",
    },
    FieldDoc {
        name: "archived",
        example: "false",
        provenance: Provenance::User,
        description:
            "View flag (orthogonal to status) hiding the spec from default list/search. Set \
             by `aida archive`, cleared by `aida unarchive`. Archive ≠ deletion.",
    },
    FieldDoc {
        name: "archived_at",
        example: "2026-05-01T12:00:00Z",
        provenance: Provenance::User,
        description:
            "UTC timestamp stamped when archived (None otherwise), used by `--older-than` \
             sweeps to compute age. Cleared on unarchive.",
    },
    FieldDoc {
        name: "deferred",
        example: "false",
        provenance: Provenance::User,
        description: "View flag (orthogonal to status and archived) for primed/conditional work \
             that returns on a trigger. Set by `aida defer`, cleared by `aida undefer`.",
    },
    FieldDoc {
        name: "deferred_at",
        example: "2026-06-10T08:00:00Z",
        provenance: Provenance::User,
        description: "UTC timestamp stamped when deferred (None otherwise). Cleared on undefer.",
    },
    FieldDoc {
        name: "deferred_until",
        example: "when a slice verb ships",
        provenance: Provenance::User,
        description: "The free-text revisit trigger — the one thing distinguishing deferred \
             (prospective) from archived (retrospective). Set via `aida defer --until`.",
    },
    // trace:TASK-1148 | ai:claude
    FieldDoc {
        name: "risk_notes",
        example: "touches the single-spec write path; low blast radius",
        provenance: Provenance::User,
        description: "Narrative residual-risk / blast-radius note not derivable from git, status, \
             or trace. Optional; set via `aida edit --risk-notes`.",
    },
    // trace:TASK-1148 | ai:claude
    FieldDoc {
        name: "test_coverage_notes",
        example: "unit + one YAML round-trip; no end-to-end drain",
        provenance: Provenance::User,
        description: "Narrative note on what was (and was not) covered, and why, beyond what CI \
             status conveys. Optional; set via `aida edit --test-coverage-notes`.",
    },
    // trace:TASK-1148 | ai:claude
    FieldDoc {
        name: "implementation_summary",
        example: "narrowed ImplementationInfo to three narrative fields",
        provenance: Provenance::User,
        description:
            "Narrative 'what shipped and why it was done this way' a commit prefix / diff \
             does not capture. Optional; set via `aida edit --implementation-summary`.",
    },
    // trace:STORY-776 | ai:claude
    FieldDoc {
        name: "execution_mode",
        example: "guided",
        provenance: Provenance::User,
        description:
            "The advisor's bless-time classification of HOW this spec runs when dispatched \
             by `aida do`: drain | drive | guided | operator | decide. None = ungroomed. \
             Advisor-authority write (`aida groom`, `aida edit --mode`, or the TTY \
             micro-groom confirm).",
    },
    FieldDoc {
        name: "custom_status",
        example: "in-review",
        provenance: Provenance::User,
        description: "Custom status string for types with non-standard statuses; takes precedence \
             over the status enum when set.",
    },
    FieldDoc {
        name: "custom_priority",
        example: "p0",
        provenance: Provenance::User,
        description: "Custom priority string for types with non-standard priorities; takes \
             precedence over the priority enum when set.",
    },
    FieldDoc {
        name: "custom_fields",
        example: "{ \"sprint\": \"2026-Q2\" }",
        provenance: Provenance::User,
        description: "Arbitrary key→value extension fields for project-specific metadata. Set via \
             the edit path.",
    },
    FieldDoc {
        name: "urls",
        example: "[{ label: \"design\", url: \"https://…\" }]",
        provenance: Provenance::User,
        description: "External URL links attached to the spec. Added via the edit path.",
    },
    FieldDoc {
        name: "attachments",
        example: "[{ name: \"mock.png\", … }]",
        provenance: Provenance::User,
        description: "File attachments on the spec.",
    },
    FieldDoc {
        name: "trace_links",
        example: "[{ file: \"schema.rs\", symbol: \"print_all\" }]",
        provenance: Provenance::ReflectionDerived,
        description: "Links to code artifacts implementing the spec, derived from `// trace:` \
             comments in the codebase.",
    },
    FieldDoc {
        name: "gitlab_issues",
        example: "[{ project: \"grp/proj\", iid: 42 }]",
        provenance: Provenance::User,
        description: "Links to related GitLab issues.",
    },
    FieldDoc {
        name: "external_refs",
        example: "[\"linear:LIN-123\", \"jira:PROJ-456\"]",
        provenance: Provenance::User,
        description:
            "One-way validated `provider:id` references composing the spec with PM systems \
             (Linear/Jira/GitHub). AIDA records the ref but never syncs state back.",
    },
    FieldDoc {
        name: "implementation_info",
        example: "{ branch: \"feat/…\", pr: 759 }",
        provenance: Provenance::Orchestrator,
        description: "Implementation metadata (branch / PR / commit linkage) captured as the spec \
             is worked.",
    },
    FieldDoc {
        name: "ai_evaluation",
        example: "{ score: 0.8, suggestions: [...] }",
        provenance: Provenance::ReflectionDerived,
        description:
            "Cached AI evaluation results, populated by the background evaluator when the \
             spec changes.",
    },
    FieldDoc {
        name: "attention_reason",
        example: "{ category: design-fork, detail: \"…\" }",
        provenance: Provenance::Orchestrator,
        description: "Why the spec is currently paused — set by `aida punt` when status flips to \
             NeedsAttention, cleared on triage. The durable history lives in the punt ledger.",
    },
    FieldDoc {
        name: "failure_reason",
        example: "{ phase: ci, detail: \"red on …\" }",
        provenance: Provenance::Orchestrator,
        description: "Why the --auto-complete orchestrator shelved the spec after a phase failure \
             (sibling to attention_reason). Sticks until triaged out of NeedsAttention.",
    },
    FieldDoc {
        name: "human_only",
        example: "false",
        provenance: Provenance::User,
        description:
            "Marks work no agent can do (a sign-off, a physical task). The pre-pickup gate \
             skips any spec with this set, so no doomed implementer is spawned.",
    },
    FieldDoc {
        name: "decision_request",
        example: "{ question: \"…\", options: [...] }",
        provenance: Provenance::User,
        description: "A structured decision the human answers outside any agent (the async \
             decision-inbox artifact). Set by `aida questions ask`, answered by `… answer`.",
    },
    FieldDoc {
        name: "interface_changes",
        example: "{ cli: [\"aida schema --explain\"] }",
        provenance: Provenance::User,
        description: "User-facing interface changes captured at close — the deterministic source \
             for the operator digest. Populated by `aida queue done`.",
    },
    // trace:STORY-631 | ai:claude
    FieldDoc {
        name: "intent",
        example: "{ layman: \"why this spec exists…\", llm: \"goal + constraints…\" }",
        provenance: Provenance::Ai,
        description: "AI-GENERATED plain-terms comprehension of WHY this spec exists, distilled \
             from the spec + its graph neighborhood. Cached + drift-stamped; generated by `aida \
             intent`. Not hand-authored ground truth.",
    },
];

/// Look up the curated semantics for a Requirement field by reflected name.
/// trace:STORY-630 | ai:claude
fn requirement_field_doc(name: &str) -> Option<&'static FieldDoc> {
    REQUIREMENT_FIELD_DOCS.iter().find(|d| d.name == name)
}

/// Convert a reflected PascalCase enum variant name into its on-the-wire
/// token — the kebab-case form every CLI parser accepts
/// (`InProgress` -> `in-progress`, `NonFunctional` -> `non-functional`,
/// `VerifiedBy` -> `verified-by`). This is the single conversion rule the
/// drift-guard pins. trace:STORY-538 | ai:claude
fn variant_to_wire_token(variant: &str) -> String {
    let mut out = String::with_capacity(variant.len() + 4);
    for (i, ch) in variant.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i != 0 {
                out.push('-');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Parse the unit-variant names out of a `TS::decl()` enum declaration line of
/// the form `type Name = "A" | "B" | { "Custom": string };`. Newtype variants
/// (the `{ "Custom": string }` arm) are skipped — they carry a user-defined
/// payload, not a fixed token. trace:STORY-538 | ai:claude
fn parse_enum_variants(decl: &str) -> Vec<String> {
    let mut variants = Vec::new();
    let bytes = decl.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            // Collect to the closing quote.
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'"' {
                j += 1;
            }
            let token = &decl[start..j];
            // A `{ "Custom": string }` arm has a `:` immediately after the
            // closing quote (modulo whitespace) — that names a newtype payload
            // key, not a unit variant. Skip it.
            let mut k = j + 1;
            while k < bytes.len() && (bytes[k] == b' ' || bytes[k] == b'\t') {
                k += 1;
            }
            if !(k < bytes.len() && bytes[k] == b':') {
                variants.push(token.to_string());
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    variants
}

/// Derive the four controlled-vocabulary enum schemas from reflection.
fn requirement_enums() -> Vec<EnumSchema> {
    let to_tokens = |decl: String| -> Vec<String> {
        parse_enum_variants(&decl)
            .iter()
            .map(|v| variant_to_wire_token(v))
            .collect()
    };
    vec![
        EnumSchema {
            field: "status",
            tokens: to_tokens(RequirementStatus::decl()),
        },
        EnumSchema {
            field: "type",
            tokens: to_tokens(RequirementType::decl()),
        },
        EnumSchema {
            field: "relationship",
            tokens: to_tokens(RelationshipType::decl()),
        },
        EnumSchema {
            field: "priority",
            tokens: to_tokens(RequirementPriority::decl()),
        },
    ]
}

/// Parse the Requirement field table out of its `TS::decl()` named-struct
/// declaration. Each field line has the shape `name: type,` or
/// `name?: type | null,` (with `/** ... */` doc-comment blocks between
/// fields, which we skip). trace:STORY-538 | ai:claude
fn requirement_fields() -> Vec<FieldSchema> {
    parse_struct_fields(&Requirement::decl())
}

/// The named-struct field parser shared by every catalog kind (and the
/// drift-guard tests). Handles both `TS::decl()` field layouts:
///   - one field per line (ts-rs-forge emits this when a field carries a
///     `/** ... */` doc-comment — the Requirement case STORY-538 shipped), and
///   - several fields on one line (`{ a: string, b: string, c: T, ` — the
///     layout ts-rs-forge uses for fields with no doc-comment; TASK-714's
///     CLI-side structs hit this).
///
/// It first strips all `/** ... */` doc blocks, then the `type X = {` header
/// and trailing `};`, then splits the remaining body on **top-level** commas
/// (commas inside nested `{}`/`<>`/`[]`/`()` are part of a type, not field
/// separators) and parses each segment as `name(?): type`. A `| null` type
/// arm is treated as optional, matching the `name?:` convention.
/// trace:STORY-538 trace:TASK-714 | ai:claude
fn parse_struct_fields(decl: &str) -> Vec<FieldSchema> {
    // 1. Strip `/** ... */` doc-comment blocks (they may span lines).
    let mut body = String::with_capacity(decl.len());
    let mut rest = decl;
    while let Some(open) = rest.find("/**") {
        body.push_str(&rest[..open]);
        if let Some(close) = rest[open..].find("*/") {
            rest = &rest[open + close + 2..];
        } else {
            rest = "";
            break;
        }
    }
    body.push_str(rest);

    // 2. Reduce to the brace body: drop everything up to and including the
    //    first `{`, and the trailing `};` / `}`.
    let inner = match (body.find('{'), body.rfind('}')) {
        (Some(o), Some(c)) if c > o => &body[o + 1..c],
        _ => return Vec::new(),
    };

    // 3. Split on top-level commas, depth-aware over `{}` `<>` `[]` `()`.
    let mut segments: Vec<String> = Vec::new();
    let mut depth: i32 = 0;
    let mut cur = String::new();
    for ch in inner.chars() {
        match ch {
            '{' | '<' | '[' | '(' => {
                depth += 1;
                cur.push(ch);
            }
            '}' | '>' | ']' | ')' => {
                depth -= 1;
                cur.push(ch);
            }
            ',' if depth == 0 => {
                segments.push(std::mem::take(&mut cur));
            }
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        segments.push(cur);
    }

    // 4. Parse each `name(?): type` segment.
    let mut fields = Vec::new();
    for seg in segments {
        let seg = seg.trim();
        if seg.is_empty() {
            continue;
        }
        let Some(colon) = seg.find(':') else {
            continue;
        };
        let name_part = seg[..colon].trim();
        let optional_marker = name_part.ends_with('?');
        let name = name_part.trim_end_matches('?').trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let ts_type = seg[colon + 1..].trim().to_string();
        // A field is optional if it was declared `name?:` OR its type admits
        // `null` (ts-rs-forge renders `Option<T>` as `T | null`).
        let optional = optional_marker || type_admits_null(&ts_type);
        fields.push(FieldSchema {
            name: name.to_string(),
            ts_type,
            optional,
        });
    }
    fields
}

/// True if a reflected TS type admits `null` — i.e. it is (or unions in) the
/// `null` literal, the shape ts-rs-forge gives an `Option<T>`. Checked on
/// top-level union arms only so a nested `{ x: T | null }` doesn't count.
/// trace:TASK-714 | ai:claude
fn type_admits_null(ts_type: &str) -> bool {
    let mut depth: i32 = 0;
    let mut arm = String::new();
    let mut admits = false;
    let mut consider = |arm: &str| {
        if arm.trim() == "null" {
            admits = true;
        }
    };
    for ch in ts_type.chars() {
        match ch {
            '{' | '<' | '[' | '(' => {
                depth += 1;
                arm.push(ch);
            }
            '}' | '>' | ']' | ')' => {
                depth -= 1;
                arm.push(ch);
            }
            '|' if depth == 0 => {
                consider(&arm);
                arm.clear();
            }
            _ => arm.push(ch),
        }
    }
    consider(&arm);
    admits
}

/// `aida schema` (no args) — the storable-object catalog. With `explain`, each
/// object also renders its lifecycle block. trace:STORY-630 | ai:claude
pub fn print_catalog(json_out: bool, explain: bool) {
    if json_out {
        // Single source: the same value `aida://schema` / the `schema` MCP
        // tool emit. trace:TASK-715 | ai:claude
        println!(
            "{}",
            serde_json::to_string_pretty(&catalog_json_inner(explain)).unwrap()
        );
        return;
    }

    println!("Storable object catalog\n");
    if explain {
        // The explanatory catalog: each kind's one-liner followed by its
        // lifecycle block. trace:STORY-630 | ai:claude
        for (i, e) in CATALOG.iter().enumerate() {
            if i > 0 {
                println!();
            }
            println!("{}\n  {}", e.name, e.description);
            println!("  Lifecycle: {}", wrap_indent(e.lifecycle, "    "));
        }
        println!(
            "\nPer-field semantics for any kind: `aida schema <object> --explain` \
             (e.g. `aida schema requirement --explain`)."
        );
        return;
    }
    let width = CATALOG.iter().map(|e| e.name.len()).max().unwrap_or(0);
    for e in CATALOG {
        println!("  {:<width$}  {}", e.name, e.description, width = width);
    }
    println!(
        "\nField detail for any kind: `aida schema <object>` \
         (e.g. `aida schema requirement`, `aida schema punt`)."
    );
}

/// Reflow a long lifecycle/description string onto wrapped lines with a hanging
/// indent, so the explanatory blocks read as prose in a terminal rather than as
/// one ragged line. Whitespace-collapsing; ~78-col target. trace:STORY-630
fn wrap_indent(text: &str, indent: &str) -> String {
    const WIDTH: usize = 78;
    let mut out = String::new();
    let mut line_len = indent.len();
    let mut first = true;
    for word in text.split_whitespace() {
        if !first && line_len + 1 + word.len() > WIDTH {
            out.push('\n');
            out.push_str(indent);
            line_len = indent.len();
            out.push_str(word);
            line_len += word.len();
        } else {
            if !first {
                out.push(' ');
                line_len += 1;
            }
            out.push_str(word);
            line_len += word.len();
        }
        first = false;
    }
    out
}

/// `aida schema --all` (and the field-included no-arg `aida schema --json`) —
/// the full dump in one pass: the catalog followed by every object's
/// reflection-derived field detail, in catalog order. Reuses the existing
/// per-object renderers ([`full_dump_json_inner`] / [`print_requirement`] /
/// [`print_object`]) — no field assembly is reimplemented here.
/// trace:TASK-799 | ai:claude
pub fn print_all(json_out: bool, explain: bool) {
    if json_out {
        // Single source: the same per-object projection `aida schema <object>`
        // and the MCP schema surface build. trace:TASK-799 | ai:claude
        println!(
            "{}",
            serde_json::to_string_pretty(&full_dump_json_inner(explain)).unwrap()
        );
        return;
    }

    print_catalog(false, explain);
    println!("\n{}\n", "=".repeat(60));
    for (i, e) in CATALOG.iter().enumerate() {
        if i > 0 {
            println!("\n{}\n", "-".repeat(60));
        }
        if e.name.eq_ignore_ascii_case("Requirement") {
            print_requirement(false, explain);
        } else {
            print_object(e.name, false, explain);
        }
    }
}

/// Look up a catalog entry by case-insensitive name. trace:TASK-714
fn catalog_entry(name: &str) -> Option<&'static CatalogEntry> {
    CATALOG.iter().find(|e| e.name.eq_ignore_ascii_case(name))
}

/// `aida schema <object>` for any catalog kind other than `requirement`
/// (which keeps its enum-augmented view in [`print_requirement`]). Renders the
/// reflection-derived field table for the kind's backing struct. The caller has
/// already confirmed `name` is a catalog kind. trace:TASK-714 | ai:claude
pub fn print_object(name: &str, json_out: bool, explain: bool) {
    let Some(entry) = catalog_entry(name) else {
        // Defensive: the dispatcher only calls this for catalog kinds.
        return;
    };
    if json_out {
        // Single source: the same value `aida://schema/<object>` / the
        // `schema` MCP tool emit. trace:TASK-715 | ai:claude
        let v =
            object_json_inner(entry.name, explain).expect("catalog kind has object_json detail");
        println!("{}", serde_json::to_string_pretty(&v).unwrap());
        return;
    }

    let fields = parse_struct_fields(&(entry.decl)());

    println!("{} — fields\n", entry.name);
    if explain {
        // Per-field semantics are curated for Requirement (Slice 1); other
        // kinds show the reflected shape plus their lifecycle block until
        // Slice 2 fills their field prose. trace:STORY-630 | ai:claude
        print_explained_fields(&fields, entry.name);
        if let Some(note) = entry.note {
            println!("\nNote: {note}");
        }
        println!("\nLifecycle\n  {}", wrap_indent(entry.lifecycle, "  "));
        return;
    }
    let name_w = fields.iter().map(|f| f.name.len()).max().unwrap_or(0);
    for f in &fields {
        let opt = if f.optional { " (optional)" } else { "" };
        println!(
            "  {:<name_w$}  {}{}",
            f.name,
            f.ts_type,
            opt,
            name_w = name_w
        );
    }
    if let Some(note) = entry.note {
        println!("\nNote: {note}");
    }
}

/// Render the per-field explanatory block: for each field, its type + (when a
/// curated doc entry exists) example value, provenance token, and the
/// when/why gloss. Fields without a doc entry (non-Requirement kinds until
/// Slice 2) print type-only with a `(prose pending)` marker so the gap is
/// visible — the drift-guard test names exactly which fields those are.
/// trace:STORY-630 | ai:claude
fn print_explained_fields(fields: &[FieldSchema], object: &str) {
    for f in fields {
        let opt = if f.optional { " (optional)" } else { "" };
        let doc = if object.eq_ignore_ascii_case("Requirement") {
            requirement_field_doc(&f.name)
        } else {
            None
        };
        match doc {
            Some(d) => {
                println!("  {} : {}{}", f.name, f.ts_type, opt);
                println!("      example    {}", d.example);
                println!("      set by     {}", d.provenance.token());
                println!("      {}", wrap_indent(d.description, "      "));
            }
            None => {
                println!("  {} : {}{}  (prose pending)", f.name, f.ts_type, opt);
            }
        }
        println!();
    }
}

/// `aida schema requirement` — the reflection-derived field table and the
/// four controlled-vocabulary enums in on-the-wire token form.
pub fn print_requirement(json_out: bool, explain: bool) {
    if json_out {
        // Single source: the same value `aida://schema/requirement` / the
        // `schema` MCP tool emit. trace:TASK-715 | ai:claude
        println!(
            "{}",
            serde_json::to_string_pretty(&requirement_json_inner(explain)).unwrap()
        );
        return;
    }

    let fields = requirement_fields();
    let enums = requirement_enums();

    println!("Requirement — fields\n");
    if explain {
        print_explained_fields(&fields, "Requirement");
    } else {
        let name_w = fields.iter().map(|f| f.name.len()).max().unwrap_or(0);
        for f in &fields {
            let opt = if f.optional { " (optional)" } else { "" };
            println!(
                "  {:<name_w$}  {}{}",
                f.name,
                f.ts_type,
                opt,
                name_w = name_w
            );
        }
    }

    println!("\nRequirement — controlled vocabularies (on-the-wire tokens)\n");
    let field_w = enums.iter().map(|e| e.field.len()).max().unwrap_or(0);
    for e in &enums {
        println!(
            "  {:<field_w$}  {}",
            e.field,
            e.tokens.join("|"),
            field_w = field_w
        );
    }

    if explain {
        let entry = catalog_entry("Requirement").expect("Requirement is a catalog kind");
        println!("\nLifecycle\n  {}", wrap_indent(entry.lifecycle, "  "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_to_wire_token_matches_cli_forms() {
        assert_eq!(variant_to_wire_token("Draft"), "draft");
        assert_eq!(variant_to_wire_token("InProgress"), "in-progress");
        assert_eq!(variant_to_wire_token("NeedsAttention"), "needs-attention");
        assert_eq!(variant_to_wire_token("NonFunctional"), "non-functional");
        assert_eq!(variant_to_wire_token("VerifiedBy"), "verified-by");
        assert_eq!(variant_to_wire_token("BlockedBy"), "blocked-by");
        assert_eq!(variant_to_wire_token("High"), "high");
    }

    #[test]
    fn parse_struct_fields_handles_inline_and_null_layouts() {
        // ts-rs-forge packs doc-comment-less fields onto one line, and renders
        // `Option<T>` as `T | null`. Both must parse. trace:TASK-714
        let decl = "type X = { a: string, b: number, c: string | null, };";
        let fields = parse_struct_fields(decl);
        let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
        // `c: string | null` → optional; the others are required.
        assert!(!fields[0].optional);
        assert!(!fields[1].optional);
        assert!(fields[2].optional);
        // A top-level comma inside a nested object/array must NOT split a field.
        let nested = "type Y = { m: Array<string>, n: { p: number, q: number }, };";
        let nf = parse_struct_fields(nested);
        let nn: Vec<&str> = nf.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(nn, vec!["m", "n"]);
    }

    #[test]
    fn parse_enum_variants_skips_custom_newtype() {
        let decl = aida_core::models::RelationshipType::decl();
        let variants = parse_enum_variants(&decl);
        // The `{ "Custom": string }` arm must NOT appear as a unit variant.
        assert!(variants.contains(&"Parent".to_string()));
        assert!(variants.contains(&"BlockedBy".to_string()));
        assert!(!variants.contains(&"Custom".to_string()));
    }

    /// DRIFT-GUARD: the enum tokens the schema reports must stay in sync with
    /// the model reflection AND with the canonical CLI/wire forms. If a variant
    /// is added/removed/renamed in `models.rs`, the reflected `decl()` changes
    /// and these expectations break — forcing the schema (and this list) to be
    /// updated deliberately rather than silently rotting. trace:STORY-538
    #[test]
    fn schema_enums_match_reflection() {
        let enums = requirement_enums();
        let by_field = |f: &str| -> Vec<String> {
            enums.iter().find(|e| e.field == f).unwrap().tokens.clone()
        };

        assert_eq!(
            by_field("status"),
            vec![
                "draft",
                "approved",
                "planned",
                "in-progress",
                "done",
                "completed",
                "rejected",
                "needs-attention",
            ]
        );
        assert_eq!(by_field("priority"), vec!["high", "medium", "low"]);
        assert_eq!(
            by_field("type"),
            vec![
                "functional",
                "non-functional",
                "system",
                "user",
                "change-request",
                "bug",
                "epic",
                "story",
                "task",
                "spike",
                "sprint",
                "folder",
                "meta",
                "principle",
                "vision",
                "constraint",
                "decision",
                "term",
                "doc",
            ]
        );
        // Relationship: the fixed (non-Custom) variants, in declaration order.
        assert_eq!(
            by_field("relationship"),
            vec![
                "parent",
                "child",
                "duplicate",
                "verifies",
                "verified-by",
                "references",
                "blocked-by",
                "blocks",
            ]
        );
    }

    /// DRIFT-GUARD: the Requirement field set the schema reports must match the
    /// fields the model reflects. A new field on `Requirement` (or a removed
    /// one) changes `Requirement::decl()` and breaks this anchor, so the schema
    /// view can't silently drift from `models.rs`. We assert a representative
    /// stable subset is present (so unrelated additive fields don't churn the
    /// test) plus that the field count tracks reflection exactly.
    #[test]
    fn schema_fields_track_reflection() {
        let fields = requirement_fields();
        let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        for expected in [
            "id",
            "spec_id",
            "title",
            "description",
            "status",
            "priority",
            "req_type",
            "tags",
            "relationships",
            "comments",
            "history",
            "archived",
        ] {
            assert!(
                names.contains(&expected),
                "schema lost reflected field `{expected}` — schema drifted from models.rs"
            );
        }
        // The parse must recover the SAME number of fields the reflection
        // emits (one field line per non-doc, non-brace line). Re-derive the
        // expected count straight from a fresh parse so the guard tracks the
        // model, not a frozen integer.
        let reparsed = parse_struct_fields(&Requirement::decl());
        assert_eq!(fields.len(), reparsed.len());
        // Sanity floor: Requirement is a wide struct; if the parser ever
        // collapses to a near-empty set the guard should scream.
        assert!(
            fields.len() >= 20,
            "expected >=20 reflected Requirement fields, got {}",
            fields.len()
        );
    }

    /// Smoke: the catalog covers exactly the ten storable kinds the MVP slice
    /// promises, and `Relationship` (the type whose presence proves the import
    /// is live) is among them.
    #[test]
    fn catalog_lists_the_storable_kinds() {
        let names: Vec<&str> = CATALOG.iter().map(|e| e.name).collect();
        for expected in [
            "Requirement",
            "Finding",
            "Brief",
            "Punt",
            "Directive",
            "Comment",
            "Lease",
            "QueueItem",
            "HistoryEntry",
            "Relationship",
        ] {
            assert!(names.contains(&expected), "catalog missing {expected}");
        }
        // Touch the Relationship type's reflection so the catalog's claim that
        // it is a storable kind stays grounded in a type that actually exists.
        let _ = aida_core::models::Relationship::decl();
    }

    /// DRIFT-GUARD (TASK-714): every catalog kind must reflect a non-empty field
    /// set through its `decl` closure, AND a representative stable field per kind
    /// must be present. A field rename/removal on the backing struct changes its
    /// `decl()` and breaks the matching anchor here — forcing the schema (and
    /// this list) to be updated deliberately rather than silently rotting. This
    /// extends `schema_fields_track_reflection` (Requirement-only) to the rest.
    #[test]
    fn schema_object_fields_track_reflection() {
        // (catalog name, a field that must survive on the backing struct).
        let anchors: &[(&str, &str)] = &[
            ("Requirement", "spec_id"),
            ("Finding", "display_id"),
            ("Brief", "spec_id"),
            ("Punt", "category"),
            ("Directive", "verb"),
            ("Comment", "content"),
            ("Lease", "scope"),
            ("QueueItem", "user_id"),
            ("HistoryEntry", "changes"),
            ("Relationship", "target_id"),
        ];
        for (name, anchor) in anchors {
            let entry =
                catalog_entry(name).unwrap_or_else(|| panic!("catalog missing kind `{name}`"));
            let fields = parse_struct_fields(&(entry.decl)());
            assert!(
                !fields.is_empty(),
                "schema reflected zero fields for `{name}` — reflection parse drifted"
            );
            let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
            assert!(
                names.contains(anchor),
                "schema lost reflected field `{anchor}` on `{name}` — \
                 schema drifted from the backing struct"
            );
        }
    }

    /// Smoke: `print_object` runs for every non-Requirement catalog kind without
    /// panicking, in both text and JSON modes. trace:TASK-714
    #[test]
    fn print_object_covers_every_catalog_kind() {
        for entry in CATALOG {
            if entry.name.eq_ignore_ascii_case("requirement") {
                continue;
            }
            // Exercises the reflection + render path; a parse that emits nothing
            // would surface as an empty table, caught by the drift guard above.
            print_object(entry.name, false, false);
            print_object(entry.name, true, false);
            // The explanatory mode must also render cleanly for every kind.
            // trace:STORY-630
            print_object(entry.name, false, true);
            print_object(entry.name, true, true);
        }
    }

    /// TASK-799: the no-arg full dump (`aida schema --all --json` / the
    /// field-included no-arg `--json`) must carry one detail object per catalog
    /// kind, each with a non-empty `fields` array — unlike the field-less
    /// catalog. Requirement additionally carries its `enums` block.
    #[test]
    fn full_dump_includes_fields_for_every_kind() {
        let dump = full_dump_json_inner(false);
        let objects = dump
            .get("objects")
            .and_then(|v| v.as_array())
            .expect("full dump has an objects array");
        assert_eq!(
            objects.len(),
            CATALOG.len(),
            "full dump must cover every catalog kind"
        );
        for obj in objects {
            let name = obj
                .get("object")
                .and_then(|v| v.as_str())
                .expect("each full-dump entry names its object");
            let fields = obj
                .get("fields")
                .and_then(|v| v.as_array())
                .unwrap_or_else(|| panic!("full-dump entry `{name}` is missing its fields array"));
            assert!(
                !fields.is_empty(),
                "full-dump entry `{name}` has an empty fields array"
            );
        }
        // The catalog form (no fields) must remain field-less, so the two
        // surfaces stay distinct.
        let catalog = catalog_json();
        let first = &catalog["objects"][0];
        assert!(
            first.get("fields").is_none(),
            "the bare catalog must NOT carry per-object fields"
        );
        // Requirement carries its controlled-vocabulary enums in the dump.
        let req = objects
            .iter()
            .find(|o| o["object"] == json!("Requirement"))
            .expect("full dump includes Requirement");
        assert!(
            req.get("enums").is_some(),
            "Requirement detail in the full dump must carry its enums block"
        );
    }

    /// Smoke: `print_all` runs in both text and JSON modes without panicking.
    /// trace:TASK-799
    #[test]
    fn print_all_runs_in_both_modes() {
        print_all(false, false);
        print_all(true, false);
        // Explanatory mode must also run clean. trace:STORY-630
        print_all(false, true);
        print_all(true, true);
    }

    /// TASK-775: `aida schema --all --json` must be a one-fetch map naming
    /// EXACTLY the catalog object set — every catalog kind appears in the dump
    /// and the dump names nothing outside the catalog. Driven off `CATALOG`
    /// itself (the same source-of-truth the catalog view and `--all` iterate),
    /// so a newly-added object auto-extends the assertion: drift-safe.
    #[test]
    fn full_dump_json_is_keyed_by_every_catalog_object() {
        use std::collections::BTreeSet;

        let dump = full_dump_json_inner(false);
        let dumped: BTreeSet<String> = dump
            .get("objects")
            .and_then(|v| v.as_array())
            .expect("full dump has an objects array")
            .iter()
            .map(|o| {
                o.get("object")
                    .and_then(|v| v.as_str())
                    .expect("each entry names its object")
                    .to_string()
            })
            .collect();
        let catalog: BTreeSet<String> = CATALOG.iter().map(|e| e.name.to_string()).collect();
        assert_eq!(
            dumped, catalog,
            "the --all --json map must be keyed by exactly the catalog object set"
        );
    }

    /// DRIFT-GUARD (STORY-630): the curated per-field doc-map for `Requirement`
    /// must cover EXACTLY the reflected `Requirement` field set — every reflected
    /// field has a complete doc entry (example + provenance + description), and
    /// every doc entry maps to a real reflected field (no orphans). This is the
    /// substrate-as-bouncer property for the `--explain` layer: a field added to
    /// `models.rs` without an explanation, or an explanation left behind after a
    /// field is removed, fails the build here. Same spirit as
    /// `schema_enums_match_reflection`. The failure message lists exactly which
    /// fields still need docs, so the guard doubles as the Slice-2 worklist.
    #[test]
    fn explain_docs_match_reflection() {
        use std::collections::BTreeSet;

        let reflected: BTreeSet<String> =
            requirement_fields().into_iter().map(|f| f.name).collect();
        let documented: BTreeSet<String> = REQUIREMENT_FIELD_DOCS
            .iter()
            .map(|d| d.name.to_string())
            .collect();

        // 1. No undocumented reflected field (the Slice-2 worklist if it fails).
        let undocumented: Vec<&String> = reflected.difference(&documented).collect();
        assert!(
            undocumented.is_empty(),
            "schema --explain: these reflected Requirement fields have NO doc entry \
             (add them to REQUIREMENT_FIELD_DOCS): {undocumented:?}"
        );

        // 2. No orphan doc entry (a doc for a field reflection no longer emits).
        let orphans: Vec<&String> = documented.difference(&reflected).collect();
        assert!(
            orphans.is_empty(),
            "schema --explain: these doc entries map to NO reflected Requirement field \
             (remove or rename them in REQUIREMENT_FIELD_DOCS): {orphans:?}"
        );

        // 3. Each doc entry is complete (non-empty example + description). The
        //    provenance token is from the closed set by construction (enum).
        for d in REQUIREMENT_FIELD_DOCS {
            assert!(
                !d.example.trim().is_empty(),
                "schema --explain: field `{}` has an empty example value",
                d.name
            );
            assert!(
                !d.description.trim().is_empty(),
                "schema --explain: field `{}` has an empty description",
                d.name
            );
            assert!(
                !d.provenance.token().is_empty(),
                "schema --explain: field `{}` has an empty provenance token",
                d.name
            );
        }

        // 4. No duplicate doc entries (would silently shadow).
        assert_eq!(
            documented.len(),
            REQUIREMENT_FIELD_DOCS.len(),
            "schema --explain: REQUIREMENT_FIELD_DOCS has duplicate field entries"
        );
    }

    /// Every catalog kind carries a non-empty lifecycle block (the object-level
    /// layer of `--explain`). A new catalog kind added without a lifecycle block
    /// fails here. trace:STORY-630
    #[test]
    fn every_catalog_kind_has_a_lifecycle_block() {
        for e in CATALOG {
            assert!(
                !e.lifecycle.trim().is_empty(),
                "catalog kind `{}` has an empty lifecycle block — \
                 add one for `aida schema --explain`",
                e.name
            );
        }
    }

    /// The `--explain` JSON for Requirement carries the per-field
    /// example/provenance/description and the object lifecycle, and the
    /// non-explain JSON does NOT (pure opt-in / byte-stable default).
    /// trace:STORY-630
    #[test]
    fn explain_json_carries_semantics_default_does_not() {
        // Default: no example/provenance/description, no lifecycle.
        let plain = requirement_json_inner(false);
        let plain_first = &plain["fields"][0];
        assert!(plain_first.get("example").is_none());
        assert!(plain_first.get("provenance").is_none());
        assert!(plain.get("lifecycle").is_none());

        // Explain: every Requirement field carries the full doc triple, and the
        // object carries its lifecycle block.
        let rich = requirement_json_inner(true);
        assert!(rich.get("lifecycle").and_then(|v| v.as_str()).is_some());
        let fields = rich["fields"].as_array().expect("fields array");
        for f in fields {
            let name = f["name"].as_str().unwrap();
            assert!(
                f.get("example").and_then(|v| v.as_str()).is_some(),
                "explain JSON field `{name}` missing example"
            );
            let prov = f
                .get("provenance")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("explain JSON field `{name}` missing provenance"));
            assert!(
                [
                    "user",
                    "advisor-gated",
                    "merge-driven",
                    "orchestrator",
                    "reflection-derived",
                    // trace:STORY-631 — AI-generated cached fields (intent).
                    "ai"
                ]
                .contains(&prov),
                "explain JSON field `{name}` has out-of-set provenance `{prov}`"
            );
            assert!(
                f.get("description").and_then(|v| v.as_str()).is_some(),
                "explain JSON field `{name}` missing description"
            );
        }
    }
}
