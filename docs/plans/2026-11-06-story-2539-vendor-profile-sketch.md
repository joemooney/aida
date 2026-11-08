> Approved by Joe on 2026-11-06 (STORY-2539 comments). PR-C merged as
> #3531; the 2026-11-07 01:52 UTC ruling authorizes implementation now.
> The proposal below is preserved verbatim, including its historical gate text.
> Implemented slices: registry, identity compatibility, capability consumers, hints,
> headless argv/settings composition and the inventoried production vendor ratchet
> (scope approved by Joe on 2026-11-07 at 10:42 UTC).
> Implemented step 3a: hook event/backend layouts, profile-driven init, shared
> handler parsing and symlink-safe scaffolding (2026-11-07 ruling).
> Remaining: resume/fork backend and effectful launch adapter migration,
> with further reductions to the vendor-debt ledger.
> STORY-782 remains deferred; BUG-2390 remains Completed. No fan-out or launch
> admission is added. Publish this slice with `(Refs STORY-2539)`.

# STORY-2539: one vendor profile beyond spawn

<!-- trace:STORY-2539 | ai:codex -->

Status: proposal only; advisor signoff required before implementation. This
lane is sketch-only per the owning spec's 2026-11-06 advisor comment. No PR,
production changes, new vendor admission, or completion is authorized here.
Source inspected at `8dcb86ca85d` on 2026-11-06.

## Existing seams and constraints

`aida-core/src/launch_policy.rs` owns closed `LaunchVendor`/`LaunchKind` sets,
aliases and posture-flag validation. `launch/context.rs` resolves a request
with caller-composed program/argv, rejects weaker containment and protected
tail flags, and keeps the resolved fields private. `launch/spawn.rs` builds
the command; callers currently assign the seat and finalize Codex afterward.
These are the merged STORY-2326 PR-B seams to extend, not replace.

Vendor behavior also lives outside those seams:

| Concern | Current source to migrate |
| --- | --- |
| Program, aliases, headless argv, posture, resume eligibility | `session.rs`: `HeadlessVendor`, `headless_vendor_args_contained_in`, `vendor_containment_posture`, `vendor_can_resume` |
| Mail capabilities and hints | `mail_wake.rs`: `WakeAdapter::for_vendor`, `launch_wake`; `mail_backlog.rs`: vendor check |
| Hook/scaffold layout and install behavior | `init_cmd.rs`, `aida-core/src/scaffolding/agent_pack.rs` and its template renderers |
| Dispatch selection and launch | `dispatch.rs`: `resolve_lane_vendor`, `resolve_vendor_bin`; use existing fixed selection semantics |
| AGY admission and candidate runtime | `launch/agy.rs`: `AGY`, `require_conformance`, `candidate_argv` |

BUG-2390 is already Completed, shipped in PR #3307. Preserve its configured
vendor default and detached Codex launch behavior; folding its theme into R5
does not reopen or redo the bug. STORY-782 remains deferred on environment
evidence: describing fan-out capability does not authorize fan-out execution.

## Ownership boundary with STORY-2326 PR-C and R4

PR-C owns the executable launch adapter/profile, the move of Codex
`prepare_launch` inside that adapter, consolidation of the eight grant issuers
into `issue_launch_child`, `control_child`/`control_env` migrations (phase,
orchestrator, zen, TUI), and its shrinking launch ratchet. R5 must not create a
second profile/finalizer or edit those launch paths concurrently. Land PR-C
first, then extend its actual interface; reconcile names with its author before
the first implementation slice. PR-C also retains ownership of its remaining
launcher/resource/environment-policy work; R5 adds no `env_clear` or grant logic.

R5 owns pure vendor descriptors and their non-spawn consumers, plus extraction
of remaining vendor behavior into that same adapter family after PR-C. R4
(STORY-2538) owns its own lifecycle design: R5 supplies capability descriptors
and renderers, never a parallel lifecycle, scheduling, or ownership model.

## Proposed module shape

Use `aida-core/src/vendors/` for pure identity and descriptor data, exposed
through one `profile(LaunchVendor) -> &'static VendorProfile` registry. Keep
`LaunchVendor` as the typed key; relocate its string parsing/rendering here
with compatibility exports from `launch_policy` during migration. Do not
invent another vendor enum. Core remains independent of cli-lib.

The one logical profile has typed fields for canonical external spellings,
accepted aliases, executable name, prompt transport, hook layout, resume
semantics, wake mechanism, and hint rendering inputs. Distinguish persisted
spellings from display/program names: existing `agy` versus `antigravity`
values must round-trip without rewriting config, registry rows or mailbox
addresses. Unknown launch vendors refuse; broader historical mailbox identities
remain inspectable and do not become launchable through normalization.

Use `aida-cli-lib/src/vendors/` for effectful per-vendor implementations,
extending PR-C's adapter rather than copying its contents. It references the
core descriptor by the typed key and renders argv/settings or invokes the
existing hook/wake/resume backend. If PR-C initially locates its profile under
`launch/`, move it once with mechanical imports in a dedicated later slice;
do not preserve two independently maintained registries. The dependency is
`launch -> vendors -> core descriptor`; effectful vendor code must not spawn
an agent directly or bypass the shared launch constructor.

