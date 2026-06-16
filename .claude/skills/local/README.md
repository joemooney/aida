<!-- AIDA Generated: v2.0.0 | checksum:e9fa1253 | DO NOT EDIT DIRECTLY -->
<!-- To customize: copy this file and modify the copy -->

# Per-project skill extensions

This directory and the `*.local.md` convention let a project extend
AIDA's stock skills without forking them. AIDA never writes inside
`local/`, never writes a `*.local.md` file, and `make sync-templates`
never touches either — both survive every upgrade and re-scaffold.

## Two mechanisms, one rule

1. **New skill** — drop `local/<my-skill>.md` here. Claude Code
discovers it the same way it discovers stock skills. AIDA will
never overwrite it.
2. **Extend a stock skill** — alongside `.claude/skills/<name>.md`
(one level up from this directory), add `<name>.local.md`. When
`/aida-<name>` is invoked, the stock skill runs first and the
`.local.md` content is **appended** as project-specific guidance
with last-word authority — later instructions override earlier
ones in normal markdown precedence.

## Tracked, not ignored

Both `local/<my-skill>.md` and `<name>.local.md` are project assets:
they are intentionally **checked into git** so the whole team picks
up the project's skill customizations on `git pull`. The scaffolded
`.gitignore` makes no exception for them; they fall under `.claude/`
which is tracked by default.

## Worked example

See `docs/extending-skills.md` for two end-to-end examples (a
brand-new project-owned skill, and a `<skill>.local.md` extension
to `/aida-pr`). trace:STORY-305
