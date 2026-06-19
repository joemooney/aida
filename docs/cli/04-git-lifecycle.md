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

**Gotchas.** `--dry-run` prints the assembled message without committing — use it to preview. Without `-a/--all`, something must be staged. The message is self-checked before the commit fires, so what you see is what the hook will accept.

**Chains with** — `git add` (stage) → `aida commit` → `aida pull` (after merge, auto-bump to Completed).

---

### `aida done`

*(Covered in [Chapter 1](01-getting-started.md#aida-done).)* The newcomer shortcut — "I finished it." Once you're on a real pipeline, **stop using it** and use `aida queue done` (lands **Done**, the precise "finished on a branch" state) so the merge can earn **Completed**. `aida done`'s simplicity is also its limitation: it doesn't know where in the lifecycle you are.

---

### `aida pr`

**One line** — the pull-request side-effects that fire around opening a PR.

**Mental model.** `aida pr` is a small family of *PR lifecycle actions*, not a single command. The headline subcommands: `ship` (the fast path — create-if-needed → watch CI → squash-merge → pull → cleanup, in one call) and `auto-queue-review` (files the reviewer story the moment the PR opens, while context is fresh). Think of it as "the things that should happen at PR boundaries, automated."

**Reach for it when**
- `aida pr ship` — you have **human-pre-approved** work that needs *no* orchestrator review phase: docs PRs, master-signed architecture work, recovery merges. It's the direct-publish counterpart to the full reviewed pipeline.
- `aida pr hold` — you want to push the branch but *deliberately not open the PR yet*, pending a manual gate (a smoke test, an out-of-band sign-off).
- `aida pr rebase` — collapse the standard 6-command "rebase a PR before review" recipe into one.

**Don't reach for it when** — the work needs **review**. `aida pr ship` *skips* the reviewer phase by design; for work that should be reviewed, use `aida queue work PR-N --auto-complete` (the full reviewer pipeline) instead. Shipping unreviewed code is the right tool only when a human already approved it.

**Gotchas.** `auto-queue-review` and `ship` detect the PR via `gh pr list --head <branch>`, so `gh` must be on PATH and authenticated. `ship` squash-merges — if you need merge commits preserved, it's the wrong verb.

**Chains with** — `aida queue done` (finish on a branch) → `aida pr` (open/ship the PR) → `aida review` (if reviewed) → merge → `aida pull` (auto-bump to Completed).

---

### `aida review`

**One line** — drive human review of a spec against its acceptance criteria.

**Mental model.** `aida review <SPEC>` is the **human-review counterpart to `aida queue work`**. It finds the spec's *review surface* (an open draft PR, else the branch + commits, else "built locally, never pushed"), runs a reviewer over the **diff** against the spec's `## Acceptance` criteria, shows you the verdict, and lets you decide: approve, request changes, open the diff, or defer. **It never auto-merges** — the decision stays yours.

**Reach for it when** — a spec is Done and you (a human, or the reviewer seat) want to actually look at the code before it merges. This is the review *gate* of the lifecycle.

**Don't reach for it when** — you only want the diff pointer without the agent analysis (`--no-agent` just locates the surface and the recommended next command); or the orchestrator is already driving review for this spec under `--auto-complete` (don't double-drive it — check `aida session leases` first).

**Key options (rationale only).**
- `--no-agent` — skip the reviewer-agent analysis, just report *where* the review surface is and the next command. For non-interactive contexts or when you only want the diff pointer.
- the `prompt` / `assemble` subcommands — generate a markdown review prompt from linked specs' acceptance criteria (from an explicit `--specs` CSV or parsed `(REQ-ID)` trailers in a PR's commit range). The building blocks when you want to review *outside* the interactive flow.

**Gotchas.** A reviewer reads **code**, an advisor reads **commit messages** — when their verdicts conflict on whether to merge, trust the reviewer. The whole reason `review` runs the agent over the *diff* is that the diff is ground truth.

**Chains with** — the verdict either passes the spec toward merge, or sends it to `aida rework`.

