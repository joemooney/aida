# Pick Up Next Queued Item

Pull the next item routed to your active role, work it, mark it done.

## Instructions

Follow the workflow in `.claude/skills/aida-pickup.md`:

1. Show the active role and queue head (`aida queue next`)
2. If `aida findings list --count` is non-zero, surface a one-line nudge that review findings await triage
3. If the queue is empty, stop — don't fabricate work
4. Confirm pickup with the user, then `aida edit <id> --status in-progress`
5. Drive the implementation, add `// trace:<id> | ai:claude` comments, commit
6. `aida queue done <id>` to atomically complete + remove from queue
7. Optionally offer to grab the next item

Pairs with the `dialog` role on the producer side (`aida queue add <id> --for <role>`).
