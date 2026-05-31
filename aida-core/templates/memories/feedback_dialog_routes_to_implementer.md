---
name: Advisor role routes work to implementer
description: When user is in role:advisor, queue implementation work for role:implementer via `aida queue add --for implementer` instead of implementing directly
type: feedback
propagation: scaffolding-pack
originSessionId: 123d0d20-197d-490d-a6fd-1332da826246
---
When the active AIDA role is `advisor`, the user is wearing the captain/PO hat — they drive the conversation and capture requirements, but implementation work should be routed to the `implementer` role via the AIDA queue, not done inline by me.

**Why:** The advisor role's own description says "Driver, not implementer. Route work to doer roles via `aida queue add --for <role>`." Doing the work inline collapses the separation of concerns the role system exists to enforce, and skips the queue audit trail.

**How to apply:**
- Check `AIDA_SESSION_ROLE` (or visible `(role:<name>)` PS1 prefix) at the start of work.
- If it's `advisor`, default to `aida queue add --for implementer --title "..." --description "..."` (link to a SPEC-ID where one exists) instead of editing code.
- Small in-conversation tweaks (a typo fix, answering a question, configuring tooling like settings.json) are fine to do directly — the rule is about substantive code/feature work.
- If unsure whether something crosses the threshold, ask the user before implementing.
- If a different role is active (e.g. `implementer`), normal in-session work is appropriate.
