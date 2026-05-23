# Extending AIDA Skills Per Project

AIDA ships stock Claude Code skills under `.claude/skills/` and matching
slash commands under `.claude/commands/`. Those files are scaffolded from
AIDA templates and may be refreshed by `aida scaffold apply`. Do not edit
the stock skill files directly for project-specific behavior; your edits
can be overwritten by a scaffold refresh.

Use the project-owned extension surfaces instead.

## Project-Owned New Skills

Create new project-specific skills under `.claude/skills/local/`:

```bash
mkdir -p .claude/skills/local
cat > .claude/skills/local/deploy-staging.md <<'EOF'
---
name: deploy-staging
description: Deploy current branch to staging
---

# /deploy-staging

Deploy the current branch to staging and run the smoke suite.
EOF
```

Files under `.claude/skills/local/` are project assets. Track them in
git when the whole team should inherit the behavior. AIDA does not
manage or overwrite files inside that directory.

## Stock-Skill Extensions

To add project-specific instructions to a stock skill, create a sibling
`.local.md` file:

```bash
cat > .claude/skills/aida-pr.local.md <<'EOF'
## Project-specific addendum

Before opening a PR, ensure the title follows this repository's release
note convention. This instruction is required and has last-word
authority over the stock `/aida-pr` guidance for this project.
EOF
```

When Claude Code loads the stock skill, the local addendum is treated as
appended guidance. Markdown order matters: later instructions have
last-word authority when they refine or constrain earlier generic
instructions.

## Why Append Instead Of Override

Append semantics are deliberately simple:

- No merge engine or heading-matching rules.
- Reviewers can read the local file as a self-contained addendum.
- Stock-skill upgrades do not depend on stable section names.

If you need to replace a whole workflow, create a new local skill under
`.claude/skills/local/` instead of trying to fight the stock one.

## What AIDA Manages

| Path | Managed by AIDA? | Upgrade behavior |
|---|---:|---|
| `.claude/skills/<name>.md` | Yes | May be refreshed |
| `.claude/skills/<name>.local.md` | No | Left untouched |
| `.claude/skills/local/<my-skill>.md` | No | Left untouched |
| `.claude/skills/local/README.md` | Yes | Refreshed as canonical guidance |

The `local/README.md` file is the one managed file in the `local/`
directory. It exists so new contributors discover the convention.

## Git Tracking

Local skills and `.local.md` addenda are usually tracked project assets,
not per-clone runtime state. Commit them when they describe how the
project does work. Keep secrets, machine-local paths, and personal
shortcuts out of tracked local skills.

## See Also

- `.claude/skills/local/README.md` for the short scaffolded reminder.
- `.claude/AIDA.md` for the always-imported AIDA conventions block.

