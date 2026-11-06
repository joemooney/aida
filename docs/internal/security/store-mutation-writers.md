# Store mutation writer inventory and review ratchet

<!-- trace:TASK-3222 | ai:codex -->

TASK-3222 is STORY-2561 **S1a only**, following the advisor's 2026-11-06
07:10 MST split and 15:31 UTC execution-mode ruling. The inventory and guard
change no runtime, permission, role, launch, IPC or writer behavior. Current
store grants and Codex native compatibility labeling remain unchanged.
Transport, identity and lifecycle belong to TASK-3225; mutation routing and
raw-store denial belong to TASK-3223; real-vendor evidence belongs to TASK-3224.
A passing inventory guard is neither C2 evidence nor permission to enable a
mutation verb. S2 still needs separate approval for the recoverable transaction
that couples mutation and dedupe outcome.

## Checked-in contract

[`scripts/store-writers.toml`](../../../scripts/store-writers.toml) is review
evidence, **not an allow-list** and not a second requirements database.
`source_revision` is the audited code commit, not a moving branch or a promise
that the inventory applies to every later commit. Version 1 has deterministic,
path-sorted `[[writer]]` records. Each record contains:

| Field | Meaning |
|---|---|
| `id`, `path` | Stable repository-relative source identity; no absolute or parent paths |
| `surface`, `selector` | CLI/MCP/internal/script surface or explicit unresolved candidate |
| `entrypoints` | Sorted source function names containing candidate sinks or wrapper calls; `<module/script>` for module-level/script evidence |
| `sink` | Sorted concrete lexical sink/wrapper names in that file, or explicit dispatch-only marker; qualify names with `path` |
| `source_sha256` | Hash of production source evidence, including destination strings, arguments, dispatch and comments |
| `artifacts` | Canonical objects, metadata, authority, cache, local coordination, store Git, external output, or explicitly unresolved |
| `lock` | Observed locking route, caller-held requirement, or explicit unresolved blocker; never inferred merely from atomic rename |
| `gate` | Observed operation policy or explicit unresolved blocker; a persistence helper does not authenticate a lane |
| `disposition` | `s2_typed`, `host_only`, `lane_private`, `read_only`, or `blocked`; none grants present authority |
| `blocker`, `refusal`, `cutover_owner` | Explicit cutover work/refusal, owned by TASK-3223 |
| `evidence` | Audit/source and reproduction references |

Records group evidence by source file rather than pretending one mixed handler
has a single destination or authority rule. Mixed surfaces remain blocked until
S2 separates operations and destinations. `entrypoints` and `sink` describe
source candidates, not a resolved Rust call graph. The broad candidate set
intentionally retains false positives (for example unrelated output writes,
read-only `OpenOptions`, or same-named methods). Boundary families below have
source-checked notes; every other record explicitly says its destination, lock
and gate are unresolved. Do not interpret the candidate count as the number of
confirmed canonical writers, or the existence of a row as completed policy review.

Every `blocked` record's `blocker` is an outstanding cutover blocker. Host-only
records also carry preservation requirements. The complete, file-addressed list
is the TOML itself; print it without running AIDA or touching a live store:

```bash
python3 - <<'PY'
import tomllib
with open('scripts/store-writers.toml', 'rb') as handle:
    rows = tomllib.load(handle)['writer']
for row in rows:
    print(row['path'], row['disposition'], row['blocker'], sep=' | ')
PY
```

## Source-checked boundary families

These are entry seams for review, not new authorization rules. The older
[store writer/lock inventory](../../plans/2026-10-19-store-writer-inventory.md)
retains its lock-history role; its old hit counts are not reused as current
completeness evidence.

