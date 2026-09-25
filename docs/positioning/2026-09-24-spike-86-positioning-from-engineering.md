# SPIKE-86: what AIDA is, read from where the engineering goes

<!-- trace:SPIKE-86 | ai:claude -->

*Dated read, 2026-09-24. Frozen once merged; supersede with a new dated file rather than editing this one.*

**Spec:** SPIKE-86, including the three operator comments of 2026-09-21 (the north star, its comparative refinement, and the agent-collaboration claim).
**Evidence base:** the git history of `main`, pinned at `9154924fd2` (2026-09-24 18:15 -0700), and the canonical YAML spec store at `.aida-store` HEAD `788c4e050d` (2026-09-24 18:21 -0700, 4,428 spec objects). As the spike requires, no figure here comes from the existing positioning documents, which are the thing under review.

---

## TL;DR

1. **Where the engineering goes:** the **control plane** (queue, drain, orchestrator phases, leases, seats, review verdicts, merge holds, gates, events). It took **50.9%** of attributed changed lines in the last 30 days and **46.1%** over 90 days. It also accounts for **57.1%** of human- or agent-authored specs filed in the last 30 days, plus nearly all of the **632** machine-filed specs. The TUI took **1,277** changed lines in 30 days, **0.7%** of the control plane's figure.
2. **Where the value sits:** the **store** (the spec graph, IDs, typed relationships, traces, criteria). The dependency test settles this (see §3): the control plane depends on the store and the store never depends on the control plane. Remove the control plane and the store still works. Remove the store and the control plane has nothing to run.
3. **Recommendation:** use **two sentences for two audiences**. The spike's counterweight (d) was right that conflating them caused the tangle.
   - **Market pitch (newcomers and users):** the operator's north star, stated comparatively. *AIDA captures the intent behind your system (requirements, decisions, rejected alternatives, and the code-to-spec links) so your agents work from it today, and the project stays far easier to rebuild tomorrow than it would be from commit messages and a README alone.* The entry point is `aida why <file:line>`, which is already the README headline.
   - **Architecture description (contributors, and what we benchmark against):** *AIDA is a git-canonical intent store with a control plane that keeps the store true while unreliable, non-deterministic agent workers change the code.* In the operator's reframe (SPIKE-86 comment 1), the control plane is the **corpus-integrity layer**.
4. **Verdict on the five theses:** T1 (VIS-1, "missing index") is **superseded**. T2 (intent traceability plus lifecycle truth) is **subsumed** as the core of the market pitch. T3 (ADR-4, "governance layer; concede the substrate race") is **split**: its governance half is subsumed, and its concede-and-interop half is **refuted by the engineering**. T4 (Trojan-horse TUI) is **superseded as positioning**, though its humility tactic survives, moved to `aida why` and the memory lane. T5 (durable-execution orchestrator with a merge queue) is **live as the architecture description only**, not as the pitch.
5. **The uncomfortable finding:** the effort ratio runs against the north star. The layer the pitch sells (intent: criteria, traces, reconstitution) got **6.6%** of attributed lines in 30 days. Only **27** criteria-level trace comments exist in the codebase, spread over **10** specs. Only the operator can decide whether that is acceptable (§8).

---

## 1. Method, and every command behind the numbers

All git measurements run on `main` pinned at `9154924fd2`, excluding merge commits. The measuring scripts are committed next to this report in [`spike-86-scripts/`](spike-86-scripts/) and regenerate every figure here exactly. Run them from the repository root with `python3` (the corpus script needs PyYAML):

```bash
python3 docs/positioning/spike-86-scripts/report.py     # sections 2.1, 2.2 and 2.5 (about 2 minutes)
git fetch origin aida-store                              # corpus.py reads the store at a pinned revision
python3 docs/positioning/spike-86-scripts/corpus.py      # sections 2.3 and 2.4; --list30 prints each 30-day spec's layer
python3 docs/positioning/spike-86-scripts/classify.py --since="2026-08-25 18:15 -0700"   # raw per-layer churn
python3 docs/positioning/spike-86-scripts/deep.py     --since="2026-08-25 18:15 -0700"   # test and dispatcher attribution
```

