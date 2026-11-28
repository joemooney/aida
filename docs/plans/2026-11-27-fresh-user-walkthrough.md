# Fresh-user walkthrough: getting started and review prepare

<!-- trace:TASK-3951 | ai:claude -->

Date: 2026-11-27. Binary: `aida 0.16.0 (sha f0ad200d8c)`. Follow-up story:
see the PR body (one draft story, one acceptance criterion per finding below).

## Setup and rules

- A new user. I read only `docs/getting-started.md`, `aida --help`, the help
  text of the commands I ran, and what those commands printed. I did not use
  repository knowledge to work around a step.
- `HOME=$(mktemp -d)`, run under `env -i` (no `AIDA_*` variables, no
  `~/.aida`, no `~/.claude`, no `~/.codex`). `PATH` = `$HOME/.local/bin` plus
  system directories.
- Project: a new git repository with `greet.py` and `README.md`, one commit,
  then a small change to `greet.py` (a `greeting` parameter and a
  `sys.argv` read).

Limitations:

- The shell was not a TTY. Interactive prompts did not appear, and list
  output used the compact agent format instead of tables. A real terminal
  user would see the tables and the init prompts.
- Outbound network was blocked. Option B could not download anything, so I
  copied the locally built release pair into `$HOME/.local/bin` instead.
  This was the brief's documented fallback.
- To test the "installed but not signed in" case, I put Claude Code and the
  Codex CLI on `PATH` without credentials under the empty HOME. Network
  blocking may have caused the Codex hang in F14.

## Transcript (abridged)

```text
$ curl -sSL https://raw.githubusercontent.com/joemooney/aida/main/scripts/install.sh | bash
curl: (56) CONNECT tunnel failed, response 403          # nothing installed, pipeline exits 0
$ aida --version
aida 0.16.0 (built 2026-11-27 19:00:06 -07:00, sha f0ad200d8c)
$ aida --help | wc -l
491
$ aida                       # bare: a short "Getting started" set (good)
$ git init -b main && git add -A && git commit -m initial
$ aida init --help | wc -l
185
$ aida init                  # 1.5 s; 69 lines of output, 11 FAIL lines
$ git log --oneline main
7a34ea7 chore: scaffold AIDA  # 247 files changed, 46410 insertions(+)
5924cf9 initial
$ aida add "Greet by name with a configurable greeting"
ℹ Issuing a node-local ID: no upstream is declared for this clone (run `aida node upstream <remote>`).
Added: TASK-5DFDECCD9FE541228EB053C8E84DA6EC-001 - Greet by name with a configurable greeting
$ aida show TASK-1
error: "Requirement not found: TASK-1 (did you mean TASK-5DFDECCD9FE541228EB053C8E84DA6EC-001?)"
$ aida review prepare --help  # 90 lines
$ aida review prepare         # uncommitted change: works, workspace mode
$ aida review prepare --run-review
error: "--run-review needs --pr or --from"
$ aida review prepare --run-review --from HEAD    # still uncommitted
... Reviewable files: 0 ... Passed: `true` (100% of 0 reviewable files accounted) ... Findings (0)
$ git checkout -b greeting && git commit -am "Configurable greeting"
✗ Commit message validation failed: ... (permissive mode, commit allowed)
$ aida review prepare --run-review --from main    # no claude on PATH
run review: using claude from the built-in default (no [agents] vendor)
error: "claude finder pass, review batch 1: claude not found on PATH"
$ aida review prepare --run-review --from main    # claude installed, not signed in
error: "claude finder pass, review batch 1: review runner failed (exit status: 1); stderr: (empty)"
$ aida review prepare --run-review --from main --agent codex   # codex installed, not signed in
(no output for 55 s; killed by timeout)
```

## Friction points

Each entry lists the command, an output excerpt and the expected behaviour.

### F1. Getting started has no first spec and no first review

- Command: reading `docs/getting-started.md` from section 2 onward.
- Observed: after `aida init`, sections 3 to 6 describe only a conversation
  with an agent. The page gives no command for creating a first spec and
  never mentions `aida review prepare`. I took the first-spec command from
  init's own output. I found `review prepare` only by reading `aida --help`.
- Expected: a short "Your first spec" step (`aida add "..."`, `aida list`,
  `aida show`) and a "Review a change" step
  (`aida review prepare --run-review --from main`), including which agent CLI
  must be installed and signed in.

### F2. The installer pipeline hides a failed download

