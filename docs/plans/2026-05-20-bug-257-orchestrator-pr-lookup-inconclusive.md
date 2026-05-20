# Plan: BUG-257 — orchestrator phase-1 PR lookup distinguishes "GH API unreachable" from "no PR exists"

Date: 2026-05-20
Specs: BUG-257
Status: In Progress
Complexity: ~210 prod LOC, ~150 test LOC, 1 commit, risk low

## Approach

Phase-1 of `aida queue work --auto-complete` runs the implementer session,
then polls `gh pr list --head <branch> --state open` to confirm a PR was
opened. The pre-BUG-257 outcome model lumped any non-success `gh` exit into
`PrLookup::GhFailed`, which the orchestrator reported as a phase-1 failure
with a hint pointing at `/aida-pr`. A transient `api.github.com unreachable`
during the lookup therefore crashed the batch with a wrong recovery hint
even when the implementer had genuinely shipped a PR (the operator just
couldn't see it from this side of the network).

The fix splits the outcome model at two layers. At the **lookup** layer a
new `PrLookup::GhUnreachable(String)` variant is produced when `gh`'s
stderr matches a small allow-list of Go `net` / `crypto/tls` connectivity
errors plus gh's own `githubstatus.com` diagnostic suffix. At the
**orchestrator** layer a new `ImplementerOutcome::Inconclusive { reason }`
is a first-class third phase-1 outcome — alongside `PrOpened` and `Punted`
— that exits `0` with a new `inconclusive_reason` on `OrchestrationResult`
and a paired `BatchDrainOutcome::Inconclusive` that pauses a batch drain at
the spec (no ship, no fail) so a retry once the API is reachable proceeds
without cleanup. Between them a `git ls-remote --heads origin <branch>`
narrowing probe runs over the git protocol (separate from the HTTPS API):
if the branch isn't on origin, no PR can exist regardless of API state and
the outcome collapses to a clean `NoPR` failure; otherwise we stay
Inconclusive.

### Diagram

```
phase-1 implementer ends
        │
        ▼
gh pr list --head <branch>           ┌─ Found        → PrOpened (continue)
        │                            ├─ NoOpenPr     → NoPR failure (current)
        ├─ stderr classifier         ├─ GhMissing    → MissingTool failure
        │                            ├─ GhFailed     → phase-1 failure (current)
        │                            └─ GhUnreachable┐
        ▼                                            │  (new — BUG-257)
git ls-remote --heads origin <branch>                │
        │                                            │
        ├─ Present       ───────► Inconclusive (PR may exist; pause)
        ├─ Absent (exit 2) ─────► NoPR failure (definite — no PR can exist)
        └─ LsRemoteFailed ──────► Inconclusive (cannot tell)
```

## Decisions

- **First-class outcome (Route A) over a refined `FailureKind`**: acceptance
  criterion #4 says *"orchestrator reports `Inconclusive`, drain pauses (not
  fails)"* — `not fails` rules out keeping the result on the failure path
  with a sharper hint. Implemented as `ImplementerOutcome::Inconclusive`,
  `OrchestrationResult.inconclusive_reason`, `finish_inconclusive`, and
  `BatchDrainOutcome::Inconclusive` paralleling the existing
  `Punted`/`EscalationSummary` shape (precedent: STORY-276, STORY-306).
- **Bounded `git ls-remote` probe**: passed via `-c http.lowSpeedLimit=1000
  -c http.lowSpeedTime=10` so a hung HTTPS dial against origin cannot
  re-introduce the very stall BUG-257 is fixing.
- **Local probe enum (`BranchOriginProbe`)** instead of reusing
  `aida_core::git_ops::remote_branch_exists`: the existing helper collapses
  *absent* and *ls-remote-failed* to one `false`, which is exactly the
  conflation BUG-257 must avoid. A 3-state enum local to the orchestrator
  caller stays the right abstraction.
- **Conservative network classifier**: a small substring allow-list
  (`error connecting to api.github.com`, `dial tcp`, `no such host`, `i/o
  timeout`, `githubstatus.com`, ...) — never a default fallback to network
  — so an auth/parse error cannot be silently re-classified as a transient
  blip and ignored.
- **Recovery hint in the epilogue, not via `recovery_hint`**: Inconclusive
  is no longer a failure, so `finish_inconclusive` prints the retry hint
  directly in its prose / JSON, matching `finish_punted` / `finish_escalated`.