The defaults pin `--rev 9154924fd2` and `--store-rev 788c4e050d`. The windows are the absolute anchors `2026-08-25 18:15 -0700` (30 days) and `2026-06-26 18:15 -0700` (90 days). These equal the `--since=30.days` and `--since=90.days` evaluations made during the spike: no commit falls between the two forms of either boundary. Corpus windows are `created_at` within N days of 2026-09-24T00:00Z. The layer rules are summarised below; `classify.py` holds them exactly.

**Layer classifier (file path to layer, first match wins).** The layers are those of the spike's stack hypothesis, with *intent* split out of *store* because theses T1 and T2 and the north star all hinge on it:

| Layer | What it matches (under `aida-cli-lib/src` and `aida-core/src`, plus the named crates) |
|---|---|
| surface | `aida-tui/`, `aida-web-react/`, `aida-server/`, `mcp*`, `cli.rs`, `statusline*`, `status_*`, `glyph*`, `help_*`, `toon`, `wiki`, `awaiting_you` |
| control | `queue*`, `drain*`, `orchestrat*`, `auto_complete`, `review*`, `merge_*`, `integrate*`, `pr_*`, `ship`, `*gate*`, `schedule*`, `seat*`, `advisor*`, `mailbox`, `presence`, `human*`, `events`, `zen*`, `lock*`, `doctor_cmd`, `health*`, `metrics`; in core: `gates`, `pickability`, `dispenser`, `liveness`, `lifecycle`, `mailbox`, `lock` |
| execution | `worktree*`, `session*`, `agent_*`, `headless*`, `forge`, `vendor_activity`, `sandbox`, `process_*` |
| intent | `reconstitute`, `criteria*`, `trace_cmd`, `intent`, `contradictions`, `interview`, `dryrun`, `exposition`, `harvest`, `evaluator`, `digest`, `doc_cmd`; in core: `ears_lint`, `ai/`, `provenance` |
| store | the rest of `aida-core/src` (object store, db/cache, models, graph, git ops), plus `graph_cmd`, `relationship*`, `store*`, `cache*`, `history`, `comment_cmd`, `import_export`, `git_backend_cmd` |
| dispatcher | `aida-cli/src/main.rs`, `aida-cli-lib/src/lib.rs` (the monolithic command router; attributed separately, below) |
| scaffold / docs / tests / ci | `aida-core/templates/`, `.claude/`, `init_*`; `docs/`, `*.md`; `tests/`, `src/tests/`; `scripts/`, `.github/`, `Makefile` |

Paths under `aida-cli/src/X.rs` from before STORY-772 are mapped to their `aida-cli-lib/src/X.rs` successors.

Commands:

```bash
# commit volume and cadence
git log --since=90.days --oneline origin/main | wc -l                      # 1046
git log origin/main --since=90.days --no-merges --format=%ad --date=format:%G-W%V | sort | uniq -c
# per-layer churn (numstat; commits > 5,000 changed lines excluded as mechanical moves)
git log 9154924fd2 --since={90,30}.days --no-merges --numstat --format=@@%H   # classify.py / deep.py
# dispatcher hunks attributed by the enclosing fn in the hunk header
git log 9154924fd2 --since={90,30}.days --no-merges -U0 -- aida-cli/src/main.rs aida-cli-lib/src/lib.rs
# current size
git ls-tree -r --name-only 9154924fd2 aida-core/src aida-cli-lib/src aida-cli/src aida-tui/src aida-server/src aida-web-react/src
# capture coverage
git log 9154924fd2 --since=90.days --no-merges --format=%s | grep -cE '\((SPEC-ID list)\)( \(#N\))?$'
git grep -hoE 'trace:[A-Z]+-[0-9][0-9-]*' 9154924fd2 -- '*.rs' '*.ts' '*.tsx' '*.py' '*.sh' | wc -l
git grep -hoE 'trace:[A-Z]+-[0-9-]+\.(ac|AC)[0-9a-f]+' 9154924fd2 | wc -l
# spec corpus: every YAML object under objects/ at aida-store 788c4e050d (corpus.py)
aida list --all --json          # 4,426 rows at first read; the YAML carries created_at + description
aida show VIS-1 | STORY-551 | CR-6 | ADR-4 | EPIC-39 | EPIC-26 | EPIC-70 | ADR-44 | TERM-5 | STORY-754
aida list --all --type principle ; aida list --all --type vision ; aida list --all --parent EPIC-39
```

