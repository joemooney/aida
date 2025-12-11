# AIDA Release Management Skill

## Purpose

Manage software releases with version bumping, release notes generation, changelog maintenance, and git tagging - all integrated with the AIDA requirements database.

## When to Use

Use this skill when:
- User wants to prepare a new release
- User asks to "bump the version" or "create a release"
- User wants to generate release notes
- User wants to tag a release in git
- User asks "what's changed since last release?"

## Workflow

### Step 1: Gather Release Context

Collect information about the current state:

```bash
# Get the last release tag
git describe --tags --abbrev=0 2>/dev/null || echo "No previous tags"

# Get current version from Cargo.toml (Rust) or package.json (Node)
grep '^version' Cargo.toml 2>/dev/null || cat package.json 2>/dev/null | grep '"version"'

# Check git status
git status --porcelain

# Check current branch
git branch --show-current
```

### Step 2: Pre-Release Validation

Run validation checks before proceeding:

1. **Git Status Check**
   ```bash
   # Must have clean working directory
   git status --porcelain
   ```
   - If files are modified, warn user and ask if they want to commit first

2. **Branch Check**
   ```bash
   git branch --show-current
   ```
   - Warn if not on main/master branch

3. **Build Check** (optional)
   ```bash
   cargo build --release 2>&1  # For Rust
   # or
   npm run build 2>&1  # For Node
   ```

4. **Test Check** (optional)
   ```bash
   cargo test 2>&1  # For Rust
   # or
   npm test 2>&1  # For Node
   ```

Display validation results:
```
Pre-Release Validation:
  [PASS] Git working directory is clean
  [PASS] On branch: main
  [PASS] Tests passing
  [PASS] Build successful
```

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

Query requirements completed since last release:

```bash
# Get date of last tag
LAST_TAG=$(git describe --tags --abbrev=0 2>/dev/null)
LAST_TAG_DATE=$(git log -1 --format=%ai $LAST_TAG 2>/dev/null)

# List requirements (filter by modified date in your analysis)
aida list --status completed
```

Also gather git commits:
```bash
# Commits since last tag
git log ${LAST_TAG}..HEAD --oneline
```

### Step 5: Generate Release Notes

Create release notes from:
1. Completed requirements (grouped by type)
2. Git commit messages (for additional context)

Format:
```markdown
## Release v{version} - {date}

### Features
- FR-0123: User authentication system
- FR-0124: Dashboard analytics

### Bug Fixes
- BUG-0045: Fixed login timeout issue
- BUG-0046: Corrected date formatting

### Changes
- CR-0012: Updated API response format

### Implementation Tasks
- IMPL-0089: Refactored database layer
- IMPL-0090: Added caching support

### Statistics
- X features added
- Y bugs fixed
- Z commits since last release
```

### Step 6: Update CHANGELOG.md

If CHANGELOG.md exists, update it:

```bash
# Read existing changelog
cat CHANGELOG.md
```

Insert new version section after `## [Unreleased]`:
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

If no CHANGELOG.md exists, offer to create one following Keep a Changelog format.

### Step 7: Update Version Files

Update version in project files:

**Cargo.toml (Rust):**
```bash
# Update version line
sed -i 's/^version = ".*"/version = "{new_version}"/' Cargo.toml
```

**package.json (Node):**
```bash
npm version {new_version} --no-git-tag-version
```

### Step 8: Export Requirements Database

Export the SQLite database to YAML for version control. This allows:
- Git-friendly diffs of requirement changes
- Easy onboarding for new developers who clone the repo
- Sync points for distributed teams

```bash
# Export SQLite to YAML
aida db migrate --from sqlite --to yaml --force

# Verify export
echo "Exported $(grep -c '^  - id:' requirements.yaml 2>/dev/null || echo 0) requirements to YAML"
```

This ensures the requirements.yaml in git always reflects the latest database state.

### Step 9: Commit and Tag

```bash
# Stage changes including requirements.yaml
git add Cargo.toml CHANGELOG.md requirements.yaml  # or package.json

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
```
Release v{version} created locally.

Push to remote?
1. Push commits and tags: git push && git push --tags
2. Push commits only: git push
3. Don't push (I'll do it manually)
```

## Example Session

```
User: Let's do a release

Claude: I'll help you prepare a release. Let me check the current state...

**Current State:**
- Version: 0.5.2 (from Cargo.toml)
- Last tag: v0.5.2 (2025-11-28)
- Branch: main
- Working directory: Clean

**Pre-Release Validation:**
  [PASS] Git working directory is clean
  [PASS] On branch: main

**Changes since v0.5.2:**

Requirements completed (5):
- FR-0170: Smart parent selection for new requirements
- FR-0172: CLI add command --parent option
- FR-0146: Folder type for organizational containers
- BUG-0089: Fixed duplicate SPEC-ID issue

Commits: 12 commits since last tag

What type of version bump?
1. patch (0.5.3) - Bug fixes only
2. minor (0.6.0) - New features
3. major (1.0.0) - Breaking changes

User: minor please

Claude: Preparing release v0.6.0...

**Generated Release Notes:**

## Release v0.6.0 - 2025-12-02

### Features
- FR-0170: Smart parent selection for new requirements
- FR-0172: CLI add command --parent option
- FR-0146: Folder type for organizational containers

### Bug Fixes
- BUG-0089: Fixed duplicate SPEC-ID issue

### Statistics
- 3 features added
- 1 bug fixed
- 12 commits

Does this look correct? (yes/edit/cancel)

User: yes

Claude:
Updated Cargo.toml version to 0.6.0
Updated CHANGELOG.md with new release section
Created commit: "chore: release v0.6.0"
Created tag: v0.6.0

Push to remote? (yes/no)

User: yes

Claude:
Pushed commits to origin/main
Pushed tags to origin

Release v0.6.0 complete!
```

## CLI Reference

```bash
# List completed requirements
aida list --status completed

# Show requirement details
aida show <SPEC-ID>

# Git operations
git describe --tags --abbrev=0     # Last tag
git log v0.5.2..HEAD --oneline     # Commits since tag
git tag -a v0.6.0 -m "message"     # Create annotated tag
git push --tags                     # Push tags
```

## Integration Notes

- Uses AIDA requirements database for change tracking
- Respects semantic versioning (semver.org)
- Follows Keep a Changelog format (keepachangelog.com)
- Creates annotated git tags with release summary
- Updates version in Cargo.toml or package.json
