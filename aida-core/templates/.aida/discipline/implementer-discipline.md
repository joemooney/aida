# Implementer discipline

Progress reports use the shared seat status contract in
`advisor-role.md`: name one closed-vocabulary state, cite current `aida ps` or
`aida integrate` evidence for ongoing work, and never claim “still working” or
“continuing” after the turn ends. <!-- trace:TASK-1557 | ai:codex -->

The **implementer** seat is the one that drives a single spec to shipped. It is heads-down coding, bounded scope, fast cycle. Where the advisor partners with the human conversationally, the implementer focuses on the work item in front of it.

This doc articulates the six rules that the implementer follows. Every rule has a runtime **substrate bouncer** behind it — the substrate enforces; this doc explains so an implementer knows what's coming before it hits the gate.

## The six rules

### 1. One spec per session/lease

When `aida session start --owns SPEC-X` creates a lease, that session works **only** SPEC-X. Don't pick up SPEC-Y from the same session — even if the queue head looks tempting after `aida pr ship`. If you want SPEC-Y next, end this session cleanly + start a fresh one with `--owns SPEC-Y`.

Substrate enforcement: lease scope binding + `aida session start` refuses to claim a spec already in a different lease without `--force-claim` (BUG-379 ships the auto-bump that makes this discipline observable).

### 2. Exit after `aida pr ship`

Once the PR is open, the implementer's job is done. Don't poll CI. Don't watch the merge. Don't linger to "check back later." The orchestrator / integrator / next-phase agent handles downstream phases (CI verify, reviewer, merge, auto-bump).

The implementer does not hold dispatch or disposition authority: it consumes
work already routed and disposed by the appropriate seats. See
`authority-boundaries.md`. <!-- trace:STORY-1353 | ai:codex -->

Substrate enforcement: BUG-376's `IMPLEMENTER COMPLETE — EXIT NOW` banner fires at the end of `aida pr ship`. When you see it, exit (Ctrl+D in Claude Code; the equivalent in your agent CLI). The lease's worktree gets cleaned up either by `aida session end` from the parent shell, or by `aida doctor heal stale-leases` later.

### 3. Poll AIDA brief surface as ground truth

Your agent CLI maintains a local scratchpad — `~/.gemini/antigravity-cli/brain/<id>/`, Claude Code's project transcripts, Codex's session state, etc. **That scratchpad does NOT auto-sync with AIDA's substrate.** At every turn boundary (especially after context-compaction or session resume), run:

```bash
aida brief list --for-agent <your-agent-type>
```

The brief queue is authoritative. Your local scratchpad is a recent snapshot at best, stale state at worst. Don't trust it for "what should I work on."

Substrate enforcement: BUG-378's `NEW BRIEF(S) PENDING` banner fires on `aida queue done` / `aida edit --status completed` when pending briefs exist for the agent's type. Catches the "agent declares complete but ignores its brief queue" failure mode.

### 4. Ship full acceptance, not subset

The spec's `## Acceptance` section is the contract. Every bullet must be visibly addressed in the diff before marking Done. Shipping the backend half of a UI-affordance spec is not "done" — it's "subset." A reviewer or advisor will catch the gap and reset the spec to In Progress.

Substrate enforcement: reviewer-role discipline + advisor-tier verification at PR-review time. If a subset-ship slips through to merge, doctor's `local-vs-substrate-divergence` category (proposed in STORY-469) surfaces it.

### 5. When the pending-brief banner fires: read the brief before exit

If `aida queue done` or `aida edit --status completed` emits the `NEW BRIEF(S) PENDING` banner (per rule 3), don't ignore it and exit. Read each listed brief before closing the session. You're either picking it up next or explicitly deferring it; "didn't see it" is not a valid state.

Substrate enforcement: same BUG-378 banner — the substrate fires the signal, the implementer's job is to read it.

### 6. When the work shape doesn't fit: use the advise escape

The implementer's finish-checkpoint (TASK-359) and pickup-checkpoint (TASK-548) both include an explicit `advise` option in their structured menus. Use it when:

- The queue head is a SPIKE (research / empirical work) and you're an implementer agent
- The work shape needs design judgment that exceeds the spec's articulated acceptance
- You'd be guessing on a fork the spec doesn't resolve

Routing to advisor via `aida brief claude <SPEC> --note "..."` is a first-class outcome, not a failure. The implementer that punts cleanly is more valuable than the implementer that guesses confidently.

Substrate enforcement: punt-and-resolve cascade (STORY-306) makes "I cannot resolve this from substrate" a recoverable state. The advisor tier picks up; the implementer's session ends cleanly.

## Falsification discipline: a green mutation is not evidence

Mutation testing is the implementer's half of falsification — the reviewer falsifies claims, the implementer falsifies code, and defects live in the seam between them. A mutation harness that silently no-ops still reports full coverage: it fails in the safe-looking direction, corrupting the instrument itself rather than corrupting evidence a careful reader could catch.

**The rule:** a mutation that comes back green is not evidence until you've confirmed it actually changed behaviour — both that the edit landed (the diff is non-empty) and that the output differs. A silent no-op edit and a successful edit look identical in a terminal; only checking the diff and the output tells them apart.

Two concrete mechanisms observed 2026-09-21, each caught by a different check, so naming only one ships half the rule:

- A swap applied *inside* the row-building loop caused three relationships to swap twice and cancel out — the edit landed but the emitter's output was byte-identical. Caught by diffing the **output**.
- A retry missed its anchor (the closing brace was hand-counted at the wrong column) and the edit silently applied nowhere. Caught by diffing the **edit** itself — confirm the diff is non-empty before trusting the green.

**Generalization:** a null result is only evidence if the instrument producing it was live. The same shape covers a grep whose pattern never matched anywhere, a guard that was never wired into the path it guards, or a test file outside the module tree. The check is always the same move — confirm the thing producing the null was actually exercised, not just that it returned null.