**Two artefacts in the raw history that I corrected for:**

- **The July refactor burst.** Eight commits in the 90-day window exceed 5,000 changed lines. The largest is STORY-772 PR-1, "main.rs becomes lib.rs", at 175,538 lines. These are mechanical moves, so they are excluded. Without the cap, "surface" read as 61.5% of 90-day churn, which is an artefact.
- **A quiet gap.** ISO weeks W31 to W33 (late July to late August) have 0 commits and W34 has 1. The 90-day window is therefore really two bursts: late June to July (W26 to W30, 501 commits) and September (W35 to W39, 544 commits). The 30-day window covers nearly all of the September burst.

**How the corpus was classified.** Each spec is assigned to one layer by a vote of its tags (weight 2) and title keywords (weight 1). A spec is **machine-filed** if its title starts with `auto-complete failure` or `Review PR-NNNN`, or if it carries the `auto-drafted` tag. Those specs are counted separately so the orchestrator's own failure stubs do not inflate the control plane's authored share. Acceptance-criteria coverage reimplements `parse_acceptance_criteria` from `aida-cli-lib/src/criteria.rs`, covering both the headed and the inline forms.

**Limits.** Keyword classification is coarse. About 20% of authored specs fall into *unclassified*; a sample of 22 was mostly forge, CI, roles, Windows and gates, with no concentration in intent. The dispatcher's hunk attribution leaves most 90-day dispatcher churn unattributed (function context `<top>` or `<mod>`, left by the July extractions). Test-file attribution strips spec-ID prefixes such as `bug_775_`. None of these limits is large enough to reverse the ordering in the tables.

---

## 2. Evidence tables

### 2.1 Engineering effort by layer (changed lines, commits of 5,000 lines or fewer)

Production files, plus test files attributed to the layer they test, plus dispatcher hunks attributed by their enclosing function:

| Layer | 30d files | 30d tests | 30d dispatcher | **30d total** | **30d share** | 90d total | 90d share |
|---|---:|---:|---:|---:|---:|---:|---:|
| control | 58,541 | 20,315 | 11,290 | **90,146** | **50.9%** | 142,364 | 46.1% |
| surface | 16,751 | 4,034 | 2,017 | 22,802 | 12.9% | 45,073 | 14.6% |
| store | 16,865 | 3,101 | 2,685 | 22,651 | 12.8% | 48,883 | 15.8% |
| execution | 10,396 | 2,529 | 3,187 | 16,112 | 9.1% | 31,787 | 10.3% |
| scaffold | 12,846 | 448 | 242 | 13,536 | 7.6% | 21,379 | 6.9% |
| intent | 8,927 | 2,601 | 193 | **11,721** | **6.6%** | 19,478 | 6.3% |
| *attributed total* | | | | 176,968 | | 308,964 | |

Unattributed and excluded churn, not in the shares: 30d tests 5,967, 30d dispatcher 17,998, 90d tests 9,136, 90d dispatcher 132,147. Docs changed 10,754 lines in 30d and 21,861 in 90d.

Commits touching each layer, as raw file paths (one commit can count in several layers): 30d: control **284**, surface 185, store 104, execution 88, intent **34**, of 543 commits. 90d: control 424, surface 343, store 191, execution 137, intent 56, of 1,039.

**Surface, broken down**, 30d / 90d: `aida-tui` **20 commits, 1,277 lines** / 46 commits, 9,252 lines (8,011 of those in `aida-tui/src/redesign`, late June). MCP is 31 commits, 1,559 lines / 50 commits, 2,565 lines. The rest of the surface is CLI rendering, chiefly `cli.rs` and `awaiting_you.rs`, which are views onto control-plane state.

### 2.2 Standing code size by layer (`9154924fd2`, `.rs`, `.ts` and `.tsx` source)