- Command: `curl -sSL https://raw.githubusercontent.com/.../install.sh | bash`
- Output: `curl: (56) CONNECT tunnel failed, response 403`. The pipeline exits
  0 and installs nothing.
- Expected: the documented command uses `curl -fsSL` (without `-f`, an HTTP
  error page can be piped into `bash`) and fails loudly. The page should also
  name an offline path, such as downloading the release tarball and running
  `install.sh` from it, for machines without direct GitHub access.

### F3. `aida --help` is a 491-line wall

- Command: `aida --help`
- Output: about 130 commands. `init` is on line 325 of 491. The pointer to
  the short "Getting started" set is only on the last line.
- Expected: `--help` leads with the same short set as bare `aida` (which
  works well), or puts the pointer at the top, so a new user does not have
  to scroll to find `init`.

### F4. `aida init` prints a wall of host-readiness FAILs

- Command: `aida init` in a plain Python repository.
- Output excerpt:
  ```text
  FAIL claude:agent-containment: not contained
    Fix: aida doctor --lane-host --fix; configure trusted [contained.vendors] for non-Claude vendors
  FAIL claude:bubblewrap: UsernsBlocked { hint: "sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0" }
  FAIL claude:auth-claude: no recognized credential source; auth readiness unverified
  FAIL codex:codex-config: user Codex workspace-write preset (network on, no extra writable roots)
  FAIL agy:auth-agy: no recognized credential source; auth readiness unverified
  FAIL build-cache: user Cargo build.rustc-wrapper configuration and executable presence (not lane reachability)
    Fix: install sccache and configure RUSTC_WRAPPER ...
  ```
  There are 11 FAIL lines in total, printed right after "AIDA initialized ✓".
  They include a Rust/sccache check on a Python project, a debug-formatted
  enum (`UsernsBlocked { hint: ... }`) and a suggestion to run `sudo sysctl`.
- Expected: a successful init does not print FAILs for optional features the
  user never asked for. Report lane-host readiness only when the user opts
  into autonomous lanes, or collapse it to one line ("Optional: run
  `aida doctor` to set up agent sandboxes").

### F5. `aida init` commits 46k lines to the user's branch without asking

- Command: `aida init`
- Observed: init created the commit `chore: scaffold AIDA` on `main`
  (247 files, 46,410 insertions) and an `aida-store` branch. It also wrote
  Codex and Antigravity hook wiring although neither CLI was on `PATH`. The
  output never mentions the commit. It says `1 files skipped (already exist,
  use --force to overwrite)` without naming the file, in a repository that
  had only two files.
- Expected: init states what it will write and commit, scaffolds only the
  agents it detects, names any skipped file, and either asks before
  committing to the current branch or says plainly that it did.

### F6. Init promises `TASK-1`, but `aida add` issues a 32-character ID

- Commands: init's suggested loop (`aida add "Build login page" → TASK-1`,
  `aida done TASK-1`, `aida show TASK-1`), then `aida add "..."`.
- Output:
  ```text
  ℹ Issuing a node-local ID: no upstream is declared for this clone (run `aida node upstream <remote>`).
  Added: TASK-5DFDECCD9FE541228EB053C8E84DA6EC-001 - Greet by name with a configurable greeting
  $ aida show TASK-1
  error: "Requirement not found: TASK-1 (did you mean TASK-5DFDECCD9FE541228EB053C8E84DA6EC-001?)"
  ```
  `aida list` shows `(agreed id pending)`. The suggested next commands use a
  lowercase form of the same ID (`TASK-5dfdeccd...`).
- Expected: a local-only single-user project gets the short IDs that init
  promises, or init's example matches what `aida add` returns. "Node-local",
  "upstream" and "agreed id" are not explained anywhere a new user would
  look.

### F7. Role and advisor jargon in init's first-run guide

- Command: `aida init`
- Output: the four-step loop includes the stray line
  `(enter advisor at a human TTY with \`aida role enter advisor\`)` between
  `aida done` and `aida show`, followed by
  `Agent bypass confirmation: on (default)` and
  `global roles installed: implementer, product, advisor, reviewer, integrator`.
- Expected: the first-run guide contains only the capture → build → link →
  done loop. Roles and bypass settings appear when the user opts into them.

### F8. The default commit hook shows a red failure on an ordinary commit

- Command: `git commit -am "Configurable greeting"` after init.
- Output:
  ```text
  ✗ Commit message validation failed:
    Commit message must follow conventional format: [AI:tool]? type(scope): description
    Example: [AI:claude] feat(auth): add login validation (FR-0042)
  Commit allowed (permissive mode). Consider fixing for consistency.
  ```
