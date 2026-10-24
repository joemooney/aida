---
name: aida-pr
description: Open implemented work for independent review through aida ship, with linked specs and validation evidence.
disable-model-invocation: true
allowed-tools:
  - Bash
  - Read
---

# AIDA PR compatibility entry point

<!-- trace:TASK-1613 | ai:codex -->

Use the CLI finish ceremony instead of reproducing commit, status, push,
PR creation, or cleanup logic. `aida ship` commits uncommitted work, rebases,
pushes, and opens or reuses the branch's PR. Its default continues through
CI, merge, sync, and cleanup; **opening work for review uses `--no-merge`**.

| Path | What happens | Why |
|---|---|---|
| ▶ Independent review | Authorized publication uses `aida ship <SPEC> --no-merge`, then `aida pr auto-queue-review` | The implementer stops before merge and hands off to an independent reviewer |
| ⇒ Preview first | Run `aida ship <SPEC> --no-merge --dry-run` before publishing | Inspects the commit, rebase, push, and PR steps without mutating Git or the forge |
| ⏸ Parent-owned publication | In corroborated `orchestrated` mode, commit and push (`aida ship <SPEC> --no-pr`), do not open the PR, then exit (touch `$AIDA_EXIT_SENTINEL` last in headless/zen) | The parent owns preflight and publication; the child must not race that boundary |

## Preview and review handoff

Resolve the implemented spec from the assignment/current session. Inspect
`aida ship --help` for supported options; legacy skill options are retired.
Before publishing, run `aida orchestrator status` (do not infer authority from a
bare environment flag):

- **Orchestrated child (`aida orchestrator status` = `orchestrated`, TASK-1289 / TASK-329):**
  the parent phase owns preflight and PR creation. Do **not** open a PR or run
  autoqueue in this child session. Commit and push the branch, then exit — in
  headless/zen mode, touch `$AIDA_EXIT_SENTINEL` as the session's absolute last
  action so the orchestrator reaps the REPL:

  ```bash
  aida ship <SPEC> --no-pr
  [ -n "${AIDA_EXIT_SENTINEL:-}" ] && touch "$AIDA_EXIT_SENTINEL"
  ```

  In interactive orchestrated mode, do not touch the sentinel; tell the user to
  press Ctrl+D so the orchestrator advances. Other managed contracts that forbid
  publication follow their own stop boundary.
- **Normal authorized review handoff (outside `orchestrated` mode):**

  ```bash
  aida ship <SPEC> --no-merge --dry-run
  aida ship <SPEC> --no-merge
  aida pr auto-queue-review
  ```

  If a non-orchestrated headless/zen caller provides `$AIDA_EXIT_SENTINEL`,
  touch it (`[ -n "${AIDA_EXIT_SENTINEL:-}" ] && touch "$AIDA_EXIT_SENTINEL"`) as
  the absolute last action after reporting the handoff outcome.

Run the last command only after PR creation/reuse succeeds. `--no-merge`
stops at the open PR: it does **not** watch CI, merge, clean up, or queue
independent review. The explicit autoqueue call is idempotent and routes
review through existing policy. Read its result: filed/already-filed means
review is routed; a by-design skip (which may exit zero) or failure does not.
Report the reason and the command's retry guidance; never claim review was
queued without that evidence.

## Agent judgment and boundaries

- Confirm the intended changes and relevant validation are ready before
  invoking the ceremony. `ship` stages all uncommitted work; avoid including
  unrelated changes. Shipped a new CLI slice? Update its parent skill to call
  it, following the existing skill/CLI symmetry discipline.
- Give the PR a clear summary, linked specs, and actual validation results
  and limits. Creation derives title/body from the latest commit; prepare
  useful commit text and inspect the resulting PR. Enrich the description
  through the existing forge tooling if needed; the optional
  [description example](examples/pr-description-template.md) can help.
  Keep every covered spec in the title's trailing parenthesized group.
- Keep review independent: the implementer hands off, never supplies their
  own approval or self-merges by default. Opening a PR is not completion;
  never mark a spec Completed before merge. Use the existing Done checkpoint
  only when acceptance is actually delivered and its gates permit it.
- Preserve draft-only/human-review policy. `ship --no-merge` is not a draft
  option: newly created PRs are non-draft. If an actual draft is required,
  use the established draft PR path instead and then the same explicit
  autoqueue call. Do not lift draft/review restrictions to finish this skill.
- Supervised sessions retain human judgment at real design forks. Managed
  or headless implementers follow their orchestrator's publication, stop,
  verdict, and handoff contract. If it forbids publishing, stop at the
  requested artifact; if publishing is allowed, stop at review handoff.
  Do not invoke a full merge path or independent session/worktree cleanup.
  Report or punt failures through the existing contract, without inventing
  approvals or bypasses.

## Authorized finish and lower-level shipping

Full `aida ship <SPEC>` is for an explicitly authorized finish-and-merge path
with required independent review already satisfied. It does not run an
independent reviewer for you. Preview with `aida ship <SPEC> --dry-run` and
respect all role, ownership, review, hold, and CI gates.

`aida pr ship` is the lower-level PR/CI/merge/sync/cleanup tail used by the
full ceremony; it is not the commit/rebase/open-for-review redirect. Calling
it instead of `aida ship --no-merge` would enter the merge path. Leave merge
and cleanup to the authorized reviewer/integrator or orchestrator.