| Layer | Files | LOC | Share |
|---|---:|---:|---:|
| control | 133 | 152,100 | 25.6% |
| surface (includes `aida-web-react`) | 224 | 111,715 | 18.8% |
| dispatcher | 2 | 106,315 | 17.9% |
| store | 74 | 73,295 | 12.3% |
| tests | 247 | 70,481 | 11.9% |
| execution | 33 | 31,301 | 5.3% |
| intent | 39 | 24,953 | 4.2% |
| scaffold | 21 | 21,210 | 3.6% |

### 2.3 The spec corpus by layer

| Window | Work specs | Machine-filed | Authored | Authored: control | surface | execution | store | intent | unclassified |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| all time | 4,340 | 975 | 3,365 | 43.3% | 11.6% | 8.8% | 8.1% | **2.6%** | 25.6% |
| created in the last 90 days | 1,977 | 659 | 1,318 | 50.9% | 9.9% | 9.8% | 5.5% | **3.0%** | 20.9% |
| created in the last 30 days | 1,399 | **632** | 767 | **57.1%** | 4.8% | 9.3% | 5.2% | **3.9%** | 19.7% |

Including the machine-filed specs, **76.5%** of the 1,399 work specs created in the last 30 days are control plane. This corroborates the spike's 36-of-49 sample with a population count. The top tags among specs created in the last 90 days are `auto-complete` (285), `auto-drafted` (284), `reviewer` (182), `failure-3` (170), `drain-safe` (115) and `ci` (96). The first intent-flavoured tag does not appear in the top 30.

### 2.4 Specs per thesis (authored specs; tag or title match; non-exclusive)

| Thesis | All time | Last 90 days | Last 30 days |
|---|---:|---:|---:|
| T1/T2: intent index and traceability | 95 (2.8%) | 53 (4.0%) | 43 (5.6%) |
| T3: human-governance layer | 435 (12.9%) | 193 (14.6%) | 96 (12.5%) |
| T4: the TUI is the product | 116 (3.4%) | 71 (5.4%) | **5 (0.7%)** |
| T5: orchestrator and merge queue | 1,120 (33.3%) | 541 (41.0%) | **407 (53.1%)** |

T1/T2 is the only thesis whose share is **rising**, from 2.8% to 4.0% to 5.6%. It is the EPIC-70 and north-star work, but it rises from a small base.

### 2.5 Capture coverage: the north star's own leading indicators

| Metric | Value | Source |
|---|---:|---|
| 90-day commits ending in a `(SPEC-ID)` trailer | **931 / 1,047 (88.9%)** | `git log --format=%s` with a trailer grep |
| `trace:<ID>` comments in tracked source | **15,601** | `git grep -hoE 'trace:…'` |
| Criteria-level traces (`trace:<ID>.ac…`) | **27, across 10 specs** | `git grep -hoE 'trace:…\.ac…'` |
| Authored specs with parseable acceptance criteria, all time | 1,070 / 3,365 (31.8%) | criteria parser reimplemented |
| Same, created in the last 30 days | 323 / 767 (42.1%) | |

Commit-level and line-level linkage is strong. Criterion-level linkage, which is what EPIC-70 and ADR-44 made the reconstitution mechanism, is close to empty: **10 specs**. The second open question in SPIKE-86's first comment ("what fraction carry criteria") is answered at 42.1% for recent authored specs, but the chain that makes criteria useful for reconstitution (criterion, then traced test) exists for almost none of them.

A tooling note: `aida criteria gap`, cited in the spike comments, is **not a subcommand**; `aida criteria <SPEC>` reads `gap` as a spec ID. This report therefore measured coverage over the corpus directly.

---

## 3. The layer model: validated, with one amendment

The spike's counterweight (a) says bug density is not identity: the control plane is new and concurrent, so it generates specs whether or not it is central. The layering has to hold on an argument other than defect clustering. Two such arguments follow.

**The dependency direction** (from the code, not the defects):

