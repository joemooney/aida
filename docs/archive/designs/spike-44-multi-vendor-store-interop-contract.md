# SPIKE-44 sketch: multi-vendor access to AIDA's git-canonical store

Date: 2026-06-04

Spec: SPIKE-44

Status: architecture sketch only. No AIDA code or on-disk format changes are
proposed in this PR.

## Scope

This sketch answers the operator question: can a non-AIDA tool read and write
AIDA's git-canonical store?

The answer is:

- **Read:** yes, already demonstrated.
- **Write:** yes only for narrow, operation-shaped writes; direct generic YAML
  rewriting is not a safe contract.
- **Recommended substrate contract:** open direct reads, bounded direct writes,
  and MCP/CLI as the default conformant write path until a formal writer spec
  exists.

## Empirical evidence

This spike reuses and cross-checks the existing SPIKE-46 work because it is
exactly the non-AIDA prototype SPIKE-44 asks for:

- `docs/architecture/spike-46-store-interop/store_interop_prototype.py`
- `docs/architecture/spike-46-store-interop/store_reader.py`
- `docs/architecture/spike-46-store-interop/FINDINGS.md`
- `docs/architecture/on-disk-serialization-surface.md`

Observed on 2026-06-04:

```text
python3 docs/architecture/spike-46-store-interop/store_interop_prototype.py read-report

Store: /home/joe/ai/aida/.aida-store
Objects parsed: 1714/1714
Objects with id/spec_id/status/req_type reachable: 1714/1714
Relationship kinds observed: BlockedBy, Child, Custom:implemented-by,
  Custom:implements, Custom:related, Custom:sprint_assignment,
  Custom:sprint_contains, Parent, References
```

This is a true non-AIDA read: Python + PyYAML, not the AIDA binary.

Additional SPIKE-44 local probe:

- Read `.aida-store/objects/SPIKE/000/SPIKE-24.yaml` with PyYAML.
- Reached `spec_id`, `status`, `req_type`, and top-level fields.
- Produced a temp/in-memory status-flip YAML prototype.
- Did not mutate the real store.

The result matches SPIKE-46's conclusion: read access is easy once the loader
handles AIDA's custom relationship YAML tag; generic write access is not
byte-clean because normal YAML emitters rewrite timestamps, scalar style, field
presentation, and optional fields.

## Current store model

AIDA's writer-of-record is the orphan `aida-store` branch exposed locally as
`.aida-store/`. Requirement objects live at:

```text
.aida-store/objects/<TYPE>/000/<SPEC-ID>.yaml
```

Typical top-level fields:

- `id`: stable UUID.
- `spec_id`: human-facing ID, for example `SPIKE-24`.
- `agreed_id`: optional alias/canonical agreed ID.
- `title`, `description`.
- `status`, `priority`, `req_type`.
- `created_at`, `modified_at`.
- `tags`, `relationships`, `comments`, `history`, `implementation_info`,
  archive fields, and other optional fields when present.

The SQLite cache (`.aida/cache.db`) is not canonical. It is a rebuildable read
projection. A non-AIDA writer should not write the cache directly.

## Minimum direct-read contract

A non-AIDA reader must:

1. Walk `.aida-store/objects/**/*.yaml`.
2. Parse YAML while accepting AIDA's `RelationshipType::Custom` external tag
   shape, e.g. `!Custom related`.
3. Treat timestamps as strings, not as lossy language-native datetime objects.
4. Treat unknown optional fields as data to preserve or ignore, not as parse
   failures.
5. Resolve relationships by UUID target ids unless it also builds an ID/alias
   lookup table.
6. Never infer project truth from `.aida/cache.db` unless it has verified cache
   freshness against the store head.

This contract is already practical. The SPIKE-46 read prototype parsed the live
store corpus.

## Minimum direct-write contract: status flip

The smallest safe write is an operation-shaped status flip on an existing spec.
A direct writer must:

1. Acquire an interprocess lock for the store/write operation. The current store
   branch is shared by multiple agents; concurrent uncoordinated writes can
   produce lost updates or divergent commits.
2. Read the target object from `.aida-store/objects/<TYPE>/000/<SPEC-ID>.yaml`.
3. Preserve unrelated bytes/fields as much as possible. Do not generic
   read-then-reemit the whole object unless the emitter has passed the
   byte-identical conformance corpus.
4. Update `status` to a valid AIDA status spelling.
5. Update `modified_at` using AIDA's RFC3339 nanosecond `Z` timestamp style.
6. Append exactly one `history` entry if the object already carries history, or
   insert a correctly shaped `history` array if the operation contract requires
   audit history for that mutation:

```yaml
history:
- id: <uuid-v7-or-compatible-id>
  author: <external-writer-id>
  timestamp: <same timestamp style>
  changes:
  - field_name: status
    old_value: <old status>
    new_value: <new status>
```

