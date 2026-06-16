---
name: aida-release
description: Manage software releases with version bumping, changelog, and git tagging integrated with AIDA requirements.
disable-model-invocation: true
allowed-tools:
  - Bash
  - Read
  - Edit
  - Write
---
<!-- AIDA Generated: v2.0.0 | checksum:e7b2c4c8 | DO NOT EDIT DIRECTLY -->
<!-- To customize: copy this file and modify the copy -->


# AIDA Release Management Skill

## Purpose

Manage software releases with version bumping, release notes generation, changelog maintenance, and git tagging - all integrated with the AIDA requirements database.

## When to Use

Use this skill when:
- User wants to prepare a new release or "bump the version"
- User wants to generate release notes or tag a release in git
- User asks "what's changed since last release?"

## Workflow

### Step 1: Gather Release Context

```bash
# Get the last release tag and current version
git describe --tags --abbrev=0 2>/dev/null || echo "No previous tags"
grep '^version' Cargo.toml 2>/dev/null || cat package.json 2>/dev/null | grep '"version"'
git status --porcelain
git branch --show-current
```

### Step 2: Pre-Release Validation

Verify before proceeding:
- **Clean working directory** -- if files are modified, ask user to commit first
- **Correct branch** -- warn if not on main/master
- **Build passes** (optional) -- `cargo build --release` or `npm run build`
- **Tests pass** (optional) -- `cargo test` or `npm test`

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

### Step 4: Gather Changes Since Last Release

```bash
# Get date of last tag
LAST_TAG=$(git describe --tags --abbrev=0 2>/dev/null)

# Requirements completed since last release
aida list --status completed

# Commits since last tag
git log ${LAST_TAG}..HEAD --oneline
```

### Step 5: Generate Release Notes

Create release notes from completed requirements (grouped by type) and git commit messages:

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

### Step 6: Update CHANGELOG.md

If CHANGELOG.md exists, insert new version section after `## [Unreleased]` following Keep a Changelog format. If none exists, offer to create one.

```markdown
## [Unreleased]

## [{version}] - {YYYY-MM-DD}

### Added
- FR-0123: User authentication system

### Fixed
- BUG-0045: Fixed login timeout issue

### Changed
- CR-0012: Updated API response format
```

### Step 7: Update Version Files

**Cargo.toml (Rust):**
```bash
sed -i 's/^version = ".*"/version = "{new_version}"/' Cargo.toml
```

**package.json (Node):**
```bash
npm version {new_version} --no-git-tag-version
```

### Step 8: Export Requirements Database

Export SQLite to YAML for git-friendly diffs and team sync:

```bash
aida db migrate --from sqlite --to yaml --force
```

### Step 9: Commit and Tag

```bash
# Stage the version + changelog (the AIDA store syncs separately, via aida push)
git add Cargo.toml CHANGELOG.md  # or package.json

# Commit version bump
git commit -m "chore: release v{version}

Release notes:
- X features added
- Y bugs fixed

Requirements database exported (X requirements).
See CHANGELOG.md for details."

# Create annotated tag
git tag -a v{version} -m "Release v{version}

{release_notes_summary}"
```

### Step 10: Offer Push

Ask user if they want to push:
1. Push commits and tags: `git push && git push --tags`
2. Push commits only: `git push`
3. Don't push (manual)

## Integration Notes

- Uses AIDA requirements database for change tracking
- Respects semantic versioning (semver.org)
- Follows Keep a Changelog format (keepachangelog.com)
- Creates annotated git tags with release summary