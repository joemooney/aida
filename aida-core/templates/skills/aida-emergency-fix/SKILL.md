---
name: aida-emergency-fix
description: Operator-invoked urgent small fix with one targeted check and deputy-attributed emergency landing.
disable-model-invocation: true
---

# Emergency fix

<!-- trace:STORY-2732 | ai:codex -->
Use only when the operator explicitly invokes this skill for an urgent fix.
Never invoke from a sweep, a lane, reviewer/watch/autoland, or on the model's
own initiative. This workflow is vendor-neutral: Claude, Codex and Antigravity
use the same operator attribution. Do not clear agent context or manufacture
a TTY to pass a human-only gate.

1. Read the owning spec and requested fix. Create a scratch worktree on a
   fresh branch off `origin/main`.
2. Make the small change with its spec trace. Run ONE targeted check that
   proves the fix; do not run the full suite. Stop on failure and report it.
3. Commit `[AI:tool] type(scope): summary (SPEC-ID)`, push the branch, and
   open a PR with `gh pr create`. Preserve the spec trailer in the PR title.
4. Write a UTF-8 shell script outside the checkout that changes to the
   operator's checkout (never a leased lane clone) and runs
   `aida pr emergency-land N`. Submit that file with
   `aida deputy shell submit --file F --wait`. The operator must already
   have started the script deputy at their terminal; if it is absent or
   submission refuses, report the refusal and stop. Do not bypass it by
   running the emergency command directly from an agent shell.
5. Branch on the submission/job exit code. Zero means the landing, protection
   restoration and requested CI watch succeeded. Exit 75 means the job is
   pending: inspect its returned job id and continue bounded status/wait
   steps; never resubmit the same landing just because a wait expired.
   Other nonzero exits are failures. Surface the exact recovery command if
   restoration failed. If post-merge CI turns red, surface the revert PR
   recipe; never revert automatically.
6. Record the cause, fix, targeted check and actual landing outcome on the
   spec with `aida comment add`.

The command saves protection, lifts admin enforcement only when enabled,
admin squash-merges, always restores and verifies protection, writes an audit,
and by default watches merge-commit CI for up to 30 minutes. `--dry-run`
previews without API writes; `--no-watch` skips the watch. The default cap is
200 changed lines and ten files; `--allow-large` is an explicit operator
choice. This command currently supports GitHub; GitLab parity is separate.