7. Stage the changed object in `.aida-store`.
8. Commit on the orphan `aida-store` branch with a message that names the spec
   and operation.
9. Do not edit `.aida/cache.db`. Let AIDA detect staleness and rebuild, or run a
   cache rebuild through AIDA after the store commit.

This operation should be validated by:

- re-parsing the object with the non-AIDA loader,
- re-parsing it with AIDA,
- confirming only the intended semantic diff exists,
- confirming `aida show <SPEC>` sees the new status after cache refresh.

## Minimum direct-write contract: new spec

Creating a new object is harder than flipping status because ID allocation and
metadata updates are involved. A direct writer must:

1. Acquire the same store/write lock.
2. Read and update `.aida-store/metadata.yaml` or whatever dispenser/counter
   state is current for the target AIDA version.
3. Derive the canonical prefix from `req_type`, not from caller-provided text:
   `Task -> TASK`, `Bug -> BUG`, `Spike -> SPIKE`, etc.
4. Allocate the next ID without racing another writer.
5. Generate a stable UUID for `id`.
6. Emit all required top-level fields in AIDA's deterministic field order.
7. Omit absent optional fields rather than writing `null`.
8. Sort tags and custom map keys.
9. Preserve AIDA's custom relationship encoding if relationships are present.
10. Write the object at the shard path derived from the new `spec_id`.
11. Stage both the object and metadata/counter file.
12. Commit on the orphan `aida-store` branch.
13. Leave cache rebuild to AIDA or explicitly invoke a cache rebuild after the
    commit.

Until this is formally specified and tested, external new-spec creation should
prefer MCP `add_requirement` or the AIDA CLI. Those paths already own prefix
selection, counter updates, cache behavior, and commit discipline.

## Gap list

1. **No published schema version in requirement YAML.** Compatibility depends on
   serde defaults and field omission conventions, not a visible format version.
2. **No standalone write-conformance spec.** `on-disk-serialization-surface.md`
   inventories hazards, but external writers still need a normative spec plus
   examples.
3. **Generic YAML emitters are unsafe.** SPIKE-46 proved semantic round-trips
   can still be zero-for-corpus byte-identical because emitters rewrite
   timestamp/scalar presentation.
4. **ID allocation is under-specified for external writers.** Prefix mapping,
   counters, dispenser behavior, and metadata commits need a public contract.
5. **History expectations are mixed.** Some objects contain `history`; many rely
   on orphan-branch git history. External writers need a rule for when to append
   object-level history versus relying on git commit history.
6. **Store lock semantics are not a public API.** Multi-agent direct writes need
   a lock file or transactional protocol; "just edit YAML" is not enough.
7. **Cache invalidation is implicit.** The cache is rebuildable, but external
   writers need explicit guidance: never write cache; commit store; then rebuild
   or let AIDA detect stale state.
8. **Relationship encoding needs a reference adapter.** The `!Custom` tag is a
   small read issue, but every vendor will rediscover it without a tiny SDK or
   documented loader snippet.

## Recommended minimal interop contract

Publish three tiers:

### Tier 1: Direct read, supported

External tools may read `.aida-store/objects/**/*.yaml` directly. AIDA should
provide:

- a concise schema inventory,
- a Python reference loader for `!Custom` relationship tags,
- examples for resolving `spec_id`, `agreed_id`, UUID, status, tags, and
  relationships.

### Tier 2: Bounded direct writes, experimental

External tools may implement specific operation contracts, starting with:

- status flip,
- add comment,
- add relationship.

Each operation must have:

- required input fields,
- exact YAML mutation semantics,
- history behavior,
- lock behavior,
- cache behavior,
- conformance tests against a fixture corpus.

### Tier 3: Generic object creation/update, not yet public

Do not bless generic external creation/update until AIDA publishes:

- ID allocation/dispenser spec,
- deterministic YAML emitter spec,
- store locking protocol,
- cache refresh contract,
- corpus conformance harness.

For now, use MCP/CLI for generic writes.

## Architecture recommendation

The right multi-vendor story is:

> **Open to read; conformant to write; MCP/CLI as the default writer until a
> formal write spec exists.**

That is stronger than claiming every vendor can safely edit YAML today. It makes
the actual moat precise: AIDA's value is not hiding the graph, but preserving
write correctness, lifecycle semantics, and auditability while keeping read
access trivial.

## Follow-up specs to file or verify

1. Promote the SPIKE-46 prototype into a maintained `docs/architecture/store-interop/`
   reference package or test fixture.
2. Add a direct-read conformance fixture with the observed relationship variants:
   `BlockedBy`, `Child`, `Parent`, `References`, and `Custom:*`.
3. Add a status-flip conformance fixture that proves a non-AIDA writer can
   mutate a temp object and AIDA can read it.
4. File a design task for the public direct-write lock protocol.
5. File a design task for explicit object schema versioning, or document why
   serde-default structural compatibility remains the chosen strategy.
