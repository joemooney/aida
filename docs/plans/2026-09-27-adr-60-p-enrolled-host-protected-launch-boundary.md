# ADR-60: P-enrolled-host is the protected boundary for every AIDA-managed full-access agent launch

**Status:** Accepted as DESIGN DIRECTION ONLY — not approved for implementation, installation, migration or deployment<br>
**Date:** 2026-09-27<br>
**Decision owner:** operator (Joe), on independent architecture signoff<br>
**Linked spec:** TASK-1549<br>
**Related:** BUG-1669 (owning bug), SPIKE-91 (backend/boundary package), TASK-1550 (host-wide admission and cutover inventory), BUG-1679 + PR #2226 (protected binary identity/admission dependency)

> **Read this first.** This ADR records a boundary that does not exist yet. Nothing
> in it describes present behaviour, and nothing in it authorises building,
> installing or deploying anything. AIDA today still launches full-access agents
> behind same-user-forgeable corroboration. That is the defect BUG-1669 tracks and
> this ADR does not fix it.

## Context

BUG-1669 found that production AIDA code paths build full-access vendor argv
(`claude -p --permission-mode bypassPermissions`,
`codex exec --dangerously-bypass-approvals-and-sandbox`) unconditionally, so any
agent permitted to run `aida *` can start a detached full-access child with a
prompt of its own choosing. The recorded review trail on BUG-1669 establishes:

* `orchestrator::detect -> classify -> run_is_live` accepts a non-empty
  `AIDA_AUTO_COMPLETE`, a UUID equal to `DrainState::run_uuid` in ordinary
  project JSON, and a live PID. It authenticates no issuer, no caller ancestry,
  no process-start identity and no operation scope. It is **run corroboration,
  not a credential**.
* `authority_stdin_is_terminal` / `authority_stdout_is_terminal` are `IsTerminal`
  observations. An agent can create a PTY and answer `yes`. A terminal proves a
  terminal, never human identity or human consent.
* The affected surface is far wider than the three commands in the bug title. The
  recorded inventory spans groups A-G: compete/zen arms and judges; intake
  assess/propose/apply/then-drain; burndown quiet/verbose/continuations; every
  shared `session.rs` fresh/exec/resume sink and its research, remedy, dry-run-AI,
  intent, harvest, reconstitution, queue implementer/reviewer and advisor-watch
  callers; direct `RealPhaseDriver` advisor/resume/CI-fix launches and all
  controller entry modes; `agent-new` and interactive/reviewer session surfaces;
  and the solo/shift/scheduler/cron/systemd/tmux/detached wrappers plus shipped
  launch scripts (`scripts/aida-demo.sh`, the four `scripts/ablations/gate-vs-rule*.sh`).

Three operator decisions on BUG-1669 settle the requirement, and one more
accepts the resulting design direction. They are quoted by ID because subsequent
work must not reopen them.

| Comment | Decision |
|---|---|
| `01a0e0a3-e301-7c72-83e7-68ab14eaf9e0` | **Literal same-user anti-forgery.** Retain literal protected authorization for full-access headless launches. A same-user-forgeable local guardrail, ambient environment flag, PID/state file, or agent-created PTY is not sufficient; do not weaken AC2. |
| `01a0e103-c774-7620-b1dd-f783c05bcb81` | **Host-wide scope.** The mechanism must cover **every AIDA-managed agent launch on this host, across projects**, not only executions enrolled in this repository. It explicitly does **not** claim control of unmanaged vendor clients launched outside AIDA, and does not authorise installation, credential migration, implementation or merge. |
| `01a0e10c-66ae-7883-9f98-f422dff413fa` | **P-enrolled-host accepted IN PRINCIPLE.** Administrator-controlled broker, isolated service-owned workers, agent-inaccessible approval, service-mediated execution credentials; full access inside that boundary. Accepts in principle replacing native burndown dispatch, finite scoped watch/schedule grants, refusal of unsupported/unenrolled launches, and interruption/migration of existing unattended launches. **Design authorization only.** |