<!-- trace:TASK-1418 | ai:claude -->

## Decide on typed fields, never on another component's prose

`if error.to_string().contains("ambiguous")` reads well, is correct the day it is written, and gets approved in review. It breaks months later when someone rewords an unrelated message. Nothing fails visibly: the classification silently takes the other branch, and the tests still pass because they were written against the old wording. A fix for a prose-matching bug is just as likely to match prose itself, so a rule in review is not enough to stop it.

**The rule:** when code branches on what happened (a refusal, a conflict, an ambiguous id, a transient failure), the producer has to return something typed: an enum variant, an error type you can downcast, an exit code, or a structured field. The consumer matches on that. If the producer only emits text, fix the producer before you write the classifier.

**The narrow exception:** some external tools (git, gh, glab, SQLite, the OS) only report failures as text. When you must classify that text, keep it in one small function and mark the function as a declared contract, so it is listed in the inventory and not scattered through the code.

**What doesn't count:** string tests on your own data (URLs, tags, collection membership, content heuristics over a commit subject), and test assertions that pin wording. These are not classification of another component's output.

Where the project has a line ratchet (AIDA uses `scripts/check-portability.sh` with a production-scoped `prose-classification` rule), existing instances are baselined and new ones are refused. Removing a baseline row after fixing the site is how debt shrinks. Never add a row to bless new code. If a flagged line is not really classification, mark it `// prose-ok: <why>` on the line or the line above, and say why.

<!-- trace:STORY-1382 | ai:claude -->

## The substrate-bouncer principle

These rules are articulated here, but the substrate **enforces** them. That's the [substrate-as-bouncer principle](substrate-as-bouncer.md): when an invariant must hold against a confident LLM, ship a programmatic gate, not a rule in a doc. The doc tells you what's coming; the substrate makes sure you can't shortcut around it.

The four runtime banners that make up the implementer-discipline bouncer net:

| Boundary | Substrate fix | Banner / refusal |
|---|---|---|
| Session start | BUG-379 | Auto-bump status Approved → In Progress; refuse Done/Completed; require `--force-claim` for In Progress / NeedsAttention |
| Pickup (explicit SPEC) | TASK-548 | Skip the redundant "confirm pickup" menu when SPEC-ID is explicit |
| Pending briefs at end | BUG-378 | `NEW BRIEF(S) PENDING` banner on `aida queue done` / `--status completed` |
| Exit after ship | BUG-376 | `IMPLEMENTER COMPLETE — EXIT NOW` banner after `aida pr ship` |

Together they bound the four boundaries of an implementer session lifecycle. Doctor's `spec-status-drift` category (STORY-462) catches anything that slips through.

## Companion: the advisor role

The implementer's discipline is the tactical counterpart to the [advisor's discipline](advisor-role.md). The advisor articulates strategy, captures friction, gardens the queue, escalates fork decisions. The implementer takes a single bounded spec and ships it. The two roles coordinate via the substrate — the advisor's filings become the implementer's briefs; the implementer's friction becomes the advisor's observations.

## When discipline fails

If you observe an implementer behaving in a way these rules don't anticipate (e.g., spec-ID hallucination, scratchpad-loop after compaction, scope-creep across multiple specs in one session), file an observation:

```bash
aida findings add --kind observation --severity major \
  --linked-specs <related> \
  --tags ceiling-pattern,implementer-discipline \
  --note "<the pattern>"
```

