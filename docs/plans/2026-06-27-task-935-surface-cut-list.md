# Usage-grounded surface cut-list (W2) — operator review

- **Date:** 2026-06-27
- **Specs:** TASK-935 (child of EPIC-49); successor to the W1 pass TASK-850 / TASK-851 / TASK-852
- **Status:** Proposal — **for operator review. NOTHING is removed/hidden in code by this PR.** Once approved, each cut ships as a follow-up.
- **Complexity:** Low (analysis + proposal)

## What this is

A second, telemetry-grounded streamlining pass over the AIDA CLI surface and the
`.claude/skills/` catalog. The deliverable is this cut-list. The operator decides
KEEP / HIDE / MERGE / DEPRECATE per item before any surface is touched.

This is **subtraction-as-progress**: the visible top-level command surface is the
number a first-user sees in `aida --help`, and EPIC-49's thesis is that a smaller,
legible surface makes the Trojan-horse depth land better.

### Command-count headline (the subtraction metric)

| | top-level **visible** commands |
|---|---|
| Current (`aida --help`) | **84** |
| Proposed-after (this cut-list applied in full) | **78** |
| Delta | **-6** |

The W1 pass (TASK-850/851/852) already hid the rare *reporting/power* verbs
(`report`, `autonomy`, `metrics`, `manual`, `solo`, `intent`) and removed the truly
orphaned zero-call ones (`session wakeup`, `session forget`). Those stay where they
are. W2's six come from a different bucket: **near-duplicate verbs** and **zero-call
subcommands whose capability is reachable another way.** Most of the surface is
healthy — this is a deliberately small, conservative tranche, not a purge.

The proposed-after figure of 78 assumes only the **top-level** HIDE/MERGE items
below are applied. Subcommand-level cuts (under `queue`, `doctor`, `changelog`, …)
do not change the top-level count but do reduce the per-parent surface.

## The telemetry (last 30 days, `~/.aida/usage.jsonl`)

Pulled with `aida usage`, `aida usage --unused 30d`, `aida usage --errors`,
`aida usage --slowest`. Counts are local to this operator's machine.

**Most-used (contrast — these are the load-bearing surface, KEEP all):**
`statusline` 49658 · `rel list` 8111 · `show` 5860 · `add` 2518 · `edit` 1632 ·
`comment add` 1483 · `list` 1234 · `pull` 626 · `queue list` 407 · `status` 316.

**Zero-call real subcommands (not used in 30d; arg-bearing noise like `show <id>` /
`search <token>` excluded):**
`session manifest`, `role end`, `role repair`, `queue rework`,
`doctor verify-relationships`, `changelog generate`, `advisor register`,
`plan helpers`, `mcp register-agent`, `headless tail`, `scaffold preview`,
`worker directives`, `skill render`, `punts analyze`, `punts promote`,
`queue load`, `load report`, `agent register`, `doctor fsck`.
(`session wakeup` / `session forget` already fully removed by TASK-850 — they only
show because the 30d window straddles the cut.)

**Highest error rates — read carefully, most are NOT deprecation signals:**

| cmd | count | errs | rate | what it actually is |
|---|---|---|---|---|
| `rel list` | 8111 | 4057 | 50% | **NOT a cut candidate** — heavily used. See BUG-573 note below. |
| `queue work` | 56 | 53 | 95% | gate-bail / no-ready-spec exits; expected non-zero. Not a cut. |
| `undefer` | 38 | 24 | 63% | "spec not deferred" no-ops; UX papercut, not a cut. |
| `config glyph` | 19 | 8 | 42% | lint-style nonzero on drift; expected. Not a cut. |
| `show not-a-real-id`, `list bogus`, `schema bogus` … | — | 100% | bad-arg invocations — pure noise, ignore. |

**Slowest (`--slowest`, p95 latency — orthogonal lens, no cuts implied):** the long
p95s are `agent new`, `solo`, `questions clarify`, `tui`, `burndown run`, `pr ship`
— all **long-lived interactive/drain sessions** where wall-clock includes human/agent
think time. None are perf bugs; none are cut candidates. Recorded here only so the
perf lens is on the record for the operator.

> **BUG-573 cross-reference.** `rel list`'s 50% error rate is the already-filed
> BUG-573 (read-only relationship verbs exit non-zero on dangling-edge warnings,
> poisoning telemetry). That bug is marked Completed, yet the 30d window still shows
> the poisoned counts — i.e. the fix landed mid-window or the telemetry predates it.
> **Action: none here** beyond flagging that `rel list`'s error column is a known
> artifact, not a quality signal. If the rate persists in the *next* 30d window,
> reopen BUG-573.

---

## Cut-list

Legend: **KEEP** (no change) · **HIDE** (drop from `--help`, stays callable) ·
**MERGE** (fold into a sibling/parent verb, keep an alias) · **DEPRECATE** (hidden
alias now, scheduled removal).

### A. Top-level visible verbs — the 6 that move the headline count

