# Chapter 4 — Git & lifecycle

This is the chapter that untangles the question every git-fluent newcomer gets wrong: **"I merged it — isn't it done?"** In AIDA, *finishing*, *merging*, *completing*, and *releasing* are four different events with four different verbs. Get this chapter and the rest of the lifecycle stops feeling fussy and starts feeling precise.

It also covers AIDA's **two-leg git model** — the fact that every sync touches *two* branches (your code, and the orphan store that holds the specs) — which is why AIDA gives you `pull`/`push`/`fetch` instead of letting you use raw git.

> Manual contract reminder: rationale, not flag tables. `aida <cmd> --help` is the source of truth for exact flags.

---

## The vocabulary, once and precisely

A spec's life after you start building it:

| Event | Verb | State it lands in | Who triggers it |
|---|---|---|---|
| Work finished on a branch, PR open | `aida queue done` / `aida pr` | **Done** | the implementer |
| Reviewer read the diff, sent it back | `aida review` → request changes / `aida rework` | **Rework** (→ In Progress) | the reviewer |
| Reviewer passed it; PR merged to main | the merge + `aida pull` | **Completed** | the merge (auto-bump) |
| A version tag cut after merge | `aida release` | **Released** | the releaser |

The two that trip everyone:
- **Done ≠ Completed.** *Done* means "the work exists on a branch / in a PR." Nothing has merged. *Completed* means "merged to the default branch." You almost never set Completed by hand — **the merge earns it**: when a commit referencing the spec lands on main, `aida pull` notices and bumps Done → Completed automatically.
- **Completed ≠ Released.** Merged code isn't shipped to users until a version tag. `aida release` flips the merged-since-last-tag specs to Released.

Keep this table in your head and every command below is obvious.

---

### `aida commit`

**One line** — author a commit message that *can't* trip the commit-msg hook, then commit.

**Mental model.** AIDA's commit-msg hook insists on the conventional shape `[AI:tool]? type(scope): description (REQ-ID)`. A reflexive `git commit -am "fix stuff"` gets rejected. `aida commit` is the *builder*: you give it the parts (`--type`, optional `--scope`, `--message`, optional `--spec`) and it assembles a compliant message, **validates it against the same rules the hook enforces**, and runs `git commit`. It's the CLI-native, plain-terminal (and Codex) counterpart to the `/aida-commit` skill, which does the same job from inside a Claude session.

**Reach for it when** — you're committing from a plain terminal and don't want to hand-format the message (or remember whether feat/fix needs a REQ-ID, or whether the `[AI:tool]` prefix is required). Especially after the hook just rejected a casual `git commit`.

