---
name: aida-docs
description: Manage project documentation including guides, reports, and keeping docs in sync with the codebase and requirements.
allowed-tools:
  - Bash
  - Read
  - Edit
  - Write
  - Glob
  - Grep
---
<!-- AIDA Generated: v2.0.0 | checksum:6abefdf6 | DO NOT EDIT DIRECTLY -->
<!-- To customize: copy this file and modify the copy -->


# AIDA Documentation Skill

## Purpose

Manage project documentation including markdown guides, HTML generation, slideshow updates, and report generation. Keep documentation in sync with the codebase and requirements.

## When to Use

Use this skill when:
- User says "update docs", "regenerate documentation", or "sync docs"
- User asks to update the slideshow or add screenshots
- User requests a requirements report or status report
- Documentation needs updating after significant feature changes
- User wants to generate HTML versions of guides

## Documentation Structure

Typical project documentation:

```
docs/
├── user-guide.md          # End-user documentation
├── user-guide.html        # Generated HTML version
├── admin-guide.md         # Administration and configuration
├── admin-guide.html       # Generated HTML version
├── DEVELOPER_GUIDE.md     # Developer/contributor guide
├── DEVELOPER_GUIDE.html   # Generated HTML version
├── slideshow.html         # Feature showcase presentation
└── images/                # Screenshots and diagrams
```

## Workflows

### 1. Update Markdown Documentation

When features change, update the relevant guide:
- **user-guide.md**: UI features, keyboard shortcuts, views
- **admin-guide.md**: Configuration, settings, multi-project setup
- **DEVELOPER_GUIDE.md**: Architecture, code patterns, contributing

**Guidelines:**
- Keep sections numbered and in logical order
- Update Table of Contents when adding sections
- Use consistent formatting (headers, code blocks, lists)
- Add cross-references between related sections

### 2. Generate HTML Versions

After updating markdown, regenerate HTML with navigation and theming:

```bash
# Generate HTML with pandoc (example)
pandoc user-guide.md -o user-guide.html --standalone \
  --metadata title="User Guide" \
  -H styles.html
```

### 3. Update Slideshow

For slideshow presentations:
- Add slides following existing HTML pattern
- Update slide count in header
- Add screenshots to `docs/images/` with naming convention `ss-<feature>.png`

### 4. Generate Requirements Report

```bash
# Basic status report
aida list > docs/reports/requirements-status.md

# Filter by status
aida list --status draft
aida list --status approved
aida list --status completed

# By priority
aida list --priority critical
aida list --priority high
```

### 5. Sync Documentation with Code

After significant code changes:
1. Check git log for recent changes
2. Identify documentation gaps
3. Update relevant markdown files
4. Regenerate HTML versions
5. Commit all changes together

## CLI Reference

```bash
# List requirements for documentation
aida list
aida list --status <status>
aida list --type <type>

# Show requirement details
aida show <SPEC-ID>

# Export requirements (formats: mapping, tree, json)
aida export --format tree
aida export --format json
```

## Best Practices

1. **Keep docs in sync** - Update docs in the same commit as code changes
2. **Use consistent formatting** - Follow existing patterns in each guide
3. **Include examples** - Show concrete usage examples
4. **Cross-reference** - Link between guides when topics overlap
5. **Commit together** - Commit markdown + HTML + screenshots as a unit