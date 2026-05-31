---
name: Capture observations as filings — backlog is a valid state; refusing to file loses the observation
description: When you identify a bug, papercut, or gap, FILE it (low priority / status approved / un-queued = backlog is fine). The over-engineering caution applies to BUILDING speculative infrastructure, not to CAPTURING observations. Capture and prioritise are separate.
type: feedback
propagation: scaffolding-pack
originSessionId: 1bf450af-f9e9-49d0-a098-79696b14cefc
---
When the advisor identifies a real observation — a CLI papercut, an outcome-model gap, a discoverability friction — **file it.** Don't withhold capture in the name of "concentration." `priority: low` / `status: approved` / un-queued IS the right captured state for non-urgent observations. The act of filing IS the substrate-building work; refusing to file LOSES the observation.

**Why:** 2026-05-19 — over the course of one session I kept identifying real papercuts (`aida queue move` false-success on an absent target; orchestrator misclassifying *"PR deliberately held"* as phase-1 failure) and saying *"won't file unless you say so"* / *"concentration discipline."* The user corrected: *"I don't understand why you would not want to at least capture things you identify as needing to be reviewed later."* Captured observations cost one `aida add`; lost observations are gone — exactly the substrate decay the autonomy vision cannot afford.

**Refinement, 2026-05-20:** even the "want me to file?" gate is wrong. User explicit: *"Perhaps you are relying on me to remember to file but I will not remember. File if only deep in the backlog but don't wait for me to concur — there are too many potential issues to consider."* Asking for permission to file is itself a friction that loses observations: the user is mid-recovery / mid-implementation / mid-context-switch and will not page back to your offered captures. **The substrate is the keeper, not the user's working memory.** File proactively; tell the user what you filed; let them dismiss or de-prioritise after the fact if they disagree.

**The distinction I was blurring:** [[feedback_pushback_on_overengineering]] is about not *building* daemon-grade infrastructure speculatively, or filing EPIC-shaped work as if it were MVP. It is NOT about refusing to *capture* small observations. The advisor-role's explicit responsibility (see [[feedback_dialog_role_responsibilities]]) includes *"capture friction as filings"* — that is the duty, not the over-reach.

**How to apply:**
- **File without asking.** Skip the *"want me to file?"* offer. If the observation clears the substantive-vs-nit bar, just file and report. The user can dismiss / de-prioritise / re-tag after the fact.
- When you observe a real bug / papercut / gap, file it the moment it's observed. Default: `priority: low` (or `medium` if it bit you in-session), `status: approved`, tagged for searchability — and add `backlog` as a tag if it shouldn't auto-be-queued.
- Backlog state = filed-but-not-queued. Use it. The spec graph carries the observation; the queue is the working subset. Don't conflate the two.
- Do not delete or downgrade captures because they "aren't urgent." Priority is the lever, not capture-or-not.
- The over-engineering caution kicks in at IMPLEMENTATION time — *"smallest valuable slice + revisit trigger"* — when the spec is picked up for work. At filing, capture cleanly.

**Composes with** [[feedback_dialog_role_responsibilities]] (the advisor's capture duty), [[feedback_aida_capture_proactive]] (proactive capture), [[feedback_capture_doc_seeds]] (capture during work), [[feedback_pushback_on_overengineering]] (pushback applies to build, not to file).