| Surface / concrete functions | Persistence and locking evidence | S2 cutover requirement |
|---|---|---|
| CLI `requirements_cli.rs::run_add/run_edit`, `edit_rebase.rs::write_edit_atomically` | Cached backend atomic writers; edit re-reads/rebases and runs its protected-spec callback and requeue check inside `update_spec_atomically_with_subject` | B05: split typed fields, preserve protection/status/concurrency rules and provenance |
| CLI `comment_cmd.rs::add_comment_cli_relayed`, interactive/edit/delete comment paths | `Storage::save`, with backend-specific canonical persistence | B05: cover every route; no whole-store replacement or duplicate comments after uncertain delivery |
| MCP `mcp.rs::tool_add_requirement/tool_update_requirement/tool_add_comment` | Storage facade, including `tool_add_comment`'s load/change/save path | B06: individual bounded tool schemas and live lane policy; no generic MCP forwarding |
| MCP `mcp.rs::tool_claim_task` | Local `.aida/sessions` lease; spec-keyed `OpenOptions::create_new` is its serialization point | B06: distinguish advisory lease creation from role authority; not a canonical-object transaction |
| CLI `queue_cmd.rs::handle_queue_command`, MCP `tool_queue_done` | Spec status and backend queues, plus role queues under `global_queue.rs` with permanent lock sidecars and atomic YAML replacement | B07: preserve ownership/PR evidence and override ledger; no status-only bypass |
| CLI `findings_gate.rs::handle_findings_add`, MCP `tool_file_finding`, automatic failure/handoff helpers | Atomic spec filing plus queue insertion and provenance, not only a function named `handoff` | B08: both filing and routing must succeed under their own rules; include `tests/test_gitlab_review_handoff.py` in later validation |
| CLI/MCP mail: `mailbox_store.rs::write_message_with_deliveries`, `digest_snapshot_to_canonical`, `write_snapshot_locked`; core `mailbox.rs::write_local_message_with_deliveries` | Local atomic messages/delivery sidecars; canonical publication requires `CanonicalPermit` then store write lock | B09: local-to-canonical bridge, ack/delivery/retirement and send/mention routes are not a generic service spool |
| Core `storage.rs`, `db/git_backend.rs::save_reporting_with_origin/update_atomically_tracked`, `db/cached_git_backend.rs` | Canonical backend store lock; cached facade also updates derived SQLite; queue ledger routes differ | B01: preserve hypothesis/evidence/provenance rules; caller authority is not supplied by backend persistence |
| Core `object_store.rs::write_object/write_object_if_changed/delete_object`, `fs_atomic.rs::write_atomic/write_config_atomic` | Generic file/whole-object helpers; store lock and destination policy are caller responsibilities | B02: host implementation details only, never arbitrary-path service verbs |
| Core `db/cache.rs::rebuild_from_store`, `db/cached_git_backend.rs::rebuild_cache`, refresh worker/request and migrations | SQLite transaction/write locking, refresh single-flight flock and advisory lock-info sidecars | B03: cache-on-read is a writer; choose host shared cache or genuinely lane-private derived state for every route |
| Legacy SQLite/Postgres/YAML backend dispatch and migration | Different SQL/file locking, not uniformly the Git store lock | B04: explicit supported routing or refusal; no legacy backend escape |
| `git_ops.rs`, `git_ops/evidence.rs`, CLI `store_sync.rs`, `child_store_commit.rs`, `store_push_pull.rs` | Store lock for reconciliation/commit, Git maintenance locks, push outside store writer lock, evidence refusals | B10: remain host-owned; no lane-selected hooks/config/argv or direct child publication |
| Core `node.rs`, `dispenser.rs`, `registry.rs` | Store lock before block dispenser lock, CAS registry/claim operations | B11: retain clone-local/HOME/direct-human authority and lock order; ID traces grant no authority |

B12 is the explicit unresolved class for remaining candidates, including
launch/lease/grant/seat and reviewer ledgers, orchestrator phases, doctor/init/
import/sync, history/regen writers, intake's separate repository, deputy/control
channels, hooks, REST/TUI and script consumers. Resolve their destinations and
callers, or prove unrelated/read-only, before cutover. A claim that all B12 rows
are lane-private or safe would be incorrect. Shared mail/deputy/coordination
state must never become an alternate general store-write channel.

## Reproducing the audit

Start with independent searches, then reconcile **both directions**: every
mutating surface to its sinks, and every sink to its callers/surfaces. Search
command registration, legacy dispatch and aliases, MCP tool registration AND
handler dispatch; include operations presented as reads that create, refresh or
migrate derived state. Follow traits, wrappers, macros and deferred children.
The guard pins `command_registry.rs`, `cli.rs`, `lib.rs` and CLI `main.rs`
even when lexical write discovery finds no sink there.

