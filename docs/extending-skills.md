# Extending AIDA skills per-project

AIDA ships ~34 stock skills under `.claude/skills/` (matching slash commands
under `.claude/commands/`). Each is a master template embedded in the `aida`
binary and re-scaffolded on `aida scaffold apply`. Editing them directly
works, but the edit gets clobbered the next time the master is upgraded — and
the project loses its customization without warning.

STORY-305 adds two mechanisms that let a project extend skills *without*
forking the masters. AIDA never writes inside the new surfaces, `make
sync-templates` never touches them, and `aida scaffold apply` never
overwrites them.

## The two mechanisms

### 1. Project-owned new skills — `.claude/skills/local/<my-skill>.md`

Drop a brand-new skill file into `.claude/skills/local/`. Claude Code
discovers it the same way it discovers a stock skill — invoke it with
`/my-skill`. AIDA never manages files inside `local/`: not on `aida init`,
not on `aida scaffold apply`, not on `make sync-templates`. The directory
is yours.

Use this for skills that are specific to a single project — domain
shortcuts, custom review checklists, repo-specific deployment routines,
anything that doesn't belong upstream in AIDA.

### 2. Stock-skill extensions — `.claude/skills/<name>.local.md`

Alongside a stock skill `.claude/skills/<name>.md`, add a sibling file
`.claude/skills/<name>.local.md`. When Claude Code invokes `/aida-<name>`,
it reads both files and treats `<name>.local.md` as **appended** to the
stock skill — project-specific guidance with **last-word authority**.
Standard markdown precedence applies: a later instruction supersedes an
earlier one on the same point.

The merge happens *for Claude Code* (skills are filesystem markdown
documents that Claude reads when the slash command fires); it is not a
build step that produces a merged file. The append-merge rule is recorded
in `.claude/AIDA.md`, which CLAUDE.md imports on every session, so the
loaded context always tells Claude to look for a sibling `.local.md`.

## Why append, not section-override?

The spec gave the implementer the call. Append wins on three counts:

- **Predictable.** No merge engine, no section-name matching, no edge
  cases when two sections share a heading. The extension reads like a
  postscript to the stock skill, top-down.
- **Diff-friendly.** A reviewer looking at `<skill>.local.md` reads a
  self-contained delta, not a half-overlay that only makes sense beside
  the master.
- **Markdown-native.** Last instruction wins is how Claude already reads
  long documents; no new semantic for Claude (or a human) to learn.

Section-override would be more powerful (e.g. "replace just the
`## Workflow` section"), but the cost is real: every override implicitly
depends on the master's heading structure, which makes upgrades fragile.
Append takes the surface that ships today and adds to the end of it.

## Worked example A — a brand-new project-owned skill

```bash
# In a project that has run `aida init`
mkdir -p .claude/skills/local
cat > .claude/skills/local/deploy-staging.md <<'EOF'
---
name: deploy-staging
description: Deploy current branch to the staging environment
---

# /deploy-staging

Push the current branch to the staging environment and run the smoke suite.

## Steps
1. `git push origin HEAD:staging --force-with-lease`
2. Wait for the staging deploy workflow on GitHub.
3. Run `./scripts/smoke-staging.sh` and report PASS/FAIL.
EOF

git add .claude/skills/local/deploy-staging.md
git commit -m "feat(skills): add /deploy-staging local skill"
```

Now `/deploy-staging` is available in this project. A teammate cloning the
repo gets it automatically. `aida scaffold apply` will not touch it.

## Worked example B — extending `/aida-pr`

Suppose this project has a non-standard PR title rule — every PR title
must start with the spec ID in square brackets. The stock `/aida-pr`
skill doesn't know about that, and editing the stock skill would lose the
rule on the next AIDA upgrade. Instead:

```bash
cat > .claude/skills/aida-pr.local.md <<'EOF'
## Project-specific addendum

Before opening the PR with `gh pr create`, enforce this project's title
convention: the title must start with `[<SPEC-ID>]` (square brackets,
spec ID, space, then the conventional-commits subject).

Example: `[STORY-305] feat(skills): per-project skill extensions`

If the title you assembled does not match `^\[[A-Z]+-[0-9]+\] `, rewrite
it before invoking `gh pr create`. This is required, not a suggestion —
the repo's PR title lint will block the merge otherwise.
EOF

git add .claude/skills/aida-pr.local.md
git commit -m "feat(skills): add aida-pr.local.md for PR title convention"
```

The next `/aida-pr` invocation will, after running the stock skill,
honor the addendum's title rule. Because the addendum lands *after* the
stock skill in Claude's context, it has last-word authority on title
formatting.

## What survives an upgrade

The contract `make sync-templates` and `aida scaffold apply` honor:

| Path                                          | Managed by AIDA? | Survives upgrade? |
|-----------------------------------------------|------------------|-------------------|
| `.claude/skills/<name>.md`                    | Yes              | Overwritten       |
| `.claude/skills/<name>.local.md`              | No               | Untouched         |
| `.claude/skills/local/<my-skill>.md`          | No               | Untouched         |
| `.claude/skills/local/README.md`              | Yes (template)   | Re-scaffolded     |

The `local/README.md` is the only exception: it is template-class so its
explanation of the convention stays canonical for new readers.

## Git tracking

Both surfaces are tracked project assets — they're checked into git so
the whole team picks up customizations on `git pull`. AIDA's scaffolded
`.gitignore` makes no exception for them: they fall under `.claude/`,
which is tracked by default. The decision was deliberate (the spec asked
for a choice between tracked and runtime); local skills are part of how
the project does work, not per-clone runtime state.

## See also

- `.claude/skills/local/README.md` — the TL;DR scaffolded into every project
- `.claude/AIDA.md` — the always-imported conventions block, which records
  the append-merge rule for Claude Code
- STORY-305 — the original spec
