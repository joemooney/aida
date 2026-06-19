# Experiment: `aida compete` quality-variance — competition-as-QA measured (cross-vendor judge, n=3)

- **Date:** 2026-06-19
- **Probe:** EPIC-48, Program 2 (cross-vendor competition, P2/L5). Spec TASK-874.
- **Status:** Run 3 of the competition program. A small-n measurement (n=3 moderate tasks), not a settled rate. The honest result is mixed — read the finding.
- **What it tests:** the open question TASK-874 inherited from the open-brief run (`2026-06-18-open-brief-convergence.md` section 7). That run reframed multi-vendor competition from *design diversity* to *quality-variance QA* — but on n=1. This run asks the measurable version of the claim, using the cross-vendor judge that now exists (TASK-869, `--judge --judge-vendor codex`):
  1. Does running N vendors surface a **measurable quality delta** between candidates (gate pass/fail, test count, edge-case handling, diff focus)?
  2. Does the judge reliably pick the **higher-quality** candidate with a defensible rationale?
  3. How often would single-vendor (take one and ship) have shipped the **worse** candidate?

## Why this design, and why not the original framing

TASK-874's original framing was a *regression-catch rate* — "X regressions caught per N competed specs that single-vendor would have merged." That needs contrived regression-inducing specs (you have to engineer a trap a vendor reliably falls into), which is hard and chancy. This run reframes to what is **directly measurable on real-ish tasks**: run both vendors on small self-contained functions that each carry a subtle correctness trap, and measure the quality variance + judge discrimination that competition surfaces. If the answer is "minimal variance at this task size," that is itself a finding — it says competition-as-QA needs bigger tasks to pay.

**Setup (cost-bounded on purpose):**

- A throwaway Rust crate (`compete-qa`) with its own `aida init` store — never the real `.aida-store`. Three task specs, each a small function with an edge case where vendors plausibly differ.
- For each: `aida compete TASK-N --vendors claude,codex --judge --judge-vendor codex --gate "cargo test --quiet"` in the throwaway repo. The gate mirrors a real CI floor (build + run the unit tests), so a gate-pass means actually-correct-on-the-stated-cases.
- The judge is **cross-vendor** (a Codex instance grading a Claude-vs-Codex run) — this removes the same-model-grades-itself caveat that capped runs 1 and 2.
- Captured per arm: gate result, test count, diff size (split out plan-doc lines from code lines), the judge's pick + rationale, **and my own independent read of the two diffs** (noting my bias — I am Claude, and on two of three tasks I am grading against the Claude candidate).

The three tasks and their traps:

