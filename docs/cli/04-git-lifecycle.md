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
- `aida pr ship` — you have **human-pre-approved** work that needs *no* orchestrator review phase: docs PRs, master-signed architecture work, recovery merges. It's the direct-publish counterpart to the full reviewed pipeline.
- `aida pr hold` — you want to push the branch but *deliberately not open the PR yet*, pending a manual gate (a smoke test, an out-of-band sign-off).
- `aida pr rebase` — collapse the standard 6-command "rebase a PR before review" recipe into one.
- `aida pr gc` — opt-in sweep of local `pr-N`/`mr-N` review-snapshot branches (created when you or a headless review fetches a change's head ref) whose change has reached a terminal state. Never removes a branch tied to a still-open change, checked out anywhere, or diverged from the fetched head; `--dry-run` previews.

**Don't reach for it when** — the work needs **review**. `aida pr ship` *skips* the reviewer phase by design; for work that should be reviewed, use `aida queue work PR-N --auto-complete` (the full reviewer pipeline) instead. Shipping unreviewed code is the right tool only when a human already approved it.

**Gotchas.** `auto-queue-review` and `ship` detect the PR via `gh pr list --head <branch>`, so `gh` must be on PATH and authenticated. `ship` squash-merges — if you need merge commits preserved, it's the wrong verb.

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

**Mirror hubs.** A project can keep a second hub (for example a GitLab mirror) level with `origin`. `aida remote mirror-sync` pushes origin's default branch and spec store to every mirror listed in `[store.sync] mirror_remotes`, one line per hub and branch (`--json` for scripts). It fetches and pushes origin's observed SHA, ignoring and reporting local-only commits even if a protected origin has rejected them. The pre-push hook only mirrors a proposed ref when origin already advertises that exact SHA; pending or rejected updates are skipped. A later mirror-sync catches up the default branch and store; `aida push` fans out code branches after origin accepts them. Raw `git push` of feature branches or tags requires retrying the mirror hook plumbing once origin confirms the ref. <!-- trace:BUG-1803 | ai:codex --> It only fast-forwards: a hub that has diverged is reported, left untouched, and the command exits non-zero. `aida pull` runs the same sync quietly after a successful pull, because a merge done on the forge never fires the local pre-push mirror hook. The hook itself is generated once, at `aida remote mirror` time: `aida doctor` (category `mirror-hook-drift`) compares the installed hook against the current script and reports when a re-run of `aida remote mirror <name>` is needed to pick up hook fixes — it recognizes only AIDA's own hook and never flags a custom pre-push hook. <!-- trace:BUG-1738 | ai:claude -->

**Chains with** — typically right after `aida init` on a project with no remote; then `aida push` works.

---

### `aida identity`

**One line** — the project's email allowlist: refuse commits and pushes that would publish a non-allowlisted author, committer, or Co-authored-by email. <!-- trace:TASK-1330 | ai:claude -->

**Mental model.** A work machine whose git config carries an employer address will happily sign every commit with it, and nothing in stock git objects. Declare the addresses this project may publish under `[identity] allowed_emails` in `.aida/config.toml`; from then on `aida identity check` reports the identity a commit made here would carry, `aida commit` refuses to assemble a commit under any other address, and the pre-push hook installed by `aida identity install-hook` refuses a push that introduces *any* commit — author, committer, or `Co-authored-by:` trailer — outside the allowlist. The push gate is fail-closed: if the gate cannot run (unreadable config, missing subcommand) while an allowlist is configured, the push is refused rather than waved through. `aida pr ship` closes the last gap: when a squash merge's body would carry a non-allowlisted co-author trailer, the body is replaced with the branch's commit messages with the offending trailers stripped. The `link` / `list` / `show` subcommands are a different concern sharing the namespace: the shared person-alias registry that collapses one human's several identity strings into one canonical person.

**Reach for it when** — a machine's git config might carry an identity this repository must never publish (work laptop, shared box), or right after an identity leak while you rewrite history — this is the recurrence guard.

**Don't reach for it when** — no allowlist is configured: every check is silent, so there is nothing to bypass or tune. The gate is opt-in per project.

**Gotchas.** The installer never clobbers a custom pre-push hook — it prints the lines to paste instead. Repos running the mirror fan-out hook get the gate embedded in that same hook (git only runs one pre-push hook); re-running `aida identity install-hook` or `aida remote mirror <name>` refreshes an older gate-less install in place. `aida doctor` (category `identity`) reports a git identity outside the allowlist and a pre-push hook that does not run the gate. A deliberate bypass is `git push --no-verify` — loud, explicit, and on you.

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
