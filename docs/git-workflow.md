# AIDA's git workflow: base-freshness and divergence

*Last updated: 2026-05-13*

AIDA's tooling distinguishes between two patterns of branch-staleness that look superficially similar but have different solutions. This doc names them, explains the layered defense, and surfaces the recovery recipes when prevention fails.

For the scaffolded user-facing version of the divergence recovery recipe, see `.claude/AIDA.md` in any AIDA-using project (it's the one that propagates via `aida scaffold upgrade`). This doc is the deeper architectural reference — it explains *why* AIDA's tooling is shaped the way it is and *where* in the lifecycle each defense fires.

---

## Two patterns

### Pattern A — Divergent branches

Local `main` has commits `origin/main` doesn't, AND `origin/main` has commits local doesn't. The branches have actually forked. `git pull --ff-only` refuses; bare `git pull` errors with "Need to specify how to reconcile divergent branches."

**How it typically happens:** an agent (or user) commits to a local branch without fetching first, while origin advanced via PR merges, GitHub Actions commits, or work on another machine.

**Recovery:** `git pull --rebase`, or if rebase would conflict, manual inspection.

### Pattern B — Stale base

Local branch has no new commits (or only its own work), but origin has advanced. The branch is *behind*, not *diverged*. `git pull --ff-only` succeeds silently; the working tree just updates.

**Where it bites:** a session-worktree started off `main` at time T1 stays on its own branch while `main` advances. The session's branch isn't broken — but when its PR opens, GitHub shows "N commits behind base." Merge may still work clean if no file overlap, but integration risk grows the longer the session runs.

**Recovery:** rebase onto the new base before opening PR (or merge main into the feature branch — AIDA's convention is rebase).

The patterns aren't mutually exclusive — Pattern A can show up *inside* a session branch if the agent commits to local main while session-side work is in progress on a feature branch. Each pattern wants its own defense.

---

## The layered defense

Every transition in the lifecycle has either tooling or docs handling base-freshness.

### Pattern A coverage

| Lifecycle moment | Defense | TASK | Status |
|---|---|---|---|
| Before any commit | Fetch + warn if branch behind origin | TASK-98 (`/aida-commit` precheck) | Approved |
| Pull-time recovery | Auto-rebase when divergence is safe (no file overlap) | TASK-97 (`aida pull --autorebase`) | Approved |
| Manual fallback recipe | Documented in scaffolded `.claude/AIDA.md` ("When `aida pull` refuses") | — | Shipped (commit `699fc5dd`) |
| User-machine config | Optional: `git config --global pull.rebase=true` + `rebase.autoStash=true` + `advice.diverging=false` | — | User decision |

### Pattern B coverage

| Lifecycle moment | Defense | TASK | Status |
|---|---|---|---|
| Session start | `aida queue work` pulls code branch (not just orphan store) before creating worktree | TASK-99 | Approved |
| During session | `aida statusline` + `aida queue list` surface "base behind by N" for active leases on stale branches | TASK-101 | Approved |
| Session end (queue done) | `aida queue done` detects stale base, offers rebase before allowing done | TASK-100 | Approved |
| Push time | `aida push` detects branch-behind-main, prompts rebase | TASK-54 | **Completed** |

### Discipline (until Layer 3 lands)

The agent-side recipe, captured as feedback memory `feedback_fetch_before_commit.md`:

```bash
# Before committing during a session that's been open more than a few minutes:
git fetch origin "$(git rev-parse --abbrev-ref HEAD)"
git rev-list --left-right --count HEAD...@{u}
# "0 N"  → behind by N: pull --rebase before committing
# "M 0"  → ahead by M: safe to commit
# "M N"  → diverged: rebase before commit if M is mine + cheap to replay
```

---

## Why the asymmetry in `aida pull`?

`aida pull` is two operations in one. The two legs deliberately use different strategies:

| Leg | Strategy | Why |
|---|---|---|
| **Code** | `git pull --ff-only` | Refuses to surprise the working tree with an auto-rebase. If the user has local commits, they want to see them and decide. Implementation: `aida-cli/src/main.rs:20120`. |
| **Store** (orphan `aida-store` branch) | `git pull --rebase` | Store conflicts are rare (the format-policy keeps writes to disjoint files most of the time) and the worktree is AIDA-managed, not user-edited. Auto-rebase is safe. Implementation: `aida-cli/src/main.rs:19678`. |

When `aida pull`'s code leg refuses, it prints `git pull --rebase origin <branch>` as a hint. The expectation is the user sees the hint and chooses — not that the tool decides for them.

TASK-97 will add `--autorebase` as an opt-in: when set, AIDA inspects the divergence (file-path overlap, ahead/behind counts) and auto-rebases when local-ahead commits are safe to replay. Until then, the user inspects and decides.

---

## Recovery recipe (full)

When `aida pull`'s code leg refuses, OR `git pull` complains about divergent branches:

```bash
# 1. Make sure we have origin's view
git fetch origin "$(git rev-parse --abbrev-ref HEAD)"

# 2. Inspect what each side has
git log --oneline @{u}..HEAD     # what you have that origin doesn't (local ahead)
git log --oneline HEAD..@{u}     # what origin has that you don't (remote ahead)

# 3. Inspect file overlap
git log --name-only @{u}..HEAD --pretty= | sort -u   # files your commits touched
git log --name-only HEAD..@{u} --pretty= | sort -u   # files their commits touched

# 4a. No overlap, working tree clean → safe to rebase
git pull --rebase

# 4b. Overlap exists → inspect the overlapping commits, rebase carefully
git rebase origin/<branch>
# resolve conflicts file-by-file: git add <file>; git rebase --continue
# or abort: git rebase --abort

# 4c. Working tree dirty → stash first, pop after
git stash push -u
git pull --rebase
git stash pop
```

### Optional machine-global config

These three settings make raw `git pull` Just Work without per-incident decisions:

```bash
git config --global pull.rebase true        # rebase instead of merge on pull
git config --global rebase.autoStash true   # preserve uncommitted changes across rebase
git config --global advice.diverging false  # silence the "Need to specify how to reconcile" hint
```

Trade-off: silent auto-rebase for fewer manual decisions. `autoStash` preserves uncommitted changes. If you'd rather see the prompt each time, leave these unset and the recipe above is your fallback.

---

## Origin story

This layered defense came from a single concrete incident on 2026-05-13:

1. Agent (Claude session) committed `docs/plans/2026-05-13-story-86-done-status.md` to local `main` without fetching first.
2. Meanwhile PR #19 (`EPIC-23 batch 2`) merged to `origin/main`.
3. User's next `git pull` failed with the divergent-branches error.
4. Manual `git pull --rebase` resolved it cleanly (no file overlap — the new plan file couldn't conflict with anything in PR #19).

The incident itself was trivial to fix. The lessons are layered:

- **Pattern A revealed**: `aida pull`'s code-side `--ff-only` is correct by design (don't surprise the worktree) but its `error → manual recovery` path is friction that compounds across many sessions.
- **Pattern B revealed**: `aida queue work` only pulls the orphan store; the code branch is whatever local `main` was. If local `main` is stale, every fresh session inherits that staleness.
- **Discipline gap**: agents committing on long-running sessions should fetch first. Captured as feedback memory.
- **Documentation gap**: the recipe for recovery should ship in scaffolded `.claude/AIDA.md` so every AIDA-using project gets it, not just AIDA-the-dev-repo.

### Followup incident — 2026-05-13 STORY-86 recovery

Later the same day, the same multi-worktree workflow surfaced a *much more serious* problem: the aida binary on `main` (which lacked the `Done` `RequirementStatus` variant that PR-21 introduces) silently **deleted 6 YAML files** when it ran `aida add` because it couldn't parse them. STORY-86 was among the deleted. Recovery required:

1. Restoring the YAML files from the parent of the destructive commit (`git checkout ab1580ee^ -- objects/...`)
2. Rebuilding aida from a branch that *did* have the `Done` variant
3. Realizing that `aida dev activate`'s "most-recently-built" heuristic was picking the wrong binary (the older `target/debug/aida` built from main, not the just-built release from epic-21-2)
4. Rebuilding debug from the correct branch and re-running `aida cache rebuild`

Filed as **BUG-96** (critical, data-loss; persist-path deletes parse-failing files) and **TASK-221** (`aida dev activate` should match binary's embedded SHA to current branch HEAD). The bigger lesson is that AIDA's multi-worktree / multi-binary-version model is unsafe today: any write operation from an older binary against a store containing newer-format YAML risks silent deletion. The skip-and-warn machinery exists for reads; the persist path must adopt the same posture before AIDA can recommend multi-worktree development unreservedly.

Until BUG-96 ships, the discipline is: **only run aida writes (add / edit / comment / queue mutation / db sync) from a binary whose source has all the enum variants the orphan store has been written with.** `aida --version`'s embedded SHA + a quick check against the writing-worktree's branch is the manual workaround.

Each gap got its own response — see the lifecycle table above.

---

## See also

- [STORY-50](../) — EPIC-21 v3: agent + release-tag integration (composes; release lifecycle interacts with base-freshness)
- [EPIC-23](../) — Session orchestration & autonomy (parent for TASK-97/98/99/100/101)
- `.claude/AIDA.md` (scaffolded into every AIDA-using project) — the recovery recipe for end users, without AIDA-internal cross-references
- `feedback_fetch_before_commit.md` (agent memory) — the agent-side discipline counterpart