The design that answers those decisions is revised sketch
`01a0e116-d863-7bd3-9a94-faa8e2c3888c`. It received an independent architecture
signoff, `01a0e11e-12db-7cd0-a3d1-39df79961a63`, **APPROVED as amended** with five
binding amendments M1-M5. Product accepted that signoff as binding design in
`01a0e125-a9e0-7810-bd94-e207548548a2` and captured TASK-1549, SPIKE-91 and
TASK-1550 as blocking prerequisites.

## What "accepted in principle" does and does not mean

This is the load-bearing sentence of this ADR.

**Accepted in principle is not approval to implement, install, migrate or deploy.**
Operator decision `01a0e10c` is explicit: *"This is design authorization only: no
privileged-service installation, VM backend selection, real credential migration,
deployment, implementation dispatch, or merge is approved."* No concrete operator
approval for any of those has been granted, and nothing in this document should be
read as one. Every such approval is listed in the inventory below and marked
**NOT YET GRANTED**.

Correspondingly, **no protection is deployed**. The independent signoff states it
"does not certify an implemented or deployed security boundary" and that "current
source still uses forgeable run corroboration and local launch paths; this verdict
claims no present protection." Any future document, commit message, release note,
CLI string or PR body that says or implies AIDA protects full-access launches today
is false.

## Decision

### 1. The protected boundary

Authorisation for a full-access agent launch lives **outside the agent-controlled
login session**, in service-held state that the agent cannot read or write, and the
service — not the caller — performs the launch.

* **Approval principal.** A full-access launch requires either (a) explicit
  operator approval delivered through an authenticated, agent-inaccessible approval
  endpoint that displays the exact normalised scoped request, or (b) an
  authenticated, still-live, scope-limited delegation from a protected controller
  that derives from such an approval. Nothing else is an issuer.
* **Atomic admission and spawn.** The broker owns both validation and the spawn. A
  client-side "yes" followed by a local spawn is not the boundary. Authority is
  server-side state plus an authenticated principal; public grant/run IDs are
  lookup handles and possession of one grants nothing.
* **Isolated workers.** Full vendor access is confined to a disposable
  service-owned isolated worker per phase attempt, with no unrestricted host HOME,
  host processes, host control sockets, shared credential mounts, direct guest
  networking or reusable upstream credentials. Vendor permission bypass inside a
  worker does not disable the external perimeter. Guest root is still untrusted.
* **Typed resource mediation.** Model, forge and artifact access goes through typed
  service adapters holding their own credentials, with pinned endpoints, allowed
  models/refs/actions, quotas charged to the grant tree, and revocation. No generic
  HTTP proxy, credential helper, arbitrary remote URL, shell adapter or writable
  host mount — each of those would recreate the bypass. A launch grant never
  confers merge authority.
* **Legacy corroboration is demoted, not strengthened.** `orchestrator::detect`
  and friends remain diagnostic classifiers. A request-specific
  protected-dispatch-authority query replaces ambient `is_orchestrated` boolean
  authority, and every current authority-consuming caller must be audited before
  migration. There is no environment-carried bearer run token.

**Explicitly rejected as substitutes**, because Joe rejected exactly these:
writable project state (`.aida/drain-state.json`, UUID equality, live PID),
environment variables and ambient flags (`AIDA_AUTO_COMPLETE`,
`AIDA_AUTO_COMPLETE_TOKEN`, `AIDA_BURNDOWN_LOCK_HELD`), PTY presence or an
`IsTerminal` pair, `SO_PEERCRED` login UID, roles and effective-role derivation,
`--force` / `--yes` / `--no-human`, acknowledgement files, client version strings,
executable paths, mode-0600 same-UID secrets, MACs whose key the agent can read,
and private Rust permit fields. Each is a usability or diagnostic property, not
provenance. The word "unforgeable" in BUG-1669's AC2 describes the required
boundary; it has never described the shipped implementation, and the shipped
implementation must not be relabelled to fit it.

### 2. Host-wide enrollment and admission rule

Coverage is **host-wide over AIDA-managed launches, across all projects on this
host**, per decision `01a0e103`. The unit of coverage is the launch route, not the
repository.

* One host authority. Every official AIDA entry point is a thin
  proposal/status/attach client; only installed service code executes an agent.
* Enrollment **enables scoped requests; absence of enrollment is refusal, not
  exemption.** An unknown or unenrolled project cannot obtain unprotected
  full-access execution.
