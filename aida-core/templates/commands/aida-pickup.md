---
description: Run /aida-pickup.
---
# Pick Up Next Queued Item

Pull the next item routed to your active role, work it, mark it done.

## Instructions

Follow the workflow in `.claude/skills/aida-pickup/SKILL.md`:

1. Show the active role and queue head (`aida queue next`)
2. If `aida findings list --count` is non-zero, surface a one-line nudge that findings await triage
3. If the queue is empty, stop — don't fabricate work
4. Confirm pickup — **an item id passed as the argument IS the consent** (the operator or
   orchestrator typed it; do not ask again), and under a headless session (`AIDA_HEADLESS=1`)
   never ask anything: there is nobody to answer, and an unanswered question ends the session
   with no work done. Only a bare `/aida-pickup` in an interactive conversation pauses to
   confirm. Then `aida edit <id> --status in-progress`
5. Render the picked-up spec as a card so its contract stays in terminal scrollback (`aida show <id> --card`; `--brief` / `--full` for autonomous drains or deep dives)
6. Drive the implementation, add `// trace:<id> | ai:claude` comments, and run targeted tests.
   In a native sandbox (`AIDA_CONTAINMENT_POSTURE=native`), Git metadata is read-only:
   leave edits uncommitted, write a final handoff with changed files, validation results and
   any blockers, then exit. The host launcher commits, pushes and opens the PR after exit;
   it preserves a Refs PR's non-completing trailer.
   For an approved partial slice, first record its subject with
   `aida ship -m '[AI:codex] type(scope): subject (Refs ID)'`. This only records
   a lease-bound declaration; failed exits always retain Refs.
   For required commit-body trailers such as `Help-Golden: unchanged`, add
   `--commit-body-file <UTF-8-file>` to that declaration. The host also accepts it
   with `--from-seat <clone> --commit-pending '<subject>'`.
   <!-- trace:BUG-2493 | ai:codex -->
   <!-- trace:TASK-2902 | ai:codex -->
   Do not attempt commit/push/PR creation
   or `aida queue done` from this seat. The host finishes queue completion after publication.
   In other postures, commit the implementation.
7. Outside a native sandbox, `aida queue done <id>` atomically completes + removes from queue
8. Under a headless drain (`AIDA_HEADLESS=1`), file conversational flags as draft findings (Step 5b)
9. Optionally offer to grab the next item

Pairs with the `advisor` role on the producer side (`aida queue add <id> --for <role>`).