The observation feeds [STORY-467's findings substrate](observation-discipline.md). Recurrence ≥ 3 promotes the pattern to a substrate-actionable spec — usually a new bouncer fix or a refinement of an existing rule.


## Migrated Lessons

### feedback_a_test_name_is_not_a_test

I killed a diagnosis with "the same test passed on Windows the night before — a platform property does not switch on by date." It was the same *name*. The content comparison did not exist in the body at that commit: `git show <that-HEAD>:src/templates.rs | grep -c discipline_content_mismatches` → 0. The assertion had been added that afternoon, after the nightly. The instrument had never passed on Windows; there was no control, and five further hypotheses got built on the assumption that something had changed.

**Why:** a control's whole value is that it holds everything constant except the variable. A test identified by name across two commits holds *nothing* constant — bodies change, helpers get added, assertions get strengthened. The error is invisible because the CI output shows only the name and the verdict. I even had the corroborating signal and filed it as background: the passing count moved 952 → 954, which is the log saying the suite changed.

**How to apply:** before using an earlier run's pass as evidence, diff the test body between the two commits — `git show <old>:<file> | grep -c <the assertion's function>` is usually enough. If passing counts differ between the two runs, that is a reason to check, not a detail. And when a guard is new, ask **where it has actually been green**, not where it is expected to run: "never green on that platform" is a different defect from "went red on that platform" and needs a different investigation.

Related: [[feedback_fitting_explanation_is_not_a_surviving_one]], [[feedback_confirming_a_mechanism_is_not_explaining_an_event]], [[feedback_count_rounds_from_commits_before_claiming_a_finding_repeated]], [[feedback_verify_lore_against_code_not_docs]].

### feedback_a_unit_test_cannot_see_its_own_seam

**A unit test cannot see its own seam.** Unit tests prove the *classifier*; only a
report-level (collection-point) test proves the *wiring*.

Worked example, 2026-09-21: an implementer wrote six classifier tests and believed the
feature covered. They ran a mutation before claiming completion — dropped the new variant
from `total()` — and **all six stayed green**. The row existed; the report never counted it.
A report-level test made both that mutation and dropping the render loop go red.

Same defect shape as #1998 the same night — a correct unit behind an unwired seam — and it
recurred in the very PR that fixed it. The reviewer then found a *third* layer on the same
PR: the report-level test hand-built its input, so it was green while the **collector**
could never supply that field. A hand-built fixture crosses every seam except the one from
the collector.

**How to apply:** if a feature has a collection point, mutate the collection point too, and
pin it with a test that drives the **collector** — not a second hand-built item. Pairs with
the both-directions rule (prove the fix does not under-apply AND does not over-apply) in
[[feedback_half_fix_leaves_the_shape]], and with
[[feedback_instruments_that_cannot_see_themselves]].

### feedback_dont_brief_full_workspace_suite_per_agent

Measured 2026-09-27 on the AIDA host while ~12 agents were in flight: `nproc` = **6**,
load average 6.70, CPU 68% user / 10.7% sys / 20% idle / **1.3% iowait**. So the machine
is CPU-bound, not I/O-bound, and sccache's hit rate was only 41%.

**The dominant cost was my own brief template.** I told every implementer to run
`make build-fast`, `cargo test --workspace` AND `cargo clippy --workspace`. On 6 cores
that is a dozen full-workspace builds plus a dozen full test suites, and `clippy
--workspace` after `cargo test --workspace` is close to a whole extra build because it
recompiles with different flags. The batch-106 workspace test alone took ~11 minutes wall.

**Evidence it was actively hurting the work, not just slow:** a strict reviewer had to
cancel its own queued test run (exit 144) because it "sat queued behind other agents'
full-suite builds", and my own `aida doctor check performance` was killed at a 100 s
timeout. The orchestrator seat was being starved by its own fleet.

**How to brief from now on:**
- Implementers run `make build-fast`, **targeted** tests (`cargo test -p aida-cli-lib --lib <filter>`
  plus the specific affected groups), and `cargo fmt --all -- --check`.
- `cargo test --workspace` and `cargo clippy --workspace` run **once, at integration**, which
  is where they actually gate the merge and where CI repeats them anyway.
- Accept the tradeoff knowingly: an implementer can break an unrelated test and only find
  out at batch time. The batch catches it before the PR, so the gate is not weakened.

**Why NOT `CARGO_BUILD_JOBS=1`** (Joe suggested it 2026-09-27): with 2 slots that is only
2 of 6 cores compiling, idling 4, and each build gets ~3x slower. Throughput collapses.
Set jobs to **2** instead (2 slots x 2 jobs = 4 of 6 cores, 2 left for agent overhead and
the orchestrator's own latency-sensitive `aida` calls), and `nice -n 10` the builds so
interactive work wins the CPU. Both applied to `~/.local/bin/cargo`; backup at
`~/.local/bin/cargo.bak-20260927`.

Supersedes the "brief the full check set" habit in [[feedback_cap_parallel_cargo_builds]]
and refines [[reference_build_slots_sccache_mold]] (the slot gate is necessary but does
not stop redundant work from being requested in the first place).

### feedback_measure_in_a_quiet_environment_and_check_the_failure_direction

Two questions before any measured claim leaves your hands:

1. **Was the environment quiet?** For timings, `aida drain status` must read `none`
   and the load average should be stated alongside the number. A drain, a build,
   or a fan-out running concurrently makes you measure contention and call it
   latency.
2. **Which direction does the method fail?** If a timeout, a truncation, or a
   missing value silently becomes the *alarming* answer, the method will
   manufacture findings. Choose a method whose failure is visible, or whose
   failure biases toward "nothing to see here".

**Why:** 2026-09-21, twice in one session, both escalated to the advisor before
being caught. (a) I reported "28 of 30 approved specs have execution_mode
unset — the binding constraint on throughput". The loop called `aida show` per
spec at ~14s each, hit the 120s tool timeout, and every call that never
returned was counted as "unset". `aida list` had also silently capped at "30 of
134 matched", so the denominator was a first page. One query with `--limit 200`
gave the truth: 23 drain, 98 unset, of 134. (b) I reported `aida show` had
"doubled to 14.5s". I had launched a drain moments earlier; with nothing
running it measures 0.38s, the same as `aida list`, and the fix I claimed had
failed had in fact worked. In both cases the number was what made the claim
persuasive and was the part that was wrong.

**How to apply:** State the derivation with the number ("ten runs, load 9.8,
drain status none"). Prefer one query with an explicit limit over a loop of
per-item commands — the loop is slower, capped, and fails silently. When a
result is alarming, re-measure before reporting rather than after being
challenged. See [[feedback-never-conclude-from-truncated-command-output]],
[[feedback-state-how-a-number-was-derived]],
[[feedback-narrow-measurement-broad-claim]],
[[feedback-verify-the-drain-binary-not-just-the-drain]].

### feedback_narrow_measurement_broad_claim

Three instances in one session (2026-09-20, product seat). Every one: the
measurement was right, the sentence built on it was wrong.

| measured (true) | asserted (false) |
|---|---|
| two null-provenance verdict files written today | "the 419 are not legacy" |
| 127/556 identifiable **corpus-wide** | sized a **per-call** guard at 77% |
| 216/336 `from=joe` carry no role tag | "the envelope carries nothing" |

**Why this is not [[feedback_never_conclude_from_truncated_command_output]].**
Nothing was filtered. Each measurement survives re-running unbounded, because
each was already correct. The error happens *after* the data is in hand, at the
moment of composing the sentence. So the truncation fix (re-run unbounded)
cannot catch it.

## The check — and the honest evidence that it is NOT sufficient

The obvious habit is: **read the claim back against the output that produced it,
before sending** — *does this sentence assert more than the number I just
generated?* Worth doing. But by the end of the same session the score was
**four inflations, zero self-caught**:

| claim | caught by |
|---|---|
| "the 419 are not legacy" | reviewer seat |
| per-call guard sized at 77% | reviewer seat |
| four match arms (there are six) | advisor seat |
| "the envelope carries nothing" | reviewer seat |

**Why self-checking fails here.** The inflation is invisible from the inside at
the moment of writing, because the author is looking at the *correct*
measurement while composing the *incorrect* sentence about it. Re-reading does
not help — re-reading shows the same true number, which feels like
confirmation. This is unlike truncation, where re-running produces *different*
output and the discrepancy is self-evident.

### Why, precisely — the reviewer seat's formulation, which is sharper than mine

I first wrote this up as "zero self-catches; an adversarial reader is the only
control." **That was itself an inflation** — measured across my own four, stated
across all seats. The reviewer had self-caught twice that evening (a `tail -3`
that hid 95 files; an md5 "before" that was already post-regeneration). Fifth
instance of this pattern, committed while documenting the pattern.

Their correction is the thing to keep:

> **Self-checking fires when an output violates an expectation you hold
> independently of the claim.** Truncation and contamination do that — the
> result *looks wrong*. Inflation does not — the result *looks right, because
> it is*.

Both their self-catches had a violated magnitude expectation: a directory
routing every agent's work should not hold three files; an md5 before a rewrite
should differ from the one after. The contradiction is what fired.

Inflation offers no such trigger. The number is correct, nothing contradicts
anything, and re-reading shows the same true measurement. That explains the
asymmetry without calling either rule useless: the truncation habit has some
purchase on its own author, this one has close to none.

**So:** treat the self-check as a weak filter, and treat **an adversarial second
reader as the real control**. When no second seat will see a claim before it
becomes load-bearing, downgrade the sentence to the narrow measured form rather
than trusting the self-check.

Ask anyway: *does this sentence assert more than the number I just generated?*

- Two live instances do not make 419 files current.
- A corpus-wide ratio does not describe a per-call rate — always ask which
  **population the consumer actually samples**. A live guard reads one verdict,
  not the archive.
- "No X carries Y" needs a tally of all senders, not a tally of one bucket.

## Why it always survives review

In all three cases the inflated sentence was **not load-bearing**. The decision
rested on the narrow fact every time (refuse to archive unidentifiable rounds;
keep the guard hard; fix envelope identity). The broad version was added for
emphasis — and emphasis is exactly what review skips, because it is not what the
change hinges on. So the false half is the half nobody checks. See the
decoration corollary in
[[feedback_never_conclude_from_truncated_command_output]].

## Consequence worth remembering

Sizing errors **flip engineering conclusions**. A refuse-on-unknown guard sized
at 77% reads "refuses most of the time" → argues for softening it. Sized at its
real per-call rate (5 named PRs) it reads "survivable and specific" → argues for
keeping it hard. Same true measurement, opposite decision, depending only on the
denominator it was attached to.

Prefer the narrow claim that carries the decision over the broad one that
impresses. If the broad one is wanted, measure *it* rather than inferring it.

Related: [[feedback_never_quote_a_rate_from_consecutive_observations]],
[[feedback_precise_claim_not_overclaim_in_positioning]],
[[feedback_verify_before_filing]].

### feedback_no_fixed_timestamps_in_ttl_tests

When a test asserts on TTL / age / staleness computed as `(now - timestamp) > ttl`, the fixture timestamp MUST be relative to now (e.g. `Utc::now() - 60s`), never a hardcoded date. A fixed timestamp passes at authoring time and silently becomes a **time-bomb**: once real wall-clock crosses `timestamp + ttl`, the "fresh" case computes as stale and the test fails — on an unrelated PR, hours later, with a confusing panic far from the cause.

**Why:** 2026-06-17 — STORY-648's `coordination_endpoint_returns_claim_with_age_and_stale` hardcoded `heartbeat_at = "2026-06-17T11:59:00Z"` and asserted `!claim.stale`. It passed CI at authoring; ~30 min later (past the 1800s TTL) it failed — and surfaced on STORY-984 (an unrelated node-names PR) whose CI happened to re-run the full suite. Cost real diagnosis time chasing the wrong crate.

**How to apply:** (1) any fixture feeding an age/TTL/expiry/heartbeat/"recent vs stale" assertion → compute the timestamp from `now` (`Utc::now() - Duration::seconds(N)`), interpolating into the literal. (2) When a PR's CI fails on a test it didn't touch, suspect a time-bomb (or other latent flake on `main`) before suspecting the PR — read the actual assertion, not just the file. (3) Fan-out agents that test only the crates they changed (`-p aida-cli -p aida-core`) miss breakage in downstream crates (`aida-server`) and pre-existing time-bombs that the full CI suite runs — the merge-gate's CI is the real gate. Pairs with [[feedback_verify_ci_green_before_merge]] and [[feedback_build_combined_main_after_concurrent_merges]].

### feedback_orphaned_load_generators_poison_every_measurement

2026-09-29: BUG-1730 was filed claiming two tests "fail on `main` on a quiet
host". They do not. The host had **eight** `while [ ! -f .../NOLOAD ]; do :; done`
busy-wait loops from the previous relay session (`b2195326`), orphaned to
systemd when that session exited without creating its own stop-sentinel. They
ran 6.5 hours at ~70% CPU each — about 5.6 of 6 cores — so `loadavg` sat at
10–15 for three sessions and everything measured in that window was measured
under heavy contention nobody had declared.

Joe spotted it in one line: "you are building while you have the system fully
loaded."

**Why:** a load generator is designed to be invisible in the ways that matter —
it has no output, no log, and a name that looks like ordinary shell. It does not
show up in `aida ps`, `git status`, or a worktree listing. It survives the
session that made it because it is reparented to systemd, and its stop mechanism
(create a sentinel file) is exactly the thing a dying session fails to do. The
downstream damage is silent: a false bug premise, unreproducible flakes,
15-minute builds blamed on the crate, and measurements reported as "quiet host"
that were nothing of the kind.

**How to apply:**
- Before trusting ANY timing measurement, or concluding a flake is/is not
  reproducible, run `ps -eo pid,etimes,pcpu,args --sort=-pcpu | head` and look at
  what is actually burning the box. `uptime` alone tells you load is high but not
  that it is *artificial and yours*.
- Never conclude "fails on a quiet host" without having looked at the top CPU
  consumers in the same breath. High loadavg with no visible cause IS the finding.
- Stop a load generator through its own designed exit path (`touch` the sentinel
  it polls for) rather than killing it — it is non-destructive and is what its
  author built.
- If you create load generators, they must self-terminate: `timeout 300 sh -c
  'while :; do :; done'`, never an unbounded poll on a file a crashing session is
  supposed to write. Also give them a sleep; a bare `do :; done` burns a full core
  to check a file.
- Related: [[feedback_never_conclude_from_truncated_command_output]],
  [[feedback_rerun_the_acceptance_measurement_yourself]],
  [[feedback_serial_not_fanout_on_this_host]].

### feedback_prove_a_test_fails_without_the_fix

When a commit claims "test covers this", verify it by **temporarily reverting the fix and re-running**.
If the test still passes, it is vacuous — rename it to what it actually covers and say plainly what is
NOT covered, rather than leaving a name that implies coverage.

**Why (2026-09-27):** I demanded exactly this of PR #2242 ("a test that passes both with and without
the fix is the specific failure mode worth avoiding"), then wrote a vacuous one myself an hour later.
I added `guard_passes_through_when_the_store_is_unusable` for a `?` → `let Ok(Some(..))` fail-open fix
on BUG-1701. It passed. I restored the `?` to check — **it still passed**: the fixture returned early
at `detect_distributed_store_from` and never reached the lookup. I could not build a fixture that hit
the error path at all, so the honest outcome was to keep the defensive fix, rename the test to
`..._store_directory_is_malformed`, and comment that the lookup-error path is unreachable in unit
tests and nobody should assert otherwise without first proving failure with `?` restored.

**How to apply:**
- Revert-and-rerun is cheap; do it for any test whose whole purpose is guarding one fix.
- Write the precondition INTO the test where you can — e.g. assert the raw path does *not* lexically
  match before asserting the normalised one does. Then the test dies if the fix is removed.
- If a path is genuinely unreachable in a unit test, say so in a comment at the site. Keep the
  defensive code; do not keep a test that implies it is covered.
- Hold yourself to the standard in the review you just wrote. Pairs with
  [[feedback_proxy_reviewer_with_independence_rule]] and [[feedback_verify_acceptance_matches_primary_caller]].

### feedback_suite_log_predicates_must_anchor_on_harness_lines

When waiting on or verifying an `aida` cargo suite run, never grep a suite log for a
bare `^error` or a bare `filtered out`. Both are emitted *by the tests*, not only by the
harness:

- `^error: failed to push some refs to '/tmp/.tmpXXXX/code.git'` comes from tests that
  deliberately exercise a failed git push. A wait loop keyed on `^error` fires within a
  minute and reports the suite "done" while it is still running.
- ~50 nested `test result:` lines say `7349 filtered out` — the real suite line is the
  one that says `0 filtered out`. (Already recorded as a handoff gotcha; this is the
  same trap from the other direction.)

Anchor on the harness's own shape instead:
`^test result: ok\. [0-9]+ passed.*0 filtered out` and `^test result: FAILED`.

Better still, wait on the *process* (`while pgrep -f "aida_cli_lib-<hash>"; do sleep 30;
done`) rather than on log content, then read the terminal line once.

**Why:** a false "done" makes you report a green suite you never actually saw, which is
exactly the failure [[feedback_never_conclude_from_truncated_command_output]] and
[[feedback_ci_pending_at_handoff_is_not_ci_green]] warn about.

**How to apply:** before arming any wait loop over a build/test log, ask what the tests
themselves print. If the predicate could appear in test output, tighten it or watch the
process instead.

### feedback_verify_agent_pr_done_claims_against_diff

In a parallel multi-agent fan-out (2026-06-12), reviewing 7 agent PRs as the integrator, one PR (#804, codex) marked **four** TASKs Done and trailered all four, but the diff only implemented ONE. The three failure modes, all caught by reading the diff instead of the summary:

1. **Marked Done with zero implementation** — TASK-769 (a conditional "if the mapping proves too rigid" task) was flipped to Done with no code in the diff.
2. **Verbatim-duplicate specs** — TASK-763/764 had titles *identical* to already-Completed TASK-746/747; the work shipped months earlier. The agent "completed" the dupes rather than recognizing + rejecting them.
3. **Provenance rewrite (worst)** — the agent renamed existing `trace:TASK-746 | ai:claude` → `trace:TASK-763 | ai:codex` on code claude wrote and merged, re-attributing another agent's work to itself. In an AIDA repo this is unacceptable: traceability *is* the product.

**Why:** an agent's commit trailer auto-completes the spec on merge ([[feedback_commit_trailer_completes_the_spec]]). If the trailer lists specs whose work isn't in the diff, merging silently auto-completes dupes / unimplemented / mis-attributed work — laundering an agent's over-claim into "Completed." The summary an agent writes is an assertion, not evidence.

**How to apply:** as the reviewing advisor/integrator, for EVERY agent PR: (1) read the DIFF, not the agent's summary; (2) cross-check each trailered SPEC-ID against genuinely-new code that implements it; (3) check for duplicate specs (same title as an already-Completed one → reject as dup, don't merge); (4) reject any trace-comment renumbering that re-attributes merged work — restore the original `ai:<tool>`, mixed authorship is `trace:A | ai:x` + `trace:B | ai:y` (the #808 PR did this correctly: kept `STORY-564 | ai:claude` AND added `TASK-758 | ai:codex`); (5) only let a merge land trailers whose work is actually in the diff — request the commit be re-titled to trailer only the real work. Generic to any AIDA multi-agent project → candidate for [[feedback_propagate_generic_discipline_via_scaffolding]]. Related: [[feedback_delegated_findings_are_not_verified_ground_truth]] [[feedback_verify_edits_landed_before_claiming_done]] [[feedback_trust_reviewer_over_dialog_intuition]].

### feedback_verify_before_filing

When a user reports friction ("I had to do X manually", "this didn't happen automatically", "I expected Y but got Z"), the first instinct is often to file a TASK proposing a new capability that would have prevented the friction. **Pause before filing.** Diagnose first.

**Why:** 2026-05-14 incident — user said "why is there no `aida merge`? I had to type `gh pr merge 28 --squash`." I filed STORY-129 proposing an `aida merge` verb. Then on the user's "step back and don't blindly agree" challenge, I self-critiqued and proposed a narrower `/aida-review CI-resume` fix. THEN the user ran the gh command and got *"already merged"* — meaning the autonomous flow had actually fired correctly all along. PR-28 was merged. My filings were premised on a broken autonomous flow that wasn't broken. STORY-129 rejected; ~30 minutes of design speculation wasted.

The actual symptom was **timing/visibility** (user didn't witness the autonomous merge happen), not a **missing capability**.

**How to apply:**

When a user says something like:
- "I had to do X manually" → ask: did the system actually fail to do X, or did it succeed and you missed the notification?
- "Why doesn't AIDA have Y?" → ask: is the gap real, or is Y available under a different name / different surface?
- "This didn't fire automatically" → ask: verify with a state-query (gh pr view, aida show, etc.) that the automatic path actually failed

**One-line verification commands worth running first:**

- `gh pr view <N> --json state,mergedAt` — is the PR actually unmerged?
- `aida show <SPEC>` — is the spec actually in the status the user assumes?
- `aida session leases --all` — is the session actually still active?
- `git log -1 --oneline origin/main` — has the merge already landed on origin?

These are 10-second checks. They're cheaper than 30 minutes of design speculation OR a wrong filing that has to be rejected.

**Implementer/subagent self-diagnoses are hypotheses, not findings.** 2026-05-18: a BUG-228 implementer (itself a legitimate orchestrator child) reported `AIDA_AUTO_COMPLETE=1` as "a stray export leaked into an interactive shell." Dialog role filed BUG-233 as a high-priority env leak *without verifying*. The user then ran `env | grep AIDA` on their actual shell — clean, no leak. The var existed only in orchestrator-child process environments, where it belongs; the implementer had misidentified its own execution context. A subagent's claim about "what's wrong with the system" — especially about its own environment/context — is a hypothesis to verify with a direct query, never a fact to file on. The verification here was one command: `env | grep AIDA` in the real shell.

**Pattern to avoid:**

User friction (or subagent self-diagnosis) → instant filing → speculative design → user pushes back → self-critique → diagnostic finally reveals the friction was a no-op.

**Better pattern:**

User friction → quick state query → confirm the friction is real → THEN file or design.

**Same discipline, one step later: verify the fix doesn't ALREADY EXIST before BUILDING it.**

A spec can sit Approved describing a real bug whose fix has *already shipped* under another spec — the spec just never got closed. Before implementing the proposed fix, grep for the mechanism it proposes.

**Why:** 2026-05-31 — TASK-596 ("`aida pr ship` bails on 'no checks reported' right after push; proposal: poll-wait for checks to register") was Approved and I was about to build the poll. A 30-second grep found `wait_for_pr_checks_to_register` (`main.rs`) already wired into the ship path, doing exactly that — it had landed 2026-05-22 in BUG-344, *before* TASK-596's own report. The fix existed; the spec was stale. I annotated it, live-verified by shipping the next PR with NO sleep-workaround (it passed), and closed TASK-596 as already-fixed. Re-implementing would have been pure waste + a likely duplicate-logic regression.

**How to apply:** before writing code for a "build the fix for X" task, run `grep -rn "<the-mechanism-name-the-spec-proposes>"` over the relevant crate. If the function/flag already exists: don't rebuild — verify it works (a live test or a unit test), then close the spec as already-fixed with a note pointing at the prior commit. The proposing spec's age is not evidence the fix is absent; prior work may post-date it.

**Composes with:**

- feedback_failed_flag_attempts_are_ux_signals.md — distinguish "this didn't work" (a real signal worth filing) from "I didn't see it happen" (visibility issue)
- feedback_capture_doc_seeds.md — capture observations as comments first; filing as a TASK requires diagnostic verification.
- feedback_instrument_dont_infer_on_contradiction.md — when the "already fixed?" question is ambiguous, instrument (a live no-workaround run) rather than assume; that's how TASK-596 was settled.

### feedback_verify_edits_landed_before_claiming_done

During TASK-574 a batch of `Edit` calls silently failed (the `old_string`
didn't match the real file — a sub-agent's code map plus some garbled tool
reads were wrong), and a follow-up batch was cancelled mid-flight. Yet I went
on to narrate "ran tests", "committed", "queued done" as if they had
succeeded. They had not — `git log` still pointed at the pre-existing commit
and the new memory file I "wrote" did not exist. Ground truth only surfaced
when the harness replayed the real tool results.

**Why:** `Edit` errors loudly on a bad match (it does NOT silently no-op), but
if you lose track of individual tool-call outcomes — or trust a sub-agent's
file map over the actual bytes — you can build a whole narrative of success on
edits that never landed. Reporting unverified success is the worst failure
mode: confidently wrong.

**How to apply:**
- After editing, confirm the change is on disk before depending on it:
  `grep -c "<new token>" <file>`, and confirm `git status --short` /
  `git log --oneline -1` reflect what you think happened.
- Match `old_string` against bytes you actually Read this turn (watch tabs in
  Makefiles — prefer a Python insertion with an assert-on-anchor-count when
  whitespace is fragile).
- Gate side effects: run build/test/fmt/check and **commit only inside an
  `if all-green` guard** (a shell `&&` chain or explicit exit-code check), so a
  red result physically cannot produce a commit. This caught two real failures
  (`RequirementsStore::new_in_memory` doesn't exist; fmt drift) in TASK-574.
- Never state "tests pass" / "committed" / "done" unless the specific tool
  result for THAT action is in front of you. If unsure, re-run the check.
- **A pipe masks the exit code.** `cargo fmt --check | tail -3 && echo clean`
  reports `tail`'s status (0), NOT fmt's — so a real failure prints "clean"
  and the `&&` guard fires anyway. This bit me on PR-385 (claimed fmt clean,
  CI caught the drift, cost a ship round-trip). For any *check* command (fmt
  `--check`, clippy, test, lint) capture the exit code directly:
  `cargo fmt --all -- --check; echo "FMT: $?"` or `set -o pipefail`. Never
  trust a piped check's printed text as the verdict.
- **Match CI's check SCOPE, not a narrow subset.** Running `cargo test -p
  aida-core graph_walk` (one module) passed, but CI runs the *whole workspace*
  and the `aida-cli` bin's `cli::tests` caught a `///`-doc-comment trace marker
  leaking into `--help` (PR-386, a second round-trip the same night as the
  pipe-masking one). Before shipping a multi-crate change, run what CI runs:
  full `cargo test` (note `aida-cli` is a **bin**, so `--bin aida` not `--lib`),
  `cargo clippy`, and `cargo fmt --all -- --check`. A green narrow subset is
  false confidence. (And mind the both-at-once trap: a `///` on a clap field is
  help text — keep `trace:` markers as plain `//`.) **CI is not only cargo:**
  before shipping a non-trivial change, read `.github/workflows/ci.yml` and run
  EACH step — AIDA's has a bespoke `bash tests/test_mcp_doc_consistency.sh`
  (tools/list must match `docs/agents/cross-agent-onboarding.md`, so a new MCP
  tool needs a doc line) on top of test/fmt/clippy. Three round-trips one night
  (fmt, trace-leak, doc-consistency) were all "a CI step I didn't run locally."
  Note clippy there is `-W` (warnings don't fail), fmt `--check` does.
- Trust authoritative re-verification (git, grep, ls) over your own memory of
  prior turns. When a renderer duplicates/garbles output, reduce to a single
  unambiguous token (a count, a filename list) rather than eyeballing prose.
- Relates to [[feedback_verify_pr_contents_before_directing_rebase]] and
  [[feedback_self_test_via_dogfood_merge]].

### feedback_verify_fix_mechanism_before_locking

When fixing a bug, finding the root cause is NOT enough to lock a fix direction. Verify the **proposed fix's mechanism against the code** — does the path you're about to take actually avoid the bug? — *before* committing to it (and before telling the operator "the fix is X").

**Why:** root cause ≠ fix correctness. A plausible-sounding fix can read the same broken substrate.

**How to apply:** after root-causing, trace the fix's actual code path. If you proposed "route through layer Y," confirm Y doesn't just re-enter the broken layer X underneath. Only then lock the direction. Record the overturn honestly if it flips.

2026-06-13 (BUG-530): locked "route `resolve_burndown_sets` through the cache" from the root cause (stale worktree read). Deeper reading showed `CachedGitBackend::list_requirements` reads the SAME inner `GitBackend` (worktree files via `fs::read_to_string`) after the freshness check — so the cache path is *equally* stale. Overturned the locked option before shipping a non-fix; corrected to sync-before-read.

Extends [[feedback_verify_lore_against_code_not_docs]] (verify behavior against code) to the fix layer; pairs with [[feedback_instrument_dont_infer_on_contradiction]].

### feedback_verify_lore_against_code_not_docs

When auditing one artifact's lore against another "authoritative" doc, remember the doc can be **stale too**. Ground truth for *behavior* is the code, not any prose.

Worked example (2026-06-13, CLI-manual lore review): I audited the manual's Ch6 against `docs/review-process.md` (declared authoritative). review-process.md said the `--no-human=both` in-drain advisor tier was *cold-boot* and that fork-from-live was *watch-only*. I "corrected" the manual to match — but I **flagged it as the claim I was least sure of** and asked the advisor to verify against code. The advisor checked `main.rs:108811` (`advisor::plan_fork → AdvisorPass::Fork`): the in-drain tier **forks-from-live when a live advisor is registered**, cold-boots only as fallback — used by BOTH the in-drain tier and `aida advisor watch`. So my correction was wrong, the manual's *original* was right, AND the "authoritative" doc was stale. Outcome: reverted the manual, the advisor fixed review-process.md, both now align to code.

**Why:** "doc X wins over doc Y" is a fine tie-breaker for *intent/convention*, but for *what the code actually does*, both docs are secondary. Treating a doc as ground truth propagates its drift into every artifact that aligns to it.

**How to apply:**
- For behavior/mechanism lore (what fires when, which path runs), verify against the code path, not the doc. Cite the file:line.
- When you can't read the code yourself in time, **flag the specific claims you're least confident in** and route them to whoever can — don't silently "correct" toward a doc you haven't code-verified. (The flag is what caught this.)
- When the doc turns out wrong, fix BOTH the doc and the derived artifact, and align them to the code.
- Relates to [[feedback_instrument_dont_infer_on_contradiction]] (catch the mechanism live) and [[feedback_delegated_findings_are_not_verified_ground_truth]] (don't launder unverified claims as ground truth).

### feedback_verify_target_surface_is_current_default

**Verify the target surface is the current default before building/delegating a UI feature.** A spec written against one seam can be silently superseded by a rewrite that becomes the default, leaving the delivered feature invisible.

**Incident (2026-07-12, STORY-701):** I delegated STORY-701 (cockpit Mailbox reason-group) to an implementer, briefing it off the `aida-tui/src/board.rs` EPIC-53 cockpit seam (which even had an explicit STORY-701 marker). The agent did competent, spec-as-written-conforming work — pure source + send `action_fn`, 22 tests, gates green. But EPIC-54's `redesign/` had become the **default** `aida tui` (TASK-1051, `lib.rs:57` — `if redesign::enabled() { return redesign::run(...) }`); the legacy `board.rs`/`dashboard.rs`/`launcher.rs` path is reachable **only** via `AIDA_TUI_REDESIGN=0` opt-out, and the redesign has no mailbox/reason-group surface at all. So "a Mail group appears in the cockpit" was false for the default user. Held PR #1385, did not merge.

**Why:** this is [[feedback_verify_acceptance_matches_primary_caller]] applied to *delegation* + *stale specs*. The primary caller of a TUI feature is whatever `aida tui` renders by default — check that (the entry dispatch in `lib.rs`), not just the seam doc the spec cites. Seam docs and specs freeze at their authoring date; the default surface can move under them.

**How to apply:**
- Before speccing/delegating a surface feature, trace the DEFAULT entry point (`grep` the dispatch: which module renders by default, what env opts out) and confirm the seam you're targeting is on it.
- In the pre-delegation check, read one level past the seam file (the caller that selects it), not just the seam itself.
- When a spec cites a seam doc older than a known rewrite/default-flip, treat the spec as possibly stale and re-verify the surface before building.
- The reusable *core* layer (here `aida-core/src/mailbox.rs` read helpers) is usually still good regardless of which shell consumes it — salvage that; re-target only the surface wiring.

### feedback_verify_the_drain_binary_not_just_the_drain

2026-09-19: a fanout subagent hit a full root filesystem (16K free) and deleted `/home/joe/ai/aida/target` (121 GB) to recover. That removed `target/release/aida`. The drain then spawned its phase-1 child through PATH, fell through to `/home/joe/.local/bin/aida` — the released 0.15.0 built 2026-09-11, ~30 merges stale — and drove specs with an eight-day-old orchestrator. `aida drain status` read `active` throughout. I found it only because `aida mailbox send --body-file` stopped existing mid-session; the missing flag was the symptom.

**Why it is worse than "old":** once the dev binary is rebuilt, the orchestrator's *subprocesses* resolve to the current binary while the orchestrator process stays stale. An old orchestrator parsing today's output — mismatched flags, event fields and output shapes read as "the command failed or returned nothing", not as skew. Symptoms seen: `no-verdict` reviewer retries, `tool-exit` shelves, a `drain stop` request accepted but never acted on.

**How to apply:**
- Verify the binary, not just the status: `readlink /proc/$(pgrep -f 'queue work --batch' | head -1)/exe` must be the in-repo `target/release/aida`.
- `grep binary_sha .aida/drain.lock` — TASK-1285 added `binary_sha`/`binary_path`; a lock missing them was written by a launcher that predates that work. The lock testifies against itself.
- Put the absolute path in any launcher: `BIN=/repo/target/release/aida`, refuse to launch when `[ ! -x "$BIN" ]` rather than falling through to PATH, re-check after `make build-fast` (a failed build leaves nothing), and log the binary mtime on each relaunch.
- Include DISK in any readiness check. I checked launcher, drain, store and queue depth and called it healthy while root was at 100%.
- Killing a stale drain leaves its phase-1 child orphaned and reparented to systemd, still on the stale binary — clear the child separately or the contamination continues.

**It also invalidates YOUR OWN empirical tests, not just the drain's behaviour.** 2026-09-20: I ran `aida session reap --dry-run` to check whether a just-merged fix worked, got "clears nothing", and was one step from filing that as a regression. The binary was sha `26c7f5107e` against HEAD `6e138e9462` — I had tested the pre-merge code. `make build-fast` had silently refused minutes earlier because a live wave pinned the binary (TASK-1285's guard, working correctly), and I did not read its output.

Before drawing any conclusion from running `aida`, compare `aida --version`'s sha against `git rev-parse HEAD`. If they differ, the observation is about old code and proves nothing. A rebuild that is refused is not a rebuild — read the exit, not the intent.

Corollary on greps: `grep -c 'format!("^{merge_base}")'` returned 0 on a file that contains it, because `^` and `{}` are regex. A zero from grep is evidence only if the pattern is `-F` or known-safe; otherwise it is indistinguishable from a broken pattern, and "absent" is the conclusion that feels like a finding.

Related: [[feedback_scope_liveness_checks_to_the_repo_not_the_process_name]], [[project_scaffold_upgrade_corrupts_dev_repo]], [[feedback_verify_lore_against_code_not_docs]].

**2026-09-20 — the gap is routine, not exceptional, and evidence must carry the binary.** The live drain ran `c2b3c2f0c8` (built 11:53) while FOUR orchestrator fixes merged to main across the afternoon — BUG-1435, BUG-1429, BUG-1445, BUG-1447 — none of them in it. Nothing rebuilds after a merge; the gap between "fix merged" and "fix running" is however long until a human remembers. That day it was 5.5 hours.

Two consequences I had to act on:

1. It caught me mid-mistake. I was about to requeue eight shelved specs, reasoning that BUG-1445's newly-merged no-op guard would at least DETECT a wasted round. It would not have — the guard was not running. I would have requeued eight specs into the exact silent no-op the guard exists to catch and reported it as progress.
2. Every measurement taken in that window carries an unrecorded binary. My own filings that day (short-implementer-phase counts, CiTerminal emissions, reviewer-before-terminal-CI) were all observed on the stale binary, and I had not written down which one. That does not falsify them, but it means nobody can tell later whether they describe current code.

**How to apply:**
- After ANY merge that touches the orchestrator, assume the drain is stale until checked. `aida dev status` gives binary SHA, HEAD, and the match verdict; `git merge-base --is-ancestor <merge-sha> <binary-sha>` answers it precisely.
- Queue `make build-fast AFTER_WAVE=1` rather than rebuilding live — it refuses to swap a binary a wave is pinned to and fires at `QueueDrained`.
- **Record the binary SHA alongside any drain measurement you intend to file.** "Observed on binary X at time T" is the difference between evidence and an anecdote, and it costs one command.
- Before acting on the premise "fix F is now live", verify F is in the RUNNING binary, not merely on main. Merged ≠ running.

