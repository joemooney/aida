# Roll-your-own friction benchmark — where the typed graph pays over markdown+git+grep, and where grep wins

- **Date:** 2026-06-19
- **Probe:** EPIC-48, proposition **P2b** (does the rich layer pay over plain markdown+git+grep?). Spec TASK-871 (MOVE 5, parent EPIC-50). Runner: `scripts/ablations/ryo-friction-benchmark.sh`. Raw data: `results/2026-06-19-ryo-friction-benchmark.csv`.
- **Status:** Complete. Deterministic grading (no LLM judge). Single-operator, synthetic specs → this measures **capability-friction**, not team-adoption. The section-10 operator-discipline confound is named, not escaped (see Honesty).

## The question

The theory paper's verdict (section 12) marks P2b as **the biggest open question**: whether AIDA's typed graph (typed edges, id-stability, code traces) actually *pays* over the honest roll-your-own setup — plain markdown + git + grep — is *unproven and confounded with operator discipline*. The skeptic's reading: "the graph is dead weight; I could do this in markdown and grep." This experiment measures the half of that claim that **is** measurable deterministically: capability-friction on representative fleet tasks, at scale.

## Design

Two arms, the **same synthetic logical spec set**, generated from one seed at scales **10 / 50 / 200 / 500**:

- **Arm RYO** — one markdown file per spec under a git repo (`specs/TASK-N.md`), relationships as plain markdown text (`Parent:` / `Blocks:` / `BlockedBy:` lines + `// trace:ID` markers in `code/`). Queried with grep/sed/git. The honest zero-install setup.
- **Arm AIDA** — the **same** specs in a throwaway `aida init` git-canonical store, relationships as typed edges (`--parent` / `--blocked-by`) + the same trace markers. Queried with `aida graph` / `aida trace check` / `aida search`.

The spec set: `ceil(N/10)` epics, the remaining specs as children spread round-robin; within each epic the children form a blocked-by chain (so "what's blocked across the epic" has a real closure); every third child carries a trace marker; one marker per arm is intentionally dangling (rot); the word "cache" is seeded into ~1/7 of titles so full-text search has real hits.

Five tasks per arm, graded deterministically — **three where the graph should pay, two FAIRNESS tasks where grep should tie or win.** A strawman grep-arm would be dishonest evidence the probe must not produce.

| Task | Question | RYO method | AIDA method | Graded on |
|---|---|---|---|---|
| T1 relational | "what's blocked across this epic?" | grep `Parent:`+`BlockedBy:` across all files, filter | `aida graph --tree` / `--impact` (cache-backed) | latency + correct closure |
| T2 rename | rename/renumber a referenced spec | string-replace across files | id-stable: one edit | #edits + #missed refs (rot) |
| T3 trace-rot | "any dangling code traces?" | grep markers + hand-rolled resolve loop | `aida trace check` | detection (found the 1 dangling?) |
| **T4 fulltext** | "every spec mentioning 'cache'" | `grep -rl cache` | `aida search cache` | latency + correct |
| **T5 flat list** | "list all specs" | `grep '^# '` | `aida list` | latency |

## Results (the friction table)

Wall-clock seconds; "got/exp" = returned vs expected count. Every run RAN — these are real numbers.

| Task | Arm | n=10 | n=50 | n=200 | n=500 |
|---|---|---|---|---|---|
| **T1** blocked query (latency) | RYO | 0.127s | 0.309s | 1.266s | **2.979s** |
| | AIDA | 0.023s | 0.022s | 0.027s | **0.038s** |
| T1 correctness (got/exp) | RYO | 8/8 | 8/8 | 8/8 | 8/8 |
| | AIDA | 8/8 | 8/8 | 8/8 | 8/8 |
| **T2** rename edits / **missed refs** | RYO | 2 / **1** | 3 / **2** | 3 / **2** | 3 / **2** |
| | AIDA | 1 / **0** | 1 / **0** | 1 / **0** | 1 / **0** |
| **T3** trace-rot detected (exp=1) | RYO | 1 | 1 | 1 | 1 |
| | AIDA | 1 | 1 | 1 | 1 |
| **T4** fulltext (latency) | RYO | **0.008s** | **0.008s** | **0.008s** | **0.010s** |
| | AIDA | 0.022s | 0.020s | 0.020s | 0.020s |
| T4 correctness (got) | RYO | 1 | 7 | 26 | 64 |
| | AIDA | 1 | 7 | 26 | 64 |
| **T5** flat list (latency) | RYO | **0.009s** | **0.008s** | **0.009s** | **0.012s** |
| | AIDA | 0.025s | 0.024s | 0.032s | 0.047s |

## What the numbers say — honestly, both directions

### Where the typed graph pays

1. **Rename blast radius (T2) is the clearest, scale-independent win.** The honest RYO rename is a string-replace, and a naive single-file edit leaves **2 dangling references at every scale ≥ 50** — the mirrored `Blocks:` line on the blocker, the `BlockedBy:` line on the dependent, and the trace marker in code all still point at the old id. AIDA renames are **id-stable: one edit, zero missed refs**, because edges are stored by UUID and traces resolve by a stable id. This is not a latency story — it is a **correctness/rot story**, and it is the load-bearing finding. At fleet scale a roll-your-own rename silently rots cross-references; the graph cannot.