## Files (in build-order)

### `aida-cli/src/main.rs` — lookup + driver

- `enum PrLookup`: add `GhUnreachable(String)` variant with BUG-257 doc.
- `fn gh_pr_list_first`: route stderr through `gh_stderr_is_network_error`
  and emit `GhUnreachable` instead of `GhFailed` when it matches.
- `fn gh_stderr_is_network_error` (new, pure): conservative substring match
  against a small allow-list of Go net/tls error families + gh's own
  `githubstatus.com` diagnostic suffix.
- `enum BranchOriginProbe` (new, local): `Present` / `Absent` / `LsRemoteFailed`.
- `fn probe_branch_on_origin` (new): `git ls-remote --exit-code --heads
  origin refs/heads/<branch>` with bounded timeout via `-c
  http.lowSpeedLimit=1000 -c http.lowSpeedTime=10`. Maps exit codes:
  `0` → `Present`, `2` → `Absent`, anything else → `LsRemoteFailed`.
- Phase-1 driver (`impl PhaseDriver for ImplementerDriver` site, just after
  `reconcile_orchestrated_branch`): add `GhUnreachable` arm that calls
  `probe_branch_on_origin` and converts `Present` / `LsRemoteFailed` to
  `Ok(ImplementerOutcome::Inconclusive { reason })` and `Absent` to
  `NoPR` failure with a "push the branch" hint.
- Resume-implementer driver (`impl PhaseDriver` resume site): symmetric
  `GhUnreachable` arm returning `Inconclusive`.
- Existing match sites updated to add a `GhUnreachable` arm where they
  previously listed `GhFailed`: workflow-hints `PrState` mapping, the
  auto-queue reviewer-story path, the `aida queue list` display, and the
  two `match &result.outcome` blocks for the batch + queue-N drain
  summaries (JSON slug + prose epilogue).

### `aida-cli/src/auto_complete.rs` — outcome model + orchestrator

- `enum ImplementerOutcome`: add `Inconclusive { reason: String }` variant
  with BUG-257 doc.
- `struct OrchestrationResult`: add `inconclusive_reason: Option<String>`.
  Update all 10 literal constructions in this file (5 production +
  5 test-helper + 1 escalation struct-update) to set
  `inconclusive_reason: None`.
- `fn finish_inconclusive` (new): mirrors `finish_punted`/`finish_escalated`
  — exit `0`, no `failed_phase`, paired JSON `phase-event` lines, prose
  epilogue with the `⏸` glyph and the retry hint
  (`gh api /rate_limit` then re-run).
- `fn orchestrate`: new `Ok(ImplementerOutcome::Inconclusive)` arm in the
  phase-1 match, returns `finish_inconclusive` directly so phases 2-6
  never run.
- `fn resolve_punt_via_advisor`: new `Ok(ImplementerOutcome::Inconclusive)`
  arm in the resume match, returning `PuntFlow::Terminal(finish_inconclusive)`
  so a network blip on the resumed PR lookup pauses the drain rather than
  surfacing as a re-punt.
- `enum BatchDrainOutcome`: add `Inconclusive` variant (no data — the
  paused spec is in `BatchDrainResult.stopped_at`).
- `fn drain_batch`: detect `result.inconclusive_reason.is_some()` *before*
  the BUG-245 mismatch / generic-success branches and return early with
  `BatchDrainOutcome::Inconclusive`, exit `0`, head un-advanced.
- Test mock (`MockPhaseDriver`): add `inconclusive: Option<String>` field,
  `inconclusive_at_implementer(reason)` factory, plumb it through
  `run_implementer`.

## Critical Files

- `aida-cli/src/main.rs`
- `aida-cli/src/auto_complete.rs`

## Reusable helpers

- `aida_core::git_ops::is_remote_reachable` / `remote_branch_exists` exist
  but **both collapse "absent" and "unreachable" into one `false`** —
  unusable for this BUG, hence the local `probe_branch_on_origin`.
- `auto_complete::finish_punted` / `finish_escalated` — direct shape
  template for `finish_inconclusive`.
- `auto_complete::BatchDrainOutcome::Stalled` — exit-code convention for a
  non-failure clean stop the drain reports; pattern reused for
  `Inconclusive` (exit `0`, but stopped at a spec).
