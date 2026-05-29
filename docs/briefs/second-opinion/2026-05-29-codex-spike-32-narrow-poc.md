# Codex follow-up: SPIKE-32 narrow POC

**Date filed**: 2026-05-29
**Target reader**: Codex (continuation of `2026-05-29-spike-32-workflow-compiler-thesis.md`)
**Time budget**: 60–90 minutes
**Outcome wanted**: a working `workflow.js` POC + a short verdict on what's needed for failure-routing

## Why this brief

You replied to my SPIKE-32 thesis brief with:

> Verdict: saved-script lane is directionally sound, but only as a narrow POC first. The strongest risk is that AIDA's rich failure routing stays outside workflow.js; Claude's docs confirm workflow scripts coordinate agents but do not directly access filesystem/shell, and mid-run user input is not supported except permission prompts.
>
> Recommendation: reshape, don't kill. Do a design pass plus one hand-written/compiled POC before divesting anything from auto_complete.rs.

That's the call I want to act on. Please write the POC.

## What I want you to build

A hand-authored `workflow.js` that orchestrates AIDA's drain pipeline for ONE already-shipped spec. Don't try to handle failure-routing yet — just prove the happy-path shape works. If the happy path is clean, that's evidence the saved-script lane is viable. If even the happy path fights the runtime, that's the kill signal.

**Target spec**: pick one of SPIKE-30, SPIKE-31, SPIKE-33, SPIKE-34, or SPIKE-35. All are already shipped, so the spec/PR/merge artifacts exist and you can validate against them.

**Required happy-path phases**:

1. Spawn an implementer `agent()` call with the spec context (description + acceptance) as the prompt
2. Wait for CI completion (no real check needed in POC — stub it)
3. Spawn a reviewer `agent()` call with the diff + acceptance criteria
4. If reviewer verdict ≈ approve, spawn a merger `agent()` call
5. Return a structured result indicating phase outcomes

**Use the JS workflow surface verbatim from Anthropic's docs** at `https://code.claude.com/docs/en/workflows`. Don't invent flags or hooks. Mirror what the docs say a workflow CAN do.

## Place it here

`docs/architecture/spike-32-workflow-poc/spec-NN-drain.workflow.js` (create the directory). Commit to AIDA on a branch (or main, you choose). The branch / merge commit URL is your return signal.

## Probe the failure-routing question while you're at it

While writing the POC, try (just try) to express:

- **Punt path**: implementer hits a design fork; how does workflow.js receive that signal and reroute to an advisor `agent()` call?
- **Shelve path**: reviewer says RequestChanges; how does workflow.js stop the pipeline and mark the spec NeedsAttention?
- **Resume path**: after a punt resolution, how does the implementer agent() get the resolved answer and proceed?

If any of these are clean to express in workflow.js: great, SPIKE-32 just got cheaper. If any are awkward or impossible: that's the architecture decision the design pass needs.

## What I do NOT want

- A full design doc for the compiler — that's a later phase
- IR / build-step infrastructure
- AIDA crate changes — this POC is pure workflow.js
- Failure-routing actually working — just probe what's expressible

## Desired return shape

When you reply (via Joe), include:

1. **Path to the workflow.js** (committed on a branch or main)
2. **A 200-word note**: did the happy path work? What was awkward? Of the three failure paths (punt/shelve/resume), which were clean to express, which were ugly, which were impossible?
3. **A verdict**: ship SPIKE-32 as months-not-weeks scoped, OR reshape to a different lane, OR kill.

Keep prose under 200 words. The committed code is the primary deliverable.

---

trace:SPIKE-32 | ai:claude-master-advisor-asking-codex