| Spec | Function | The trap |
|---|---|---|
| TASK-1 | `parse_roman` | subtractive notation (`IV`=4, `MCMXCIV`=1994) — naive additive parsing gets it wrong |
| TASK-2 | `compare_versions` | numeric-not-lexical components (`1.10` > `1.9`); plus arbitrary-length components |
| TASK-3 | `rle_decode` | multi-digit counts (`a12` = twelve a's), malformed input, zero counts, count overflow |

## Per-task results

Code lines exclude the `docs/plans/*.md` artifact some arms wrote (the ultraplan-assembled brief includes AIDA's 11-section plan structure, so a vendor that follows it inflates its diff with a process artifact, not code).

| Spec | Vendor | Gate | Tests | Code lines | Judge total /20 | Judge pick |
|---|---|:--:|:--:|:--:|:--:|:--:|
| TASK-1 roman | claude | pass | 5 | 110 | 18 | |
| TASK-1 roman | codex | pass | 5 | 77 | **19** | **codex** |
| TASK-2 version | claude | pass | 4 | 75 | 17 | |
| TASK-2 version | codex | pass | 4 | 69 | **19** | **codex** |
| TASK-3 rle | claude | pass | 8 | 109 | **19** | **claude** |
| TASK-3 rle | codex | pass | 6 | 74 | 17 | |

All six arms built and passed the gate. So on the *stated* acceptance criteria there was zero variance — every candidate was mergeable. The variance lives below the gate, and it differs sharply by task.

### TASK-1 (roman) — minimal variance, near-tie

Both vendors converged on the identical algorithm: a right-to-left scan, subtract a symbol smaller than the one to its right. Both correct, both case-insensitive, both reject non-Roman characters. The only deltas were cosmetic: Codex was more compact (77 vs 110 code lines); Claude's tests covered a couple more rejection cases (embedded/trailing spaces). **My read: a genuine near-tie**, Claude marginally ahead on test thoroughness, Codex marginally cleaner. The judge picked Codex 19-18 on diff focus. A defensible tie-break, but not a quality call — single-vendor would have shipped a correct parser either way.

### TASK-2 (version) — real, demonstrable variance; the QA signal fires

This is the one task where competition earned its cost.

- **Claude** parses each component as `u64` (`s.trim().parse::<u64>().ok()`), with non-numeric falling back to 0. Correct on every stated case — but a version component exceeding `u64::MAX` silently parses to `None` and is treated as **0**, mis-ordering the version.
- **Codex** never parses to an integer: it strips leading zeros, then compares by `len().cmp().then(lexical)`. This is correct for **arbitrary-length** components, and Codex wrote a test for exactly that case.

The defect is real and reproducible. Claude's function returns `Equal` for two clearly-unequal versions:

```
compare_versions("1.999999999999999999999999", "1.1000000000000000000000000")
  claude => Equal      (both components overflow u64 -> both become 0)
  correct => Less      (9.99e23 < 1.0e24)
```

This is precisely the competition-as-QA case: **a single named brief, two vendors, one ships a latent correctness defect on an unstated-but-real edge case, the other doesn't, and only running both surfaces it.** The cross-vendor judge caught it — its rationale explicitly cited "avoids integer overflow" — and recommended Codex 19-17. **My independent read agrees with the judge**, and notably the judge picked *against* the Claude candidate, which is the bias control working in the right direction. Single-vendor that happened to take Claude would have shipped the overflow-fragile version.

### TASK-3 (rle) — minimal code variance; the judge's margin was a process artifact, not code quality

Both implementations are correct and both handle every trap: multi-digit counts, leading-digit rejection, trailing-char-without-count rejection, zero-count rejection, and count overflow (Claude via `parse().ok()?`, Codex via `checked_mul`/`checked_add`). Codex even added a nice `"a01" -> "a"` leading-zero test. **My read of the code: a near-tie.** But the judge picked Claude 19-17, dinging Codex on spec-adherence (3/5) because **Codex omitted the plan-doc deliverable** the ultraplan-assembled brief asks for; Claude wrote a 130-line `docs/plans/` file. So the judge's margin here turned on a **process artifact, not code correctness** — an honest caveat about what the judge is actually scoring when the brief carries AIDA's plan discipline.

## The finding

> **Measurable quality variance from multi-vendor competition is real but task-dependent, and at this task size it was the exception, not the rule: 1 of 3 tasks showed a substantive, demonstrable correctness delta (the version-overflow edge case); the other two were near-ties in code quality where the judge's margin came from diff focus or a process artifact. The cross-vendor judge discriminated correctly on the one task where it mattered — including picking against the Claude candidate — but its margins on the two near-ties reward conscientiousness and brief-process-adherence, not code quality. Competition-as-QA caught a defect single-vendor would have shipped on 1/3 of these tasks; on small, sharply-specified functions the modal outcome is convergent, equally-correct candidates where the choice is a wash.**

Three things this establishes and one it does not:

1. **Competition-as-QA is not a null effect.** The version-overflow case is a clean existence-proof at n>1 that running two vendors surfaces a real correctness delta on an unstated edge case that a passing gate did not catch. The reframe from the open-brief run survives a second, cross-vendor-judged datapoint.

2. **The cross-vendor judge can discriminate.** On the one task with a genuine quality gap, a Codex judge correctly identified it, cited the specific reason, and recommended the better candidate over the same-family one. That is the discrimination the QA framing needs.

3. **The variance rate scales with the unstated surface, not the task.** Variance appeared exactly where the brief left an edge unspecified (huge version components) and the substrate did not force the answer — consistent with the open-brief run's "divergence survives only in the residual the substrate leaves unspecified." Roman parsing and RLE decoding had their edges fully enumerated in the brief, so both vendors nailed them and there was nothing left to vary on. **Corollary: to make competition-as-QA pay, compete on tasks with a large unstated-edge surface** (open briefs, fuzzy correctness, real-codebase integration) — not small functions whose every edge is in the acceptance list.

4. **What it does NOT establish:** a *rate*. n=3, two vendors, one judge vendor, tiny tasks — "1 of 3" is an anecdote with a denominator, not a measured catch-rate. The judge's near-tie margins also show it is scoring conscientiousness + process adherence as much as correctness, so a judge-driven "winner" is not a clean quality signal on convergent candidates. A real rate needs larger, less-fully-specified tasks (where variance is the rule, per finding 3) across more vendor pairs and both judge directions.

## Honesty / limits

- **n=3, small tasks, one vendor pair, one judge direction.** Pilot-scale. The single positive (TASK-2) is one observation, not a frequency.
- **Self-relevance, reduced but not gone.** The judge is cross-vendor (Codex grading Claude-vs-Codex), which is the right control — but *my* independent read is a Claude instance grading two diffs, one of them Claude's. I called TASK-2 against the Claude candidate, which is the bias pushing the honest direction; I cannot rule it out on the two near-ties.
- **The gate is necessary, not sufficient — by design, and that is the point.** All six arms passed the gate; the variance was entirely sub-gate. This is the whole case for a judge/competition layer on top of CI, and equally the warning that the layer's signal is weak when candidates converge.
- **Task selection plausibly suppressed variance.** Fully-specified small functions are close to the convergent regime the open-brief run predicted. A run on larger or under-specified tasks is the obvious next row and would likely raise the catch-rate — which is itself the finding (finding 3), not a flaw to apologize for.
- The throwaway store and all `compete/*` branches + worktrees were cleaned up after capture; nothing touched the real `.aida-store`.

## What it does / doesn't do for the product

- **Does:** give `aida compete`'s QA framing a second, cross-vendor-judged datapoint with one clean catch — and a sharper pitch: *compete on the unspecified, not the specified.* The value is highest exactly where a single vendor is most likely to quietly drift on an edge nobody wrote down.
- **Doesn't:** justify routing every small spec through N vendors. On sharply-specified small work the modal outcome here was "two correct, equivalent candidates, pick either" — the competition cost (2 implementations + 1 judge) bought nothing on 2 of 3. The honest product rule this run supports: **reserve competition for high-unstated-surface work, not as a blanket QA pass.**

<!-- trace:TASK-874 EPIC-48 -->
