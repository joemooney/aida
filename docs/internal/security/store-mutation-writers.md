# Store mutation writer inventory and review ratchet

<!-- trace:TASK-3222 | ai:codex -->

TASK-3222 is STORY-2561 **S1a only**, following the advisor's 2026-11-06
07:10 MST split and 15:31 UTC execution-mode ruling. The inventory and guard
change no runtime, permission, role, launch, IPC or writer behavior. Current
store grants and Codex native compatibility labeling remain unchanged.
Transport, identity and lifecycle belong to TASK-3225; mutation routing and
raw-store denial belong to TASK-3223; real-vendor evidence belongs to TASK-3224.
A passing inventory guard is neither C2 evidence nor permission to enable a
mutation verb. Per the advisor's 2026-11-06 20:29 UTC ruling, this inventory is
a best-effort static ratchet; completeness is not a merge gate. TASK-3223's
shared boundary denying raw store writes is the enforcement boundary.
The canonical redo transaction design was subsequently approved in
[the S2 sketch](../../plans/2026-11-06-task-3223-mediated-transactions.md).

## Staged lane routing

<!-- trace:TASK-3223 | ai:codex -->

`store_mutation_entry.rs` selects an explicit mediated profile before CLI
identity, telemetry, aliases, extension dispatch and cache setup. The bounded
Rust SEQPACKET client supports inline `comment add` and title/description-only
`edit`. Its separate MCP loop translates `add_comment` and `update_requirement`
without backend setup, agent-registry writes, mail acknowledgements or respawn.
It retains MCP profile and stakeholder restrictions, narrows advertised input
properties, and refuses extra fields rather than silently dropping semantics.
Unsupported commands/tools refuse before ordinary dispatch. Neither transport
falls back to direct persistence or retries uncertain delivery with a new UUID.

This is staged routing, **not production admission or completed S2a evidence**.
The launcher and host mutation verbs remain gated. The inventory still requires
operation-by-operation reconciliation; the early refusal boundary does not
reclassify its unresolved rows. Remaining work includes the other CLI/MCP
writer families, host publication integration, contained-Codex profile selection
and the live same-lane raw-write-denial/mediated-comment positive. `own_claim`
and `queue_done` remain unsupported under TASK-3326. Native compatibility and
Claude launch selection are unchanged.

## Checked-in contract

[`scripts/store-writers.toml`](../../../scripts/store-writers.toml) is review
evidence, **not an allow-list** and not a second requirements database.
Version 2 keys each writer row by **path + enclosing item + sink kind**.
There are no file hashes or source-revision pins. Line numbers, comments,
whitespace, arguments, and edits elsewhere in a file do not change site identity.
Repeated calls of the same kind in one item share a site, per the advisor's
2026-11-06 16:22 UTC ruling. A new kind or enclosing item needs a new row;
a removed site makes its row stale. Policy schema validation remains strict.

| Field | Meaning |
|---|---|
| `id`, `path`, `item`, `sink` | Stable site identity; item includes enclosing Rust module/impl/function, Python class/function, or shell function (`<script>` only at top level) |
| `lock`, `gate` | Observed lock and operation policy, or explicit caller-owned/unresolved cutover requirement |
| `disposition`, `blocker` | Reviewed disposition and outstanding TASK-3223 work; no present authority |
| `proposed_operation` | Candidate typed operation or explicit unresolved/refusal |
| `evidence_tests` | Reproduction fixtures or operation-specific evidence to preserve |

The `operation` array separately accounts for each registered CLI command and
each dispatched MCP tool, and every immediate operation in the comment, queue,
findings, mailbox and session command enums. Concrete mappings include `comment
add`, `queue done`, `findings add`, `session handoff`, `session handoff standing-orders add` and `mailbox send/digest`.
Each row names qualified entrypoints and references actual sink-row IDs.
Empty `sites` means unresolved, **never** read-only. Even nonempty mappings
remain blocked for S2 until receiver/trait, alias, legacy dispatch and deferred
child edges and host authority are reviewed. Per-operation lock/gate notes
preserve the concrete known routes for spec edits, comments, claims, completion,
findings and mail. The inventory does not forward generic CLI/MCP requests.
Each operation also carries a checked `resolution_note` explaining its route or
its specific unresolved entrypoint. MCP queue add/rework include their reviewed
inner receiver handlers, and punt resolve/escalate include their imported core
handlers. MCP queue work, db sync, pull, session start/end and role enter/end
are PEEK tools returning CLI commands; their rows explicitly retain the CLI
cutover blocker. `queue done` selects sinks and qualified calls from the concrete
`QueueCommand::Done` arm, rather than all arms of the dispatcher. Its sink rows
still use the enclosing function identity required by the inventory contract.

`defer resurface` has a separate blocked operation row for the reviewed
`run_defer -> defer_resurface::run -> sweep` route. Its explicit entrypoints
bridge the bare `sweep` call, which qualified lexical propagation cannot resolve.
The row includes both `update_spec_atomically` and `queue_add`; TASK-3223 must
review the spec update and human-queue authority before routing this operation.

