# Plan: Web UI Skill Invocation (Pilot: `/aida-compiler-warnings`)

## Context

AIDA skills are currently Claude Code prompts that can only be run from the CLI. This plan adds the ability to invoke skills from the React dashboard, starting with `/aida-compiler-warnings` as a pilot.

## Related Requirements

- Skill runner infrastructure for web UI
- Compiler warnings analysis and categorization
- Action handlers (auto-fix, create defect/task)
- Context-aware AI chat for skill results

## Implementation

### Phase 1: Server — `aida-server/src/skill_runner.rs`
- SSE streaming endpoint: `POST /api/v2/skills/:name/run`
- Action endpoint: `POST /api/v2/skills/:name/action`
- Chat endpoint: `POST /api/v2/skills/:name/chat`
- Clippy JSON parsing and warning categorization by risk level
- Auto-fix via `cargo clippy --fix`
- Create defect/task requirements from warnings

### Phase 2: React UI
- `aida-web-react/src/api/skillRunner.ts` — API client
- `aida-web-react/src/hooks/useSkillRunner.ts` — SSE hook
- `aida-web-react/src/components/skills/WarningsReport.tsx` — Structured results display
- `aida-web-react/src/components/skills/SkillRunnerPanel.tsx` — Slide-out runner panel
- `aida-web-react/src/components/skills/SkillCard.tsx` — Added "Run" button
- `aida-web-react/src/components/skills/SkillsView.tsx` — Wired up runner panel

### Phase 3: Chat Integration
- `aida-web-react/src/components/skills/SkillChat.tsx` — Context-aware AI chat
- Server-side chat endpoint with warnings report as system context
- Starter questions and streaming responses

## Status

Completed