- The `recovery_hint` table is bypassed: Inconclusive is not a
  `PhaseFailure`, so its hint lives in `finish_inconclusive`'s prose.

## Risks + gotchas

- **Misclassification false-positives**: a stderr line that *isn't* a
  transient network error gets re-classified as `GhUnreachable` and the
  drain pauses forever. Mitigated by the conservative allow-list — every
  match phrase is anchored to a Go `net`/`tls` error family or gh's own
  diagnostic suffix; the `non_network_failures_are_not_network` test pins
  9 real auth/parse/rate-limit / HTTP-status messages as non-network.
- **`git ls-remote` itself hanging**: the probe runs the SAME HTTPS dial
  the failing API call did. Bounded via `-c http.lowSpeedLimit=1000 -c
  http.lowSpeedTime=10` so the probe times out in ~10s instead of dragging
  phase-1 to an idle freeze.
- **Status-bumping on Inconclusive**: deliberately *no* status change. The
  spec stays at whatever the implementer left it in (`InProgress`,
  `Done`). On retry, the lookup re-confirms the PR (or its absence)
  against ground truth.
- **Batch drain stop-at-this-spec**: a single-spec drain that hits
  Inconclusive exits 0 — a calling shell script that just checks
  `if [ $? -ne 0 ]` will treat that as success. Telemetry differentiates
  via `auto-complete` phase-event status = `"inconclusive"`.

## Tests (named)

In `aida-cli/src/main.rs` (`mod bug_257_gh_stderr_network_classifier_tests`):

- `observed_origin_incident_stderr_is_network` — the verbatim stderr from
  the BUG-257 TASK-299 drain incident.
- `githubstatus_pointer_alone_is_network` — gh's diagnostic suffix is
  load-bearing.
- `dial_dns_tls_error_families_are_network` — 12 Go `net`/`crypto/tls`
  error strings.
- `classification_is_case_insensitive` — survives a gh upgrade that
  capitalizes phrasing.
- `non_network_failures_are_not_network` — 9 auth / rate-limit / parse /
  HTTP-status messages stay `GhFailed`.

In `aida-cli/src/auto_complete.rs` (`mod tests`):

- `orchestrate_inconclusive_at_phase1_is_not_a_failure` — acceptance #4:
  exit `0`, no `failed_phase`, `inconclusive_reason` set, no punt, no
  escalation, nothing shipped, only phase 1 ran.
- `drain_batch_inconclusive_pauses_without_advancing` —
  `BatchDrainOutcome::Inconclusive`, exit `0`, `stopped_at = head`, head
  un-advanced (the queued TASK-260 stays for the next drain).
- `orchestrate_resume_inconclusive_is_terminal_pause` — a network blip on
  the resumed implementer pauses the drain, advisor spawned exactly once.

## Verification

```bash
cargo test -p aida-cli --bin aida bug_257
cargo test -p aida-cli --bin aida inconclusive
cargo test -p aida-cli --bin aida auto_complete
cargo build --workspace
cargo fmt --all -- --check
```

## Followups

- Real-world: collect a corpus of actual gh-error stderr lines from
  headless drains (TASK-266 telemetry) and audit the `gh_stderr_is_network_error`
  allow-list against them quarterly — gh upgrades can rephrase.
- `aida_core::git_ops::probe_branch_on_origin` — promote the local probe
  helper into `aida-core` once a second caller appears (e.g. a future
  `aida fetch` narrowing of "branch missing" vs "remote unreachable").

## Related

- **BUG-241** — parent class: phase-agnostic reconcile against reality
  before declaring failure. BUG-257 extends the same principle ("don't
  collapse distinct phase outcomes") to the PR-lookup classifier.
- **BUG-250** — sibling outcome-model gap: "PR deliberately held" is
  another non-failure phase-1 state distinct from `NoPR`.
- **BUG-254** — sibling outcome-reporting gap: phase 5 silently reports
  success when the code-leg pull failed. Same family: outcome reporting
  must reflect underlying state.
- **STORY-276** — `ImplementerOutcome::Punted` set the third-outcome
  precedent; `Inconclusive` is the fourth on the same template.
- **STORY-306** — `EscalationSummary` set the field shape mirrored by
  `inconclusive_reason`.
