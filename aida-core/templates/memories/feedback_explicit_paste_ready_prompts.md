---
name: Be explicit about paste-ready prompts vs advisor-to-user framing — mark the boundary
description: When an advisor response contains text the user should relay to an agent (implementer, reviewer, orchestrator) — or a command the user themselves should run — mark it clearly as paste-ready (labelled block, fenced code, blockquote). Don't blur it with advisor-to-user prose framing.
type: feedback
propagation: scaffolding-pack
originSessionId: 1bf450af-f9e9-49d0-a098-79696b14cefc
---
When the advisor writes a response that contains BOTH:

- *Strategic framing / reasoning* — advisor-to-user prose ("here's why X, here's what's at stake"); and
- *A directive the user should relay or execute* — text to paste to an implementer/reviewer/orchestrator session, or a command to run themselves;

the two MUST be visually distinguished. The user shouldn't have to ask *"is that what I tell the agent?"* — the structure should answer it before they ask.

**Why:** 2026-05-19, several incidents in one session. I wrote responses like *"Go — land the fix on story-306 and re-smoke"* as my advisor-to-user framing for the recommendation; the user (correctly) had to ask *"is that what I tell the agent?"* before they could act. The verbatim feedback: *"as an advisor, it helps if you are explicit in terms of paste-ready prompts for the implementer or reviewer or human."*

**How to apply:**

- When the response contains text the user will *paste* to an agent (or a command they should *run* themselves), present it in a **labelled, set-apart block** — a labelled blockquote, fenced code, or section header like *"Paste to the implementer:"* / *"Run this in terminal 2:"*. Don't bury it in framing prose.
- Advisor-to-user framing stays as prose paragraphs *around* the paste-ready blocks. The visual contrast is the signal.
- For longer relays, a section header (`## Paste to the implementer`) is clearer than a bold line.
- When the user is at a prompt awaiting their relay decision, **lead with the paste-ready block, then explain *why* below** — so the action is visible without scrolling through framing.
- Label the *audience* explicitly (implementer / reviewer / human / shell) — *"paste to the implementer"* beats generic *"paste this"*; the user often has multiple terminals open with different roles.

**Counter-pattern to avoid:**

Mixing imperative directive prose (*"Go — do X"*) with advisor-to-user framing in the same paragraph. The user can't tell which voice is which, and has to ask before acting.

**Composes with:**

- [[feedback_finish_checkpoint_clarity]] — the rubric for the *agent's* outbound prompts (state, deciding factor, recommendation, consequence-laden options, advise escape). This memory is the *symmetric* rubric for the advisor's outbound prompts: mark the audience and the form (paste vs framing) so the consumer knows what to do.
- [[feedback_parallel_vs_sequential_ui]] — UI shape signals consumption mode. Same idea: structural cues telegraph intent.
