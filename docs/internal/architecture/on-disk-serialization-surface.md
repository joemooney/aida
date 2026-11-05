# On-disk serialization surface

**Status:** reference · **Specs:** TASK-590, feeds SPIKE-44 · **Last updated:** 2026-05-31

This is the inventory of every place AIDA reads or writes a *persisted* format,
the serde behaviours that affect on-disk compatibility, and the concrete
hazards a **non-AIDA tool** must handle to interoperate with the git-canonical
store. It exists to ground SPIKE-44 — *"can a non-AIDA tool read/write the
store?"* — the question the multi-vendor multi-agent moat hinges on.

Every claim is cited `file:line`. The inventory was machine-generated and the
load-bearing custom (de)serializers were spot-verified against source on
2026-05-31; line numbers drift — re-verify with `aida plan verify` or a symbol
grep before relying on a specific line.

## Formats at a glance

| Format | Path | Role |
|---|---|---|
| Canonical spec YAML | `objects/TYPE/SHARD/SPEC-ID.yaml` | **Writer of record** |
| SQLite cache | `.aida/cache.db` | Rebuildable read projection |
| Operation log YAML | `oplog.yaml` | Append-only CRDT event stream |
| TOML config | node / workspace / agreed-counters / preferences | Static config |
| JSON | `.aida/mailbox/*.json`, `.aida/cache.db.lock-info` | Message layer + metadata |

The cache is **rebuildable from git** (`aida cache rebuild`) and carries a
`schema_version` in its `cache_meta` table (currently `"2"`, bumped when
`archived_at` was added — STORY-441). A non-AIDA tool never needs to *write*
the cache; it can rebuild it. The canonical YAML is the only writer-of-record
surface, so it is where interop correctness actually matters.

## Canonical spec YAML — the surface that matters

Writer: `object_store::write_object` / `write_object_if_changed`
(`aida-core/src/object_store.rs:116,146`). Reader: `read_object` /
`read_object_from_path` (`object_store.rs:184,199`). The root type is
`Requirement` (`models.rs:3259`), with nested `Comment`, `HistoryEntry`,
`FieldChange`, `Relationship`, `UrlLink`, `Attachment`, `TraceLink`,
`ImplementationInfo`, `AttentionReason`, `FailureReason`, `StoredAiEvaluation`.

There is **no explicit schema-version field in the YAML**. Forward-compat is
achieved structurally: `#[serde(default)]` + `skip_serializing_if` on optional
fields (missing → `None`/empty; empty → omitted on write), plus the
fallback-tolerant custom deserializers below. Old files stay readable after
schema additions; new files stay readable by old binaries because unknown
relationship variants fall back to `Custom`.

## Interop hazards (ranked) — what a non-AIDA writer must replicate

### 1. CRITICAL — fallback-tolerant custom deserializers
- **`RelationshipType`** has a hand-written `Deserialize`
  (`models.rs:465`, normalizer `from_str` at `models.rs:382`). It accepts
  **three wire shapes** — bare strings (`"parent"`, `"blocked-by"`,
  case-insensitive), YAML external tags (`!Custom foo`), and JSON maps
  (`{"Custom":"foo"}`) — and **falls unknown variants through to
  `Custom(name)` rather than erroring**. This is what lets a newer binary's
  relationship type round-trip through an older one. A non-AIDA reader that
  hard-codes a closed variant set will *drop or reject* data.
- **`node_id`** uses untagged int-or-string deserializers
  (`deserialize_node_id` `node.rs:59`; `deserialize_node_id_oplog`
  `oplog.rs:141`): pre-EPIC-9 numeric IDs (`node_id = 1`) and string IDs
  (`node_id = "JM"`) both deserialize, numeric coerced to decimal string. A
  tool that assumes one type fails on legacy files.

### 2. HIGH — deterministic sorted-collection serializers
`Requirement.tags: HashSet<String>` and `custom_fields: HashMap<String,String>`
serialize through `yaml_helpers::serialize_sorted_string_set` /
`serialize_sorted_string_map` (`yaml_helpers.rs:13,30`; wired at
`models.rs:3323,3369`). They **sort before emitting** so the YAML is
byte-deterministic — which is what makes `write_object_if_changed`
(`object_store.rs:146`) able to detect a no-op write and skip a spurious git
diff. A non-AIDA writer that emits tags/keys in hash order produces a diff on
every save even when nothing changed, breaking the "idempotent, no spurious
diffs" guarantee that keeps the orphan branch's history meaningful.

### 3. HIGH — field-name transformations
serde `rename_all` / `rename` mean on-disk names differ from code names:
`PuntCategory` kebab-case (`design-fork`; `models.rs:66`), `IdFormatPolicy`
kebab-case (`node.rs:108`), `IdCounterScope` kebab-case (`node.rs:184`),
`UrlOpenMode` snake_case (`new_tab`; `models.rs:2016`), `IssueReport.issue_type`
renamed to `type` (`ai/responses.rs`), `AgreedCounters` `#[serde(flatten)]` →
flat TOML table (`node.rs:830`). A hand-parser must know each rule per type.

### 4. MEDIUM — internally-tagged enums
`Recipient` (`mailbox.rs:25`, `tag = "kind", content = "agent",
rename_all = "snake_case"`) and `DeploymentMode` (`node.rs:913`, `tag = "mode"`
with renamed variants). The tag field name/value must match exactly.

### 5. MEDIUM — optional-field omission
Pervasive `skip_serializing_if = "Option::is_none"` / `"Vec::is_empty"` /
`"std::ops::Not::not"` (bools). A writer must omit empty fields to avoid
spurious diffs and must default missing fields on read.

## Bottom line for SPIKE-44

- **Easiest to interoperate with:** the SQLite cache (standard SQL, rebuildable
  — read it, never write it), `HistoryEntry`/`FieldChange` (plain derived serde),
  HLC timestamps.
- **Hardest:** the canonical spec YAML — specifically the fallback-tolerant
  `RelationshipType`, the untagged `node_id`, and the sorted-collection
  determinism requirement.
- **Read-only interop is cheap** (deserialize is tolerant by design). **Write
  interop is the hard part**, and it concentrates in exactly three behaviours
  (hazards 1–2): replicate the sorted serializers, the three-shape
  RelationshipType, and the optional-field omission rules, and a non-AIDA
  writer produces byte-identical, diff-clean YAML. That is a *bounded,
  specifiable* surface — which is the encouraging answer for the multi-vendor
  thesis: the moat is real (a naïve writer corrupts history) but the
  conformance contract is small enough to publish.

## Related
- SPIKE-44 — multi-vendor substrate access (this doc is its groundwork)
- `docs/plans/2026-05-02-git-canonical-storage.md` — the storage model
- `aida-core/src/{object_store,models,yaml_helpers,node,oplog}.rs` — the surface
