# AIDA probe — ablations & controlled experiments

The experimental arm of EPIC-48 (AIDA as a research probe). Each file is a dated, pre-registered experiment with deterministic grading where possible. These are the **evidence** behind the propositions in the theory paper (`../2026-06-16-coordinating-multi-vendor-agent-fleets.md`); read them when the paper cites a finding and you want the raw result.

Two programs ran. Read them in order within each.

## Program 1 — gate-vs-rule (P1, substrate-as-bouncer): COMPLETE, terminus reached

The question: *does a programmatic gate beat a stated rule for holding an invariant against a capable LLM?* Five controlled cells, every single-variable conjecture pre-registered and falsified, ending in a methodological conclusion.

| # | File | Cell | Result | What it falsified |
|---|---|---|---|---|
| design | `2026-06-17-gate-vs-rule.md` | — | the pre-registered design + interpretation | — |
| I1 | `2026-06-18-gate-vs-rule-pilot.md` | output-shape, trivial, Claude | **100% rule-only, gate idle** | "a stated rule just fails" (blanket P1) |
| I2 | `2026-06-18-gate-vs-rule-i2.md` | output-shape, **buried**, Claude **+ Codex** | **100%** (both vendors) | **attention-distance**; then **vendor** (cross-vendor leg) |
| I3 | `2026-06-18-gate-vs-rule-i3.md` | **procedural**, trivial, Claude | **100%** | **invariant-type** (output-shape vs procedural) |
| I4 | `2026-06-18-gate-vs-rule-i4.md` | procedural, **complex multi-file**, Claude | **100%** | **task-complexity / cognitive-load** |
| applied | `2026-06-19-applying-gate-vs-rule-to-aidas-gates.md` | — | the audit of AIDA's own gates | the glib "remove the gates" reading |

**Terminus (in I4):** five cells, five ceilings. A clean ablation *cannot reproduce rule-dropping at all* — the one observed drop (the bake-off, below) lives in a real-codebase/long-autonomy regime controlled designs structurally can't reach. The only remaining evidence path is **field telemetry** (`SPIKE-67`), not a sixth ablation. Product consequence (in the audit): AIDA's gates survive — they are mostly authorization/concurrency/integrity (out of scope for the evidence) or run at the real-repo CI boundary; the one live calibration is STORY-499's `--block` flip, which should be data-driven off its report-only record.

## Program 2 — cross-vendor competition (P2/L5, substrate-shapes-output): COMPLETE

The question: *what does running N vendors on the same work actually buy?*

| File | Setup | Finding |
|---|---|---|
| `2026-06-17-competitive-claude-vs-codex.md` | prescriptive brief, Claude vs Codex, rubric judge | Claude won; the contest measured **conscientiousness, not creativity** — the loser lost at the fine print (skipped a named gate). This is the program's one observed rule-drop, and it is uncontrolled (real-ish task, n=1) — the anchor the gate-vs-rule program kept failing to reproduce. |
| `2026-06-18-open-brief-convergence.md` | **open** brief (conjecture: open briefs diverge) | Conjecture **falsified** — convergence is **substrate-driven**, not brief-driven. Multi-vendor competition pays as **quality-variance QA**, not design diversity. Productized as `aida compete`. |

## How to read a result honestly

- **Pre-registered interpretation** sections were fixed *before* the run — check them against the result; a finding that matches a pre-registered threshold is stronger than a post-hoc story.
- **Deterministic grading** (no LLM judge) is noted where used — prefer it; judge-graded results (the competitive bake-offs) carry self-evaluation-bias caveats (theory paper §10).
- `results/` holds the raw CSVs (one row per trial) behind the tallies.
- Dated docs are **immutable observations** — later learning is a new dated doc, not an edit (e.g. I2's "vendor is the variable" was superseded by cross-vendor I2 + I3; the doc records the supersession rather than rewriting history).

<!-- trace:EPIC-48 STORY-655 | ai:claude -->
