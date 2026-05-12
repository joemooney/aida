# Capture Living Documentation

Capture the WHY behind recently-touched specs as `Doc` requirements.

## Instructions

Follow the workflow in `.claude/skills/aida-doc.md`:

1. Pick the candidate spec — from `$ARGUMENTS` if provided, otherwise from `aida role show` recent activity
2. If the candidate already has Doc entries, surface them first (`aida doc show <SPEC>`)
3. Ask at most three seed questions: scenario, motivation, alternatives, example
4. Draft a short Markdown body from the answers — only sections the user actually answered
5. `aida doc add --title "..." --about <SPEC> --scenario "..." --audience ... --description-from-file <tmpfile>`
6. Confirm the new `DOC-N` id and offer to capture another candidate or stop

Pairs naturally with `/aida-pickup` (after a spec ships), `/aida-review` (after merge), and `/aida-capture` (at session end). Skip when the work was pure refactor/rename or the user says don't capture.
