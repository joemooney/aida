# Strategic recompose Round 2 — Code Review, Actions, GitLab + workflows-lane correction

**Date**: 2026-05-30
**Status**: addendum + extension to the 2026-05-29 Round 1 synthesis
**Audience**: Joe + future AIDA-maintaining advisor sessions
**Round 1**: `docs/competitive-analysis/2026-05-29-strategic-recompose-post-2.1.154.md` (frozen — per `feedback_dated_artifacts_immutable`)

This is the Round 2 sweep. Three new Claude Code surfaces — managed Code Review, the GitHub Actions integration, and the GitLab CI/CD integration — plus one substantive correction to the Round 1 workflows positioning. Round 1 covered runtime orchestration; Round 2 covers the **review/CI seam** plus a sharper architectural framing for SPIKE-32.

---

## TL;DR

### Workflows lane (Round 1 correction)

Round 1 framed Claude Code Workflows as the "deterministic orchestration runtime." That's **inverted**. Dynamic Workflows are explicitly the dynamic-generation lane — decide fan-out at runtime per invocation. The deterministic lane is the opposite: save a run's script as a command, replay that fixed artifact. Determinism comes from REPLAY, not from the JS-script generation surface.

This refines (but does not break) AIDA's compose architecture. AIDA's "compile spec → emit workflow.js" thesis (SPIKE-32) lives in the **saved-script lane**: the compiler runs once per spec change, the workflow.js is a checked-in build artifact (like `Cargo.lock`), Claude Code's runtime replays it deterministically. This is cleaner than the muddled Round 1 framing and matches how AIDA's drain loop actually wants to work (same-plan-every-time).

Memory captured: `feedback_workflows_saved_script_lane.md` so I don't re-conflate.

### Three new surfaces

| Surface | What ships | Round 2 verdict |
|---|---|---|
| **Managed Code Review** | Team/Enterprise GitHub App. Multi-agent review on Anthropic infra. `🔴 / 🟡 / 🟣` severity. Reads `CLAUDE.md` (low priority) + `REVIEW.md` (highest priority — reviewer-only injection). Machine-readable `bughunter-severity: {…}` in the check run. Triggers: `@claude review` (subscribe) / `@claude review once`. $15-25 per review. ZDR holdouts. Local equivalent: `/code-review --comment --fix`. | **COMPOSE — heavy** |
| **`claude-code-action@v1`** | Published GitHub Action. `@claude` mentions + arbitrary events. Skills + plugin marketplace support. Multi-cloud (Anthropic/Bedrock/Vertex). `prompt` + `claude_args` interface. | **COMPOSE — light** (distribute AIDA reviewer as a wrapped action) |
| **GitLab CI/CD** | Beta, GitLab-maintained. `claude -p` in `.gitlab-ci.yml`. WIF auth. Uses `gitlab-mcp-server`. | **COMPOSE — defer** (no near-term GitLab users; pattern noted) |

---

## The reviewer seam — where AIDA's orchestrator phase 3 meets Code Review

This is the most consequential overlap in either round.

**AIDA today (phase 3 reviewer):**
- Reads PR diff via `gh pr diff`
- Reads spec description + acceptance via AIDA MCP
- Spawns a Claude session that judges diff against acceptance
- Posts a verdict via `gh pr comment` / `gh pr review`
- Orchestrator parses verdict → merge / shelve / escalate

**Anthropic now ships:**
- Multi-agent review pipeline (specialized agents per finding class)
- Verification step (filters false positives by checking actual behavior)
- Inline comments with collapsible reasoning
- Machine-readable severity tally on the check run
- `REVIEW.md` as a highest-priority injection surface
- 20-minute median latency

**The composed architecture:**

```
┌──────────────────────────────────────────┐
│ AIDA spec graph                          │
│   acceptance criteria for SPEC-N         │
└────────────────┬─────────────────────────┘
                 │ SPIKE-35: emit
                 ▼
┌──────────────────────────────────────────┐
│ REVIEW.md (checked-in artifact)          │
│   acceptance-grounded reviewer rules     │
│   severity calibration by spec status    │
│   skip-rules from cross-spec trace graph │
└────────────────┬─────────────────────────┘
                 │ injected into
                 ▼
┌──────────────────────────────────────────┐
│ Anthropic-managed Code Review             │
│   multi-agent verification               │
│   inline comments + bughunter-severity   │
└────────────────┬─────────────────────────┘
                 │ SPIKE-36: parse
                 ▼
┌──────────────────────────────────────────┐
│ AIDA orchestrator phase 3                │
│   parse severity tally                   │
│   gate decision: merge / shelve / escalate│
└──────────────────────────────────────────┘
```