* Host registry state (host/broker identity, enrollment generation, canonical repo
  ID, approved forge identity and refs, import/export roots, adapter/image
  versions, supported protocol schemas, policy) is protected service state. It is
  **not** project `.aida` configuration, which the agent can write.
* All postures, including anything currently called "contained", use the same
  external worker boundary. There is no retained local contained launch that could
  escalate or invoke an unmediated descendant. Vendor flags may tighten guest
  permissions; they cannot weaken the boundary.
* The broker normalises typed operations itself and trusts no caller-supplied
  client version, executable path, PATH lookup, git remote, `.git` indirection,
  `AIDA_DRIVE_ROOT`, role, argv or grant ID. Unknown adapters, custom executables,
  arbitrary extra flags and `AIDA_AGENT_CMD` / `AIDA_COMPETE_JUDGE` /
  `AIDA_OS_WRAP` overrides refuse unless converted to an enrolled, reviewed typed
  adapter configuration.
* Execution code and images are pinned **at the service**, never by trusting a hash
  the requesting CLI reports. An agent-built `target/release` binary is not a
  trusted deployment artifact.

### 3. Cross-repo scope: coverage is not a grant

Coverage across projects is **not** authority across projects.

* Approval for repo A conveys no authority in repo B. There is no cross-repo parent
  grant in v1, and no implicit sharing of artifacts, transcripts, model context or
  credentials.
* Every grant, artifact and adapter call binds repo ID + enrollment generation +
  base artifact + spec/plan content digests + operation/phase. Same path, same spec
  number, cloned remote, symlink switch, renamed checkout or copied `.aida-store`
  cannot transfer authority.
* Sibling worktrees import into the same enrolled identity only through validated
  snapshot manifests.
* A new repository inherits nothing — no existing watch, wave or schedule consent.
* A host-wide scheduler may display multiple jobs, but each job has independent
  authority and independent counters.
* Per M2, isolation covers **observation as well as mutation**: cross-repo requests
  for transcripts, prompts, logs, artifacts, guest consoles or interactive input
  refuse too.

### 4. Refusal of unprotected and unknown routes

Fail closed, with no lateral escape.

* No protected backend, no enrollment, unsupported platform, unsupported adapter or
  vendor, expired/revoked authority, corrupt state, exhausted budget, stale epoch,
  failed isolation attestation, unauthorised IPC peer or unsupported protocol
  schema ⇒ **typed nonzero refusal and no agent launched.**
* No cooperative fallback: no local spawn, no alternate vendor, no alternate
  backend, no automatic downgrade to a "contained" vendor, no detached/systemd
  fallback after a denial or an ambiguous start. A proven not-started transport
  failure may retry **the same reserved attempt** through another protected
  transport; ambiguity stays reconciling.
* No launch-associated mutation before a valid preparation reservation. Refusal
  leaves the worktree, queue, leases, logs, transcripts and state unchanged; the
  broker's own refusal audit is the single exception, and protected audit writes
  fail closed for launches.
* Refusal propagates instead of being swallowed. A denied compete arm must not
  become a failed arm followed by an authorised-looking judge; `solo_cycle` must
  stop its chain rather than continue into burndown/integration and retry next
  tick; `--force` may skip the old role guard but never protected admission.
* Legacy native Claude burndown fan-out is **not grandfathered**. Until a
  broker-mediated wave controller replaces it, the native dispatch path refuses
  with an explicit unsupported-boundary error. Native subagents may compute inside
  an already-granted worker and gain no separate principal, sibling or parent
  rights; prompts and hooks are not enforcement.
* Read-only surfaces stay useful: previews, pure argv renderers, dry-run output,
  `schedule due`/`status`, diagnostic drain-state and config writers are non-launch
  operations. They confer no authority and must be tested to confirm they cannot
  later supply any.

### 5. Exclusion: unmanaged independent vendor clients

The promise is bounded, and the bound is stated honestly rather than buried.

* **Out of the promise:** an unrelated vendor client launched outside AIDA, by an
  unrestricted host login, using its **own independent credentials**. P-enrolled-host
  prevents such a client from obtaining protected AIDA execution resources. It does
  not and cannot prove that arbitrary programs on an unconfined login cannot run
  their own vendor clients.
