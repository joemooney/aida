# BUG-2017 slices 1 and 2: allocation admission inventory

<!-- trace:BUG-2017 | ai:codex -->

The signed SKETCH v2 and advisor acceptance override replace the original grant
proposal. This slice introduces the common-Git-directory authority setting and
shared allocation/persistence admission. It does not enroll the operator's clone.
Advisor round2 extends PR2919 to include full signed slice 2 alongside slice 1.
Pseudonymous bootstrap and HOME promotion are implemented below; full
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
Slice 2 adds the direct-human `config allocation enroll-home` workflow; implementation
never executes it on real HOME. Only exact synthetic test repositories receive an
authoritative setting during validation.

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
  existing UUID/global-ID edits remain valid. A new UUID cannot overwrite an
  existing local handle; bulk collisions use the existing local dispenser to
  select an unused ID. Internal global counter mutations refuse on WORK,
  including repair/reset/import paths. Empty failed-allocation identities are
  refused before metadata/oplog/objects on WORK and HOME; valid HOME formatting
  remains unchanged.
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
  resolved before clone discovery. Lazy and supplied providers both require the
  bound local mode, including an aliased counter outside the clone. First-use
  initialization shares the permanent dispense lock, so concurrent loaded-model
  writers cannot reset consumed counters. SQLite existing-mode preflight is
  read-only before writable setup; exhausted counters refuse without wrapping.
  Independent centralized database backends retain their own allocation behavior.
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

Legacy global-ID CRUD/relationship/lock fixtures explicitly model synthetic HOME.
Their cfg(test)-only helpers constrain both canonical root and common Git directory
to temporary locations. There is no automatic global test-template enrollment;
WORK candidate fixtures deliberately leave the authority setting absent. The CLI
case seeds an imported global block and verifies local allocation plus unchanged
cached consumption. Parent completion and actual HOME enrollment remain excluded.


## Slice 2 implementation and acceptance ledger

Advisor round2 (pickup 2026-10-25T111616Z) keeps slices 1+2 together on PR2919.
The parent is incomplete; commits use `Refs BUG-2017`. This ledger describes
source coverage, not a claim that all gates have passed.

| Signed row | Implementation / decisive checks |
| --- | --- |
| Pseudonymous binding | Additive `work_binding.rs`: only version and opaque UUID, common-Git local. Legacy NodeConfig schema and serialized records keep their meanings. Fresh WORK creates no host/user registry entry. |
| Persistent provider | Common-Git FileDispenser shared by linked worktrees; store writer precedes dispenser lock. One-time high-water migration; interrupted publication recovers the same UUID. Missing/mismatched/linked/FIFO state refuses ordinary allocation and explicit repair. Direct alternate counter paths refuse. |
| Before first write | Default worktree and separate-store init bootstrap after real Git identity exists, load the actual backend/provider, then seed. Post-clone and sibling attach establish runtime binding; CLI and MCP migrate explicitly registered legacy clones. Unsupported raw missing-binding backend cases still refuse. |
| Model policy | WORK uses node-aware per-type sequences, including meta and override IDs, regardless of legacy global numbering. Lowercase node IDs use canonical snapshot/save-set keys, preserving files and stale-write CAS. |
| Global registration/repair | Existing implicit first/refill helpers skip WORK; explicit blocks/legacy registration still refuse. WORK node acquire only repairs opaque identity; host/name/hijack registration options refuse. |
| Explicit HOME workflow | `config allocation show` is pre-storage/read-only; `enroll-home` requires direct human terminal, writes exact common-Git setting only. Never run on real HOME by implementation. |
| Promotion and aliases | Core merge-gate checks HOME at entry and under store writer. Post-pull caller skips promotion on WORK/HUB. Promotion retains native ID plus agreed canonical display alias, UUID, relationships, comments and traces; repeated promotion does not alter objects/counters/ref. Destructive retirement of native node aliases explicitly refuses, rather than discarding lookup or repurposing agreed_id. |
| Production contract | `tests/test_bug2017_bootstrap.py`: no-flag init, first FR/BUG, CLI/MCP restart, concurrency, copied tracked false authority, global refusal/no-write snapshots, private ref-only HUB and synthetic HOME promotion/old lookup/idempotency. Called by distributed CI contract. |
| F2 distributed fixture | Directory-only legacy global store1 now owns exact private Git/common root. Default WORK init and sibling/attach remain no-authority fixtures, with isolated legitimate test grants for approved actions. Init/sync errors are not swallowed. |

Local validation must include the focused bootstrap/allocation modules, affected
native CLI/MCP and distributed contracts, fast gates, touched-test selection and
compile-only workspace check. Author check findings are not independent review.
Final exact-head pass/failure evidence belongs in the PR and canonical comments.

## Remaining slice 3 / deployment boundary

All-conflict reconciliation preflight/audit (global ranges, node bindings and
display-ID/UUID assignments), STORY-1742 serialization/scanner integration, and
final acceptance documentation remain mandatory. The current post-pull caller
condition changes only promotion; it does not introduce a new transport executor,
scanner exemption, prepared-candidate policy engine or outer lock. Actual HOME
enrollment and live rollout remain explicit operator actions after source review;
no merge/deployment hold is cleared by this implementation.
