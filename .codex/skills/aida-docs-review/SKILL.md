---
name: aida-docs-review
description: Exhaustive documentation quality review — finds stale, inconsistent, unprofessional, and hyped content. Produces a before/after diff report.
allowed-tools:
  - Bash
  - Read
  - Glob
  - Grep
  - Write
  - Agent
---
<!-- AIDA Generated: v2.0.0 | checksum:beaee887 | DO NOT EDIT DIRECTLY -->
<!-- To customize: copy this file and modify the copy -->


# AIDA Documentation Review Skill

## Purpose

Perform an exhaustive quality review of all project documentation. Find problems, fix them, and produce a before/after diff report showing every change.

This is not a spell checker. It finds **substantive issues**: outdated information, contradictions between documents, marketing hype in technical docs, broken references, stale examples, and inconsistent terminology.

## When to Use

- Before a release — ensure docs match the actual product
- After a large feature push — catch docs that weren't updated
- Periodically (monthly) — prevent documentation rot
- When someone says "the docs are wrong" or "I can't follow the setup guide"

## Quality Dimensions

Check every document against these 8 dimensions:

### 1. Accuracy
- Do code examples actually work? (check file paths, command syntax, API endpoints)
- Do version numbers match Cargo.toml / package.json?
- Do feature descriptions match what's actually implemented?
- Are CLI examples showing correct flags? (`aida --help` is the authority)

### 2. Freshness
- When was each doc last meaningfully updated? (check git blame)
- Are there references to features that have been renamed or removed?
- Do "coming soon" or "planned" items still apply?
- Are screenshots or output examples current?

### 3. Consistency
- Same feature described differently in different docs?
- Terminology varies? (e.g., "store" vs "database" vs "backend" vs "repository")
- Numbers disagree? (skill count, backend count, requirement count)
- Installation instructions differ between docs?

### 4. Tone & Professionalism
- Marketing hype in technical documentation?
- Buzzword density too high?
- Claims without evidence?
- Superlatives ("the best", "revolutionary", "cutting-edge")?
- First person ("we built this amazing...") instead of factual statements?

**Hype words to flag:**
```
revolutionary, game-changing, cutting-edge, best-in-class, world-class,
next-generation, paradigm shift, disruptive, synergy, leverage (as verb),
unlock, supercharge, empower, seamless (when things aren't seamless),
blazing fast (without benchmarks), enterprise-grade (without evidence),
battle-tested (without production usage data), state-of-the-art,
unprecedented, magical, delightful
```

**Acceptable alternatives:**
```
Instead of "blazing fast" → "sub-millisecond queries" (with benchmark)
Instead of "enterprise-grade" → "supports PostgreSQL with connection pooling"
Instead of "seamless" → describe the actual integration steps
Instead of "battle-tested" → "used in production by N projects" (if true)
```

### 5. Completeness
- Setup prerequisites listed? (Rust version, OS, dependencies)
- Error scenarios covered? (what happens when X fails?)
- All features mentioned in README have corresponding detailed docs?
- Uninstall/removal instructions present?

### 6. Navigability
- Can a new user find what they need in under 2 minutes?
- Links between docs working? (relative paths correct?)
- Table of contents present for long docs?
- Duplicate content across docs? (DRY principle for documentation)

### 7. Code/Doc Drift
- File paths mentioned in docs still exist?
- Function names mentioned still exist?
- Configuration options mentioned still work?
- Docker commands still valid?