2. **Relational-query latency (T1) degrades visibly for grep and stays flat for the graph.** RYO's "what's blocked across this epic" is a grep-loop over every spec file: **0.13s → 0.31s → 1.27s → 2.98s**, roughly linear in fleet size. AIDA answers from the cache projection in **~0.02-0.04s, effectively flat.** Both return the correct closure (8/8) at every scale — grep is *correct*, just *slower and slower*. At 500 specs the grep query is ~80x slower; the crossover where grep's latency becomes user-noticeable is around the low hundreds of specs.

3. **Trace-rot detection (T3): a tie on detection, a gap in ergonomics.** Both arms detected the one dangling trace at every scale. But RYO needed a hand-rolled "grep the markers, loop, resolve each against the spec files, print the misses" pipeline — correct only because *I wrote the resolver correctly*. AIDA's `aida trace check` is one command with a `--block` CI mode. The friction here is not detection rate (both 100%) but **whether the resolver exists at all** — the RYO arm only detects rot because the operator built and maintained the check; the graph ships it.

### Where grep genuinely wins or ties (the fairness half)

4. **Full-text search (T4): grep WINS on latency, ties on correctness.** Both arms return the exact same correct set (1/7/26/64 cache specs). But `grep -rl cache` is **consistently faster** (~0.008-0.01s) than `aida search` (~0.02s) at every scale — the FTS5 query plus process startup loses to raw grep on a few hundred small files. Full-text "find every spec mentioning X" is a task markdown+grep does *better*.

5. **Flat enumeration (T5): grep WINS.** `grep '^# '` beats `aida list` at every scale (0.008-0.012s vs 0.024-0.047s). A flat dump of ids+titles is exactly what a text store is good at.

6. **Not measured but real, and conceded to RYO:** zero-install / zero-tooling (RYO needs only git+grep, already on every box), human-readability, and git-diffability of plain markdown. These are genuine RYO advantages the benchmark does not try to take away.

## The crossover and the synthesis

> **The graph pays on operations that are *relational* or *rename-sensitive*; grep wins on operations that are *textual* or *enumerative*.** The axis is not "graph vs grep is better" — it is **what shape the question has.** Ask a graph question (transitive blocked-by closure, "rename this without rotting refs") and the typed layer pays, increasingly so with scale. Ask a text question (full-text match, flat list) and grep is faster and needs no install.

The crossover for the *latency* story sits around the low hundreds of specs (T1's grep cost becomes noticeable between 200 and 500). But the **load-bearing** difference is not latency — at this scale every absolute time is small. It is **correctness under mutation**: the RYO rename rots 2 references at every scale ≥ 50, and that rot is *silent* — git diffs cleanly, grep finds nothing wrong, the broken cross-reference just sits there. That failure mode has no scale floor and no latency tell; it is a structural property of representing edges as string-matched text. **That is the honest answer to "is the rich layer dead weight": for textual/enumerative work, yes — use grep; for relational/rename-sensitive work, no — the graph prevents a class of silent rot that markdown+grep cannot.**

## What this does / does NOT establish for P2b

**Establishes:**
- The typed graph delivers a **real, measurable, scale-growing capability-friction advantage** on relational queries (latency) and rename blast-radius (silent-rot prevention) — P2b's "dead weight" reading is **falsified for relational/rename-sensitive fleet operations.**
- grep+markdown is **genuinely better** for full-text and flat-enumeration — so the honest posture is *both/and by task shape*, not "AIDA replaces grep."

**Does NOT establish:**
- **Team-adoption.** Single-operator, synthetic specs. Whether a *team* keeps the markdown cross-references consistent by hand (the discipline the graph automates) is exactly the section-10 operator-discipline confound, and this experiment **cannot escape it** — it measures what the tooling makes easy/hard, not what humans actually maintain. The RYO arm's 2 missed refs are what a *careful* operator misses with a naive rename; a *disciplined* operator running their own grep-based rename script would catch them — at the cost of writing and maintaining that script (which is, recursively, the thing AIDA ships).
- **That the graph is worth the install.** T4/T5 show real overhead. For a project that only ever asks textual/enumerative questions, the rich layer *is* overhead. The graph pays in proportion to how relational the actual work is.

## Honesty / limits

- Synthetic spec set with a regular structure (round-robin epics, linear blocked-by chains); real fleets are messier — both directions could shift, though the rename-rot mechanism is structural and won't.
- Deterministic grading throughout (no LLM judge → no judge bias), which is the right call here.
- `aida search` / `aida list` carry process-startup cost the grep one-liners don't; at larger scales (thousands) the FTS5 index would eventually overtake raw grep on T4, but that is past the range tested and not claimed.
- The epic-1 blocked closure is a fixed size (~8) across scales by construction, so T1 grades the *query mechanism* at growing fleet size, not a growing closure — which is the intended fleet-scale-latency probe.
- n=1 build per (scale, arm); latencies are single-shot wall-clock, fine for the order-of-magnitude story (80x at T1/500) but not for sub-millisecond claims.

## Reproduce

```bash
scripts/ablations/ryo-friction-benchmark.sh --smoke              # scale 10 sanity, seconds
scripts/ablations/ryo-friction-benchmark.sh                      # full 50/200/500
```

No LLM calls. Throwaway stores under `$TMPDIR` are deleted on exit (`--keep` to retain).

<!-- trace:TASK-871 EPIC-48 | ai:claude -->