| # | item | calls/30d | proposal | justification | risk |
|---|---|---|---|---|---|
| A1 | `punts` (top-level) | `punts analyze` 0, `punts promote` 0 (whole group cold) | **HIDE** | The punt **ledger browser** is zero-call across its subcommands; punting itself uses the separate top-level `punt` verb (kept). Capability preserved — `aida punts …` stays callable for the rare audit. | Low. `punt` (the action) is untouched. Grep shows 0 non-cli.rs refs to the analyze/promote subcommands. |
| A2 | `load` (top-level) | `load report` 0 | **HIDE** | `load report` is zero-call; the load/capacity reporting is a power-user lens already de-emphasized in W1. Hide the parent; keep callable. | Low. 0 external refs to `load report`. |
| A3 | `worker` (top-level) | `worker directives` 0 | **HIDE** | `worker directives` is zero-call. The directive surface is orchestrator-internal; humans don't drive it by hand. | Low–Med. 11 codebase refs (mostly docs + cli.rs); **confirm no harness/MCP path shells out to `aida worker directives`** before hiding (grep showed no non-doc caller, but verify). |
| A4 | `headless` (top-level) | `headless tail` 0 | **HIDE** | `headless tail` zero-call; the `--no-human` drain is driven through `queue work` flags, not a hand-run `headless` verb. | Med. 6 refs — **verify the orchestrator / `aida-burndown` skill don't invoke `aida headless …`** before hiding. If they do → KEEP. |
| A5 | `scaffold` (top-level) | `scaffold preview` 0 | **HIDE** | `scaffold preview` zero-call; scaffolding is exercised through `aida init` / `aida-sync`. Hide the standalone parent. | Low. 3 refs (aida-sync skill doc, cli.rs, one old plan). Confirm `aida-sync` skill calls the real verb it needs, not `scaffold preview`. |
| A6 | `skill` (top-level) | `skill render` 0 | **HIDE** | `skill render` zero-call; skill rendering is an internal template op, not a daily human verb. | Low–Med. 5 refs — confirm `make sync-templates` / build path doesn't depend on the *visible* verb. |

> All six are **HIDE, not remove** — capability stays, the verb just leaves `aida --help`.
> Net visible top-level: 84 → **78**.

### B. Zero-call subcommands — hide/merge under their parent (no headline change)

| # | item | calls/30d | proposal | justification | risk |
|---|---|---|---|---|---|
| B1 | `changelog generate` | 0 | **MERGE → `changelog refresh`** (hidden alias) | `refresh` is the canonical idempotent rewrite (used by release). `generate` is the old print-only twin — fold it, keep an alias for any script. | Low. 2 refs (changelog.rs + one plan). |
| B2 | `doctor fsck` | 0 | **MERGE → `doctor check`** (hidden alias) | Two near-identical integrity verbs under `doctor`. `check` is the documented one. | Low. 2 refs. |
| B3 | `doctor verify-relationships` | 0 | **HIDE** under `doctor` | Zero-call; relationship integrity is already covered by `doctor check` / `rel list --dangling`. Keep callable. | Low–Med. 6 refs — verify no skill (`aida-doctor`) drives it by name. |
| B4 | `queue load` | 0 | **HIDE** under `queue` | Zero-call queue capacity readout; overlaps `aida ps` / `status`. | Low. 0 external refs. |
| B5 | `queue rework` | 0 (direct CLI) | **KEEP** | Zero **direct** human calls, but the `aida-pr` and `aida-review` skills + the MCP tool + `auto_complete.rs` drive it. Capability is load-bearing in the review loop — do **not** cut. Flagged only to record the verify. | n/a |
| B6 | `role repair` | 0 | **HIDE** under `role` | Zero-call recovery verb; keep callable for the rare corrupted-role-file case. | Low. 1 ref (main.rs). |
| B7 | `session manifest` | 0 | **KEEP / investigate** | 19 codebase refs — likely harness/session-lifecycle dependency. Zero **human** calls but probably machine-driven. Do not cut without confirming the harness doesn't call it. | High if cut blind — KEEP pending grep. |
| B8 | `advisor register`, `agent register`, `mcp register-agent` | 0 each | **KEEP** | Registration verbs the `--no-human=both` orchestrator and MCP onboarding depend on. Zero **human** calls is expected (machines call them). Recorded for completeness; not cut candidates. | High if cut — KEEP. |
| B9 | `plan helpers` | 0 | **KEEP** | Part of the documented `/ultraplan` plan-tooling loop (CLAUDE.md). Zero recent calls but a deliberate capability; low surface cost (already under `plan`). | n/a |

### C. Near-duplicate skills — merge candidates (`.claude/skills/`)

49 skills today. Skills are invoked through Claude Code, not the `aida` binary, so
they don't appear in `aida usage` directly — these proposals are grounded in
**description overlap + the backing-verb call counts**, and are softer than the
verb cuts. Operator judgment weighted higher here.