* This is **not** P-managed. Confining every agent or vendor client on the
  workstation, and migrating unrelated third-party credentials, is explicitly **not**
  a prerequisite of this decision.
* **The exclusion is not a relabelling device.** An official AIDA launch route that
  still launches using host credentials after cutover is a **FAILED cutover case**,
  not an "unmanaged client". No provenance detector or version allowlist is claimed
  to solve reconstructed legacy programs; that is why the boundary is stated as
  what it protects, not as what it forbids everywhere.
* If the real host requires an official route to retain independent host execution,
  that concrete incompatibility returns to Joe as a deployment blocker. It is never
  an inferred exception, and the perimeter is not quietly narrowed to fit it.

### 6. The five gates stay separate

| # | Gate | State |
|---|---|---|
| 1 | **Independent architecture signoff** on the revised sketch | **CLOSED** — `01a0e11e-12db-7cd0-a3d1-39df79961a63`, APPROVED as amended with M1-M5, architecture only. |
| 2 | **Concrete installer / backend / credential proposal**, independently reviewed | **OPEN** — SPIKE-91 and TASK-1550 produce the package; it returns to Joe for explicit authorization. |
| 3 | **Disposable privileged proof** on a real installed boundary | **OPEN** — a privileged disposable environment is neither authorised nor available; mocks and source review cannot substitute, and a skipped privileged suite is a block, not a pass. |
| 4 | **Deployment approval** (installation, enrollment, credential migration, cutover, current-run handling, recovery/rollback) | **OPEN** — never granted; `01a0e10c` withholds all of it. |
| 5 | **Operator-held merge gates** | **OPEN** — BUG-1669 stays implementation-held even after its prerequisites complete (`01a0e125`); PR #2226 stays operator-held (below). |

Gate 1 being closed unblocks *dependent design and planning work only*. It is not
dispatch approval, not a queue change, not an installation ticket and not a merge
authorisation. Approved lifecycle status, an existing queue entry, and the
architecture comment itself are each explicitly **not** dispatch approval (M5).

### 7. Binding amendments M1-M5

Each amendment from `01a0e11e` is part of the approval, not optional follow-up.
Where each lands in this ADR:

* **M1 — host-wide coverage is an admission AND cutover invariant.** §2 (host
  registry, every official entry point a thin client, enrollment is not exemption),
  §4 (fail-closed refusal), §5 (no relabelling; concrete mismatch returns to Joe),
  and the §8 handoff to TASK-1550, which owns the per-route manifest. Credential
  revocation alone does not prove a credential-free, custom or local-model route is
  blocked: execution behaviour for those routes must be recorded and tested, and
  negative tests must run from the untrusted host principal against stale, copied
  and replaced clients.
* **M2 — repo/principal authorization covers observation and attachment.** §3
  (cross-repo isolation includes observation) and §4. Public proposal/status IPC may
  expose only deliberately public, non-sensitive metadata; opaque IDs must not
  unlock transcripts, prompts, logs, artifacts, guest consoles or interactive input.
  Worker status/attach/result APIs stay scoped to the authenticated attempt;
  operator data access uses a separately authenticated surface. No generic
  FD/socket/terminal bridge. Rendered guest output is hostile and must be
  neutralised on trusted approval/admin displays. Repo-A-requests-repo-B
  observation is a required test case. Carried into SPIKE-91's scope.
* **M3 — freeze a versioned, canonical, domain-separated signed request encoding
  before protocol implementation.** §1 (the approval endpoint displays the exact
  normalised request the broker verifies) and §3 (digest binding). Reject
  duplicate, ambiguous or unknown authority fields; bind operation, artifact/input
  policy, broker identity and epoch, single-use nonce, limits and detach policy;
  keep certificate purposes and endpoints separate; bind channel generation and
  request IDs to the authenticated transport so replay across reconnect, boot or
  endpoint neither repeats effects nor restores quota. **An artifact hash
  establishes content identity, never permission to read or import it** — the
  enrolled source and approved lineage authorization are also required. Parser
  differential, request substitution, role-confusion and cross-repo
  artifact-hash tests are required. Owned by SPIKE-91 before any protocol code.
