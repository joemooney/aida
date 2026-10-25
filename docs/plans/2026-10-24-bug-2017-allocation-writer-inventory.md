# BUG-2017 slice 1: allocation admission inventory

<!-- trace:BUG-2017 | ai:codex -->

The signed SKETCH v2 and advisor acceptance override replace the original grant
proposal. This slice introduces the common-Git-directory authority setting and
shared allocation/persistence admission. It does not enroll the operator's clone.
Pseudonymous bootstrap and home canonical promotion belong to slice 2; full
reconciliation conflict preflight/audit and scanner integration belong to slice 3.
No transport profile or scanner files are changed. Exact shared-method ownership
was recorded in canonical BUG-2017 and STORY-1742 comments before resumed edits.
BUG-2017 owns model/dispenser/node/block and backend candidate-ID/allocation
admission methods; STORY-1742 consumes the admitted candidate plan later. A live
attached dispenser persists a sequence reservation during candidate assembly,
including `BulkWriter::add`; it is not a pure serialization preview. The later
content gate must reject unsafe candidates before counters/oplog/objects/refs,
without taking an outer transport-policy lock inside an existing store lock.

The local file `aida-allocation.toml` in the common Git directory contains exactly
`version = 1` and `authoritative = true|false`. Missing means node-local only;
malformed, linked, oversized, or shared-writable settings refuse. Ordinary clones
never inherit this file. `aida node team --format json` inspects it without mutation.
This slice intentionally offers no enrollment command. Only synthetic test
repositories receive an authoritative setting during validation.

## Writer checklist

- [x] CLI add (`requirements_cli.rs`): resolve effective policy before pre-pull,
  block refill, dispense, and object writes. WORK forces node-aware-only;
  explicit blocks-only refuses. No new global agreed ID can reach persistence.
- [x] MCP ordinary add, finding/punt/review/schedule filing: model/dispenser
  admission before allocation, then GitBackend admission. The tool name is not
  an authority boundary. Backend tests exercise these same model operations;
  a candidate-binary MCP fixture verifies actual stdio behavior.
- [x] Models (`models.rs`): legacy assign/peek and an attached dispenser's error never fall back to
  internal counters. Distributed two-level generation refuses. Legacy add and
  meta allocation use the dispenser. Infallible legacy APIs return an invalid
  empty identity on refusal; GitBackend refuses it before persistence.
- [x] Backend direct add, full save/import, atomic closure, bulk add/finish and
  collision reassignment (`db/git_backend.rs`): load clone policy and disk UUID
  identities independently of caller state. New global spec/agreed IDs refuse;
  existing UUID/global-ID edits remain valid. Internal global counter mutations
  refuse on WORK, including repair/reset/import paths.
- [x] CachedGitBackend and Storage: directory-backed operations delegate to the
  guarded GitBackend. An unattached model or centralized source cannot bypass
  the destination's admission by saving/importing into a distributed store.
- [x] Core object writers (`object_store.rs`): direct writes inside a clone use
  the same admission. File-only exports outside Git retain serialization behavior.
- [x] Block allocation (`db_block.rs`): explicit claims and the shared auto-claim
  inner function require local authority before network or registry mutation.
  Initial allocation and implicit capacity/first-block helpers skip on WORK.
- [x] Low-level BlockRegistry persistence (`node.rs`): Within a Git clone WORK cannot persist block
  claims or advance imported block counters. In-memory registry construction is
  not authority; serialized registries do not carry an authority override.
- [x] File and SQLite dispensers: clone mode/binding admission before opening
  state or spending counters, regardless of filename; aliased destinations are
  resolved before clone discovery. Independent centralized database backends
  retain their own allocation behavior.
- [x] Legacy node acquire (`node_cmd.rs`): refuses on WORK before CAS/global
  registration; slice 2 supplies pseudonymous WORK bootstrap. Existing explicit
  node bindings are required for slice-1 node-local allocation.
- [x] Dispenser loader (`rel_cards.rs`): WORK never assumes default node 1.
  Backend-installed clone dispenser checks explicit binding and supplied dispenser
  mode, including stale centralized or other-node dispensers. Missing/broken
  binding/dispenser refuses, without global fallback.
- [x] Doctor block writers: all block saves reach the guarded BlockRegistry;
  counter/ID migration through Storage/GitBackend reaches metadata and identity
  admission. Unsupported global repair on WORK refuses.
- [x] Server REST/gRPC/skill runner and doc/report/regen/init scaffolding: reach
  model + Storage/GitBackend or guarded object-file writes. This slice adds no
  exception for an internal caller or generated requirement.
- [x] Core merge_gate: explicit global canonical promotion requires local authority
  before lock/counter/object/ref changes. Slice 2 integrates automatic caller
  behavior and verifies WORK-to-HOME alias-preserving promotion end to end.
- [ ] All pull/rebase/fast-forward/union preflights and read-only audit: slice 3.
- [ ] Pseudonymous UUID binding without identifying fields: slice 2. Existing
  binding schema is preserved in slice 1; no automatic legacy registration on WORK.

## Validation plan

Target core policy tests use isolated synthetic WORK/HOME repositories. Cover
missing/malformed settings, shared fake settings, direct/bulk local IDs, explicit
bare IDs and aliases, full save/import/atomic repair, missing/broken dispensers,
imported global-ID edits, cached blocks, and blocks-only refusal. Candidate CLI/MCP
integration uses fake HOME and temporary projects only. Run the relevant core
suite, `make check-ci-fast`, touched-test selection and workspace test compilation
because this slice changes shared persistence infrastructure. Do not run broad
unfiltered test suites. No service, forge publication, or real HOME authority
configuration is part of fixture validation.