Print unresolved operations without touching a live store:

```bash
python3 - <<'PY'
import tomllib
with open('scripts/store-writers.toml', 'rb') as handle:
    rows = tomllib.load(handle)['operation']
for row in rows:
    print(row['id'], len(row['sites']), row['blocker'], sep=' | ')
PY
```

For the TASK-3223 routing slice, run
`python3 scripts/check-store-writers.py --profile mediated`. It first checks
source reconciliation, then prints JSON listing unresolved writer sites and
operation IDs and exits nonzero while any remain. The default compatibility
check still validates inventory reconciliation only. Neither result grants
launch authority. Version 2 requires every operation mapping to remain blocked.
Only `host_only` and `lane_private` writer rows are excluded from unresolved
counts. The compatibility labels `s2_typed` and `read_only` remain unresolved
under the mediated profile: v2 does not verify evidence for either label.
Every operation mapping remains unresolved regardless of its disposition.
The routing implementation must introduce reviewed operation policy and evidence
before this profile check can certify zero unresolved mappings. This diagnostic
does not implement routing, prove runtime coverage, or exempt TASK-3326 writers
from their existing blockers.
<!-- trace:TASK-3223 | ai:codex -->

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
Registration and MCP dispatch selectors are reconciled separately from sinks;
unresolved selectors are explicit operation blockers.

```bash
rg -n 'update_atomically|update_spec_atomically|add_requirement_atomic|save_reporting|write_object|with_store_write_lock|\.save\(' aida-core/src aida-cli-lib/src aida-cli/src aida-server/src aida-tui/src
rg -n 'fs::(write|rename|remove_file|create_dir_all|copy)|OpenOptions|File::create|write_atomic|write_all|execute_batch|transaction|queue_(add|remove|reorder)' aida-core/src aida-cli-lib/src aida-cli/src aida-server/src aida-tui/src
rg -n 'write_cas|write_batch|register_node|acquire_claim|release_claim|digest_snapshot_to_canonical|publish_to_store|commit_record|git_ops::(add|commit|push)|pull_rebase|fast_forward_store' aida-core/src aida-cli-lib/src
rg -n 'REGISTRY|CmdPolicy|Command::|tool_[a-z_]+|handoff|mailbox|deputy|claim|cache|migrat' aida-cli-lib/src/command_registry.rs aida-cli-lib/src/cli.rs aida-cli-lib/src/lib.rs aida-cli-lib/src/mcp.rs
rg -n 'write_text|write_bytes|write\(|rename|replace|unlink|open\(|git (add|commit|push)|aida (add|edit|comment|queue)' scripts aida-core/templates aida-cli-lib/src/containment
python3 scripts/check-store-writers.py --candidates > /tmp/store-writer-candidates.json
python3 scripts/check-store-writers.py
python3 -m unittest tests/test_store_writer_inventory.py
python3 scripts/check-ci-gates.py --audit
```

The scanner discovers `*.rs`, `*.py` and `*.sh` in workspace-style
`aida-*/src` roots, and `*.py`/`*.sh` under `scripts`, `aida-core/templates`
and optional `hooks`/`.githooks` roots. This includes shell hook templates
ending in `.sh` and terminal Python templates, but excludes extensionless
hooks. New crate roots are discovered automatically. The test-name and
directory exclusions are listed under Limits below. Only exact `#[cfg(test)]`
Rust function/module items are masked, preserving production after inline tests.

Rust comments and string contents are masked before calls are matched. Wrapper
propagation uses qualified function identities, including enclosing impl types,
and reaches a fixed point only through actual bodies containing sinks or calls
to classified wrappers. It never promotes a bare method name: `path.join`,
`display`, `is_empty`, and unrelated free functions with colliding names do not
become writers. Explicit `crate::writer::add` remains detectable. Unresolved
imports, bare free-function calls, receiver methods and trait dispatch require
S2 reconciliation; the scanner does not guess their target. Python uses AST
call nodes; shell uses lexical tokens with comments/quoted payloads opaque.
Shell function forms `name() {`, `function name {` and `function name() {`
have separate site identities. Redirects to `/dev/null` do not create writer
sites. Shell/Python argv classification skips global flags (including value
arguments for known Git/AIDA options) before finding the subcommand. Literal
executables and shell variables named `AIDA`/`GIT` or with uppercase suffixes
such as `AIDA_BIN` are candidates. AIDA uses a read-only allowlist of `help`
and `version`; every other subcommand, including unknown commands, is a
conservative writer candidate because even apparent reads can refresh caches.
Shell scanning also inspects literal `sh -c`/`bash -c` payloads and lexical
command substitutions. Python distinguishes `Path.open(mode)` from
`open(path, mode)` and includes remove/touch/move/copy, literal argv subprocess
writes through git/aida, and literal command strings with `shell=True` or
`os.system` parsed through `shlex`.

After the host merges **current main**, regenerate against that merged tree:

```bash
python3 scripts/check-store-writers.py --regenerate
git diff -- scripts/store-writers.toml
python3 scripts/check-store-writers.py
python3 -m unittest tests/test_store_writer_inventory.py
make check-ci-fast
```

Regeneration retains existing site and operation policy, adds newly discovered
sites as **blocked**, removes stale sites, and refreshes operation-to-site
references. Review the additions, removals and mapping changes before committing.
It grants no routing or cutover authority. The failure diagnostic includes this
recipe; concurrent main changes require regeneration after merging, rather than
hand-copying a digest. Unrelated edits produce no inventory diff.

## Limits and refusal tests

This is a source-review ratchet, not a Rust/Python/shell compiler or a security
boundary. It cannot prove destination resolution, alias/macro/function-pointer
call graphs, dynamic command construction, generated/extension code behavior,
foreign-language I/O, or every implicit writer. Direct seed APIs remain conservative lexical candidates, including non-store
output writes; qualified wrappers do not use same-name receiver inference. A novel write API outside the seed vocabulary needs scanner
and inventory review; presence of a path/row does not settle that question.
These limits require manual reconciliation in TASK-3223 before claiming
raw-store denial; they do not authorize a bypass or make static completeness
a merge gate for this inventory. Shell heredocs, expansion and dynamic command construction still require
manual review; lexical tokens do not establish an executable shell call graph. No lane code/config/hooks are run
by this audit.

Explicit discovery and Python limits accepted by the advisor:

- Discovery skips directories named `test`, `tests`, `fixtures` and
  `__pycache__`, files named `tests.rs`, every file starting with `test_`, and
  files ending in `_tests.rs`. The filename skip applies even without
  `#[cfg(test)]`; a production module using such a name can escape the ratchet.
- Extensionless executable hooks, including
  `aida-core/templates/hooks/aida-commit-msg`, are not scanned. A shebang does
  not add them to discovery.
- Python subprocess calls through imported aliases such as
  `from subprocess import run; run(...)`, non-literal argv/executable values
  such as `[AIDA, 'edit', spec]`, and filesystem APIs such as `os.symlink`
  and `shutil.copytree` are not covered. String-command coverage is limited
  to literal `subprocess.*(..., shell=True)` and `os.system(...)` strings
  parsed with `shlex`; this does not interpret shell operators or expansion.

**Named Rust Git-plumbing cutover blocker (TASK-3223):** the Rust seed scan
does not inspect subprocess argument strings. Raw `git(&["update-ref", ...])`,
`commit-tree`, `hash-object -w`, `push`, and `commit`, or
`Command::new("git"/"aida")` writes may therefore have no sink row when the
enclosing item has no other seed. In particular, manually reconcile
`aida-core/src/git_ops.rs::fetch_rebase_target` and all raw Git/AIDA command
constructors before cutover. An incidental lock/helper row does not establish
coverage of that subprocess. This is the explicit blocker alternative accepted
by round-3 finding 5; it must be closed before claiming raw-store denial.

`tests/test_store_writer_inventory.py` uses disposable source trees. Required
mutants each refuse **before** classification and pass only after an explicit
fixture inventory update: a raw filesystem writer, a caller of an existing
writer wrapper, and a cache-on-read SQLite writer. Additional tests cover
transitive qualified wrappers, bare-name collisions (negative), generic writer
names (positive), real hook templates, unrelated edits (negative), new crates,
new sink kinds inserted in an existing item,
missing sink/lock/gate/disposition, stale/duplicate rows, source symlinks,
comments/raw strings, production following inline tests, Python/shell writes,
and actual CI fast-tier wiring.
Round-3 fixtures additionally cover findings/session hook commands, shell
function identity with an existing null redirect, Path.open write modes,
Python subprocess/filesystem writers, explicit MCP inner edges, and concrete
queue-done arm selection. The three original refusal mutants remain intact.
Round-6 fixtures cover global Git/AIDA flags, variable executables, done/pull
and unknown AIDA verbs, Python literal shell strings, and the concrete
multi-clone harness variable commands and `sh -c` Git writes at lines 429-433.

The CI Build job runs the standalone guard and mutation fixtures. Both steps
are `fast` in `scripts/ci-gate-tiers.toml`; `make check-ci-fast` derives them
through `scripts/check-ci-gates.py`, without a second command list in Makefile.
The LIGHT gate is the Python fixtures plus `make check-ci-fast`; its
workflow-derived command list remains the source of truth. Report the actual
aggregate result and any environment refusal. A local pass is not a claim that
the remote PR Gates shard has run at the unpublished head.

Runtime malformed/oversized/path-bearing request, expired/revoked/sibling
identity, endpoint replay/replacement and absent-service tests belong to S1b
(TASK-3225). Persistence-before-reply crash recovery, stale revisions and
mutation concurrency belong to S2, after transaction approval. Raw terminal
AND file-tool create/append/rename negatives, with parent-observed unchanged
sentinels and host-mediated positive controls, remain S2/S3 evidence. None is
claimed by these static fixtures.