- The store and intent modules `graph_cmd`, `relationship_cmd`, `trace_cmd`, `criteria`, `reconstitute`, `history`, `comment_cmd`, `store_cmd` and `cache_cmd` each contain **0** imports of a control-plane module (`crate::(queue|drain|orchestrat|lease|review_verdict|merge_|integrate|auto_complete|supervisor)`).
- The control plane depends on the store. `queue_cmd.rs` has **156** `aida_core::` references and `auto_complete.rs` has 17.
- `aida-core`'s `Cargo.toml` has **0** dependencies on `aida-cli-lib`.

**The removal test** (what a user loses first):

| Remove… | What still works | What the user loses |
|---|---|---|
| the control plane | capture, `show`, `search`, `why`, `trace`, `criteria`, `reconstitute`, MCP reads: the whole memory lane | unattended drains, merge gating, verdicts, and multi-agent coordination at scale |
| the store | nothing meaningful; the queue drains specs, verdicts name specs, and gates read spec state | **everything**, because every other layer operates on the store |
| the execution layer | the store and a hand-driven workflow | isolated parallel agent work |
| the TUI | the CLI and MCP cover every operation; the TUI got 1,277 lines of 176,968 attributed in 30 days | a convenience view |

**Validated:** the store is the foundation and the payload, the control plane is a consumer layered above it, and the surfaces are peripheral. **Amendment:** split *intent* out of *store*. Criteria, traces, reconstitution, contradictions and intent quality are what make the store worth keeping, and they are the thinnest layer by effort. Folding them into "store" hides that.

**Caveat:** crate boundaries are not layer boundaries. 29 of the 92 Rust files under `aida-core/src` (`find aida-core/src -name "*.rs"`) name control-plane concepts (drain, orchestrator, lease, verdict), so `aida-core` is "the store plus the control plane's core types", not the store alone.

**Why the control plane dominates without being the identity:** the workers are non-deterministic LLM agents. Every defect class in the 30-day corpus (false green, stale evidence read as current, unknowable seat state) is the substrate asserting something untrue. The control plane is expensive **because** it keeps the store honest under unreliable writers. That is the operator's corpus-integrity reframe (comment 1, deliverable 5), and the evidence supports it: PRIN-4, 5, 6 and 7 are all integrity rules about surfaces not lying.

---

## 4. Reconciliation of the five theses

The governing test, per SPIKE-86 comment 1, is whether each thesis serves the north star.

