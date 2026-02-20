# Skills Browser View

## Context
AIDA has 15 skills and 13 commands that are symlinked to master templates. This plan adds a web UI to browse, view, and edit them.

## Phases
1. Backend - Skills API endpoints (Rust) in aida-server/src/rest.rs
2. Frontend API + Hooks in aida-web-react/src/api/skills.ts and hooks/useSkills.ts
3. Skills View Components (SkillsView, SkillCard, SkillDetailPanel)
4. Routing + Sidebar integration

## Related Requirements
- Skills browser feature for AIDA web dashboard

## Status
Completed
