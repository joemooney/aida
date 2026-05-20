# Plan: BUG-266 — headless implementer reports phase-1 `Inconclusive` on a transient Anthropic-API outage

Date: 2026-05-20
Specs: BUG-266
Status: Done
Complexity: ~140 prod LOC, ~165 test LOC, 1 commit, risk low

## Approach

Phase-1 of `aida queue work --auto-complete --no-human=both` spawns a
headless `claude -p` implementer via `exit_signal::spawn_and_wait`. The
pre-BUG-266 driver collapsed every non-zero exit into a `PhaseFailure`
with the wording *"the implementer session exited 1 — it was aborted or
errored"* — and the orchestrator treated that as a genuine phase-1
failure. The result: a transient Anthropic 529 / 5xx / stream-timeout
during a long unattended drain killed the implementer mid-work, marked
the spec failed (escalate-eligible), and the drain moved on to the next
item — losing the real partial work the implementer had genuinely
shipped on a branch.

BUG-257 already shipped the right outcome shape for the parallel case on
the GH-API leg — `ImplementerOutcome::Inconclusive { reason }` —
plumbed through `finish_inconclusive`, `BatchDrainOutcome::Inconclusive`,
and the resume-implementer path. The orchestration is leg-agnostic
once a leg produces `Ok(Inconclusive)`: drain pauses, exit 0, no
`failed_phase`, no status flip, the next drain retries. BUG-266 reuses
that whole path; the change is at the classifier and at the two
implementer call sites in `RealPhaseDriver`.

### Diagram

