---
name: aida-sync
description: Sync AIDA templates and scaffolding. Use after modifying templates to verify integrity and propagate changes.
disable-model-invocation: true
allowed-tools:
  - Bash
  - Glob
---
<!-- AIDA Generated: v2.0.0 | checksum:f78b198b | DO NOT EDIT DIRECTLY -->
<!-- To customize: copy this file and modify the copy -->


# AIDA Sync Skill

## Purpose

Maintain consistency between AIDA templates and scaffolded projects. This is a meta-level skill for AIDA development that ensures template updates propagate correctly.

## When to Use

- You've modified templates in `aida-core/templates/`
- You've updated `CLAUDE.md` guidance
- You want to check if scaffolded projects need updates
- At the end of an AIDA development session
- User asks to "sync templates" or "check scaffold status"

## Workflow

### Step 1: Detect Environment

Determine if we're in the AIDA source repo or a scaffolded project:

```bash
ls aida-core/templates/ 2>/dev/null && echo "AIDA_REPO" || echo "SCAFFOLDED_PROJECT"
```

- **AIDA Repo**: Has `aida-core/templates/` and workspace `Cargo.toml`
- **Scaffolded Project**: Has `.claude/skills/` with generated files, no `aida-core/templates/`

### Step 2: AIDA Repo Mode

#### 2a. Check Symlinks and Git Hooks

Verify `.claude/` symlinks and `.git/hooks/` point to `aida-core/templates/`:

```bash
# List all symlinks and check for broken ones
find .claude/commands .claude/skills -type l -exec ls -la {} \;
find .claude/commands .claude/skills -xtype l 2>/dev/null

# Check git hooks are symlinked (not copies)
if [ -L .git/hooks/commit-msg ]; then
    echo "OK: commit-msg is symlinked"
else
    echo "FIX: ln -sf ../../aida-core/templates/hooks/commit-msg .git/hooks/commit-msg"
fi
```

Report: missing symlinks, broken symlinks, non-symlink files that should be symlinks, and hooks that need relinking.

#### 2b. Check Template vs Binary

After modifying templates, the binary needs rebuilding:

```bash
find aida-core/templates -newer target/debug/aida 2>/dev/null | head -5
```

If templates are newer, prompt: `Run cargo build to update embedded templates.`

#### 2c. Check CLAUDE.md Consistency

Verify all skills are documented:

```bash
ls aida-core/templates/skills/
grep -E "^### /aida-" CLAUDE.md
```

Report any skills not documented in CLAUDE.md.

#### 2d. Version Bump Reminder

If templates changed significantly, remind to bump `SCAFFOLD_VERSION` in `aida-core/src/scaffolding.rs`.

### Step 3: Scaffolded Project Mode

#### 3a. Check Scaffold Status

```bash
aida scaffold status
```

Interpret: **matching** (in sync), **modified** (user customized, don't overwrite), **missing** (new templates available), **extra** (project-specific, fine to keep).

#### 3b. Offer Updates

```bash
aida scaffold preview   # Preview changes
aida scaffold apply     # Apply updates (safe for unmodified files)
```

For modified files, offer: keep yours, view diff, or merge manually.

### Step 4: Generate Sync Report

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

When modifying AIDA templates:

1. **Edit master templates** in `aida-core/templates/` only
2. **Run `make sync-templates`** to verify symlinks
3. **Update CLAUDE.md** if adding/removing skills
4. **Rebuild binary** with `cargo build` to embed changes
5. **Consider version bump** in `scaffolding.rs` for significant changes
6. **Test** with `aida scaffold status` in the repo

## CLI Reference

```bash
aida scaffold status       # Check scaffold status
aida scaffold preview      # Preview scaffold changes
aida scaffold apply        # Apply scaffold updates
make sync-templates        # Verify symlinks (in AIDA repo)
cargo build -p aida-cli    # Rebuild binary with new templates
```

## Template Architecture

In the **AIDA source repo**, `.claude/skills/`, `.claude/commands/`, and `.git/hooks/commit-msg` are **symlinks** pointing into `aida-core/templates/`. This keeps the working copies and master templates in sync during development.

In **scaffolded projects**, `aida scaffold apply` creates **copies** of templates from the binary's embedded templates into `.claude/` and `.git/hooks/`. These copies can be customized per-project.

## Related Skills

- `/aida-capture`: Capture requirements at session end
- `/aida-commit`: Commit with requirement linking