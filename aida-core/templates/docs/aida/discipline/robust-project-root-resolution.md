# Robust Project Root Resolution for Skill Rendering

When AIDA is run in a brand-new workspace or in environments that lack a standard git repository, `find_project_root()` can fail. To ensure seamless operation, AIDA implements robust root resolution for its skill rendering engine.

## Rationale & Behavior

AIDA's skill-rendering command retrieves and outputs custom or default workspace skills. Because these skills reside under the `.claude/skills/` directory, resolving the project root is necessary. 

The fallback discipline ensures that:
1. AIDA first attempts to locate the standard `.git` directory via parent directories to find the project root.
2. If this search fails (e.g., in a non-git directory, dynamic CI environment, or brand-new workspace initialization), AIDA gracefully falls back to the **current working directory** (`std::env::current_dir()`) instead of crashing.
3. If the skill file exists in `.claude/skills/` within the resolved folder, it is parsed and rendered successfully.

## Verification

If you suspect skill loading is failing due to project-root misconfiguration:
1. Verify the current working directory contains the `.claude/skills/` directory.
2. Check that the target skill is named `<name>.md` or `<name>.local.md`.
3. Execute the standard `aida skill render <name>` or `aida mcp skill` command.
