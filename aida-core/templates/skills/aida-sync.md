---
name: aida-sync
description: Sync AIDA templates and scaffolding. Use after modifying templates to verify integrity and propagate changes.
disable-model-invocation: true
allowed-tools:
  - Bash
  - Glob
---

# AIDA Sync Skill

## Purpose

Maintain consistency between AIDA templates and scaffolded projects. This is a meta-level skill for AIDA development that ensures template updates propagate correctly.

## When to Use

Use this skill when:
- You've modified templates in `aida-core/templates/`
- You've updated `CLAUDE.md` guidance
- You want to check if scaffolded projects need updates
- At the end of an AIDA development session
- User asks to "sync templates" or "check scaffold status"

## Workflow

### Step 1: Detect Environment

Check if we're in the AIDA source repository or a scaffolded project:

```bash
# Check for AIDA repo markers
ls aida-core/templates/ 2>/dev/null && echo "AIDA_REPO"
ls .claude/skills/ 2>/dev/null && echo "HAS_SKILLS"
```

**AIDA Source Repo indicators:**
- Has `aida-core/templates/` directory
- Has `Cargo.toml` with `[workspace]` containing aida-core, aida-cli, etc.

**Scaffolded Project indicators:**
- Has `.claude/skills/` with generated files (AIDA headers)
- No `aida-core/templates/` directory

### Step 2: AIDA Repo Mode

If in the AIDA source repository:

#### 2a. Check Symlink Integrity

Verify `.claude/` and `.git/hooks/` symlinks point to `aida-core/templates/`:

```bash
# List all symlinks in .claude/
find .claude/commands .claude/skills -type l -exec ls -la {} \;

# Check for broken symlinks
find .claude/commands .claude/skills -xtype l 2>/dev/null

# Check git hooks symlinks
ls -la .git/hooks/commit-msg 2>/dev/null
```

Report:
- Missing symlinks (template exists but no symlink)
- Broken symlinks (symlink target doesn't exist)
- Non-symlink files (should be symlinks in AIDA repo)

#### 2b. Check Git Hooks

Verify git hooks are symlinked to templates (not copies):

```bash
# Check if commit-msg hook is a symlink
if [ -L .git/hooks/commit-msg ]; then
    echo "✓ commit-msg is symlinked"
    ls -la .git/hooks/commit-msg
else
    echo "✗ commit-msg is NOT a symlink - should be: ../../aida-core/templates/hooks/commit-msg"
fi

# Compare hook content if not symlinked
if [ ! -L .git/hooks/commit-msg ] && [ -f .git/hooks/commit-msg ]; then
    diff .git/hooks/commit-msg aida-core/templates/hooks/commit-msg && echo "Content matches" || echo "Content differs!"
fi
```

To fix a non-symlinked hook:
```bash
rm .git/hooks/commit-msg
ln -s ../../aida-core/templates/hooks/commit-msg .git/hooks/commit-msg
```

#### 2c. Check Template vs Embedded

After modifying templates, the binary needs rebuilding:

```bash
# Check if binary is older than templates
ls -la target/debug/aida 2>/dev/null
find aida-core/templates -newer target/debug/aida 2>/dev/null | head -5
```

If templates are newer than binary, prompt:
```
Templates have been modified since last build.
Run `cargo build` to update embedded templates in binary.
```

#### 2d. Check CLAUDE.md Consistency

Verify the CLAUDE.md documents all current skills:

```bash
# List skills in templates
ls aida-core/templates/skills/

# Check CLAUDE.md mentions them
grep -E "^### /aida-" CLAUDE.md
```

Report any skills not documented in CLAUDE.md.

#### 2e. Version Bump Reminder

If templates changed significantly, remind about version:

```
Consider bumping SCAFFOLD_VERSION in aida-core/src/scaffolding.rs
Current version: <version>
```

### Step 3: Scaffolded Project Mode

If in a scaffolded project:

#### 3a. Check Scaffold Status

```bash
aida scaffold status
```

Interpret output:
- **matching**: Files are in sync with AIDA templates
- **modified**: User has customized files (don't overwrite)
- **missing**: New templates available in AIDA
- **extra**: Project-specific additions (fine to keep)

#### 3b. Offer Updates

If there are missing or outdated templates:

```bash
# Preview what would change
aida scaffold preview

# Apply updates (safe for unmodified files)
aida scaffold apply
```

For modified files, offer options:
1. **Keep yours**: Preserve customizations
2. **View diff**: Show what changed in AIDA
3. **Merge manually**: User reviews and applies changes

### Step 4: Generate Sync Report

Present summary:

```
## AIDA Sync Report

### Status
- Environment: [AIDA Repo / Scaffolded Project]
- Template Version: <version>

### Actions Needed
- [ ] Rebuild binary (templates modified)
- [ ] Create missing symlinks: <list>
- [ ] Fix git hooks (not symlinked): <list>
- [ ] Update CLAUDE.md skill documentation
- [ ] Run `aida scaffold apply` on downstream projects

### Files Checked
- Skills: X matching, Y modified, Z missing
- Commands: X matching, Y modified, Z missing
- Hooks: X symlinked, Y need fixing
```

## AIDA Development Checklist

When modifying AIDA templates, follow this checklist:

1. **Edit master templates** in `aida-core/templates/` only
2. **Run `make sync-templates`** to verify symlinks
3. **Update CLAUDE.md** if adding/removing skills
4. **Rebuild binary** with `cargo build` to embed changes
5. **Consider version bump** in `scaffolding.rs` for significant changes
6. **Test with `aida scaffold status`** in the repo

## CLI Reference

```bash
# Check scaffold status
aida scaffold status

# Preview scaffold changes
aida scaffold preview

# Apply scaffold updates
aida scaffold apply

# Verify symlinks (in AIDA repo)
make sync-templates

# Rebuild binary with new templates
cargo build -p aida-cli
```

## Template Architecture Reference

```
AIDA Source Repo                    Scaffolded Project
==================                  ==================
aida-core/templates/
├── skills/                         .claude/skills/
│   ├── aida-req.md        →        ├── aida-req.md (copy)
│   ├── aida-implement.md  →        ├── aida-implement.md (copy)
│   └── ...                →        └── ...
├── commands/                       .claude/commands/
│   ├── aida-req.md        →        ├── aida-req.md (copy)
│   └── ...                →        └── ...
├── hooks/                          .git/hooks/
│   └── commit-msg         →        └── commit-msg (copy)
└── CLAUDE.md.template     →        CLAUDE.md (generated)

.claude/                   (symlinks in AIDA repo)
├── skills/ → aida-core/templates/skills/
└── commands/ → aida-core/templates/commands/

.git/hooks/                (symlinks in AIDA repo)
└── commit-msg → ../../aida-core/templates/hooks/commit-msg
```

## Related Skills

- `/aida-capture`: Capture requirements at session end
- `/aida-commit`: Commit with requirement linking