| # | skills | proposal | justification | risk |
|---|---|---|---|---|
| C1 | `aida-status` + `aida-doctor` | **KEEP both** | Considered merging (both "project health"), but `status` is the one-shot snapshot (`status` verb, 316 calls) and `doctor` is the multi-agent drift *healer*. Different jobs. No merge. | n/a |
| C2 | `aida-queue` + `aida-pickup` | **KEEP both** | `queue` = read-only peek; `pickup` = consume + work + done. The read/write split is intentional and documented. No merge. | n/a |
| C3 | `aida-drain-queue` + `aida-burndown` + `aida-solo` | **REVIEW for consolidation** | Three overlapping "drain the backlog autonomously" skills. `drain-queue` assembles a `/goal` prompt; `burndown` fans out implementers; `solo` is the warm interactive driver. Real distinctions, but a first-user can't tell them apart. **Propose: keep all three, add a one-line "which one?" chooser at the top of each.** Not a deletion — a legibility fix. | Low (doc-only). |
| C4 | `aida-doc` + `aida-docs` + `aida-docs-review` | **REVIEW** | `aida-doc` (capture WHY as Doc specs) vs `aida-docs` (keep guides in sync) vs `aida-docs-review` (quality audit). Names are confusingly close. **Propose: keep all three, rename for disambiguation** (e.g. `aida-doc-capture` / `aida-docs-sync` / `aida-docs-review`) — a follow-up, not this PR. | Low–Med (renames break muscle memory / slash-command names). Operator call. |
| C5 | `aida-assess` + `aida-backlog-groom` | **KEEP both** | `assess` = headless cold-boot intake judgment; `backlog-groom` = guided interactive grooming. Producer (headless) vs operator-driven. Different seats. No merge. | n/a |
| C6 | `aida-decide` + `aida-clarify` | **KEEP both** | `decide` drains the decision inbox; `clarify` authors missing acceptance. Complementary, both referenced by the `questions`/`decide` verb pair. No merge. | n/a |

**Skills net:** no deletions proposed in this tranche — only two legibility
follow-ups (C3 chooser line, C4 renames) for the operator to greenlight separately.
Skills are cheap to keep and expensive to mis-merge; the bar for cutting one is
higher than for a CLI verb.

---

## Summary for the operator

**Approve to ship as follow-ups (highest-confidence, lowest-risk):**

1. **HIDE 6 top-level zero-call parents** → `punts`, `load`, `worker`, `headless`,
   `scaffold`, `skill` (A1–A6). Visible top-level **84 → 78**. Each stays callable.
   *Gate each on the one-line grep noted in its Risk column (A3/A4/A5/A6) before
   hiding.*
2. **MERGE 2 zero-call subcommands** into their canonical sibling with a hidden
   alias → `changelog generate`→`refresh`, `doctor fsck`→`doctor check` (B1, B2).
3. **HIDE 4 zero-call subcommands** under their parent → `doctor verify-relationships`,
   `queue load`, `role repair` (B3, B4, B6). Capability preserved.

**Explicitly KEEP (zero-call but machine-driven / load-bearing — do NOT cut):**
`queue rework`, `session manifest`, `advisor register`, `agent register`,
`mcp register-agent`, `plan helpers` (B5, B7, B8, B9). Recorded so a future pass
doesn't re-flag them.

**Do NOT treat as quality signals:** `rel list` 50% error rate (BUG-573 artifact),
`queue work` / `undefer` / `config glyph` expected-nonzero exits.

**Skills:** no deletions this tranche; two optional legibility follow-ups
(C3 chooser line on the three drain skills, C4 disambiguating renames on the doc
trio).

## Verification (executable)

```bash
# Current visible top-level count (should read 84 before any cut):
aida --help | sed -n '/Commands:/,/Options:/p' | grep -E '^  [a-z]' | wc -l

# The three telemetry lenses this cut-list is grounded in:
aida usage --unused 30d --limit 200
aida usage --errors --limit 200
aida usage --slowest

# After the A1–A6 HIDE follow-ups land, re-run the count → expect 78.
```

## Followups (filed only after operator approves the cuts)

- [ ] A1–A6: hide the 6 top-level parents in `aida-cli/src/cli.rs` (one `hide(true)` each), gated on the per-item grep.
- [ ] B1–B2: hidden-alias merges (`changelog generate`→`refresh`, `doctor fsck`→`doctor check`).
- [ ] B3, B4, B6: hide the 3 subcommands.
- [ ] C3: add a "which drain skill?" chooser line to `aida-drain-queue` / `aida-burndown` / `aida-solo`.
- [ ] C4: operator decision on the doc-skill renames.
- [ ] Re-check `rel list` error rate in the next 30d window; reopen BUG-573 if still ~50%.

## Related

- EPIC-49 (parent) — AIDA streamlined + niche-fit.
- TASK-850 / TASK-851 / TASK-852 (W1 pass) — the cuts that produced today's 84.
- BUG-573 — `rel list` non-zero-on-dangling telemetry poisoning (Completed; watch).
