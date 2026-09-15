---
name: aida-release
description: Prepare software releases with version bumping, changelog, gate checks, and an explicit stop before git tagging.
disable-model-invocation: true
allowed-tools:
  - Bash
  - Read
  - Edit
  - Write
---

# AIDA Release Management Skill

## Purpose

Prepare software releases with version bumping, release notes context,
changelog maintenance, and release-gate checks - all integrated with the AIDA
requirements database. This skill must stop before any release commit, git tag,
tag push, or binary publish.

## When to Use

Use this skill when:
- User wants to prepare a new release or "bump the version"
- User wants to generate release notes or review a release candidate
- User asks "what's changed since last release?"

Do not use this skill to publish autonomously. Release publishing is an
operator-guided boundary.

## Workflow

### Step 1: Gather Release Context

```bash
# Get the last release tag and current version
git describe --tags --abbrev=0 2>/dev/null || echo "No previous tags"
awk '/^\[workspace\.package\]/{f=1; next} f && /^version/{print; exit}' Cargo.toml
git status --short --branch
git branch --show-current
```

### Step 2: Pre-Release Validation

Verify before proceeding:
- **Clean working directory** - if files are modified, ask user to commit first
- **Correct branch** - warn if not on main/master
- **Quiet main** - inspect live drains/sessions so no autonomous worker is pushing
- **No release blockers** - search/list specs tagged `release-blocker`
- **Cross-platform gate** - `scripts/pre-release-check.sh` reports a recent green run

### Step 3: Determine Version Bump

Ask user for bump type:
```
Current version: 0.5.2
Last release tag: v0.5.2

What type of version bump?
1. patch (0.5.2 -> 0.5.3) - Bug fixes only
2. minor (0.5.2 -> 0.6.0) - New features, backwards compatible
3. major (0.5.2 -> 1.0.0) - Breaking changes
4. custom - Specify version manually
```

### Step 4: Gather Shipped List

```bash
LAST_TAG=$(git describe --tags --abbrev=0 2>/dev/null || true)
aida changelog refresh --released-as "v{version}"
git log ${LAST_TAG}..HEAD --oneline --no-merges
```

### Step 5: Summarize Release Notes

Summarize completed requirements and commits by type for the operator:

```markdown
## Release v{version} - {date}

### Features
- FR-0123: User authentication system

### Bug Fixes
- BUG-0045: Fixed login timeout issue

### Changes
- CR-0012: Updated API response format

### Statistics
- X features added, Y bugs fixed, Z commits since last release
```

### Step 6: Mechanical Prep

```bash
scripts/release.sh --prepare-only {major|minor|patch|<version>}
```

This performs the version bump, dependency-pin bump, `cargo check --workspace`,
CHANGELOG refresh, docs gate, tag-note generation, and cross-platform release
gate, then exits before any commit/tag/push. trace:STORY-1125 | ai:codex

### Step 7: Stop For Operator Decision

Stop and report:
- version prepared
- previous tag and shipped-list summary
- release-gate verdicts
- dirty files from the mechanical prep
- tag notes path printed by `scripts/release.sh --prepare-only`

Do not run `git commit`, `git tag`, `git push`, `scripts/release.sh --yes`, or
any equivalent publish action unless the operator gives a new explicit command
after reviewing the prep.

## Integration Notes

- Uses AIDA requirements database for change tracking
- Respects semantic versioning (semver.org)
- Follows Keep a Changelog format (keepachangelog.com)
- Produces annotated-tag notes, but leaves tag creation to the operator