| # | Thesis (source, status) | Engineering support (30d) | Serves the north star? | **Verdict** | Substrate edit that makes it true |
|---|---|---|---|---|---|
| T1 | "Your project's missing index, of intent, not just code" (VIS-1, **approved**, empty description) | store + intent = 25.4% of attributed lines | partly: "of intent" yes, "index" no | **Superseded** | a successor VISION (operator-worded); then `aida edit VIS-1 --status superseded --superseded-by <VIS-N>`. Not *rejected*: it governed. |
| T2 | Intent traceability + lifecycle truth (STORY-551, CR-6, **completed**) | intent 6.6% of lines; 15,601 traces; 88.9% trailers | **yes**, this is its mechanism | **Subsumed** into the successor vision as the pitch's "how" | none beyond the successor vision; CR-6's decision is honoured by it |
| T3 | Human-governance layer; concede the substrate race to Beads/Gas Town; interop (ADR-4, EPIC-39, **draft**, parked) | governance hits 12.5% of authored specs; no product source mentions Beads (**0** hits in `aida-*/src`; across all tracked `.rs/.ts/.tsx/.py/.sh` the only mention is `scripts/demo-lifecycle-authority.sh`, 4 times, a demo contrasting AIDA with Beads, not interop code); EPIC-39 has **0** children | the governance half does (gates keep the corpus true); the concede half does not (the store *is* the north star) | **Split**: governance is **subsumed** into the control plane; "concede the substrate, interop" is **refuted** by nine months of store and control-plane investment | ADR-4 moved to `rejected` or `superseded`, with a comment recording which half survived; EPIC-39 closed with it (operator decision) |
| T4 | Trojan horse: "the TUI is what people think AIDA is" (CLAUDE.md and repository guide, 2026-05-14; OVERVIEW §"Public face"; EPIC-26 **completed**) | TUI: 20 commits, 1,277 lines; 5 authored specs (0.7%) | no; it is a presentation choice | **Superseded as positioning**; the *humility tactic* survives, moved to `aida why` and the memory lane | a successor DECISION spec (deliverable 5), then a doc filing to replace the Trojan-horse passages in CLAUDE.md, OVERVIEW.md and `docs/agents/aida-repository-guide.md` |
| T5 | A durable-execution workflow orchestrator with a merge queue, whose workers are LLM agents (this spike's corpus read) | **50.9%** of lines; **57.1%** of authored specs; 632 machine-filed stubs | yes, as the **means** (corpus integrity), not the end | **Live, as the architecture description only**; not the pitch, because ADR-4's own red-team records that Gas Town already ships "a gated merge queue" | TERM-5 is amended to say "corpus-integrity layer"; a contributor-facing architecture doc is filed separately |

**A sixth face the spike did not list.** The README now opens with "AIDA: ask your codebase *why*". That is STORY-754's 60-second moment and the operator's earlier north-star memo. OVERVIEW line 4 says "agent-collaboration layer", while line 6 still carries the VIS-1 "missing index" headline, the one CR-6 retired. The recommended pitch absorbs both: `aida why` is the entry point, and agent collaboration is the "works from it today" half. The ADR-4 red-team's own conclusion, "cross-vendor **intent** governance … and the code-to-spec trace loop", is consistent with the recommended pitch, which is a further sign that T3's surviving half belongs inside it.

---

## 5. The recommendation

**Primary positioning (the market pitch).** It is ranked first because every thesis that survives above serves it, and the removal test puts its layer at the foundation:

> *AIDA captures the intent behind your system (requirements, decisions, rejected alternatives, and the links from code back to them) so your coding agents work from it today, and your project is far better placed to be rebuilt tomorrow than it would be from commit messages and a README alone.*

- **Audience:** newcomers, users, README and OVERVIEW headline.
- **Proof in 60 seconds:** `aida why <file:line>`.
- **Honest scope, per operator comment 2:** comparative, not absolute. It makes no whole-system regeneration promise, and it is validated on a project that used AIDA from its first commit (the `~/ai/aida-hub` probe), not on AIDA's own corpus.
- **The two value claims, per operator comment 3:** survival (the corpus outlives the code) and collaboration (the corpus improves work in flight, especially through the negative space the code cannot show: rejected alternatives, contradictions, and deferred intent).

**Architecture description (secondary, for contributors):**

> *A git-canonical intent store (the data plane), a control plane that keeps that store true while unreliable agent workers change the code (the corpus-integrity layer), an execution layer of isolated worktrees and sessions, and peripheral surfaces (CLI, MCP, TUI, web).*

- **Audience:** contributors, reviewers of architecture sketches, and whoever picks what AIDA benchmarks against.
- **What it explains:** why half the engineering goes into the control plane without making the control plane the product.

**Demoted or subsumed, stated explicitly:**

- **T1 "missing index"** is demoted to history. It is superseded by the successor vision.
- **T2 "intent traceability + lifecycle truth"** is subsumed as the mechanism of the primary pitch.
- **T3 ADR-4:** its governance half is subsumed into the control plane's integrity role. Its "concede the substrate race to Beads/Gas Town; interop, don't compete" half is **demoted as refuted**; the engineering went the opposite way.
- **T4 Trojan-horse TUI** is demoted from positioning to tactic. The TUI is a view onto the control plane. "Look humble, discover depth" moves to `aida why` and the memory lane.
- **T5 durable-execution orchestrator** is demoted from pitch candidate to architecture description.

---

## 6. Do the vs-* neighbours need durable-execution and merge-queue entries?

**Evidence:** a whole-word search for Temporal, Airflow, Argo, Nomad, Zuul, Bors, Mergify and Aviator finds **0** hits in `docs/positioning/` and **0** in `docs/competitive-analysis/` (`grep -rnwE`). The only "merge queue" mentions (2 files) are about ECC's claims.

**Verdict: a real gap, but on the architecture side, not the market side.**

- **Market side:** no new vs-* pages are needed. The pitch does not place AIDA in the durable-execution market, and inviting that comparison would sell AIDA against Temporal, where it loses.
- **Architecture side:** file **one** contributor-facing comparison: the AIDA control plane against durable-execution engines (Temporal-style replay and checkpointing) and against merge queues (Bors, Mergify, Zuul, GitHub merge queue). Put it under `docs/architecture/`, not `docs/positioning/`. That is where half the engineering goes, and it is the right benchmark for the corpus-integrity layer. The round-2 moat document already named "resumable orchestrator checkpointing" as gap P1, which is a durable-execution property.

---

## 7. Proposed substrate edits (text for operator wording; not filed by this spike)

The spike's non-goal forbids rewriting README, OVERVIEW and CLAUDE.md here. Operator comment 1 asks for the vision and principle "for operator wording and approval". The following are therefore proposals, filed after operator sign-off:

1. **VIS-N (successor vision):** the primary-positioning sentence in §5.
2. **PRIN-N (corpus sufficiency, comparative form):** *"Work is not done until the store holds what a future agent would need to rebuild it: the intent, the acceptance criteria, and the trace from each criterion to a test. A change that leaves the store less able to explain the system than the code is a regression."* It is testable today at commit level (88.9% trailers) and at criterion level (10 specs), which shows where it bites.
3. **VIS-1:** superseded by VIS-N.
4. **ADR-4 and EPIC-39:** superseded or rejected, with a closing comment: governance subsumed into the control plane; substrate concession refuted.
5. **Successor to the Trojan-horse decision** (deliverable 5), as a DECISION spec: *"The TUI is a view onto the control plane, not the product's face. The humble first surface is `aida why` plus the memory lane; depth is discovered through the store, not through the TUI."*
6. **TERM-5:** amended to name the control plane the corpus-integrity layer.
7. **Follow-up filings:** the §6 architecture comparison doc; a capture-coverage surface (trailer share, criteria share, criterion-to-test share) as a *product* metric (operator comment 2, point 3); and a fix for `aida criteria gap`, either as a real subcommand or by removing the references to it.

---

## 8. Decision needed from the operator

Only Joe can decide the following:

1. **The vision's wording.** Approve, edit or replace the §5 pitch sentence as the successor to VIS-1. This is brand copy and needs design signoff (CR-6 precedent).
2. **Retire the Trojan-horse framing?** Reaffirm it, or replace it with the §7.5 successor decision. It was a deliberate 2026-05-14 strategy, so overturning it needs the operator's explicit decision, not an agent's.
3. **ADR-4's fate.** The engineering refutes "concede the substrate race, interop with Beads". Choose between **rejected**, **superseded** by the new VIS, or **kept parked** because a distribution-driven interop is still wanted despite the build record.
4. **The effort-ratio question: is the imbalance a problem?** The pitch sells the intent layer, which gets 6.6% of attributed lines against the control plane's 50.9%, and criterion-level tracing covers 10 specs. Either (a) accept it, on the grounds that the control plane is the integrity cost of unreliable workers and will taper as it settles, or (b) set a target that rebalances towards intent capture (for example a floor on criterion-to-test traces per completed spec). This decides the next quarter's roadmap.
5. **The success criterion for the north star.** Decide whether STORY-1425's comparative measure (AIDA against a no-AIDA control arm, on `~/ai/aida-hub` rather than AIDA itself) becomes the north star's executable check. Without it, the pitch is asserted rather than measured.
6. **The PRIN-N wording and enforcement level.** Decide whether corpus sufficiency is advisory, graded (PRIN-8's "distil the rule into code" rung), or a gate.

---

## Appendix: the numbers the spike started from, now reproduced

- The spike's premise, "36 of 49 recent specs in the control plane" (73%), is corroborated by population counts: **76.5%** of all 1,399 work specs created in the last 30 days and **57.1%** of the 767 authored ones.
- The spike's premise, "Only 6 of the last 120 specs mention acceptance criteria": by the parser's definition, **42.1%** of recently authored specs *carry* parseable criteria. The spike's figure measured *discussion of* criteria, not their presence, so the two do not conflict. Complexity is still not arriving through the requirements door, but criteria are increasingly attached to what does arrive.
- PRIN count: the store now holds **8** principles (PRIN-8, draft, was added after the spike's "7"). None states the north star; this spike's §7.2 closes that gap.
