---
name: aida-ecosystem-scan
description: Execute the ecosystem watch scan. Guided walkthrough to scan competitors, classify capabilities (Compete/Complement/Integrate/Ignore), write entries to docs/competitive-analysis/ecosystem-watch.md, and file requirements to close the engineering feedback loop.
allowed-tools:
  - Bash
  - Read
  - Write
---

# AIDA Ecosystem Scan Skill

## Purpose

Maintain AIDA's strategic and technical edge by systematically reviewing shifts in the AI coding agent landscape (specifically Claude Code, Cursor, Windsurf, Aider, Cline, and protocol layers like MCP and Skillfold), documenting findings, and converting intelligence directly into backlog items.

---

## When to Use

Execute this scan when:
1. **Quarterly Interval**: A scheduled deep-dive landscape review is due.
2. **Signal Trigger**: Any critical market event specified in [signals-to-watch.md](../../docs/competitive-analysis/signals-to-watch.md) fires (e.g. a terminal-agent tool hits 10k stars, Anthropic launches Agent Teams).
3. **Release Hook Warning**: Promoted by `scripts/release.sh` during minor/major version bumps.

---

## Workflow Playbook

### Step 1: Research and Gather Intelligence
Collect release notes, technical specs, and community telemetry. Key sources include:
- **Anthropic Claude Code**: Changelogs on NPM or the Anthropic developer portal.
- **IDE Agents**: Cursor and Windsurf release logs.
- **OS Agent Runners**: GitHub release feeds for Cline, Aider, OpenHands.
- **Protocol layers**: MCP specification changes and `byronxlg/skillfold` repository.

### Step 2: Classify Findings
Evaluate each capability using the AIDA strategic matrix:
- **Compete**: Directly challenges AIDA's core value. Requires defensive roadmap alignment and positioning doc updates.
- **Complement**: Adjacent utility we can wrap, pair with, or leverage (e.g. specialized PTY or mobile standby hooks).
- **Integrate**: Emerging standard we should natively support (e.g. MCP-as-bus or Skillfold schemas).
- **Ignore**: Out-of-scope or low momentum. Monitor only.

### Step 3: Log in Ecosystem Watch
Append a dated, structured entry to `docs/competitive-analysis/ecosystem-watch.md`.
Use the following format for each finding:

```markdown
### [Feature Name] — [Short Description]
- **Competitor/Source**: [e.g. Anthropic Claude Code v0.5.0]
- **AIDA Classification**: [Compete | Complement | Integrate | Ignore]
- **Technical Analysis**:
  [Explain the architectural difference and AIDA's defensive or cooperative edge]
- **Action & Backlog Loop**:
  [Specify the backlog status, task IDs, or positioning update details]
```

### Step 4: Update Positioning
If the analyzed competitor has a corresponding positioning paper under `docs/positioning/` (e.g., `vs-claude-code-subagents.md`), update it to reflect the latest changes. If it is a new competitor of high strategic interest, file a task to create a new positioning paper.

### Step 5: Close the Loop (Backlog Integration)
Market intelligence is useless without engineering action. For every action-required finding:
1. File a spec in AIDA's database:
   ```bash
   aida queue add --for advisor --title "Assess integration gap with X" --desc "Description of features..."
   # OR
   aida req add --title "support native compilation to format Y"
   ```
2. Link the generated Spec or Task ID directly inside the [ecosystem-watch.md](../../docs/competitive-analysis/ecosystem-watch.md) backlog loop section to ensure a perfect audit trail.
