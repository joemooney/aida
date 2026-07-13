---
description: "Reconcile every `aida human` item so the report is true from the graph — triage each into needs-you / fix-state / file-tool-bug, act, and hand back a ledger."
---
# Review the Coordination Inbox

Reconcile every `aida human` item so the "does this need me?" report is true
**from the graph**, not narrated away in chat.

## Instructions

Follow the workflow in `.claude/skills/aida-review-human.md`:

1. Run `aida human` + `aida awaiting --json`; enumerate every surfaced item
   (reviews-awaiting, needs-attention, findings, mail, verdicts, escalations).
2. Triage each into exactly one and act:
   - **A — genuinely needs you** → keep it, state the decision + recommendation + consequence.
   - **B — stale state** → fix the spec (`aida edit --status` / `aida defer --until "<trigger>"` / `aida archive` / add a `blocked-by`/`parent`) so it self-corrects.
   - **C — tool false-positive** → file or link the reporter bug (e.g. BUG-722); do **not** edit the spec to paper over it.
3. **Verify each B edit** actually changes the report (re-run `aida human`); if it doesn't, it's really **C** — reclassify + file.
4. Hand back a ledger: item → verdict → action → `aida human` delta, leading with the A items.
5. `aida push` any edits.

NEVER auto-approve or auto-merge to silence an item — that is an explicit human
judgment (bucket A).
