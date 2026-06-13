# Drain the Decision Inbox

Sweep the whole decision inbox and loop the operator through each pending item
interactively — record the chosen resolution, or author the missing acceptance —
until the inbox is empty. The human-decision drain: the operator-at-the-keyboard
mirror of `aida burndown run` (which drains the decision-FREE ready set headless).

## Instructions

Follow the workflow in `.claude/skills/aida-decide.md`:

1. If `$ARGUMENTS` has `--sweep`, refresh detection first:
   `aida questions sweep --apply`. Otherwise drain what's already in the inbox —
   don't auto-mutate.
2. Read the inbox: `aida questions list`. Empty → stop, don't fabricate work.
   Report how many pending items you're about to drain.
3. Loop each pending item, routing by what it asks for:
   - **Discrete choice** → present the distilled question + enumerated choices
     with AskUserQuestion (recommended default first), then record:
     `aida questions answer <SPEC> <CHOICE>`.
   - **Under-specified spec** (needs criteria written, not one-of-N) → drop into
     the acceptance-authoring loop: `aida questions clarify <SPEC>` (or
     `/aida-clarify <SPEC>` inline).
   - Operator says "take all defaults" → `aida questions answer --all-defaults`
     for the discrete items, then clarify the rest.
4. Re-read `aida questions list` after each resolution; loop until empty (one
   answer can unblock another, so the inbox is the loop condition).
5. Report honestly: decisions recorded, specs clarified, anything left
   unresolved (and why). The just-unblocked specs are now on the burndown ready
   set — offer `/aida-burndown` to ship them.

Distinct from `/aida-advise` (the headless advisor tier that *escalates*
decisions to the human): this skill is the human *consuming* that inbox. It's
interactive — no human at the keyboard → stop.