---

### `aida rework`

**One line** — the single verb for the implementer → reviewer → fixup recovery loop.

**Mental model.** When review says "changes needed," the spec has to go *backward* — out of Done, back to active work, re-queued, with the reason captured. `aida rework <ID>` does that whole sequence in one verb (a top-level alias for `aida queue rework`): it flips the status to the smart target, re-queues to the routing role, and optionally relaunches the session.

**Reach for it when** — a reviewer (or you) requested changes on a Done spec and it needs another implementer pass. Also for any "this shipped wrong, reopen it" recovery.

**Don't reach for it when** — the spec is genuinely fine and you just want a comment (use `aida comment add`). And mind the terminal-status guard: rework refuses on already-terminal specs unless you pass `--force` — that guard is there to stop you accidentally reopening Completed work.

**Key options (rationale only).**
- `--reason` — capture *why* it's being reworked as a comment at rework time. Do this; future-you will want the audit trail.
- `--work` (+ `--resume`, `--steal`, `--permission-mode`) — also launch (or resume) a session immediately, so rework-and-pickup is one step. The pass-through flags only matter with `--work`.
- `--force` — bypass the terminal-status / already-in-progress guards. The escape hatch for "yes, I really am reopening this."

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

**Reach for it when** — starting work, syncing after others merged, or right after a merge to trigger the auto-bump. The `--dry-run` (and `--json`) variant shows what *would* come down — the safe "what changed upstream?" check.

**Don't reach for it when** — the code leg refuses with "diverged" — that's not a `pull` failure, it's `pull` correctly refusing to auto-rebase. Follow the printed hint (`git pull --rebase` after inspecting) or use `aida rebase`. The `--auto` flag handles *stacked-branch* re-basing specifically; it deliberately refuses anything the classifier flags `diverged-risky`.

**Key options (rationale only).**
- `--dry-run` / `--json` — fetch both legs and show what each would pull, without merging. The pre-flight.
- `--no-gate` — skip the post-pull merge-gate (which promotes pending node-aware IDs to their agreed short form). It's idempotent and cheap, so skip it only in tight loops; `AIDA_AUTO_MERGE_GATE=false` is the project-wide opt-out.
- `--auto` — auto-rebase tracked *stacked* branches whose base just merged. Narrow, powerful, and self-limiting (refuses risky cases into `/aida-rebase`).

**Gotchas.** If the auto-bump "misses" (the YAML was unreadable at pull time, or the spec flipped to Done *after* its commit already landed), recover with `aida db reconcile-status` — a manual replay of the same scan over a wider window. You don't hand-set Completed; you re-run the bump.

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
- `--dry-run` — classify only. The "what would happen" that should precede any real rebase.
- `--auto` — execute the *safe* classes (behind-only, diverged-safe) without prompting; risky still prompts. The right default for "just catch me up if it's clean."
- `--no-fetch` — classify against the already-cached upstream (when you just fetched and don't want to again).

---

### `aida remote`

**One line** — wire up a git `origin` for a project that has none (guided bootstrap).

**Mental model.** AIDA's store and sync verbs need a remote. `aida remote create` walks you through getting one — GitHub (via `gh`), a remembered personal GitLab (push-to-create over SSH, no token needed), another GitLab host, or attach-an-existing-URL — and `aida remote attach <url>` wires a repo you made elsewhere.

**Reach for it when** — `aida push` said "no origin — skipping," or you're setting up a fresh project and want the remote wired without leaving the CLI.

**Don't reach for it when** — you're on a corporate GitLab where push-to-create is disabled: don't fight it — create the repo in the UI, then `aida remote attach <url>` (the clean fallback the menu offers).

**Gotchas.** Non-interactive (no TTY, no route flag) it prints the manual recipe and exits cleanly rather than hanging — so it's CI-safe. Pre-select a route (`--github` / `--gitlab <host>` / `--attach <url>`) to stay scriptable.

**Chains with** — typically right after `aida init` on a project with no remote; then `aida push` works.

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
