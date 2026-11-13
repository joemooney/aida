# Integration flow: fast coding on `integration` (STORY-2737)

Operator decision, 2026-11-12: AIDA's per-change ceremony made coding too slow.
Day-to-day work now lands on the `integration` branch with minimal formality;
`main` is the batched formal-verification checkpoint, promoted by a separate
advisor session.

## Who does what

| Session | Where | Job |
|---|---|---|
| **Integration (you)** | `~/ai/aida-worktrees/integration`, branch `integration` | Implement open items fast; commit and push to `integration`; rebuild the live binary |
| **Main (advisor)** | primary checkout `~/ai/aida` | Promote `integration` -> `main` as one PR (full CI + one review), bisect red promotions, keep specs and PR queue honest, merge approved PR branches into `integration` |

## Rules for the integration session

Keep (cheap, preserves months of work):

1. **Specs are the to-do list.** `aida list --no-focus`, `aida show <ID>`,
   `aida search`. Add new findings with `aida add`. Do not keep a separate
   TODO file.
2. **Commit format** `[AI:claude] type(scope): summary (SPEC-ID)`. The trailing
   ID links code to the spec; promotion to main completes specs from it.
3. **One `aida comment add <ID> "..."`** per finished item: what changed, how it
   was checked. One or two lines.
4. **Fast gate before every push:** `make check-ci-fast` (about 90 s on an idle
   machine). For a Rust change, also run one targeted test:
   `cargo test --profile agent -p <crate> <filter>`. Never the full suite.
5. **Push to `integration` often:** `git push origin integration`.
6. **Refresh the live binary after a batch:** `aida dev build --allow-dirty`
   from this worktree. If the new binary misbehaves: `aida dev rollback`.

Skip (this is what made it slow):

- No worktree per spec, no `aida claim` / leases / `aida queue work`.
- No per-change PR, review, train, merge hold, or sketch signoff.
- No waiting on GitHub CI; it does not run for pushes to `integration`.
- Ignore session-start nudges about claiming a worktree or delegating; this
  brief overrides them for this session.

Never:

- Commit to or push `main`. Never force-push `integration` or anything else.
- Rewrite history on `integration` (it is shared with the main session).
- Run `cargo test --workspace`, `make check-ci`, or `make check`.

## Integration <-> main protocol (operator ruling 17:30)

- The integration session is the **sole writer** of `origin/integration`.
  The advisor no longer pushes it.
- Merge a PR branch into `integration` only when it is approved at its
  **exact current head**: `aida review verdict PR-N` must show APPROVED against
  that head. Your own commits still go straight to `integration`.
- After every push, message the advisor seat: `integration pushed at <sha>`.
  Canary draft PR #3905 runs full CI on it; the advisor cuts
  `promote/integration-N` only from a head that is green there.
- When the advisor says `promotion N landed at <main sha>, sync main`:
  check `git diff origin/main <promoted integration sha>` is empty and that
  sha is an ancestor of `origin/integration`, then
  `git merge -s ours origin/main`, push, and report the new sha.

## Parallel work (optional)

Codex lanes on dell1 still work for independent items. The main session can
launch them; their branches get merged into `integration` by the main session.
Ask in the main session ("put SPEC-X on dell1").

## Picking work

Start with what unblocks speed, then the open list:

1. BUG-2540: `aida queue rework` hangs 180 s (stalls autoland dispatch).
2. BUG-2539: overlay fingerprint test breaks on Rust 1.100 (CI pinned to 1.99.1
   meanwhile; unpin in the same change). A dell1 lane may already have a fix.
3. BUG-2538: train coverage proof exceeds the 16 MiB evidence cap. A dell1
   lane may already have a fix.
4. TASK-3789: timestamps on `aida pr train` progress lines. A dell1 lane may
   already have a fix.
5. STORY-2737 slices: (a) `aida dev build` promotes from a clean `integration`;
   (b) a "landed in integration" spec marker; (c) lane base branch from config;
   (d) event-driven promotion with configurable trigger.
6. Then `aida list --no-focus`, highest value first.

Before starting an item, check `git log origin/integration --oneline -20` and
`aida comment list <ID>` so you do not redo a dell1 lane's work.

## Starting the session

From a plain terminal (not one that carries the advisor's role or grant
environment):

```bash
cd ~/ai/aida-worktrees/integration && git pull --ff-only origin integration && claude
```

First message: `Read docs/plans/2026-11-12-integration-flow.md and follow it.`

<!-- trace:STORY-2737 | ai:claude -->
