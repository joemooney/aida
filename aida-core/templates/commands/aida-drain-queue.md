# Drain Your Role's Work Queue

Assemble a correctly-phrased `/goal` autonomous loop that drains your
active role's queue — one item at a time, until it is empty.

## Instructions

Follow the workflow in `.claude/skills/aida-drain-queue.md`:

1. Resolve the parameters from `$ARGUMENTS`: `--mode review|merge`
   (default `review`), `--role <name>` (default the active role),
   `--batch <tag>`, `--max <N>`, `--dry-run`.
2. Confirm the queue is non-empty: `aida queue list --role <role>`
   (add `--batch <tag>` when set). If empty, stop — nothing to drain.
3. Assemble the `/goal` text from the skill's template. Two rules the
   skill exists to enforce:
   - **Real command flags only** — `aida queue work` with no id picks
     up the head item; there is no `--next`.
   - **Mechanism clause matches the mode** — `review` keeps the
     reviewer in the loop via `aida session end` (files a Review PR-N
     story); `merge` autonomously merges and skips the reviewer.
4. With `--dry-run`: print the assembled `/goal` text and stop.
   Otherwise: invoke it as `/goal <text>` to start the drain.

Pairs with `/aida-pickup` (the per-item loop body) and `aida goal`
(machine-checkable completion conditions).

ARGUMENTS: $ARGUMENTS
