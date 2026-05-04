# Manage Release

Bump version, generate changelog, and tag a release tied to AIDA requirements.

## Instructions

Follow the workflow in `.claude/skills/aida-release.md`:

1. Confirm release scope (major/minor/patch or explicit version) with the user
2. Verify the working tree is clean and on the release branch
3. Use `scripts/release.sh {major|minor|patch|<version>}` to bump, tag, and push
4. Cross-reference completed requirements since the previous tag in release notes
5. Confirm the GitHub release workflow kicked off

Use when the user is ready to cut a release.