```bash
rg -n 'update_atomically|update_spec_atomically|add_requirement_atomic|save_reporting|write_object|with_store_write_lock|\.save\(' aida-core/src aida-cli-lib/src aida-cli/src aida-server/src aida-tui/src
rg -n 'fs::(write|rename|remove_file|create_dir_all|copy)|OpenOptions|File::create|write_atomic|write_all|execute_batch|transaction|queue_(add|remove|reorder)' aida-core/src aida-cli-lib/src aida-cli/src aida-server/src aida-tui/src
rg -n 'write_cas|write_batch|register_node|acquire_claim|release_claim|digest_snapshot_to_canonical|publish_to_store|commit_record|git_ops::(add|commit|push)|pull_rebase|fast_forward_store' aida-core/src aida-cli-lib/src
rg -n 'REGISTRY|CmdPolicy|Command::|tool_[a-z_]+|handoff|mailbox|deputy|claim|cache|migrat' aida-cli-lib/src/command_registry.rs aida-cli-lib/src/cli.rs aida-cli-lib/src/lib.rs aida-cli-lib/src/mcp.rs
rg -n 'write_text|write_bytes|write\(|rename|replace|unlink|open\(|git (add|commit|push)|aida (add|edit|comment|queue)' scripts aida-cli-lib/src/containment
python3 scripts/check-store-writers.py --candidates > /tmp/store-writer-candidates.json
python3 scripts/check-store-writers.py
python3 -m unittest tests/test_store_writer_inventory.py
python3 scripts/check-ci-gates.py --audit
```

The scanner covers Rust/Python/shell files in the workspace-style `aida-*/src`
roots, plus `scripts`, `hooks` and `.githooks`, including Python containment
helpers. New `aida-*/src` roots are discovered automatically. Explicit test/
fixture directories, `tests.rs`, `test_*` and `*_tests.rs` files are excluded.
In mixed Rust files only exact `#[cfg(test)]` function/module items are masked;
production after inline tests remains scanned. Complex test cfg expressions,
test fields/arms and ambiguous files are retained conservatively. The scanner
itself is exempt: it reads source and TOML and prints diagnostics/candidates;
its behavior and CI wiring have independent fixtures. Candidate printing does
not rewrite or bless inventory records.

For Rust discovery, comments and string contents are masked before matching
calls; destination strings remain in the hashed production evidence. Writer
wrapper names are learned from current source and propagated to callers to a
fixed point, so adding a caller in an uninventoried file is checked as well as
changing an existing writer. Ambiguous common names such as `new`, `run`,
`collect` and `push` are not propagated; direct persistence seeds are still
matched. A changed candidate file requires evidence review even if the change
looks unrelated. This intentionally conservative hash also notices production
comments/whitespace, and changes to the masked span's length can require refresh.

For each change, inspect the changed functions, destination construction,
operation policy, lock, wrapper/trait implementations and both CLI/MCP callers.
Update the affected record's classification and source evidence explicitly;
remove retired records. `--candidates` is only an aid to that review. There is
no baseline rewrite/auto-approval flag. Review the inventory diff alongside the
code; never update only a digest to make the guard green.

## Limits and refusal tests

This is a source-review ratchet, not a Rust/Python/shell compiler or a security
boundary. It cannot prove destination resolution, alias/macro/function-pointer
call graphs, dynamic command construction, generated/extension code behavior,
foreign-language I/O, or every implicit writer. Same-named methods may produce
false positives. A novel write API outside the seed vocabulary needs scanner
and inventory review; presence of a path/row does not settle that question.
These are S2 cutover blockers requiring manual reconciliation, not exclusions
that authorize a bypass. Scripts are intentionally scanned conservatively as
text, including string/comment candidates. No lane code/config/hooks are run
by this audit.

`tests/test_store_writer_inventory.py` uses disposable source trees. Required
mutants each refuse **before** classification and pass only after an explicit
fixture inventory update: a raw filesystem writer, a caller of an existing
writer wrapper, and a cache-on-read SQLite writer. Additional tests cover
transitive wrappers, new crates, writes inserted in an existing record,
missing sink/lock/gate/disposition, stale/duplicate rows, source symlinks,
comments/raw strings, production following inline tests, Python/shell writes,
and actual CI fast-tier wiring.

The CI Build job runs the standalone guard and mutation fixtures. Both steps
are `fast` in `scripts/ci-gate-tiers.toml`; `make check-ci-fast` derives them
through `scripts/check-ci-gates.py`, without a second command list in Makefile.
At this audit revision that aggregate also includes `cargo check -p aida-cli
--no-default-features` and formatting. A no-Cargo lane can run the targeted
Python checks, gate audit and dry-run the Make target, but cannot claim the
whole aggregate passed. Host/CI must run the remaining aggregate checks.

Runtime malformed/oversized/path-bearing request, expired/revoked/sibling
identity, endpoint replay/replacement and absent-service tests belong to S1b
(TASK-3225). Persistence-before-reply crash recovery, stale revisions and
mutation concurrency belong to S2, after transaction approval. Raw terminal
AND file-tool create/append/rename negatives, with parent-observed unchanged
sentinels and host-mediated positive controls, remain S2/S3 evidence. None is
claimed by these static fixtures.