Prefer closed capability enums to independent booleans: wake is per-turn
hooks, exact-thread queue, or unsupported; resume distinguishes recorded-session
resume from fork and unsupported; background distinguishes daemon handoff from
host-detached execution. Runtime version, recorded thread identity, opt-out,
trusted project configuration and admission evidence remain separate inputs.
A static capability cannot claim that a particular operation succeeded.

Prompt expansion accepts structured inputs and produces argv elements, not a
shell command. Prompt text remains data, including flag-shaped text. Generic
coordination code requests a capability without matching vendor variants.
Hook layouts specify paths/events and adapter identity; existing safe writers
still enforce symlink, ownership and write-boundary rules. No arbitrary
runtime plugins, executable config strings, new persistent formats or probes
during pure profile lookup are proposed.

## Posture and operator output

Profile posture support is an input to existing trusted configuration and
launch resolution, never the final authority. Preserve the Claude legacy/shared
wrapper distinction, explicit Codex native opt-in, shared Codex launch receipts,
surface-specific refusals, and monotonic parent restrictions. Native sandbox
continues to grant no AIDA containment credit.

The story's AGY outer-only wording must be reconciled with current source:
`launch/agy.rs` has incomplete auth inventory and unconditional all-tool
conformance refusal. Model candidate outer-boundary policy separately from
admitted launch surfaces; retain the refusal and no native credit. Moving a
table, seeing a hostname, or advertising a capability cannot open that gate.
Fresh independent evidence and its owning approval remain prerequisites.

Render command examples, unsupported-operation hints and supported-vendor
lists from the same profile and the resolved/refused operation. Keep dynamic
refusal details (failed Git lookup, missing evidence, unsupported version)
intact. A hint must not recommend a surface the admission check rejects.
Do not globally replace product prose or historical records with templates.

## Migration and acceptance proposal

1. After PR-C lands, snapshot identity/argv/posture behavior for every vendor
   and supported launch surface. Add core descriptors and extend its adapter;
   preserve public exports and wire formats while migrating consumers.
2. Move wake selection and operator hints first. Keep opt-out precedence,
   version checks, exact-thread binding, persisted state transitions and the
   distinction between queue acceptance and inbox acknowledgement unchanged.
3. Move hook layout, prompt rendering and resume behavior in bounded slices.
   Delete superseded switches as consumers move. Dispatch must retain project
   default selection and asynchronous Codex lanes. No fan-out implementation.
4. Enforce vendor literals only under `vendors/` in production policy code.
   Propose a token-aware Rust ratchet with mutant fixtures, using existing
   repository scanner conventions and no new parser dependency. Inventory
   existing violations by path and enclosing item; allow no additions and
   shrink the inventory per slice. Typed vendor references needed to implement
   policy must also live behind adapters; string-only scanning is insufficient.
   Compile-time public CLI spellings and protocol/template assets need explicit
   inventoried treatment, not a blanket allowlist. Tests, historical docs and
   generated artifacts are outside the production-code rule. Approval of this
   precise scope is requested before writing the guard.

Targeted validation for implementation: table-driven alias/round-trip tests;
descriptor completeness; all-vendor argv tests including flag-shaped prompts;
posture/surface refusal and unknown-input tests; mail capability/version/opt-out
and exact-thread tests; hook path/event fixture parity and safe-writer tests;
resume/fork/default-vendor/detached-dispatch regressions; ratchet mutants that
detect literals, qualified vendor branches and attempted inventory expansion.
Run existing `story_2326`, `compete`, mail-wake, scaffolding and dispatch filters
for the slices touching those areas, formatting, and the required LIGHT gate.
No Rust tests are needed for this sketch-only lane.

## Signoff requested

Approve or amend the core-descriptor/effectful-adapter split, the PR-C-first
ownership boundary and later single move into `vendors/`, the production-code
ratchet scope, and preservation of AGY's current admission refusal. Until then,
punt STORY-2539 as `design-fork`; retain the publication veto and open no PR.

## Step 3a round 4 review corrections (2026-11-07)

The signed pure/effectful boundary remains in force: `vendors/hooks.rs` owns
layout-derived parsing and event-contract validation only; filesystem installation
remains in `scaffolding/codex_hooks.rs`. Init iterates profiles by HookBackend.
Compatibility repair entry points keep explicit-path calls so the store-writer
scanner retains both original init wrappers and the new profile wrapper. The
inventory regression reads production sources to pin these call-chain edges.

Hook writers reuse BUG-718/BUG-1645 skip-and-warn semantics per file and check
ancestors with the same helper. Tests cover live and dangling file/directory
links under force and non-force, unchanged external targets, no writes below
blocked ancestors, and continued installation of unrelated handlers/wiring.
The removed all-or-nothing guard is no longer an installation policy. Two more
literal-ledger keys disappear through registry-derived repair identities.

Rework publication stays `(Refs STORY-2539)`. All PR CI lanes and the existing
merge-hold clearance remain host-side gates after native-seat publication.

Round 4 local validation: 16 executed core `codex_hooks` tests passed; all 42
store-writer inventory fixtures passed; 41 non-build fast gates (including
formatting) passed. The two fast-tier Cargo checks were delegated to the lane
runner. The CLI `story_2539` ratchet job timed out after 900 seconds during
compilation, without executing tests, and reported `seed: overlay-stale` because
the primary rewrote a file in the pinned generation. Ratchet and compile checks
therefore require a fresh host runner/generation; this is not a green LIGHT gate.