### 8. Inclusivity & Accessibility
- Gendered language? (use "they" not "he")
- Ableist language? (avoid "simple", "easy", "obvious" — they're subjective)
- Jargon explained on first use?
- Acronyms expanded on first use?

## Automated Prose Linting (Optional)

If `vale` is installed, run it for automated prose quality checking:

```bash
# Check if vale is available
vale --version 2>/dev/null || echo "Install: brew install vale (or see vale.sh)"

# Run vale on all docs
vale docs/ README.md CLAUDE.md OVERVIEW.md 2>&1 | head -50

# Run with specific style (Google developer docs style)
vale --config='.vale.ini' docs/ 2>&1
```

### Setting Up Vale for This Project

Create `.vale.ini` in the project root:

```ini
StylesPath = .vale/styles
MinAlertLevel = suggestion

[*.md]
BasedOnStyles = Vale, write-good, proselint

# Disable rules that conflict with technical writing
Vale.Spelling = NO
```

Install styles:
```bash
mkdir -p .vale/styles
vale sync  # downloads configured styles
```

Vale catches things the AI review might miss:
- Passive voice ("was implemented" → "we implemented")
- Weasel words ("very", "really", "basically")
- Cliches (535 known phrases)
- Readability scores (Flesch-Kincaid, Gunning Fog)
- Style guide violations (Google, Microsoft, write-good)

The AI review focuses on accuracy, freshness, and consistency.
Vale focuses on prose quality and readability. Use both.

## Workflow

### Step 1: Inventory All Documentation

```bash
# Find all documentation files
find . -name "*.md" -not -path "*/node_modules/*" -not -path "*/target/*" -not -path "*/.git/*" | sort
```

Categorize each file:
- **Root docs**: README.md, CLAUDE.md, OVERVIEW.md, AGENTS.md
- **User docs**: docs/*.md
- **Plan docs**: docs/plans/*.md (skip these — they're historical records)
- **Skill docs**: .claude/skills/*.md, aida-core/templates/skills/*.md
- **Config docs**: inline in Cargo.toml, docker-compose files

### Step 2: Cross-Reference Check

For each document:
1. Extract all **factual claims** (numbers, file paths, command examples, feature lists)
2. Verify each claim against the actual codebase
3. Cross-reference claims between documents for consistency

```bash
# Check if mentioned file paths exist
grep -oP '`[a-zA-Z0-9_./-]+\.(rs|ts|yaml|toml|sh|json)`' docs/*.md | while read match; do
  file=$(echo "$match" | cut -d: -f2 | tr -d '`')
  [ ! -f "$file" ] && echo "BROKEN PATH: $match"
done

# Check CLI examples against actual help
aida --help 2>/dev/null
aida init --help 2>/dev/null
```

### Step 3: Tone Analysis

Read each document and flag:
- Sentences with more than 2 adjectives (likely hype)
- Exclamation marks in technical content (unprofessional)
- Vague claims without specifics ("extremely fast", "very scalable")
- Marketing-style bullet points that don't convey information

### Step 4: Freshness Audit

```bash
# When was each doc last updated?
for f in $(find docs -name "*.md"); do
  echo "$(git log -1 --format='%ai' -- "$f") $f"
done | sort
```

Flag any document not updated in the last 30 days if the code it describes has changed.

### Step 5: Generate the Diff Report

For every issue found:
1. Record the **file**, **line**, **issue type**, and **severity**
2. Write a **proposed fix** (the new text)
3. Generate a **before/after diff** for each file

Save the report as `docs/review-report.md` with this structure:

```markdown
# Documentation Review Report

Generated: YYYY-MM-DD
Files reviewed: N
Issues found: N (N critical, N important, N minor)

## Summary

| File | Critical | Important | Minor |
|------|----------|-----------|-------|
| README.md | 2 | 3 | 1 |
| ...

## Issues by File

### README.md

#### CRITICAL: Feature count wrong (line 42)
- **Before**: "15 Claude Code skills"
- **After**: "21 Claude Code skills"
- **Why**: Actual count of .claude/skills/aida-*.md is 21

#### IMPORTANT: Hype language (line 58)
- **Before**: "blazing fast requirements management"
- **After**: "requirements management with sub-millisecond SQLite queries"
- **Why**: "blazing fast" is marketing language; cite the actual performance

...

## Diffs

### README.md
```diff
- AIDA ships 15 Claude Code skills
+ AIDA ships 21 Claude Code skills
```
```

### Step 6: Apply Fixes (Optional)

Ask the user:
- **Apply all fixes?** (generate a commit with all changes)
- **Apply by severity?** (critical only, critical + important, all)
- **Review individually?** (present each fix for approval)

If applying, commit with:
```bash
git commit -m "docs: fix N issues from documentation review

Critical: N fixes (accuracy, freshness)
Important: N fixes (consistency, tone)
Minor: N fixes (formatting, style)"
```

### Step 7: Generate Web Report (Optional)

Save an HTML report at `docs/review-report.html` with:
- Side-by-side before/after diffs (use `<ins>` and `<del>` tags)
- Color coding by severity (red/orange/yellow)
- Collapsible sections per file
- Summary statistics at top

## Severity Levels

| Level | When to use | Examples |
|-------|-------------|---------|
| **Critical** | Factually wrong, will cause user failure | Wrong command syntax, broken file paths, incorrect version |
| **Important** | Misleading or inconsistent, confuses users | Contradictions between docs, stale feature descriptions |
| **Minor** | Style/tone issues, not technically wrong | Hype language, inconsistent formatting, missing links |

## Anti-Patterns to Flag

### The "Trust Me" Pattern
Claims without evidence: "AIDA is the most comprehensive..." → requires data

### The "TODO" Fossil
`<!-- TODO: update this section -->` left in published docs

### The "Works On My Machine" Guide
Setup instructions that assume specific OS, paths, or tools without saying so

### The "Feature Cemetery"
Documenting planned features as if they exist: "AIDA supports..." when it doesn't yet

### The "Copy-Paste Plague"
Same paragraph appears in 3+ documents, inevitably they diverge

### The "Version Amnesia"
No date or version context: "the latest version supports..." (which version?)

## CLI Reference

```bash
# Find all docs
find . -name "*.md" -not -path "*/node_modules/*" -not -path "*/target/*"

# Check git blame for freshness
git log -1 --format='%ai %s' -- docs/getting-started.md

# Verify CLI examples
aida --help
aida init --help
aida github --help

# Count skills
ls .claude/skills/aida-*.md | wc -l

# Count storage backends
grep -c "BackendType::" aida-core/src/db/traits.rs
```