# Experiment: cross-vendor competitive implementation (Claude vs Codex), headless

- **Date:** 2026-06-17
- **Probe:** EPIC-48 (multi-vendor agent coordination), L4/L5. Spec SPIKE-64.
- **Status:** Run once (n=1 task). A pilot — the findings are hypotheses to repeat, not settled results.
- **What it demonstrates first:** that a single orchestrator can drive **Claude** (Agent/`claude -p`) and **Codex** (`codex exec`) **headlessly, in parallel, in isolated git worktrees, from an identical brief, and judge the results objectively.** Antigravity has no headless agent CLI (it's a GUI editor) → it is the human-in-the-loop arm, briefed through AIDA's cross-vendor substrate. That asymmetry is itself an L5 datapoint: *cross-vendor coordination is only as headless as the least-scriptable vendor.*

## Setup

- **Task:** implement `aida spec dryrun <SPEC>` (a deterministic readiness score + a gated `--ai` gap report + `--json`) — a real, bounded, testable feature (STORY-656).
- **Arms:** Claude (worktree) and Codex (`codex exec --dangerously-bypass-approvals-and-sandbox`, worktree), **identical brief** (`/tmp/compete-dryrun-brief.md`), commit-not-PR.
- **Judge:** a third agent, given both diffs + the brief + a 6-dimension rubric, scoring 1–5 with code citations, blind to nothing but instructed to cite specifics.

## Result

| Dimension | Claude | Codex |
|---|:--:|:--:|
| Spec adherence | 5 | 3 |
| Correctness & robustness | 5 | 4 |
| Test quality (coverage) | 5 | 3 |
| Simplicity & clarity | 4 | **5** |
| Design soundness | 5 | 3 |
| Integration cleanliness | 4 | 4 |
| **Total /30** | **28** | **22** |

Both compiled, passed the full suite, and **independently converged** on the same command surface, the same module split (pure scorer file + thin `main.rs` handler mirroring `intent.rs`), the same six dimensions, the same weights-sum-to-100 model, and the same parent heuristic. Claude: 909 lines, 23 tests. Codex: 628 lines, 11 tests.

The decisive gap: the brief said the `--ai` path must be "gated like `aida intent`." Claude reproduced the TTY/headless fence exactly; **Codex did not gate it at all** — it would spawn `claude -p` inside an unattended drain — and left the entire AI path untested. The compact arm cut the one corner the brief named.

## Findings (hypotheses for repetition)

1. **A prescriptive brief collapses the design space; the contest then measures conscientiousness, not creativity.** With the command, the gating analog, the JSON shape, and the module pattern all named, both vendors built nearly the same thing. The *divergence* was diligence/style — Claude over-builds for safety + testability; Codex optimizes compactness and trusts the happy path. Architecture didn't differ; care did.

2. **Synthesis ≈ winner + micro-grafts here, NOT "two halves make a better whole."** The judge's honest synthesis was "ship Claude as-is; optionally graft Codex's `get_requirement_by_spec_id` reuse and `&'static str` dimension names (~20 min, modest payoff)." So **on a prescriptive brief the payoff of competition is SELECTION + VERIFICATION, not emergent combination.** Conjecture: synthesis pays more when the brief is *open* (real design freedom). → next experiment: rerun on an under-specified spec and measure whether the arms diverge architecturally and whether a true synthesis emerges.

3. **The intent substrate (L1) decides what multi-agent competition can even measure.** This is the non-obvious one. Brief quality isn't just "reduces coordination cost" (the standing L1 claim) — *the prescriptiveness of the spec determines whether a vendor bake-off tests creativity or conscientiousness.* A vague brief would have produced divergent architectures (a design tournament); a sharp brief produced a diligence test. **You tune what the competition is for by tuning the spec.** That reframes spec-quality from a cost lever into a *control surface for multi-agent work.*

4. **The loser lost at the fine print, not the core.** Both got the hard part (the scorer) right; the verdict turned on a single named constraint Codex skipped. Implication for autonomous multi-vendor drains: **a programmatic gate on the named constraint (e.g. "the `--ai` path must be TTY-fenced") would have caught Codex's miss mechanically** — i.e. P1 (substrate-as-bouncer) is the safety net that makes cross-vendor competition trustworthy without a human judge in the loop. The competitive experiment and the gate-vs-rule ablation point at the same conclusion from opposite directions.

## Cost / honesty

- One task, one judge, one operator's account per vendor — a pilot, not a study. Self-evaluation bias applies (the judge is a Claude instance grading a Claude-vs-Codex contest); a cross-vendor judge (have Codex judge too) is the obvious next control.
- Cost: 2 headless implementations + 1 judge ≈ modest. Repeatable.

## What ships from it

The Claude arm ships as the real `aida spec dryrun` feature (it met every brief quality-bar item). The two Codex micro-grafts are a tracked polish follow-up. The reusable method — brief → parallel headless arms → rubric judge → select/synthesize — is the harness; this doc is its first run.

<!-- trace:SPIKE-64 | ai:claude -->
