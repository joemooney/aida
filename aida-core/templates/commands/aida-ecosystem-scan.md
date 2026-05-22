# AIDA Ecosystem Scan

Systematically scan the competitor landscape, log capability changes, update positioning, and file backlog tasks.

## Usage

```
/aida-ecosystem-scan
```

## Instructions

Follow the detailed playbook in `.claude/skills/aida-ecosystem-scan.md`:

1. **Research**: Review release feeds (Claude Code, Cursor, Aider, MCP, Skillfold) for new agent capabilities.
2. **Classify**: Categorize findings under the AIDA matrix: **Compete**, **Complement**, **Integrate**, or **Ignore**.
3. **Log**: Record structured entries in `docs/competitive-analysis/ecosystem-watch.md` dated with the active date.
4. **Update Positioning**: Amend relevant positioning papers in `docs/positioning/` if needed.
5. **File Backlog**: Use `aida queue add` or `aida req add` to file new TASK/BUG cards for any gaps or integration requirements.