**Don't reach for it when** — you're already in a Claude session (use `/aida-commit`, which also links specs), or you genuinely want a non-conventional message (then it's `git commit --no-verify`, deliberately).

**What it infers.** If you omit `--spec`, it scans the staged diff for `// trace:SPEC-ID` comments — exactly one distinct spec → it becomes the `(REQ-ID)`; multiple or none → no trailer (fine for chore/docs). The `[AI:tool]` prefix is added only when an AI-authored trace (`trace:ID | ai:...`) is staged, matching the hook's own rule; `--ai <tool>` forces it on, `--no-ai` forces it off. `feat`/`fix` require a REQ-ID, so it errors early with guidance if none can be resolved.

On GitHub, `ship` polls per-check rows with a 20-minute bound, rather than
waiting for every job through `gh pr checks --watch`. Pending and failed checks
matching `[ci].informational_workflows` or `informational_checks` are ignored
unless branch protection requires them. Other pending checks are awaited and
other failures block shipping, even when branch protection lists no required
checks. GitLab and other providers retain their existing pipeline watch.
<!-- trace:TASK-1331 | ai:codex -->

**Gotchas.** `--dry-run` prints the assembled message without committing — use it to preview. Without `-a/--all`, something must be staged. The message is self-checked before the commit fires, so what you see is what the hook will accept.

**Chains with** — `git add` (stage) → `aida commit` → `aida pull` (after merge, auto-bump to Completed).

---

### `aida done`

*(Covered in [Chapter 1](01-getting-started.md#aida-done).)* The newcomer shortcut — "I finished it." Once you're on a real pipeline, **stop using it** and use `aida queue done` (lands **Done**, the precise "finished on a branch" state) so the merge can earn **Completed**. `aida done`'s simplicity is also its limitation: it doesn't know where in the lifecycle you are.

<!-- trace:TASK-1328 | ai:codex -->
`aida queue done` recognizes both the resolved requirement's display ID and its
stored origin ID when checking branch ownership and commit evidence. A session
can finish on its original branch after the store assigns a new display ID.
Every recognized requirement ID in a scoped branch must belong to that same
requirement (including existing dashed child variants); an unrelated branch
cannot gain ownership through an alias in a commit trailer. No branch rename
or forced completion is needed for an ID remapping.

---

### `aida pr`

**One line** — the pull-request side-effects that fire around opening a PR.

**Mental model.** `aida pr` is a small family of *PR lifecycle actions*, not a single command. The headline subcommands: `ship` (the fast path — create-if-needed → watch CI → squash-merge → pull → cleanup, in one call) and `auto-queue-review` (files the reviewer story the moment the PR opens, while context is fresh). Think of it as "the things that should happen at PR boundaries, automated."

**Reach for it when**
- `aida pr ship` — you have **already-approved** work (an independent approval is recorded on the PR head) that needs *no* orchestrator review phase: docs PRs, master-signed architecture work, recovery merges. It's the direct-publish counterpart to the full reviewed pipeline.
- `aida pr hold` — you want to push the branch but *deliberately not open the PR yet*, pending a manual gate (a smoke test, an out-of-band sign-off).
- `aida pr rebase` — collapse the standard 6-command "rebase a PR before review" recipe into one.
- `aida pr gc` — opt-in sweep of local `pr-N`/`mr-N` review-snapshot branches (created when you or a headless review fetches a change's head ref) whose change has reached a terminal state. Never removes a branch tied to a still-open change, checked out anywhere, or diverged from the fetched head; `--dry-run` previews.
- `aida pr list` — one row per open PR/MR: the spec IDs it maps to (from its title, else branch, else body, else a lease on its branch), comma-joined when there are several (`spec_id` in `--json`, `null` with `mapping_source: none` when it maps to nothing; `--mapped-only` hides those), plus review status, CI, and whether a rebase is needed. The CI column reads "Failing" when any red check is *not* matched by the `[ci]` `informational_workflows` / `informational_checks` allow-list in `.aida/config.toml` (default: workflows named `Cross-platform*`). Branch-protection required checks are not consulted, so a red check that does not actually gate the merge still reads "Failing" unless you allow-list it; when every red check is allow-listed the column shows "Passing (N informational red)".

**Don't reach for it when** — the work has not been **reviewed** yet. `aida pr ship` does not run a reviewer phase; it merges only when an approval of the PR head has already been recorded by a session that did not author the work, and refuses otherwise. For work that still needs review, use `aida queue work PR-N --auto-complete` (the full reviewer pipeline) instead.

**Gotchas.** `auto-queue-review` and `ship` detect the PR via `gh pr list --head <branch>`, so `gh` must be on PATH and authenticated. `ship` squash-merges — if you need merge commits preserved, it's the wrong verb.

**Non-completing slices.** The squash subject's trailing `(SPEC-ID)` group is
what completes a spec at merge time. Ship derives it from an explicit title
trailer first, then from the branch name — so a slice PR with no id in its
title but a `<spec-id>-…` branch would complete `<spec-id>` after one of many
PRs. Three things stop that: `aida pr ship --no-complete`, a PR-body line
reading `aida: no-complete` or `Non-completing slice` (either one withholds
the trailer from every source, so the merge completes no spec), and, with no
declaration at all, the branch-name fallback is skipped when the spec carries
`closure:pending` or still has open slice children (a Done child counts as
finished). An explicit title trailer is always honoured. Before merging, ship
prints the trailer it derived and its source — `from the PR title`, `from the
branch name`, or `none` with the reason it was withheld — and names the ids
the merge will complete; `--dry-run` prints the same line.
<!-- trace:BUG-1919 | ai:claude -->

A non-completing squash ship writes the `aida: no-complete` line into the
squash commit's body, so the pull on every clone reads the declaration from
main. This works on GitHub (`gh pr merge --body`), GitLab (inside the squash
commit message, after the MR title) and pure-git (the squash commit's body).
The cost is on GitHub: an explicit body replaces GitHub's default one, so
GitHub does not add `Co-authored-by:` trailers for the other commit authors
on that merge. Trailers already written in commit messages are kept. An
ordinary squash ship leaves the body to the forge.
<!-- trace:TASK-1951 trace:TASK-2115 trace:TASK-2116 | ai:claude -->

A title ending in `(Refs SPEC-ID[, …])` is a reference, not a trailer: ship
appends no completion trailer to it, not even from the branch name, and the
merge completes nothing. Merge-time completion reads intent from the line
carrying the trailer: an id the subject names in a `(Refs …)` group is not
completed even when a bare `(SPEC-ID)` group follows it, while a squash-body
bullet such as `* slice (Refs SPEC-ID)` never suppresses the subject's trailer.
<!-- trace:BUG-2203 | ai:claude -->

**Drive-owned PRs.** A live drive's PR remains protected by its independent
reviewer. A direct human terminal invocation reports the owning wave/PID
and phase, plus recent headless activity when that member's session is known.
Activity from another member is never attributed to this PR. Missing activity
is reported as unavailable; a live PID alone does not establish progress.
The explanation describes the outcome: the drive merges if authorized or
escalates the merge decision to the operator. It does not prompt.

Use `aida pr ship <N> --wait 120` to poll ownership every two seconds for up to
120 seconds; bare `--wait` uses 300 seconds. Release continues through the
ordinary CI, approval, and merge-hold gates after refreshing PR metadata. If
the drive merged while waiting, ship runs post-merge sync/cleanup without
watching CI, merging again, or claiming merge credit. Shared run identity is
reported unavailable because phase entry can leave a sibling run UUID in the
snapshot. Timeout and ownership refusals
exit non-zero. Drive seats (including headless callers) refuse immediately,
even with `--wait`, so they cannot wait on their own pipeline. The explicit
`AIDA_PR_SHIP_ALLOW_IN_DRIVE=1` override remains available and never bypasses
merge holds. Operator explanations use the same direct-TTY/non-managed-agent
predicate as direct role grants.
<!-- trace:TASK-1602 | ai:codex -->

**Preflight and CI deadline.** Ship checks mergeability, approval at the current
head, verdict reconciliation, review-in-progress, merge-holds (including a
label without a local marker), and stale CI definitions before watching CI.
It checks again before merging because the head and gates can change during
the wait. The stale-check and stale-approval overrides apply in both phases;
they do not bypass verdict reconciliation or holds.

`aida pr ship [N] --wait [SECS]` bounds drive ownership, registration, and
CI settlement with one deadline. Bare `--wait` uses 300 seconds. Omit `--wait` for an unbounded wait. Pending state is printed
on each poll; expiry leaves the PR open. Informational checks remain excluded
unless required by branch protection.

| Exit | Outcome |
| --- | --- |
| 0 | Merged or already merged (dry-run only previews) |
| 1 | Other command/forge error |
| 20 | CI red |
| 21 | Ownership or CI wait timed out |
| 22 | Needs rebase / forge reports not mergeable |
| 23 | Needs review, approval at head, or verdict repair |
| 24 | Stale CI definition |
| 25 | Merge-hold present |

Draft-only, supervised, and drive-owned review handoffs return 23, leaving
the PR open. Scripts can branch on these codes instead of parsing messages.
<!-- trace:TASK-1606 | ai:codex -->

**Completion credit.** Ship uses an explicit trailing `(REQ-ID …)` title group,
then branch-name recovery; each ID must resolve unambiguously in the store.
Mid-title references and PR-body prose do not become completing trailers.
An older branch-head trailer is ignored when the PR has a title. For partial
work, use a descriptive title and neutral branch without completion IDs; ship
preserves the title without adding a trailer. Existing constituent commit
trailers in squash bodies remain landing evidence for the auto-bump scanner.
<!-- trace:TASK-1600 | ai:codex -->

**Rebase merge mode (`--rebase` / `--merge-method rebase`).** `aida pr ship [N]
--rebase` runs the identical preflight, CI, independent review/approval,
stale-head, and merge-hold gates as squash, pins the merge with
`--match-head-commit <HEAD_SHA>`, and invokes `gh pr merge --rebase`. When
`scripts/split-lib/verify_stack.py` is present and the PR range contains
`refactor(cli-lib): extract|extend <m>.rs` slice commits, ship runs
`scripts/split-lib/verify_stack.py <base> <head>` before merging. Because rebase
merges land slice stacks incrementally without a squash subject, `--rebase`
leaves the owning spec's status unchanged by default unless `--final-slice`
(`--complete-spec`) is passed to complete the spec on the final slice.
<!-- trace:STORY-1663 | ai:antigravity -->

**Merge train (`aida pr train <N...>` / `--all-ready`).** Lands several
approved PRs together so that two PRs that are each green cannot break main
together. Every member passes ship's gates (approval at head or carried over a
clean merge from main, no merge hold, clear verdicts, open, on the default
branch). Drafts are refused with a reason, since CI never runs Build on them.
The train is main plus one squash commit per member, with the subject ship
would write. Each commit's patch-id must equal the member's approved diff, and
a member that conflicts or whose diff changes when stacked is dropped with its
reason. The train opens as a `train/*` PR, and CI and merge-hold-gate (train
mode: every listed member's labels) run on it. When it is green and main has
not moved, it is rebase-merged with `--match-head-commit <tip>` under the
merge lease. Each member PR is then closed with a pointer to its commit, and
`aida pull` completes specs from the trailers. A red train is rerun once, then
bisected in halves (two sub-trains at a time). A one-member red train leaves
that PR open with a comment. `--dry-run` prints the gate verdicts and the
plan; `--dry-run --local-ci <CMD>` simulates the whole run with a local CI
command; `--max N` caps the batch (default 10), and naming more PRs than
`--max` is refused. The command exits non-zero when a named PR was not
landed. The train is GitHub-only for now: GitLab's merge-hold gate has no
train mode, so `aida pr train` refuses a GitLab project; ship its merge
requests one at a time with `aida pr ship N`.
<!-- trace:STORY-1690 | ai:claude -->

**Chains with** — `aida queue done` (finish on a branch) → `aida pr` (open/ship the PR) → `aida review` (if reviewed) → merge → `aida pull` (auto-bump to Completed).

---

### `aida review`

**One line** — drive human review of a spec against its acceptance criteria.

**Mental model.** `aida review <SPEC>` is the **human-review counterpart to `aida queue work`**. It finds the spec's *review surface* (an open draft PR, else the branch + commits, else "built locally, never pushed"), runs a reviewer over the **diff** against the spec's `## Acceptance` criteria, shows you the verdict, and lets you decide: approve, request changes, open the diff, or defer. **It never auto-merges** — the decision stays yours.

**Reach for it when** — a spec is Done and you (a human, or the reviewer seat) want to actually look at the code before it merges. This is the review *gate* of the lifecycle.

**Don't reach for it when** — you only want the diff pointer without the agent analysis (`--no-agent` just locates the surface and the recommended next command); or the orchestrator is already driving review for this spec under `aida queue work --auto-complete` (don't double-drive it — check `aida session leases` first).

**Key options (rationale only).**
- `--no-agent` — skip the reviewer-agent analysis, just report *where* the review surface is and the next command. For non-interactive contexts or when you only want the diff pointer.
- `--target-branch <branch>` — the branch a newly offered merge or pull request should target. Before offering one, AIDA checks that this branch exists on `origin` and differs from the source branch. An explicit value always overrides any saved recovery state.
- the `prompt` / `assemble` subcommands — generate a markdown review prompt from linked specs' acceptance criteria (from an explicit `--specs` CSV or parsed `(REQ-ID)` trailers in a PR's commit range). The building blocks when you want to review *outside* the interactive flow.
- `mode mass-change on|off|status` — the switch for the temporary mass-change review mode, in which reviewers defer acceptance gaps as tracked findings instead of blocking on them. `on` writes `[review] mass_change_mode` to the main checkout's `.aida/config.toml` together with the start time, who turned it on, and the expiry: one week by default, or `--days N` (1-30). Running `on` while the mode is active is refused, so the window cannot be extended without turning it off first. When the window closes the mode turns itself off, and nobody has to run anything. While the mode is on, bare `aida status` prints one line such as `mass-change mode: ON, day 3 of 7 (expires …), 12 deferred findings open`. After it expires, `aida status` says it expired. When it is off, `aida status` prints nothing about it. `off` ends the mode early and reports how many deferred findings are still open. Nothing already merged is blocked again. Deferred findings are open specs tagged `review:deferred-finding`. `--json` prints the state for scripts.

- the `record` subcommand's finding class — each recorded finding may carry a defect class from a small fixed vocabulary (incomplete-fix, fail-open, absent-evidence-reads-as-good, untested-path, contract-drift, race, perf, portability, stale-base, scope-creep), paired with the findings by position. An unknown class warns and records the finding unclassified; it never blocks the verdict.
- the `classes` subcommand — counts recorded findings per class across every verdict, archived rounds included, optionally limited to a recent window. It answers "how often does this defect class recur?" without a keyword sweep that cannot tell an approval from a refusal. Findings recorded before classes existed stay unclassified; nothing is back-filled.

**Gotchas.** A reviewer reads **code**, an advisor reads **commit messages** — when their verdicts conflict on whether to merge, trust the reviewer. The whole reason `review` runs the agent over the *diff* is that the diff is ground truth.

**Chains with** — the verdict either passes the spec toward merge, or sends it to `aida rework`.

---

### `aida rework`

**One line** — the single verb for the implementer → reviewer → fixup recovery loop.

**Mental model.** When review says "changes needed," the spec has to go *backward* — out of Done, back to active work, re-queued, with the reason captured. `aida rework <ID>` does that whole sequence in one verb (a top-level alias for `aida queue rework`): it flips the status to the smart target, re-queues to the routing role, and optionally relaunches the session.

**Reach for it when** — a reviewer (or you) requested changes on a Done spec and it needs another implementer pass. Also for any "this shipped wrong, reopen it" recovery. It is also the one-keystroke **requeue** for a spec parked in Needs Attention (a punt, a drain shelve, or an advisor escalation): once you have triaged it, `aida rework <ID>` moves it to Approved, clears the parking markers the drain left, records on the spec why it came back, and puts it back on the queue, so the next drain picks it up. `aida findings list` and `aida awaiting` print this command next to each parked spec. Run `aida rework` with no ID at a terminal to walk every parked spec: each one shows the status it would land in, its queue, and any dependency it still waits on, then takes one key (`[r]` requeue, `[s]` skip, `[o]` show, `[d]` decide when a decision is open, `[q]` quit). Without a terminal it prints the requeue commands instead, and flags such as `--work` or `--status` need an ID. Returning a parked spec is an advisor decision, so it needs the advisor role or an interactive terminal, the same as `aida edit <ID> --status approved`. An advisor's `needs-human` escalation tag is cleared only when a human runs the command at an interactive terminal; an agent or drain phase gets the status change but the spec stays parked, with a warning saying so. A spec with an unanswered decision stays parked until that decision is answered (`aida questions`).

**Don't reach for it when** — the spec is genuinely fine and you just want a comment (use `aida comment add`). And mind the terminal-status guard: rework refuses on already-terminal specs unless you pass `--force` — that guard is there to stop you accidentally reopening Completed work.

**Key options (rationale only).**
- `--reason` — capture *why* it's being reworked as a comment at rework time. Do this; future-you will want the audit trail.
- `--work` (+ `--resume`, `--steal`, `--permission-mode`) — also launch (or resume) a session immediately, so rework-and-pickup is one step. The pass-through flags only matter with `--work`.
- `--force` — bypass the terminal-status / already-in-progress guards. The escape hatch for "yes, I really am reopening this." It does not override another session's live claim on the spec: end that session first (`aida session end <id>`).

**Gotchas.** `--no-pull` and the session flags are no-ops without `--work` — they only apply to the launched session.

**Chains with** — `aida review` (request changes) → `aida rework` → back to the implementer → `aida queue done` again.

---

## The two-leg git model: `fetch` / `pull` / `push`

Here's the thing raw git doesn't know about your AIDA project: **there are two branches that must move together** — your **code** branch, and the orphan **`aida-store`** branch that holds the specs. `aida fetch`/`pull`/`push` are the verbs that keep both legs in sync, because doing one and forgetting the other is the mistake everyone makes. Each takes `--code-only` / `--store-only` to scope to one leg when you mean to.

### `aida fetch`

**One line** — refresh both remotes' refs without merging or touching your working tree.

**Mental model.** The *read-only* leg-aware counterpart to `pull`. It updates what `origin/<branch>` points at — for both code and store — so downstream checks (the statusline "behind by N," queue prechecks, rebase dry-runs) see current reality, without the cost of two `git fetch`es or the surprise of an implicit merge.

**Reach for it when** — you want to *know* whether you're behind before deciding to pull/rebase; or a background caller (statusline, hook) needs fresh refs cheaply (`--quiet`).

**Don't reach for it when** — you actually want the changes in your tree (that's `pull`). `fetch` never merges.

**Chains with** — `fetch` → inspect → `pull` or `rebase`.

### `aida pull`

**One line** — bring both legs down from origin (code via fast-forward, store via rebase).

**Mental model.** Symmetric to `push`. The code leg is `git pull --ff-only` *by design* — it refuses to surprise your working tree with an auto-rebase; on divergence it hands you the explicit rebase command rather than guessing. The store leg uses rebase (store conflicts are rare and the worktree is AIDA-managed). **`pull` is also where Done→Completed auto-bump happens** — after the store pulls, it promotes any spec whose referencing commit just landed on main.

Store pulls (`aida pull` and `aida db sync --pull`) fetch only the named branch
into a temporary ref, resolve its commit, remove the ref, and rebase onto that
SHA. Concurrent code fetches and overlapping fetch refspecs cannot change the
chosen rebase target through shared `FETCH_HEAD`. Structural conflict merging
and plain-pull abort recovery still apply. A failure reports the Git error;
absence of an active rebase does not imply a transient network problem. This
target isolation does not serialize concurrent store commits or rebases.
<!-- trace:TASK-1604 | ai:codex -->

**Reach for it when** — starting work, syncing after others merged, or right after a merge to trigger the auto-bump. The `--dry-run` (and `--json`) variant shows what *would* come down — the safe "what changed upstream?" check.

**Don't reach for it when** — the code leg refuses with "diverged" — that's not a `pull` failure, it's `pull` correctly refusing to auto-rebase. Follow the printed hint (`git pull --rebase` after inspecting) or use `aida rebase`. The `--auto` flag handles *stacked-branch* re-basing specifically; it deliberately refuses anything the classifier flags `diverged-risky`.

**Key options (rationale only).**
- `--dry-run` / `--json` — fetch both legs and show what each would pull, without merging. The pre-flight.
- `--no-gate` — skip the post-pull merge-gate (which promotes pending node-aware IDs to their agreed short form). It's idempotent and cheap, so skip it only in tight loops; `AIDA_AUTO_MERGE_GATE=false` is the project-wide opt-out.
- `--auto` — auto-rebase tracked *stacked* branches whose base just merged. Narrow, powerful, and self-limiting (refuses risky cases into `/aida-rebase`).

**Gotchas.** If the auto-bump "misses" (the YAML was unreadable at pull time, or the spec flipped to Done *after* its commit already landed), recover with `aida db reconcile-status` — a manual replay of the same scan over a wider window. You don't hand-set Completed; you re-run the bump.

**Deliberate reopen.** A status edit or CLI/MCP rework out of Done/Completed
records the code-repository HEAD. Both pull-time auto-bump and manual replay
ignore completion evidence at or before that reopen point, including closure
holds and review propagation. A genuinely later commit can complete the spec.
The marker is best-effort when the code repository cannot be read.
<!-- trace:TASK-1600 | ai:codex -->

### `aida push`

**One line** — push both legs to origin (the two operations you routinely forget to do together).

**Mental model.** Symmetric to `pull`: `git push` on your branch *plus* `aida db sync --push` for the store. The entire reason it exists is that pushing code but forgetting the store (or vice versa) leaves a collaborator with code that references specs they can't see.

**Reach for it when** — you've committed work and/or filed/edited specs and want both visible to others. `-m/--message` commits pending store changes in the same breath.

**Don't reach for it when** — you're in CI/scripted context where the interactive pre-push checks ("branch behind main," "PR already merged") would hang on stdin — pass `--no-rebase-check`. And `--dry-run` first whenever you're unsure what's pending on either leg.

**Gotchas.** `AIDA_PUSH_DEFAULT=code|store` flips the default scope if your workflow leans one way. **One hard-won caution** (a real near-miss): in a *shared working tree* where sibling agents have branches checked out, your local branch's upstream can get silently repointed — a "Everything up-to-date" after a real commit is the red flag. Verify `git rev-parse --abbrev-ref @{u}` before pushing, and prefer your own worktree.

### `aida rebase`

**One line** — detect, classify, and (optionally) execute a rebase of the current branch onto its upstream.

**Mental model.** Four phases: **detect** (ahead/behind + file-path overlap), **classify** (clean / ahead-only / behind-only / diverged-safe / diverged-risky), **execute** (auto for safe cases, prompt for risky), **report**. It's stateless and safe to invoke anywhere — its first job is to *tell you what kind of divergence you have* before doing anything.

**Reach for it when** — `aida pull` refused with "diverged," or you just want to know your rebase situation (`--dry-run` classifies and exits without touching anything).

**Don't reach for it when** — you have a dirty tree you don't want auto-stashed (`--no-stash` makes it refuse instead) — decide consciously. And risky (file-overlap) cases will still prompt; don't `--auto` your way through a conflict-likely rebase blind.

**Key options (rationale only).**
- `--dry-run` — classify only. The "what would happen" that should precede any real rebase. Run from inside a linked worktree (`git worktree add`), it classifies that worktree's branch and working tree, not the primary checkout's. <!-- trace:TASK-1321 | ai:claude -->
- `--auto` — execute the *safe* classes (behind-only, diverged-safe) without prompting; risky still prompts. The right default for "just catch me up if it's clean."
- `--no-fetch` — classify against the already-cached upstream (when you just fetched and don't want to again).

---

### `aida remote`

**One line** — wire up a git `origin` for a project that has none (guided bootstrap).

**Mental model.** AIDA's store and sync verbs need a remote. `aida remote create` walks you through getting one — GitHub (via `gh`), a remembered personal GitLab (push-to-create over SSH, no token needed), another GitLab host, or attach-an-existing-URL — and `aida remote attach <url>` wires a repo you made elsewhere.

**Reach for it when** — `aida push` said "no origin — skipping," or you're setting up a fresh project and want the remote wired without leaving the CLI.

**Don't reach for it when** — you're on a corporate GitLab where push-to-create is disabled: don't fight it — create the repo in the UI, then `aida remote attach <url>` (the clean fallback the menu offers).

**Gotchas.** Non-interactive (no TTY, no route flag) it prints the manual recipe and exits cleanly rather than hanging — so it's CI-safe. Pre-select a route (`--github` / `--gitlab <host>` / `--attach <url>`) to stay scriptable.

**Mirror hubs.** A project can keep a second hub (for example a GitLab mirror) level with `origin`. `aida remote mirror-sync` pushes origin's default branch and spec store to every mirror listed in `[store.sync] mirror_remotes`, one line per hub and branch (`--json` for scripts). It fetches and pushes origin's observed SHA, ignoring and reporting local-only commits even if a protected origin has rejected them. The pre-push hook only mirrors a proposed ref when origin already advertises that exact SHA; pending or rejected updates are skipped. A later mirror-sync catches up the default branch and store; `aida push` fans out code branches after origin accepts them. Raw `git push` of feature branches or tags requires retrying the mirror hook plumbing once origin confirms the ref. <!-- trace:BUG-1803 | ai:codex --> It only fast-forwards: a hub that has diverged is reported, left untouched, and the command exits non-zero. `aida pull` runs the same sync quietly after a successful pull, because a merge done on the forge never fires the local pre-push mirror hook. The hook itself is generated once, at `aida remote mirror` time: `aida doctor` (category `mirror-hook-drift`) compares the installed hook against the current script and reports when a re-run of `aida remote mirror <name>` is needed to pick up hook fixes — it recognizes only AIDA's own hook and never flags a custom pre-push hook. <!-- trace:BUG-1738 | ai:claude --> `aida remote mirror <name>` lists the hub in `.aida/config.toml` and then settles that change: run non-interactively it commits only that file (`[AI:aida] chore(remote): list <name> as a store mirror`); on a terminal it asks first. If you decline, or the file already had uncommitted edits, nothing is committed and it prints `config changed: .aida/config.toml — commit it with: …`. It ends with next steps: run `aida remote mirror-sync` to bring the new hub level now, since the first push skips a mirror until origin confirms the ref, then `aida remote status` to confirm every hub is in sync. `aida init` handles its end-of-run config refresh (forge provider, project capabilities) the same way: committed when you accepted the scaffold commit, otherwise announced. <!-- trace:BUG-1938 | ai:claude -->

**Chains with** — typically right after `aida init` on a project with no remote; then `aida push` works.

---

### `aida identity`

**One line** — the project's email allowlist: refuse commits and pushes that would publish a non-allowlisted author, committer, or Co-authored-by email. <!-- trace:TASK-1330 | ai:claude -->

**Mental model.** A work machine whose git config carries an employer address will happily sign every commit with it, and nothing in stock git objects. Declare the addresses this project may publish under `[identity] allowed_emails` in `.aida/config.toml`; from then on `aida identity check` reports the identity a commit made here would carry, `aida commit` refuses to assemble a commit under any other address, and the pre-push hook installed by `aida identity install-hook` refuses a push that introduces *any* commit — author, committer, or `Co-authored-by:` trailer — outside the allowlist. The push gate is fail-closed: if the gate cannot run (unreadable config, missing subcommand) while an allowlist is configured, the push is refused rather than waved through. `aida pr ship` closes the last gap: when a squash merge's body would carry a non-allowlisted co-author trailer, the body is replaced with the branch's commit messages with the offending trailers stripped. The `link` / `list` / `show` subcommands are a different concern sharing the namespace: the shared person-alias registry that collapses one human's several identity strings into one canonical person.

**Reach for it when** — a machine's git config might carry an identity this repository must never publish (work laptop, shared box), or right after an identity leak while you rewrite history — this is the recurrence guard.

**Don't reach for it when** — no allowlist is configured: every check is silent, so there is nothing to bypass or tune. The gate is opt-in per project.

**Gotchas.** The installer never clobbers a custom pre-push hook — it prints the lines to paste instead. Repos running the mirror fan-out hook get the gate embedded in that same hook (git only runs one pre-push hook); re-running `aida identity install-hook` or `aida remote mirror <name>` refreshes an older gate-less install in place. `aida doctor` (category `identity`) reports a git identity outside the allowlist and a pre-push hook that does not run the gate. A deliberate bypass is `git push --no-verify` — loud, explicit, and on you. Pushes from the `.aida-store` worktree (the spec store) are gated by the owning project's allowlist — the store's own `.aida/config.toml` is never consulted. A store checkout whose owning project cannot be resolved (for example a standalone clone of the `aida-store` branch outside `<project>/.aida-store`) is refused; attach the store as a linked worktree at `<project>/.aida-store` (as `aida init` does) or bypass deliberately with `git push --no-verify`. A linked code worktree without its own `.aida/config.toml` uses the main worktree's allowlist. <!-- trace:BUG-1926 | ai:claude -->

**Chains with** — `aida commit` (commit-time enforcement point), `aida remote mirror` (shared pre-push hook), `aida doctor` (drift reporting), `aida pr ship` (squash-body sanitizer).

---

### `aida changelog`

**One line** — generate `CHANGELOG.md` mechanically from git tags + the spec graph.

**Mental model.** The changelog is *derived*, not hand-written: `changelog` walks `v*` tags as release boundaries, scans the commits between them for `(SPEC-ID)` trailers, classifies each referenced spec (Features / Fixes / Documentation / Infrastructure / Internal / Other), and renders one structured section per release. Same git state → byte-identical output, so it's safe to regenerate any time.

**Reach for it when** — cutting a release (it's part of the release flow), or any time you want the changelog to reflect what actually merged. `refresh` writes the file (idempotent); `generate` prints to stdout/`--out`; `preview` shows only the `[Unreleased]` section.

**Don't reach for it when** — you want to *hand-edit* prose into the changelog — it's mechanically regenerated, so manual edits get overwritten. If a release needs narrative, that belongs in release notes, not `CHANGELOG.md`.

**Gotchas.** The classification depends on the `(SPEC-ID)` trailer convention — commits without a trailer land in "Other." This is one more reason the commit-trailer discipline matters.

**Chains with** — driven by `aida release` (which regenerates it as part of the version bump); reads the same `(SPEC-ID)` trailers that earn Done→Completed.

---

## Where to go next

You now own the back half of the lifecycle and the two-leg sync model. Next:
- **[Chapter 3 — Work & autonomy](03-work-autonomy.md)**: the front half — queue, pickup, and letting agents drain work (where Done comes *from*).
- **[Chapter 8 — Reporting](08-reporting.md)**: `history` / `metrics` to *see* the lifecycle you just drove.
- The full state machine, with every edge and edge-case, is [`docs/lifecycle.md`](../lifecycle.md).


### `aida merge-hold auto-release`

Release an eligible Rework-only hold from a fresh recorded approval at the
current PR head. Omit the PR number to inspect every local hold, or use
`aida merge-hold auto-release <pr>` for one. Any seat may retry this command;
it has no override flag and never merges. Refused candidates remain held and
produce a nonzero exit with a reason. The review-record handler runs the same
release after a verified APPROVED write. A hold composed with any non-Rework
kind keeps the human clear, and so does a sensitive spec unless a mandate
started with `--include-supervised` covers it. See [clearance policy](08-reporting.md#merge-hold-gate-and-clearance-limits)
for grant, arming-round, metadata and audit requirements.

- **Composed Rework holds.** When refusals on several specs compose into one
  hold, each refusal must be answered by its own fresh approval at the PR
  head. One unanswered or unreadable refusal keeps the whole hold, and the
  reason names it. The clearance records the primary approval and lists the
  others under `also_released_by`.
- **Busy lock.** The release's second check and the marker removal run under
  a per-PR hold lock, which hold placements take too. If another placement or
  release holds it for 10 seconds, the command refuses with
  `merge-hold PR-N is busy` and changes nothing; re-run it.
- **Withdrawn clearances.** If the second check refuses after the clearance
  was written (for example, the head moved), the clearance is moved aside to
  `.aida/merge-hold-clearances/PR-N.superseded-<timestamp>.json` for audit, an
  "authorization withdrawn" comment is posted, and the hold stays armed. A
  superseded file is never read as a clearance. When two releases of one PR
  overlap, the one that finishes second leaves the first one's clearance in
  place.
- **Order.** Auto-release records the clearance, posts the audit comment,
  drops the forge labels, and removes the marker last, so a forge failure
  leaves the hold armed for the next retry. The human `aida merge-hold clear`
  records the clearance, removes the marker, then drops the labels; the
  operator sees any label failure and re-runs it.
<!-- trace:STORY-1687 | ai:codex -->
<!-- trace:TASK-1951 trace:TASK-2286 | ai:claude -->

### `aida mandate`

**One line** — a time-boxed fire-fighting mandate that lets the advisor seat
ship held PRs while the operator is away.

**Mental model.** Normally only a human at a terminal can release a merge-hold.
`aida mandate start --ship-held` (human at an interactive terminal only) mints a
mandate bound to the advisor seat of the person who started it. While it is
active, `aida pr ship <pr>` from that advisor seat may release the hold and
merge, but only when an approval with a verified reviewer identity covers the
PR's current head, that approval came from neither the implementer nor the
shipping session, the advisor did not author the work, and CI is green.
The stale-approval override of `aida pr ship` is never honoured under a
mandate.

**Options.** `--ttl` sets the duration (`8h` by default, `90m` style also
accepted, at most `24h`). A mandate cannot be extended: stop it and start
another. `--reason` records why. `--include-supervised` also covers sensitive
specs (security, containment, review or merge authority, guided, keystone).
Specs tagged `operator-only` or with execution mode `operator` or `decide`
always stay human-only. Exclusion is sticky: a spec that was ever in an
excluded class, in any stored version or in the local exclusion ledger
(`~/.aida/mandates/exclusions.jsonl`, append-only), stays excluded after the
tag is removed. A mandate releases only a typed hold marker; a missing marker,
a label-only hold or a recusal hold always needs a human.

Mandate exclusions also consider committed trace IDs in changed source hunks and
their enclosing Rust functions/impls, plus the file’s leading header traces
(including doc comments), on both base and head. Rust module comments outside
fn/impl items and their leading prefixes also add exclusions, including traces
between items and at EOF. Inline-module headers apply even when attached to an
unchanged first function. Other unchanged fn/impl prefixes remain scoped.
Deletions and renames
retain the old traces. When a hunk cannot be attributed safely, the check uses
the full file; other text formats (including docs, configuration and hooks) use
that conservative fallback. Every changed text path is scanned, including nested
build directories. A stale local `origin/<base>` can include older changes and
therefore exclude more specs; fetch the current base before retrying.
These IDs only add exclusions, with the same sticky history/ledger rules. They
grant no completion credit or author/reviewer authority. Unreadable evidence or
a changed/dirty store refuses mandate coverage; fetch missing source commits or
finish the store write before retrying. See OVERVIEW.md for source scope and
fallback limits.
<!-- trace:TASK-1790 | ai:codex -->

**Stopping.** `aida mandate stop` (human at a terminal) ends it at once. Expiry
is checked by every ship at the moment it decides, again under the merge lease,
and once more immediately before the merge call. `aida mandate status [--json]`
shows the current mandate; the statusline shows `MANDATE <time left>` while one
is active.

**Audit.** Each release is recorded in the merge-hold clearance ledger and in a
PR comment as "released under mandate <id> by advisor"; start and stop are
recorded in `.aida/events.jsonl`. The authority record lives only in the
user's AIDA home (`~/.aida/mandates/`, mode 0600), never in the shared store.
Each record is re-validated on every read (24h cap, no future issue time,
advisor seat, current user, a 0600 regular file you own, never a symlink); an
invalid record is ignored with a warning. Stopping writes a `<id>.stopped`
tombstone that is never removed, so editing a record cannot revive it.

**Trust boundary.** Start and stop require a human at an interactive
terminal. A same-uid process that forges files in the AIDA home is outside the
threat model, the same as for session grants: the mandate keeps work moving
while the operator is away, and is not a defense against a malicious agent
running as the same user. Accepted risk: a process that wraps itself in a
pseudo-terminal passes the terminal check, as it does for `aida role enter`.
<!-- trace:STORY-1698 | ai:claude -->

#### Drive mandates

**One line** — `aida mandate start --drive` makes the system drive the open
list while you are away: the advisor seat may ship held PRs, sign off sketches
and dispatch fixers, and a 15-minute tick routes every stalled lane.

**What it covers.** `--drive` implies `--ship-held` and covers sensitive specs
unless you pass `--exclude-supervised`. Everything above still applies: the
same human-only start and stop, the 24h cap, no extension, sticky exclusion,
and specs tagged `operator-only` (or in `operator` / `decide` mode) stay
human-only.

**The tick.** The `mandate-drive` schedule job runs `aida fleet drive --tick`
every 15 minutes (with no drive mandate it does nothing). The tick runs no
model and holds no seat grant. It ignores a grant, role or purpose in its
environment and removes them from anything it starts. For each relay run and
each held PR:

- **Substrate actions it does itself:** expiring leases of dead sessions,
  notifying you, and ledgering every decision.
- **Seat actions it routes to the advisor:** shipping an approved held PR,
  signing off the sketch a blocked relay waits on, answering a blocked relay's
  question, re-dispatching a crashed, capped or slot-starved run, and
  dispatching a fixer for an approved spec nobody owns (no live claim, no
  branch, no open PR). Each one arrives once as a due item in
  `aida awaiting` and an advisor mail carrying the exact command. A blocked
  relay counts as a sketch signoff only when its question names a spec that
  is open with a sketch comment newer than its last `ADVISOR SIGNOFF`;
  otherwise it is a question to answer.
- **What it sends to you:** an advisor-seat run, a run stopped on API credits,
  and anything behind a human gate.

It never ships or clears a hold itself. Before each action it re-reads the
mandate, so a stop or an expiry takes effect mid-tick.

**Crashed relays.** The tick restarts a relay itself only when the relay died
without its exit handler running (killed or crashed, not stopped, capped,
blocked or finished), and only when all of these hold:

- the run's `relay.launch` record shows that you started it at a terminal,
  that it belongs to the mandate's principal, and that its profile is unchanged;
- it is not an advisor-seat or forced-seat run, and no `relay.norestart` marker
  is present (the relay's no-restart option writes one);
- the run's spec is known: from its status, or from the spec named at
  launch (the relay's spec option or the profile's `spec` key), and not an operator-only spec;
- no agent session of the run is still alive (a `kill -9` of the relay alone
  leaves its session running; stop it first);
- it has been restarted fewer than 3 times, after waiting 1, 2 and then 4 ticks;
- sessions are left in its budget and the mandate has at least 30 minutes left;
- the mandate was started with the `relay-restart` scope (`--drive` adds it;
  a drive mandate started before it existed does not cover it).

The restart runs the record's resume arguments (the same run, its existing
handoff, no initial prompt), with its session limit set to the sessions
left. It uses no grant and no session role. Any other crashed run
goes to the advisor as before. After the third restart you get one
crash-loop notice.

**No advisor.** If no advisor seat has been active in the last 30 minutes, you
get one notice per absence. Seat actions wait in `aida awaiting` for the next
advisor; the substrate actions continue.

**Human gates (opt-in).** List the actions that stay human:

```toml
[mandate.human_gates]
ops = ["ship"]   # any of: ship, sketch-signoff, dispatch, hold-clear, relay-restart
```

- The list is empty by default. An unknown op is a configuration error.
- It is read from `origin/main:.aida/config.toml` and from
  `~/.aida/config.toml`, never from the working tree. Each layer can only add
  gates, so a branch cannot remove one.
- If the committed layer cannot be read, every action is gated.
- A PR that changes `.aida/config.toml` or the mandate code
  (`mandate.rs`, `mandate_gates.rs`, `mandate_drive.rs`, `mandate_ship.rs`,
  `mandate_traces.rs`, `relay_launch.rs`, `rework_hold_release.rs`, `review_authority.rs`,
  `solo_review.rs`, `review_verdict.rs`,
  `fleet_lanes.rs`, `seat_authority.rs`, `pr_ship.rs`, `merge_hold.rs`,
  `pr_ship_batch.rs`) never
  ships under a mandate, and a mandate never releases its rework hold either.
  Listing `ship` or `hold-clear` stops mandate rework releases too.
- `[mandate.drive] max_actions_per_tick` (default 10) is honoured from
  `~/.aida/config.toml` only.
- Drive never clears holds; `hold-clear` exists so you can state that
  explicitly.

**Where you see it.**

- `aida mandate status --line` prints
  `MANDATE drive · 5h40m left · by joe · <reason> · gates: none`.
- Every session prints that line at start and on every turn.
- `aida session handoff --show` prints it first, read live, never copied into
  the handoff.
- Each seat sees one line when the mandate expires or is stopped.
- `aida mandate status` lists the action counts and the last 10 actions, and
  `aida digest` has a "Mandate-driven" section.

**Before you leave.** `aida mandate start --drive` and
`aida doctor check mandate-drive` warn in these cases:

- no schedule driver is installed;
- no enabled job runs the tick;
- the driver would run as a different user than the one who started the
  mandate. The fix is `AIDA_USER=<you>` in the driver's environment.

`aida fleet drive` on its own previews what `aida fleet drive --tick` would
route, and changes nothing. The preview applies the same holds as the tick: an
unreadable store, `[mandate.human_gates]`, and the operator-only floor of every
spec a lane touches. `aida fleet drive --json` prints that preview as JSON, and
`aida fleet drive --tick --json` prints the tick report.
<!-- trace:STORY-1700 | ai:claude --> <!-- trace:TASK-1955 | ai:claude -->

### `aida pr ship-batch`

**One line** — ship several PRs that need a human merge, one at a time, in your
order, while you only confirm each ship.

**Mental model.** A PR that changes the mandate or merge-gate code, or carries
an operator-only spec, needs a human to ship it. Each such merge moves main,
so the next one goes stale: it needs a refresh from main, a new CI run and a
new independent review before `aida pr ship` accepts it. `aida pr ship-batch
2792 2798` does that waiting for you. For each PR, in the order given:

1. A merged PR is skipped. A closed, draft or conflicting PR stops the batch.
2. The branch is refreshed from main (GitHub's update-branch, pinned to the
   head it read) only when GitHub reports it behind, or when its green
   predates a CI-definition change on main (the check `aida pr ship` refuses).
3. It waits for CI on that exact head. Checks from an earlier head never count.
4. It queues the PR for review and waits for an independent approval recorded
   at that exact head, printing once a minute that a reviewer has to act. It
   never records a verdict, and an approval from an earlier head does not count.
5. It asks you to type the PR number. If the PR has a merge-hold, it then asks
   you to type `clear`, and clears it with `aida merge-hold clear`.
6. It ships with `aida pr ship`, every gate in force and no override.

**Options.** `--review-timeout` sets how long to wait for each approval
(default `60m`). `--dry-run` reads each PR and reports what the batch would
do, changing nothing. It does not run `git fetch`, so the stale-definition
check reads your local `origin/*` refs as they are; fetch first for a current
answer.

**Record.** The start, each PR's outcome and the stop are written to the
events feed; `aida history events --kind ship-batch-step` shows the run.

**Stopping.** Any failure stops the batch and ships nothing further: red CI,
CI or review timeout, a negative verdict, a head that keeps moving, an
unconfirmed prompt, end of input, or a refused ship gate. Run the same command
again to continue; merged PRs are skipped. The command first prints the order
and waits for `go`; it refuses duplicate PR numbers and more than 20 PRs.

**Who can run it.** Only a human at an interactive terminal, on a GitHub
project. Each batch, PR outcome and stop is logged to
`.aida/advisor-activity.jsonl`.
<!-- trace:STORY-1887 | ai:claude -->
