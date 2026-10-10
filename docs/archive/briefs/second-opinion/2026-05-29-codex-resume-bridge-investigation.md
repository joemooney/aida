# Codex follow-up: the resume-bridge — SPIKE-32's one real blocker

**Date filed**: 2026-05-29
**Target reader**: Codex (continuation of the SPIKE-32 POC you shipped, PR-352)
**Time budget**: 45–75 minutes — investigation, not a build
**Outcome wanted**: a yes/no on whether AIDA's existing punt handshake can bridge run-local ↔ substrate-local resume, with evidence

## Why this brief

Your SPIKE-32 POC (`docs/architecture/spike-32-workflow-poc/`, merged PR-352) settled the happy path and surfaced exactly one architecture-blocking question. Your own verdict:

> punt reroute is expressible only with structured agent output; shelve branching is easy but marking NeedsAttention needs AIDA CLI/MCP/supervisor callback; **resume is the hard part because Claude workflow resume is run-local while AIDA punt resolution is substrate-local.**

That resume gap is THE thing gating the whole compiler. Everything else (compile spec → emit workflow.js → replay) is tractable. If resume can't bridge, the compiler is parked indefinitely. If it can, SPIKE-32 unblocks. So this is the highest-value next investigation.

## The specific question

AIDA already has a run-local ↔ substrate-local resume mechanism for the headless drain: the **punt → advisor → resume cascade** (STORY-306). When a headless implementer hits a design fork it can't resolve:

1. It runs `/aida-punt`, which writes a signal to `AIDA_PUNT_SIGNAL_FILE`
2. The orchestrator detects the punt via that file handshake
3. It spawns a headless advisor (`/aida-advise`) that resolves or escalates
4. On resolution, the implementer session **resumes** via `claude -p --resume` with the judged answer injected
5. The drain continues

That's already a working bridge between a run-local session (the implementer's `claude -p`) and substrate-local state (the punt file + advisor verdict + lease).

**The question:** can a Claude Code **workflow.js** participate in that same handshake? Specifically:

- When a workflow `agent()` step hits a design fork, can it write to `AIDA_PUNT_SIGNAL_FILE` (or call an AIDA MCP tool) the same way `/aida-punt` does?
- Can AIDA's supervisor detect that punt and drive the advisor resolution exactly as it does for the headless-drain case?
- Can the workflow then **resume** the punted `agent()` step with the resolved answer — OR does Claude Code's workflow runtime have no resume-with-injected-context primitive, forcing a different shape?

## What to actually do

1. **Read the AIDA side of the existing handshake** (don't reimplement — understand):
   - `aida-cli/src/punt.rs` — how `/aida-punt` writes the signal
   - `aida-cli/src/auto_complete.rs` — how the orchestrator detects the punt + drives the advisor + resumes the implementer (search for `AIDA_PUNT_SIGNAL_FILE`, `--resume`, advisor spawn)
   - `docs/architecture/autonomy-and-escalation.md` — the cascade design
   - `aida-cli/src/advisor.rs` — the advisor tier

2. **Read the Claude Code workflow runtime's resume model** at `https://code.claude.com/docs/en/workflows` (use the `.md` suffix for clean content). Find: does a workflow `agent()` call support resuming with injected context after an external signal? Is there a "wait for external resolution" primitive? Or is the workflow's only pause-point the permission prompt?

3. **Reach a verdict** on one of three outcomes:
   - **(A) Bridge works as-is**: the workflow can punt to AIDA's existing signal file and resume — describe the exact call shape. SPIKE-32 unblocks.
   - **(B) Bridge works with a new primitive**: AIDA needs to add one thing (e.g. an MCP tool the workflow calls to block-until-resolved). Describe what AIDA must add.
   - **(C) No bridge**: Claude Code workflows have no resume-with-context primitive; the run-local/substrate-local gap is unbridgeable without Anthropic adding a feature. Document the wall precisely so SPIKE-32 stays parked with a clear reason.

## What I do NOT want

- Building the compiler
- Reimplementing the punt cascade
- A workflow.js that actually does the resume — just determine if it's *possible* and the shape

## Place the verdict

`docs/architecture/spike-32-workflow-poc/resume-bridge-verdict.md`. Commit to a branch + report.

## Desired return shape

When you reply (via Joe):

1. **Verdict: A, B, or C** (one line)
2. **The evidence** — what in AIDA's punt.rs/auto_complete.rs + what in the workflow docs led you there (cite specifics)
3. **If B**: the one primitive AIDA needs to add (so I can scope it as a STORY)
4. **If A**: the exact call shape a workflow uses to punt + resume

Under 300 words. The verdict file is the deliverable.

---

trace:SPIKE-32 | ai:claude-master-advisor-asking-codex