* **M4 — verify actual confinement and supervision before Running, not successful
  process creation.** §1 (isolated workers) and §4 (failed isolation attestation
  refuses). The supervisor must attest intended cgroup membership, limits, jail
  ownership, closed inherited FDs and private endpoint mapping **before** guest work
  or resource access begins; failure kills, reconciles and denies. Guardian health
  must itself participate in admission, and loss of the guardian must cause another
  independent live component to stop admissions and terminate domains. Broker,
  supervisor and guardian kill/hang cases, active adapter closure and
  startup/cleanup races must be qualified. The 10s termination bound and the suspend
  fence cannot be asserted from configuration — the Firecracker jailer documentation
  permits a cgroup-v2 configuration with a missing parent cgroup to proceed without
  moving the process, so an exit-status-only check is insufficient. Owned by
  SPIKE-91's failure model.
* **M5 — preserve gate separation in product capture and all dependent work.** §6
  (the five gates) and §§9-10 (acceptance proposal and NOT-YET-GRANTED inventory).
  This ADR is the record M5 requires: the reconciled AC wording and these
  amendments are captured in the owning design trail **before** any dispatch. A
  material departure from the amended boundary returns for architecture review.

### 8. Scope of this ADR versus its sibling prerequisites

This ADR records the boundary, the rule and the gates. It deliberately does not
select mechanisms:

* **SPIKE-91** owns backend comparison (the sketch's Firecracker + jailer candidate
  against at least one viable alternative), service/admin/worker principals,
  authenticated approval and one-boot worker channels, private endpoint binding, the
  M3 request encoding, typed adapters, the M4 guardian/supervision failure model,
  installer/trust-root/credential custody, the disposable privileged fixture plan
  and the real-boundary test matrix.
* **TASK-1550** owns the host-wide launch entry-point inventory across projects, the
  per-repo identity/enrollment generations, cross-repo isolation proof, the
  credential-free/custom/local-model route treatment, the current-run
  interruption/migration plan, the cutover manifest and its fixture matrix.
* **BUG-1669** remains the owning defect and remains implementation-held.

Naming the Firecracker + jailer candidate in the signed sketch is **not** Joe
selecting a backend. It is a candidate for review.

### 9. Proposed BUG-1669 acceptance update (proposal only)

BUG-1669's acceptance predates the operator decisions. AC2 names
`orchestrator::detect` and "a human confirmed at a real terminal" as the two
authorities; AC4 asks for a positive fixture proving "the drain-token path still
works". Under `01a0e0a3` both readings are now wrong: `detect` reads writable state
and a live PID, and an agent-created PTY satisfies a terminal check. The old
positive fixture — hand-written `DrainState` plus a matching UUID plus a live
fixture PID — **demonstrates the forgery** rather than the protection, so keeping
it as the positive test would certify the vulnerability.

**Acceptance changes are product's call. This ADR proposes text; it does not edit
BUG-1669, and the same proposal is posted as a comment on BUG-1669 for product to
adopt, amend or reject.** The proposal strengthens rather than weakens the literal
requirement.

Proposed text (the wording is the sketch's §1 interpretation, tightened here for
the acceptance trail):

* **AC1 —** Inventory every AIDA-managed production agent launch across projects on
  this host, including configured, foreground, background, resumed, wrapped,
  scripted and IDE/MCP routes, and each route's *effective resolved* vendor posture.
  List each in the PR body with its protected admission or its explicit refusal.
