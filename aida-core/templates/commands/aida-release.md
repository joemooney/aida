---
description: Run /aida-release.
---
# Manage Release

Prepare a release tied to AIDA requirements, verify the release gates, and stop
before any tag or push.

## Instructions

Follow the workflow in `.claude/skills/aida-release/SKILL.md`:

1. Confirm release scope (major/minor/patch or explicit version) with the user
2. Verify the working tree is clean and on the release branch
3. Gather shipped context since the previous tag with `aida changelog` and `git log`
4. Verify release gates: cross-platform CI, quiet main, and no release blockers
5. Run `scripts/release.sh --prepare-only {major|minor|patch|<version>}`
6. Stop before any release commit, tag, push, or binary publish

Use when the user is ready to prepare a release. Publishing still requires an
explicit operator decision after reviewing the prepared diff and tag notes.
