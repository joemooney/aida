# Config trust boundary: code-executing config reads from the default branch

trace:TASK-969

## The threat

AIDA's autonomous drain / CI runs `aida` commands from inside git worktrees
that are checked out to **arbitrary branches** — including a freshly-pushed
branch that no human has reviewed yet. A few `.aida/config.toml` fields name a
**shell command** that AIDA then executes. If such a field is read from the
*branch-local working copy*, a pushed branch can edit it to run arbitrary code
the moment an unattended drain touches that branch. That is a supply-chain RCE:
the attacker needs only push access to a branch, not merge access to `main`.

This mirrors the boundary the `no-mistakes` tool enforces by loading its
process-launching config (test / lint / format commands, the agent backend)
only from a pinned default-branch SHA, fail-closed.

## The rule

**Code-executing config fields are read from the TRUSTED DEFAULT BRANCH, not
the checked-out branch.** The trust anchor is `origin/<default>` (the
human-reviewed remote tip — a local `main` could have been advanced by an
unmerged commit). The value is read with `git show <sha>:.aida/config.toml`.

**Non-executing config stays branch-local.** Thresholds, display preferences,
and policy enums cannot launch a process, so reading the branch's own copy is
fine (and often desirable — a branch may legitimately tune its own intake
bias or hint verbosity).

**Fail-closed.** If the trusted copy can't be read — offline fresh clone,
detached checkout, no default branch, or the file simply isn't present at that
SHA — the configured command is **dropped** and the safe **built-in default**
applies. The branch-local copy is *never* used as a fallback.

## What is covered

| Config field | Section | Executing? | Source after TASK-969 |
| --- | --- | --- | --- |
| `smoke_check` | `[pr-rebase]` | **Yes** — run via `sh -c` in the rebased worktree | **Trusted default branch** (fail-closed to built-in default) |
| `disposition_bias`, `on_apply`, `do_not_approve_classes` | `[intake]` | No (policy enums / class list) | Branch-local |
| `fork_mode`, `calibration_mode`, `allow_mtime_fallback`, `keep_fork_jsonls`, `max_source_size_mb` | `[advisor]` | No (behavior toggles / thresholds) | Branch-local |
| `auto_release_dormant_leases`, `stale_lease_threshold_minutes` | `[orchestrator]` | No (toggle / threshold) | Branch-local |
| `strategy` | `[integrate]` | No (merge-strategy enum) | Branch-local |
| `workflow_hints` | `[hints]` | No (display toggle) | Branch-local |

`[pr-rebase] smoke_check` is the **only** `.aida/config.toml` field that AIDA's
own drain/CI code reads and then executes as a shell command. The agent backend
is not a per-project config value — it is hardcoded (`claude`) and gated by the
machine-global `~/.aida/agents.toml`, which is local-trusted and not under a
pushed branch's control. Git hooks (`.git/hooks/*`) are invoked by git itself,
not by AIDA's drain code, and are likewise not checked into the branch.

## Where it lives in code

- `aida-cli/src/trusted_config.rs` — the trust anchor:
  `trusted_default_branch_sha` (resolve `origin/<default>` → SHA) and
  `read_trusted_config_toml` (`git show <sha>:.aida/config.toml`, `None`
  fail-closed). Does not interpret fields; each consumer keeps its own
  executing-vs-not classification.
- `aida-cli/src/pr_rebase.rs` — `read_pr_rebase_config` now sources
  `smoke_check` through `trusted_config`; `pr_rebase_config_from_trusted` is
  the pure, unit-tested `Some(body)→parse / None→fail-closed` selection.
- `aida-cli/src/main.rs` — `pr_rebase_handler` step 7 runs the resolved
  `smoke_check`; the comment there points back to this boundary.

## Adding a new code-executing config field

If you add a `.aida/config.toml` field whose value is a shell command, a
program name, or a path to a script the drain executes:

1. Read it through `trusted_config::read_trusted_config_toml`, not a plain
   `std::fs::read_to_string` of the branch-local file.
2. Fail-closed: when the trusted read returns `None`, drop the configured
   value and use a safe built-in default — never the branch copy.
3. Add a row to the table above.

## Tests

- `aida-cli/src/trusted_config.rs` — a real-git fixture proves the trusted
  read returns the **default-branch** body even when the checked-out branch's
  working copy holds a hostile `smoke_check`, plus fail-closed cases (no git
  repo; file absent on the default branch).
- `aida-cli/src/pr_rebase.rs` — `pr_rebase_config_from_trusted` selection:
  trusted body is honored; `None` fail-closes to the default config; a hostile
  command can only reach execution via the trusted body (which it cannot).