* **AC2 —** A full-access launch requires **either** explicit operator approval
  through an authenticated, agent-inaccessible approval endpoint that displays the
  exact scoped request, **or** an authenticated, still-live, scope-limited
  controller delegation derived from such an approval. The broker validates and
  performs the launch atomically. A terminal is a request and display surface only,
  unless it *is* that separately protected endpoint. `IsTerminal`, a fabricated PTY
  answering yes, legacy `orchestrator::detect`, `AIDA_AUTO_COMPLETE` and its token,
  `DrainState` UUID/PID corroboration, effective role, `--force`/`--yes`/`--no-human`
  and any other ambient environment value grant nothing. *(Replaces AC2's
  "unforgeable drain run-token check (orchestrator::detect)" and "human confirmed
  at a real terminal"; `detect` is retained as a diagnostic classifier only.)*
* **AC3 —** Otherwise a typed nonzero refusal and no agent launched, with no
  alternate vendor, backend or local fallback, and no launch-associated mutation
  before a valid preparation reservation. *(Extends the existing AC3 with the
  no-fallback and no-side-effect clauses.)*
* **AC4 —** Retain fake HOME, injectable terminal observations and real-handler
  per-sink refusal coverage for every inventoried branch. **Reverse the old
  writable-state / live-PID positive fixture into a required forgery negative** —
  hand-written `DrainState` with a matching UUID and a live PID must now FAIL, as
  must a fabricated PTY answering yes and both `IsTerminal` observations forced
  true. The positive case is authenticated approval → protected controller → scoped
  phase worker → allowed next phase, over the real protected channels. Add both
  hermetic protocol coverage **and** disposable actual-boundary evidence; a
  privileged suite that skips is not a pass. Test injection seams are `cfg(test)`
  only and never a production authorization feature.
* **AC5 (new) —** No claim that protection is deployed may appear in code, CLI
  output, docs, release notes or the PR body until the gate-3 real-boundary
  evidence exists and has been independently reviewed.

Note the consequence product must weigh when adopting this: under the proposed
AC2/AC3, closing BUG-1669 without the protected backend means every affected
full-access route **refuses**, which interrupts today's unattended solo, burndown,
wave, watch and scheduled work. That consequence was accepted in principle by
`01a0e10c`; its execution is gate 4 and is NOT YET GRANTED.

### 10. BUG-1679 dependency and PR #2226

BUG-1679 (`AIDA_BIN` is exported but never read) is **blocked-by BUG-1669** and is
a direct instance of this ADR's boundary, not an unrelated cleanup.

* Advisor decision `01a0e115-11f5-7281-b3d2-a76d51b8ec1e` found that BUG-1679's own
  AC1 — "prefer `AIDA_BIN` when it names an executable file" — cannot be satisfied
  safely in the shared authoritative resolver. Executability is a usability
  property, not provenance; a child cannot distinguish an authentic coordinator
  export from an attacker-supplied variable; and absolute paths, canonicalization,
  `--version` probing (which executes the candidate), same-UID ownership, writable
  allowlists, UUID/PID state, a PTY or another env marker all fail to supply the
  missing protected identity.
* Therefore **protected binary identity and admission is owned by this boundary**:
  a different coordinating build may be selected only through independently
  established protected installation/admission/update policy, with a launch plan
  bound to that admitted identity. `AIDA_BIN` may at most name a candidate that
  mechanism has already admitted. Unknown or unadmitted identity, or unavailable
  protected authority, refuses the managed protected launch — with no arbitrary
  PATH or vendor fallback. **Identity admission is necessary but is not itself a
  scoped launch grant.** SPIKE-91's "pinned executable/dependency identity
  admission" is the sibling deliverable.
* Operator decision `01a0e11a-a8b6-7de1-a987-b8bdcec6b079` directs: wait for
  BUG-1669's protected binary-admission design; do **not** narrow BUG-1679 to a
  pre-architecture repair and do not resume its implementer.
* **PR #2226 stays operator-held** at `e63efc3997a85df8cb1ff463f48222900ed5f7d6`
  with `aida:merge-hold`, after strict `REQUEST_CHANGES`. Per `01a0e115`, "neither
  green CI, mechanical P2 repairs, revised acceptance, nor a future reviewer pass
  removes it"; only Joe can release it, and then only after reconciled acceptance,
  the architecture and implementation gates, and a fresh independent review at the
  new exact head. This ADR changes none of that. Correction
  `01a0e33d-980a-7270-9cd5-b0129da2cc5c` records that a proxy already mis-dispatched
  this spec once on the mistaken premise that a fresh APPROVED verdict lifts the
  hold; it does not.

## Design decisions recorded here

These are settled by this ADR and must not be reopened by dependent work:

1. Authorisation lives outside the agent login; the service validates and spawns
   atomically.
2. Literal same-user anti-forgery is retained. Writable state, environment
   variables and PTY presence are never corroboration of identity.
3. Coverage is host-wide over AIDA-managed launches across all projects.
4. Enrollment enables scoped requests; absence of enrollment is refusal, not
   exemption.
5. Coverage is not a grant: no cross-repo authority, artifacts, transcripts,
   context, credentials or observation in v1; a new repo inherits nothing.
6. Full access is confined to a disposable service-owned isolated worker per phase
   attempt; there is no local contained-mode exception.
7. Resource access is typed and service-mediated; no generic proxy, credential
   helper, shell adapter or writable host mount. A launch grant never confers merge
   authority.
8. Legacy `orchestrator::detect` is demoted to a diagnostic classifier; a
   request-specific protected authority query replaces ambient boolean authority.
9. Every refusal is typed, nonzero, side-effect-free before reservation, and has no
   fallback route; refusal propagates through wrappers instead of degrading into a
   failed step that later work treats as authorised.
10. Native burndown fan-out is not grandfathered; the old path refuses until a
    broker-mediated wave controller replaces it.
11. Watch, schedule and solo get independent, finite, scoped grants with expiry and
    reapproval; timers and user units are unauthenticated wake hints.
12. Unmanaged independent vendor clients are excluded from the promise, and that
    exclusion may never be used to relabel a surviving official AIDA route.
13. Protected binary identity/admission governs `AIDA_BIN` and the coordinating-binary
    contract (BUG-1679); `AIDA_BIN` is untrusted input.
14. M1-M5 are binding on all dependent work.
15. Grant state is server-side with public handles; there is no portable bearer run
    token.

## Unresolved concrete operator approvals — NOT YET GRANTED

None of the following has been approved. Each requires an independently reviewed
concrete package returning to Joe for explicit authorization (gate 2 → gate 4).

| Approval needed | Status | Owner of the proposal |
|---|---|---|
| **Backend / platform selection** — hypervisor and exact pinned host/kernel/VMM/guest/vendor/TLS versions and digests; whether this host qualifies at all | **NOT YET GRANTED** (the signed sketch names Firecracker + jailer as a *candidate*; naming it is not selecting it) | SPIKE-91 |
| **Privileged service installation and trust root** — installing broker/controller/supervisor/adapters, administrator-owned paths and policy, offline signing trust, immutable packaging, effective-admin/ACL verification, update authority, approval-device pairing and custody | **NOT YET GRANTED** (no service installation is authorised; an agent-built `target/release` is never a trusted artifact) | SPIKE-91 |
| **Actual credentials** — creating, rotating and scoping real model/forge/artifact adapter credentials, and deciding which AIDA-managed identities move | **NOT YET GRANTED** (no credential migration is authorised) | SPIKE-91 + TASK-1550 |
| **Disposable privileged test environment** — an authorised disposable Linux host/VM with nested KVM, separate admin/agent/approval principals, fixture-only enrolled device, and its cleanup | **NOT YET GRANTED** (gate 3; mocks and source review are not protection proof) | SPIKE-91 |
| **Current-run migration and cutover** — the named stop/checkpoint window for existing drains, watches, schedules and detached workers; disabling legacy official routes; revoking AIDA-managed execution credentials; per-project enrollment order | **NOT YET GRANTED** (existing runs are UNPROTECTED and must never be adopted as authorised by PID, UUID, role or state) | TASK-1550 |
| **Rollback and recovery** — installer/uninstaller, epoch rotation, anti-rollback fence, backup/restore policy that restores configuration and audit for inspection but never active approvals or budgets, device-loss replacement through the same trust root, suspend/reboot fencing | **NOT YET GRANTED** | SPIKE-91 |
| **Runtime ceilings and budgets** — the proposed 2h/24h ordinary, 1h and 6-start watch, 24h scheduler, 60s preparation, 1s/5s/10s supervision bounds are *proposals to accept and display*, not consent | **NOT YET GRANTED** | SPIKE-91 |
| **Repository enrollment mappings** — concrete per-repo identity, snapshot/import/export mappings, approved forge identity and allowed refs | **NOT YET GRANTED** | TASK-1550 |
| **BUG-1669 implementation dispatch** | **NOT YET GRANTED** — held even after TASK-1549, SPIKE-91 and TASK-1550 complete (`01a0e125`) | product → Joe |
| **BUG-1679 / PR #2226 merge** | **NOT YET GRANTED** — operator-held with `aida:merge-hold`; only Joe releases it | Joe |
| **Any claim that protection is deployed** | **NOT YET GRANTED** and currently false | — |

## Rejected alternatives

* **Option B / the cooperative same-user guardrail** — live-run corroboration plus a
  terminal prompt, with same-user file/env/PTY forgery outside the guarantee. Offered
  as an explicit operator choice in `01a0e08a` and **rejected** by `01a0e0a3`. It is
  settled and is not reopened.
* **Option H / deny-only containment** — refuse every affected bypass path and leave
  protected execution unavailable. Not selected; it leaves BUG-1669's
  authorized-drain functionality absent, so it cannot close the bug. A separately
  approved deny-only containment change remains *optional and unapproved*, not
  presumed.
* **P-managed / workstation-wide confinement** — moving every agent entry point and
  credential on the workstation into a managed domain. Explicitly **not** required by
  `01a0e103`; unrelated unmanaged clients are outside this prerequisite.
* **Repository-local opt-in coverage (plain P-enrolled)** — rejected by `01a0e103`
  as insufficient, because it permits protecting only selected enrolled resources.
* **Hardening the existing token** — stronger UUID entropy, `chmod 0600`, caller
  ancestry, a MAC with an agent-readable key, or private Rust permit fields. All
  rejected: they do not cross the same-user boundary.
* **A privileged daemon alone, retaining arbitrary host execution and shared
  credentials** — rejected in `01a0e0ad`: it does not establish the perimeter.
* **Treating scheduler or service provenance as approval** — "but systemd started
  it", a cron tick, an invoker/cgroup probe, `SO_PEERCRED`, a cached sudo session or
  a user-polkit exception. All rejected as scheduling evidence, not consent.
* **A localhost web page or local PTY as the approval channel** — rejected; the
  approval endpoint must be inaccessible to agents.
* **Grandfathering native vendor fan-out** — rejected; a native subagent has no
  independently authenticatable spec/phase identity.

## Consequences

**Now (design only).** Nothing in the running system changes. No launch path, live
scheduler, watch, credential or service is touched by this ADR. TASK-1550 and
SPIKE-91 become workable as read-only design and research. BUG-1669 stays blocked
and implementation-held; BUG-1679 stays parked behind it; PR #2226 stays
operator-held.

**If the later gates are eventually granted.** Unattended AIDA work that is not
enrolled stops launching. Native burndown dispatch is replaced. Watch and schedule
sets are frozen, finite and require reapproval. Unsupported platforms, vendors,
adapters and custom executables refuse. Existing drains either finish inside a
separately accepted migration window with no protection claim, or are stopped.
Provider compatibility is not guaranteed on day one: a vendor requiring an
unrestricted login HOME, a generic network proxy or a reusable OAuth credential is
unsupported until separately designed. These consequences were accepted **in
principle** by `01a0e10c`; each concrete step still needs its own authorization.

**Honest limits of the design itself.** Even fully built and deployed, this boundary
assumes a trusted administrator, kernel, hypervisor and approval device. It does
not prove semantic model obedience, prevent collusion among processes inside one
compromised worker, prevent local denial of service, undo completed external
effects (a sent model request, a pushed ref, an exported artifact), or survive a
compromised trust root. Revocation is not retroactive. The architecture signoff is
conditional on each of those premises being established by implementation and
deployment evidence — which does not exist.

## Related

* BUG-1669 — owning defect; comments `01a0df0b`, `01a0e086`, `01a0e08a`, `01a0e0a3`,
  `01a0e0a8`, `01a0e0ad`, `01a0e103`, `01a0e106`, `01a0e10c`, `01a0e116`, `01a0e11e`,
  `01a0e125` form the decision trail.
* SPIKE-91 — protected broker/worker boundary and disposable proof plan.
* TASK-1550 — host-wide AIDA launch admission and cutover inventory.
* BUG-1679 / PR #2226 — protected binary identity and admission dependency.
* `docs/architecture/control-plane-vs-durable-execution-and-merge-queues.md` — the
  existing merge-hold and verdict gating layers this boundary does not replace.
* `docs/architecture/autonomy-and-escalation.md` — the autonomy ladder whose
  unattended modes gate 4 would interrupt.

<!-- trace:TASK-1549 | ai:claude -->
