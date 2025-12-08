# AIDA Documentation Skill

## Purpose

Manage AIDA project documentation including markdown guides, HTML generation, slideshow updates, and report generation. Keep documentation in sync with the codebase and requirements.

## When to Use

Use this skill when:
- User says "update docs", "regenerate documentation", or "sync docs"
- User asks to update the slideshow or add screenshots
- User requests a requirements report or status report
- Documentation needs updating after significant feature changes
- User wants to generate HTML versions of guides

## Documentation Structure

AIDA documentation lives in `/docs/`:

```
docs/
├── user-guide.md          # End-user documentation
├── user-guide.html        # Generated HTML version
├── admin-guide.md         # Administration and configuration
├── admin-guide.html       # Generated HTML version
├── DEVELOPER_GUIDE.md     # Developer/contributor guide
├── DEVELOPER_GUIDE.html   # Generated HTML version
├── slideshow.html         # Feature showcase presentation
├── style.css              # Shared styles
└── images/                # Screenshots and diagrams
    ├── ss-overview.png
    ├── ss-kanban.png
    └── ...
```

## Workflows

### 1. Update Markdown Documentation

When features change, update the relevant guide:

```bash
# Check which guide needs updating
# - user-guide.md: UI features, keyboard shortcuts, views
# - admin-guide.md: Configuration, settings, multi-project
# - DEVELOPER_GUIDE.md: Architecture, code patterns, contributing
```

**Guidelines:**
- Keep sections numbered and in logical order
- Update Table of Contents when adding sections
- Use consistent formatting (headers, code blocks, lists)
- Add cross-references between related sections
- Include keyboard shortcuts where applicable

### 2. Generate HTML Versions

After updating markdown, regenerate HTML with navigation and theming:

```bash
# Generate all HTML guides with consistent styling
cd /home/joe/ai/aida/docs

# User Guide
pandoc user-guide.md -o user-guide.html \
  --standalone \
  --metadata title="AIDA User Guide" \
  -H <(cat <<'HEADER'
<style>
:root { --bg: #1a1a2e; --text: #e4e4e7; --accent: #60a5fa; --code-bg: #0f0f23; }
.light-mode { --bg: #f8fafc; --text: #1e293b; --accent: #2563eb; --code-bg: #f1f5f9; }
body { font-family: system-ui, sans-serif; background: var(--bg); color: var(--text); max-width: 900px; margin: 0 auto; padding: 2rem; line-height: 1.6; }
h1,h2,h3 { color: var(--accent); }
code { background: var(--code-bg); padding: 0.2em 0.4em; border-radius: 4px; }
pre { background: var(--code-bg); padding: 1rem; border-radius: 8px; overflow-x: auto; }
a { color: var(--accent); }
.nav { background: var(--code-bg); padding: 1rem; margin-bottom: 2rem; border-radius: 8px; display: flex; justify-content: space-between; align-items: center; }
.nav a { margin: 0 1rem; }
.theme-toggle { cursor: pointer; padding: 0.5rem 1rem; border: 1px solid var(--accent); border-radius: 4px; background: transparent; color: var(--text); }
</style>
<script>
function toggleTheme() {
  document.body.classList.toggle('light-mode');
  localStorage.setItem('theme', document.body.classList.contains('light-mode') ? 'light' : 'dark');
}
document.addEventListener('DOMContentLoaded', () => {
  if (localStorage.getItem('theme') === 'light') document.body.classList.add('light-mode');
});
</script>
HEADER
) \
  -B <(echo '<nav class="nav"><div><a href="user-guide.html">User Guide</a><a href="admin-guide.html">Admin Guide</a><a href="DEVELOPER_GUIDE.html">Developer Guide</a><a href="slideshow.html">Slideshow</a></div><button class="theme-toggle" onclick="toggleTheme()">Toggle Theme</button></nav>')

# Repeat for admin-guide.md and DEVELOPER_GUIDE.md with appropriate titles
```

### 3. Update Slideshow

The slideshow (`slideshow.html`) showcases AIDA features with screenshots.

**Adding a New Slide:**
1. Add slide HTML following existing pattern
2. Update slide count in header
3. Add screenshot if needed

**Screenshot Naming Convention:**
```
ss-<feature>.png     # e.g., ss-kanban.png, ss-timeline.png
```

**Screenshot Checklist:**
- Capture at consistent window size
- Show relevant data/content
- Include both dark and light theme versions if showcasing themes
- Place in `docs/images/`

**Slide Template:**
```html
<!-- Slide N: Feature Name -->
<div class="slide" data-slide="N">
    <div class="slide-content">
        <h2>Feature Title</h2>
        <div class="two-column">
            <div>
                <p>Description of the feature.</p>
                <h3>Key Points</h3>
                <ul>
                    <li><strong>Point 1</strong> - Details</li>
                    <li><strong>Point 2</strong> - Details</li>
                </ul>
            </div>
            <div class="screenshot has-image">
                <img src="images/ss-feature.png" alt="Feature screenshot">
            </div>
        </div>
    </div>
</div>
```

### 4. Generate Requirements Report

Create a requirements status report:

```bash
# Basic status report
aida list --format markdown > docs/reports/requirements-status.md

# Filter by status
aida list --status draft --format markdown > docs/reports/draft-requirements.md
aida list --status approved --format markdown > docs/reports/approved-requirements.md

# By feature
aida list --feature "Core Features" --format markdown > docs/reports/core-features.md
```

**Custom Report Template:**
```markdown
# Requirements Status Report
Generated: $(date)

## Summary
- Total: $(aida list | wc -l)
- Draft: $(aida list --status draft | wc -l)
- Approved: $(aida list --status approved | wc -l)
- Completed: $(aida list --status completed | wc -l)

## By Priority
### Critical
$(aida list --priority critical)

### High
$(aida list --priority high)

## Recent Changes
$(aida list --modified-after "7 days ago")
```

### 5. Sync Documentation with Code

After significant code changes:

1. **Check for new features:**
   ```bash
   git log --oneline --since="1 week ago" -- "*.rs"
   ```

2. **Identify documentation gaps:**
   - New keyboard shortcuts? Update user-guide.md Section 5
   - New settings? Update admin-guide.md
   - New code patterns? Update DEVELOPER_GUIDE.md
   - New views? Update slideshow

3. **Update and regenerate:**
   - Edit relevant markdown files
   - Regenerate HTML versions
   - Update slideshow if visual changes
   - Commit all changes together

## CLI Reference

```bash
# List requirements for documentation
aida list
aida list --status <status>
aida list --type <type>
aida list --feature <feature>

# Show requirement details
aida show <SPEC-ID>

# Export requirements
aida export --format markdown
aida export --format json
```

## Best Practices

1. **Keep docs in sync** - Update docs in the same commit as code changes
2. **Use consistent formatting** - Follow existing patterns in each guide
3. **Include examples** - Show concrete usage examples, not just descriptions
4. **Cross-reference** - Link between guides when topics overlap
5. **Test HTML** - Open generated HTML in browser to verify formatting
6. **Commit together** - Commit markdown + HTML + screenshots as a unit

## Checklist for Documentation Updates

- [ ] Updated relevant markdown guide(s)
- [ ] Updated Table of Contents if sections added
- [ ] Regenerated HTML version(s)
- [ ] Updated slideshow if UI changed
- [ ] Added/updated screenshots if needed
- [ ] Cross-referenced related documentation
- [ ] Committed all changes together