Round 1 said AIDA divests "process supervision of Claude Code instances." Round 2 extends: **AIDA divests the multi-agent review work**, contributes spec-grounded instructions via REVIEW.md, and consumes the structured verdict back. The substrate-as-bouncer pattern carries through — AIDA decides what gets reviewed and how findings are gated; Anthropic's multi-agent fleet does the reviewing.

---

## SPIKEs filed (Round 2)

| # | Title | Priority | Effort | Verdict |
|---|---|---|---|---|
| 35 | Emit REVIEW.md from spec graph | High | Medium | The substrate-as-bouncer move for the reviewer surface. Same shape as SPIKE-31 for path-gated rules. |
| 36 | Parse `bughunter-severity` as orchestrator phase 3 gate | High | Small | Cheapest delegation move; consumes the check-run JSON tally. |
| 37 | Trigger Code Review via `@claude review once` from `/aida-review` | Medium | Small | Comment-trigger compose; pairs with SPIKE-36. |
| 38 | Publish `aida-review` GitHub Action wrapping `claude-code-action@v1` | Medium | Medium | Distribution surface; other AIDA-using projects inherit reviewer behavior in CI. |
| 39 | Abstract forge integration (gh vs glab) | Low | Large | Forge-portability; defer until first GitLab user. |

File-order priority: 35 + 36 land in days-not-weeks. 37 + 38 are days-not-weeks but optional. 39 is months-not-weeks; hold until demand.

---

## What this means for SPIKE-32 (workflow compiler)

The workflows-lane correction does NOT collapse SPIKE-32 — it sharpens it.

**Old framing (Round 1, muddled):** AIDA's orchestrator becomes the compile-target generator for Claude Code's "deterministic orchestration runtime." Sounded like AIDA → workflows pipeline at runtime.

**New framing (Round 2, corrected):** AIDA's compiler runs once per spec change, emits `workflow.js` as a checked-in build artifact. Claude Code's runtime replays that fixed artifact on every drain. Same-plan-every-time IS the requirement; saved-script lane IS the right home.

SPIKE-32's spec description has been updated with this framing. The pre-req gate (SPIKE-30 + SPIKE-31 must confirm direction) is now met: both Completed. SPIKE-32 stays months-not-weeks, but the design pass can start when the operator says.

Second-opinion brief written: `docs/briefs/second-opinion/2026-05-29-spike-32-workflow-compiler-thesis.md`.

---

## What this means for AIDA's positioning docs

`docs/positioning/vs-ultrareview.md` and `vs-claude-code-subagents.md` predate Code Review's GA. Both need refresh. The new line:

> AIDA scaffolds REVIEW.md per spec; Claude Code's managed Code Review consumes it; AIDA's orchestrator parses the severity tally and decides lifecycle. AIDA isn't trying to BE the reviewer — AIDA is the substrate that makes Code Review spec-grounded.

Not refreshing inline in this doc — the positioning docs are living guidance and should be updated in their own commits.

---

## Surfaces still NOT fetched (Round 3 pending)

- `/sub-agents` — subagent definition format (referenced by SPIKE-34)
- `/skills` — full skill manifest format
- `/hooks` — SessionStart/Stop/PreToolUse hooks (relevant to AIDA's auto-bump SessionEnd story)
- `/mcp` — Claude Code's MCP primitives (compose with AIDA's MCP)
- `/worktrees` — full worktree configuration surface
- `/plugins` — distribution mechanism (relevant to SPIKE-38)
- `/permissions` — permission rule syntax
- `/best-practices` — context-window hygiene patterns
- `/github-enterprise-server` — referenced from Code Review docs
- `/agent-sdk` — referenced as the foundation for both Actions integrations

Round 3 should cover at minimum `/sub-agents`, `/skills`, `/plugins` (the distribution-surface trio). The others add polish, not architecture.

---

## Calibration notes

Round 1 → Round 2 took ~3 hours from URL paste to this doc. Three substantive surfaces, one correction, five new SPIKEs filed, two second-opinion briefs, one shipped synthesis. SPIKE-35 ship pending.

The pattern that's working: **paste-bomb → parallel fetch → file SPIKEs eagerly → ship the cheapest one → leave briefs for what needs adversarial review.** Repeating this through Round 3+ should keep paying.

trace:SPIKE-35 trace:SPIKE-36 trace:SPIKE-37 trace:SPIKE-38 trace:SPIKE-39 | ai:claude
