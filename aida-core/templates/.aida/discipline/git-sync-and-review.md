# Git sync & review workflow

Read-on-demand detail moved out of the always-in-context `.claude/AIDA.md` so a
consumer project pays for it only when it's relevant. trace:TASK-636 | ai:claude

## When `aida pull` refuses (divergent branches)

`aida pull` is two operations in one: a `git pull` of your code branch and a
`git pull --rebase` of the orphan `aida-store` branch. The two legs are
deliberately asymmetric:

- **Code leg**: `git pull --ff-only` — refuses if the branch has diverged from
  origin. Won't surprise your working tree with an auto-rebase.
- **Store leg**: `git pull --rebase` — store conflicts are rare and the worktree
  is AIDA-managed.

When the code leg refuses (or raw `git pull` complains about divergent
branches), the recovery recipe:

```bash
git fetch origin "$(git rev-parse --abbrev-ref HEAD)"
git log --oneline @{u}..HEAD     # what you have that origin doesn't
git log --oneline HEAD..@{u}     # what origin has that you don't
git log --name-only @{u}..HEAD --pretty= | sort -u   # files you touched
git log --name-only HEAD..@{u} --pretty= | sort -u   # files they touched
# No overlap → safe: git pull --rebase
# Overlap   → inspect; rebase + resolve, or git rebase --abort
```

To make raw `git pull` Just Work without per-incident decisions (one-time,
machine-global):

```bash
git config --global pull.rebase true
git config --global rebase.autoStash true
git config --global advice.diverging false
```

Trade-off: silent auto-rebase for fewer manual decisions. `autoStash` preserves
uncommitted changes across the rebase. If you'd rather see the prompt each time,
leave these unset and the recipe above is your fallback.

## Review workflow

`aida review prompt --pr N` (or `--specs FR-1,STORY-2,…`) generates a markdown
review prompt that lifts each linked requirement's `## Acceptance` section
verbatim — paste it into a fresh Claude Code review session, or write it to a
file with `--write`.

- **Install `gh` or `glab` for `--pr` mode.** AIDA shells out to
  [`gh pr view`](https://cli.github.com) / [`glab mr view`](https://gitlab.com/gitlab-org/cli)
  to resolve the PR's base + head refs. Without them, AIDA falls back to
  `base=main` and a local review branch named `pr-N` / `mr-N` — that path works
  when the PR was started via `aida session start --owns PR-N` (STORY-61),
  surprising otherwise.
- **Acceptance sections are the contract.** Write a `## Acceptance`, `## Verify`,
  `## Tests`, `## Test cases`, or `## Verification` section in every STORY / BUG
  description so the review prompt has something concrete to lift.
  `aida doctor convention-check` lints for the gap.