```
headless claude -p exits non-zero
        │
        ▼
read_headless_log_for_session(project_root, session_uuid)
   (glob `.aida/headless-logs/*-{session_uuid}.jsonl`)
        │
   on read:
        ▼
claude_log_indicates_api_outage(content)
   patterns (case-insensitive):
     - `API Error: 5\d\d`        (Anthropic 5xx family)
     - `Overloaded`              (Anthropic capacity-shed signal)
     - `upstream connect error`  (Envoy / proxy connectivity)
     - `stream timeout` /
       `stream disconnected`     (SSE-stream drop)
        │
   Some(reason)  →  Ok(ImplementerOutcome::Inconclusive {
                       reason: "Anthropic API outage during the headless
                                implementer: <excerpt>",
                       retry_hint: Some("Anthropic API was unavailable;
                                        resume with: aida queue work
                                        <spec> --resume <session-uuid>"),
                   })
                   →  finish_inconclusive (BUG-257 path)
                   →  drain pauses, exit 0, spec status untouched
   None          →  PhaseFailure (existing path — genuine implementer
                                  failure, not an outage)
```

## Decisions

- **Compose, don't fork the outcome.** Acceptance #5 says *"compose with
  BUG-257"*; the inconclusive classification path is shared. Implemented
  by extending the existing `Inconclusive` variant rather than adding a
  new outcome — keeps the orchestrator's match arms unchanged, keeps
  `BatchDrainOutcome::Inconclusive` working unmodified, keeps the
  resume-implementer "Inconclusive is terminal" rule shared between the
  two legs.
- **Per-leg retry hint via `retry_hint: Option<String>`.** BUG-257's
  default `finish_inconclusive` hint was GH-API-flavored (`gh api
  /rate_limit` then re-run). That's wrong for the Anthropic-API path —
  the right action is `aida queue work <spec> --resume <session-uuid>`,
  which re-attaches to the exact session via Claude Code's persisted
  JSONL. Added an optional override field rather than a new enum or a
  new function — `None` preserves BUG-257's wording at the existing 3
  GH sites, `Some` carries the Anthropic-specific hint with the live
  spec + session IDs interpolated.
- **Classifier scoped to `--no-human=both`.** The orchestrator only
  invokes `claude_log_indicates_api_outage` when
  `no_human.wants_headless_implementer()` is true — an interactive
  `--auto-complete` phase 1 doesn't write to the headless-logs dir, so
  the classifier has no input there anyway, but the explicit guard
  makes the intent obvious and avoids a spurious file-stat on every
  interactive failure.
- **Glob by session-UUID suffix, not by branch.** The log path is
  `.aida/headless-logs/<branch>-<session-uuid>.jsonl`, but the branch
  isn't known at the failure point (it's discovered AFTER the
  implementer exits, via `discover_orchestrated_lease` — which itself
  may fail for an early-crashed implementer). A UUID-suffix glob is
  branch-independent and robust to lease-discovery failures.
- **Conservative `API Error: 5\d\d` anchor.** The classifier matches
  `api error: 5` followed by two ASCII digits — so `API Error: 400 Bad
  Request` and `API Error: 429 rate limited` do NOT classify as outage
  (correctly — those are genuine errors, not transient capacity issues).
  Negative-coverage test pins this.
- **Bounded reason excerpt.** A `claude -p` assistant turn can be
  multi-KB; the orchestrator's epilogue is one line. `reason_excerpt`
  caps at 160 chars, anchored around the matched phrase, char-boundary
  safe for UTF-8.

## Files (in build-order)

### `aida-cli/src/auto_complete.rs` — outcome model + epilogue

- `enum ImplementerOutcome`: extend `Inconclusive` to `{ reason: String,
  retry_hint: Option<String> }`. Updated docs to call out BUG-266 as the
  second leg sharing this outcome.
- `fn finish_inconclusive`: add `retry_hint: Option<&str>` parameter;
  use `Some` value as the printed `→ ...` line when provided, else fall
  back to BUG-257's default GH-API wording. JSON event gains a
  `retry_hint` field too so telemetry sees what the operator was told.
- The 2 destructure call sites (`resolve_punt_via_advisor`,
  `orchestrate`) updated to forward `retry_hint.as_deref()` through to
  `finish_inconclusive`.
- `MockPhaseDriver::run_implementer`: construct
  `Inconclusive { reason, retry_hint: None }` so existing tests stay
  GH-flavored.
- `tests::orchestrate_resume_inconclusive_is_terminal_pause`: same
  field addition on the mock-driver `resume` site.
- New test `tests::inconclusive_with_anthropic_hint_overrides_default`:
  asserts the BUG-266 reason flows through `OrchestrationResult` and
  the orchestrator halts at phase 1.

### `aida-cli/src/main.rs` — classifier + driver wiring

- `fn claude_log_indicates_api_outage(content: &str) -> Option<String>`
  (new, pure): conservative pattern match against the four families
  named in the BUG-266 acceptance. Returns a bounded reason excerpt so
  the epilogue shows what the substrate said.
- `fn reason_excerpt(line, match_start) -> String` (new, pure): char-
  boundary-safe 160-char window around a match, used by the classifier.
- `fn read_headless_log_for_session(project_root, session_uuid) ->
  Option<String>` (new): glob `.aida/headless-logs/*-{session_uuid}.jsonl`
  and read the matching file. Best-effort.
- `RealPhaseDriver::run_implementer`: at the existing exit-status check
  (just after `exit_signal::spawn_and_wait`), classify on non-zero exit
  via the new helpers (only under `--no-human=both`); on a match,
  return `Ok(Inconclusive { reason, retry_hint: Some("aida queue work
  <spec> --resume <session-uuid>") })` instead of `PhaseFailure`.
- `RealPhaseDriver::resume_implementer`: symmetric classification at the
  existing exit-status check — the resume leg already has the log path
  directly, so no glob needed; same `Inconclusive` shape, same
  spec + session-id-interpolated hint.
- 3 existing `ImplementerOutcome::Inconclusive` construction sites
  (BUG-257 GH paths in `run_implementer` and `resume_implementer`)
  updated to pass `retry_hint: None` — preserves their existing
  GH-flavored epilogue.

## Critical Files

- `aida-cli/src/main.rs` — classifier + driver wiring
- `aida-cli/src/auto_complete.rs` — outcome model + epilogue

## Reusable helpers

- `auto_complete::ImplementerOutcome::Inconclusive` — BUG-257's outcome
  variant, extended (not forked) for BUG-266.
- `auto_complete::finish_inconclusive` — BUG-257's terminal-pause
  epilogue, extended with a per-leg retry-hint override.
- `auto_complete::BatchDrainOutcome::Inconclusive` — BUG-257's
  drain-pause shape; reused unchanged.
- `gh_stderr_is_network_error` (BUG-257's classifier) — direct
  structural template for `claude_log_indicates_api_outage`: pure,
  case-insensitive, anchored allow-list, paired with a negative test
  set that pins non-matches.
- `session::spawn_claude_headless` / `exec_claude_headless` — write the
  JSONL log the classifier reads; the log-path naming convention
  `<branch>-<session-uuid>.jsonl` is the contract.

## Risks + gotchas

- **Misclassification false-positives.** A clean session that happens to
  mention "Overloaded" in an unrelated context (an assistant explaining
  HTTP status codes, say) would misclassify. Mitigated by anchoring
  patterns conservatively and by the `non_outage_failures_are_not_outage`
  negative test — and by the explicit `5\d\d` requirement on
  `API Error:` so the much-more-common 400/401/403/404/429 errors do
  NOT promote.
- **The headless log may not exist yet.** A spawn-time failure
  (claude binary missing, `.aida/headless-logs/` dir not writeable)
  exits non-zero before the JSONL is written. `read_headless_log_for_session`
  returns `None` cleanly in that case and the existing PhaseFailure path
  fires — no silent classification of "the spawn itself failed" as an
  API outage.
- **No status change on Inconclusive.** Deliberately. The spec stays at
  whatever the implementer left it (`InProgress`, or even `Done` if it
  managed to get there). On retry, `--resume <session-uuid>` re-attaches
  to the persisted Claude session and the work continues from where the
  API outage interrupted.
- **Resume hint targets the session-id, not a lease-id.** The spec's
  acceptance wording used `<lease>` colloquially; `aida queue work
  --resume <SESSION-ID>` is the actual CLI surface (the
  session-id-flavored form resumes a specific recorded session). The
  hint uses the live `session_uuid` so the operator's next retry
  resumes the exact session this drain interrupted.
- **The orchestrator-mode `inconclusive` test uses the mock driver,
  which can't carry a `retry_hint` through `Ok(Inconclusive)`.** The
  classifier itself is unit-tested in `main.rs` with synthetic JSONL
  fixtures; the orchestrator-boundary test in `auto_complete.rs` proves
  the reason flows through `OrchestrationResult.inconclusive_reason`
  and that only phase 1 ran. The hint string itself is plumbed to
  `finish_inconclusive` at the production call site — covered by the
  type system.

## Tests (named)

In `aida-cli/src/main.rs` (`mod bug_266_anthropic_api_outage_classifier_tests`):

- `verbatim_529_overloaded_is_outage` — exact text from the BUG-266
  origin (TASK-358 drain on 2026-05-20).
- `api_error_5xx_family_is_outage` — 500 / 502 / 503 / 504 / 520 / 599.
- `overloaded_keyword_alone_is_outage` — Anthropic's capacity-shed
  signal in isolation.
- `upstream_connect_error_is_outage` — Envoy / proxy connectivity.
- `stream_timeout_is_outage` — SSE-stream drop variants.
- `classification_is_case_insensitive` — survives an Anthropic
  rephrasing that uppercases or lowercases a phrase.
- `clean_session_log_is_not_outage` — negative: a clean run + empty log
  return `None`.
- `non_outage_failures_are_not_outage` — negative: permission /
  HTTP-4xx / parse / `API Error: 400` errors stay outside the outage
  classifier.
- `reason_excerpt_is_bounded` — long assistant turns get truncated to
  ≤200 chars; matched phrase stays visible.

In `aida-cli/src/auto_complete.rs` (`mod tests`):

- `inconclusive_with_anthropic_hint_overrides_default` — exercises the
  new `retry_hint` field at the orchestrator boundary: the BUG-266 reason
  surfaces in `OrchestrationResult.inconclusive_reason`, phases 2-6
  never run.

## Verification

```bash
cargo test -p aida-cli --bin aida bug_266
cargo test -p aida-cli --bin aida bug_257    # regression — BUG-257 untouched
cargo test -p aida-cli --bin aida inconclusive
cargo test -p aida-cli --bin aida auto_complete
cargo test --workspace
cargo build --workspace
cargo fmt --all -- --check
```

Results: 9 BUG-266 classifier tests, 5 BUG-257 classifier tests
(regression: green), 4 inconclusive orchestrator tests, 89
auto_complete tests, 1144+ workspace tests — all pass; fmt clean.

## Followups

- **Audit a real headless-drain log corpus.** Once a handful of
  `--no-human=both` overnight drains have run with this code, audit
  `.aida/headless-logs/` for actual Anthropic-error stderr shapes and
  pin the classifier's allow-list against them — there may be wording
  Anthropic uses that none of the four families above catch.
- **Telemetry on the inconclusive path.** `~/.aida/auto-complete.jsonl`
  already records `phase-event` with `status: "inconclusive"` and the
  reason — once the data exists, a `aida usage --inconclusive 30d`
  histogram of which leg (GH / Anthropic) and which pattern (5xx /
  Overloaded / stream-timeout) is dominant would tune the recovery
  hints further.
- **Auto-retry with bounded backoff.** Out of scope for BUG-266 (a
  conscious decision — the acceptance pins classification + recovery
  hint only). A future TASK could let `--auto-complete` retry the
  paused spec once after a short delay before pausing the drain
  outright. The right shape is unclear (per-leg backoff strategy,
  retry budget across the drain) — file when the inconclusive path has
  enough live data to grade the choice.

## Related

- **BUG-257** — the parallel for the GH-API leg; BUG-266 *composes*
  with it (same `Inconclusive` variant, same `finish_inconclusive`
  terminal path, same `BatchDrainOutcome::Inconclusive` shape).
- **TASK-358** — the spec whose drain attempt produced the verbatim 529
  evidence that motivated BUG-266.
- **STORY-276** — `ImplementerOutcome::Punted` set the third-outcome
  precedent; `Inconclusive` (BUG-257) was the fourth on the same
  template; BUG-266 grows the fourth outcome rather than adding a fifth.
- **STORY-306** — the advisor tier's resume-implementer path; BUG-266's
  resume-leg classification reuses the same exit-status check on the
  same headless substrate.
- **Memory `feedback_headless_advisor_is_cold_boot`** — the headless
  implementer's external-dependency surface is broader than just the
  substrate; BUG-266 makes one of those dependencies (the Anthropic
  API itself) a first-class non-failure when it goes transient.