- Expected: in permissive mode, a non-alarming note (or nothing at all),
  without the red ✗ "failed". The example should use the ID style this
  project actually issues, not `FR-0042`. Getting started should mention
  that init installs a commit hook (`--no-hooks` opts out).

### F9. `aida review prepare --help` is written for AIDA developers

- Commands: `aida review --help`, `aida review prepare --help`
- Observed: `aida review` is described as "Drive human review of a held
  spec", with 19 subcommands. `prepare` is described as "Deterministically
  classify diff inventory, match path-scoped review playbooks, bundle
  token-budgeted review batches, verify 100% file coverage ledgers, and
  anchor quoted code snippets to exact 1-based line numbers". The `--path`
  example uses AIDA's own source tree
  (`--path aida-core/src --path aida-cli-lib/src`). `--run-review` says
  "review settings come from the merge base". The help has no usage example.
- Expected: a one-line plain description ("Review a change with an AI agent
  and get line-anchored findings"), two or three examples
  (`aida review prepare --run-review --from main`,
  `... --agent codex`, `... --sarif review.sarif`), and a neutral `--path`
  example.

### F10. `--run-review` refuses uncommitted work without saying what to do

- Command: `aida review prepare --run-review` with uncommitted changes.
- Output: `error: "--run-review needs --pr or --from"`. Plain
  `aida review prepare` inventories the same working tree without
  complaint.
- Expected: `--run-review` reviews the working tree like plain `prepare`
  does, or the error says what to do: "commit your change, then run
  `aida review prepare --run-review --from main`".

### F11. An empty review range reports a green pass

- Command: `aida review prepare --run-review --from HEAD`, with uncommitted
  changes.
- Output: `Reviewable files: 0`, `Passed: true (100% of 0 reviewable files
  accounted)`, `Findings (0)`, exit 0.
- Expected: when the range is empty and the working tree is dirty, the
  command warns ("0 files in range; you have uncommitted changes") and does
  not report a pass.

### F12. A missing agent CLI fails late, with internal wording

- Command: `aida review prepare --run-review --from main`, with no `claude`
  on `PATH`.
- Output:
  ```text
  run review: using claude from the built-in default (no [agents] vendor)
  error: "claude finder pass, review batch 1: claude not found on PATH"
  ```
- Expected: a check before any work starts, with plain text: "Reviewing
  needs Claude Code or the Codex CLI. Install one, or pass `--agent codex`."
  "Finder pass", "review batch" and "[agents] vendor" are internal terms.

### F13. A signed-out agent CLI is reported as `stderr: (empty)`

- Command: the same, with Claude Code installed but not signed in.
- Output:
  `error: "claude finder pass, review batch 1: review runner failed (exit status: 1); stderr: (empty)"`.
  Run directly, Claude prints `Not logged in · Please run /login` on stdout,
  but AIDA drops that line.
- Expected: AIDA shows the agent's own message and the fix ("Claude Code is
  not signed in: run `claude` and use /login, then retry"), ideally from a
  sign-in check before the review starts.

### F14. A signed-out Codex hangs silently

- Command: `aida review prepare --run-review --from main --agent codex`,
  with Codex installed but not signed in.
- Output: nothing at all for 55 s (the tool time limit), not even the
  `run review: using ...` line that the Claude path prints. Blocked network
  may explain the hang, but the silence does not depend on it.
- Expected: a progress line when the review starts and for each batch, a
  Codex sign-in check before the review starts, and a bounded timeout with a
  message naming the agent and the likely cause.

### F15. Rough edges in the prepare report

- Command: `aida review prepare --from main --specs <ID>`
- Observed: inline code spans are padded with spaces
  (`` ` greet.py ` ``, `` **Head**: ` 48f3c91... ` ``), unlike the neighbouring
  fields. A linked spec with no acceptance criteria injects an empty
  ` ```text ``` ` block silently.
- Expected: consistent code spans, and a visible note when a linked spec has
  no acceptance criteria ("TASK-...: no acceptance criteria; the review
  checks only general quality").

## What worked well

- Bare `aida` gives a short, friendly "Getting started" command set.
- `aida init` finished in 1.5 s, and `aida add` in 0.17 s.
- Plain `aida review prepare` on a working tree produced a clear inventory
  and matched the Python playbook automatically.
- `--specs TASK-1` failed with a clear error that named the missing ID.
