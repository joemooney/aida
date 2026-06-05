# SPIKE-16: Claude Code Skills System — Frontmatter Schema and Lifecycle Integration

**Date**: 2026-06-05
**Source-verified**: Yes — verified CLI command parameters using `claude --help` (version 2.1.162); cross-referenced official docs at [code.claude.com/docs/en/skills](https://code.claude.com/docs/en/skills) and custom skill implementations.
**Verdict**: **COMPOSE + ALIGN** — AIDA should leverage the frontmatter schema fields (`allowed-tools`, `disallowed-tools`, `disable-model-invocation`) in its own `.md` skill templates, providing first-class generation compatibility when exporting AIDA skills to Claude Code's native format.

---

## 1. Skill Frontmatter Schema Fields

Claude Code skill templates (usually placed in `~/.claude/skills/` or project `.claude/skills/` as `SKILL.md` files) define execution constraints using YAML frontmatter. The complete frontmatter fields are:

| Field Name | Type | Purpose | Behavior |
|---|---|---|---|
| `name` | String | Unique identifier | Registered as `/name` slash command in the interactive session. |
| `description` | String | Skill summary | Displayed in slash menus and used by the model for semantic auto-triggering. |
| `allowed-tools` | List/String | Tool restriction | Limits Claude to the specified subset of tools (e.g., `Read`, `Grep`) and auto-approves them. |
| `disallowed-tools` | List/String | Tool restriction | Explicitly blocks specific tools (e.g., `Bash`, `Write`) from execution while active. |
| `disable-model-invocation` | Boolean | Selection control | If `true`, stops Claude from choosing to run the skill autonomously (user slash-command execution only). |
| `user-invocable` | Boolean | Menu visibility | If `false`, hides the skill from slash commands but allows Claude to invoke it autonomously. |
| `effort` | String | Thinking effort | Sets the `CLAUDE_EFFORT` level (e.g., `low`, `medium`, `high`) during the skill turn. |

---

## 2. Skills System Features and Lifecycle

*   **Helper Subfolders**: Skills can be modularized by placing supportive resources under subfolders (such as `templates/` for prompt fragments and `examples/` for few-shot demonstrations). This keeps the main `SKILL.md` file focused on core execution instructions.
*   **Dynamic Reloading (`/reload-skills`)**: Introduced in 2.1.152 to avoid restarting the CLI. Forces Claude Code to scan skill directories mid-session and update active prompts. Alternatively, a `SessionStart` hook can return `{ "reloadSkills": true }` to automate the refresh on startup.
*   **Effort Context Exposure (`CLAUDE_EFFORT`)**: Exposes the active effort level (`low`, `medium`, `high`, `xhigh`) to sub-processes (like hook scripts and `Bash` tool runs), allowing scripts to prune expensive execution paths when budget is low.
*   **Telemetry tracking**: Fires `skill_activated` OpenTelemetry events to monitor skill performance, duration, and token usage.

---

## 3. Comparative Map: AIDA vs. Claude Code Skills

| Dimension | AIDA Templates (`aida-core/templates/skills/`) | Claude Code Skills | Gap / Action |
|---|---|---|---|
| **Storage Location** | Committed in repository source | `.claude/skills/` and `~/.claude/skills/` | AIDA must compile/sync templates to `.claude/skills/` on init/sync. |
| **Tool Restrictions** | Documented in markdown text | Schema-enforced (`allowed-tools` / `disallowed-tools`) | AIDA must map text directives to schema fields during generation. |
| **Model Disablement** | Checked by human review | Schema-enforced (`disable-model-invocation`) | Critical for purely mechanical tasks (e.g. format runs). |
| **Reloading** | Manual process restart | Command-driven (`/reload-skills`) | AIDA hooks should leverage `reloadSkills: true`. |

---

## 4. Integration Recommendations for AIDA

1.  **Enhance AIDA Frontmatter Generation**
    *   Update AIDA's skill scaffold generation to inject YAML frontmatter containing `allowed-tools` and `disallowed-tools` to match the target CLI capability.
    *   For security-sensitive tasks (e.g., code audits or documentation review), automatically populate `disallowed-tools: ["Bash", "Write", "Edit"]` to prevent unauthorized file mutations.

2.  **Support Helper Folders**
    *   Align AIDA's template builder to recognize and copy nested `templates/` and `examples/` helper folders from `aida-core/templates/skills/` into the destination workspace `.claude/skills/` subdirectory.

3.  **Automate Reloading in AIDA Hooks**
    *   Configure AIDA's default `SessionStart` hook script to return `reloadSkills: true` whenever local `.claude/skills/` files differ from the main template source, ensuring immediate propagation of updates without manual operator intervention.
