# AIDA Project Evaluation (2026-02-28)

## Scope
This document captures a full project evaluation of `aida` as of **February 28, 2026**, including:
- product and codebase strengths/weaknesses
- technical and operational risks
- fitness for use as a reusable agentic platform
- market comparison against current tools
- prioritized feature roadmap, with focus on Codex compatibility

## Method
- Static review of core modules: `aida-core`, `aida-server`, `aida-cli`, `aida-web-react`
- Build/test checks run locally:
  - `cargo test -p aida-core` (68 passed)
  - `pnpm --filter aida-web-react check` (TypeScript build check passed)
- Market scan using official product documentation and pricing pages (sources listed at end)

## Executive Assessment
AIDA is a serious, high-scope platform with unusually strong domain depth for requirements traceability and multi-interface delivery (CLI + server + desktop + React). It is already useful in practice.

The biggest strategic issue is **platform lock-in to Claude-specific workflows** across scaffolding, skill templates, AI endpoints, and attribution conventions. If your goal is to make this your default substrate for Codex-supported projects, this is solvable but should be treated as a first-class migration initiative, not a small tweak.

## What Is Strong

### 1) High product breadth with real implementation depth
- Workspace has substantial code volume and feature surface.
- Approximate source LOC:
  - `aida-core/src`: 20,544
  - `aida-server/src`: 10,617
  - `aida-cli/src`: 9,112
  - `aida-web-react/src`: 14,039
  - `shared/types.ts`: 1,110
- Total analyzed source lines (core + server + cli + react + shared): ~55,422.

### 2) Clear domain model for requirements engineering
- Rich entities: requirements, custom types, relationships, comments/reactions, trace links, history, queue, baselines.
- Human-friendly SPEC-ID design combined with UUID internals is strong for both UX and data integrity.

### 3) Multi-backend storage architecture is pragmatic
- YAML, SQLite, PostgreSQL supported via shared abstractions.
- Migration paths exist and are tested in core.
- Strong practical value for solo, small-team, and enterprise-adjacent usage.

### 4) Multi-interface strategy is unusually complete
- CLI, gRPC/REST server, desktop/egui app, lightweight WASM client, and React dashboard.
- This makes AIDA resilient to deployment context and user preference.

### 5) Test foundation in `aida-core` is good
- Core tests cover many business-critical paths.
- `cargo test -p aida-core` passed cleanly (warnings only).

## What Is Weak

### 1) Capability mismatch between legacy single-project mode and multi-project mode
This is currently the most important product architecture gap.

Evidence:
- Multi-project router exposes relatively narrow REST surface ([rest.rs](/home/joe/ai/aida/aida-server/src/rest.rs#L31)).
- Legacy router includes far broader v2 endpoints (skills, docs, settings, queue, reload, etc.) ([rest.rs](/home/joe/ai/aida/aida-server/src/rest.rs#L63)).
- Server startup mounts chat/evaluate/skill-runner only in single-project mode ([main.rs](/home/joe/ai/aida/aida-server/src/main.rs#L369), [main.rs](/home/joe/ai/aida/aida-server/src/main.rs#L372), [main.rs](/home/joe/ai/aida/aida-server/src/main.rs#L373)).
- Multi-project mode uses a chat stub that explicitly says unsupported ([chat.rs](/home/joe/ai/aida/aida-server/src/chat.rs#L79)).

Impact:
- Feature behavior depends on deployment mode.
- This increases support load and creates user confusion/regressions.

### 2) Hard Claude coupling across the stack
Evidence:
- Core AI client is Claude-centric and “Direct API” path is not implemented ([client.rs](/home/joe/ai/aida/aida-core/src/ai/client.rs#L228)).
- Server chat/evaluate/skill-chat call Anthropic API directly ([chat.rs](/home/joe/ai/aida/aida-server/src/chat.rs#L191), [evaluate.rs](/home/joe/ai/aida/aida-server/src/evaluate.rs#L118)).
- Scaffolding/templates are Claude-specific (`.claude/*`, `CLAUDE.md`, `ai:claude:*` conventions) ([mod.rs](/home/joe/ai/aida/aida-core/src/scaffolding/mod.rs#L4), [aida-implement.md](/home/joe/ai/aida/aida-core/templates/skills/aida-implement.md#L44)).

Impact:
- Limits adoption where Codex/OpenAI is the default agent.
- Increases migration friction per project.

### 3) Security posture is not production-ready yet
Evidence:
- No authentication/authorization layer is visible on REST endpoints.
- CORS currently allows all origins (`allow_origin(Any)`) and `cors_origins` arg is not used ([main.rs](/home/joe/ai/aida/aida-server/src/main.rs#L162), [main.rs](/home/joe/ai/aida/aida-server/src/main.rs#L235)).
- Admin API-key endpoints are mounted generally; only rebuild endpoint checks dev mode ([admin.rs](/home/joe/ai/aida/aida-server/src/admin.rs#L113), [admin.rs](/home/joe/ai/aida/aida-server/src/admin.rs#L216)).
- Skill runner can execute workspace commands (`cargo clippy`) from HTTP-triggered routes ([skill_runner.rs](/home/joe/ai/aida/aida-server/src/skill_runner.rs#L155), [skill_runner.rs](/home/joe/ai/aida/aida-server/src/skill_runner.rs#L310)).

Impact:
- Unsafe for exposed/shared environments without additional controls.

### 4) Some schema/feature drift is acknowledged but unresolved
Evidence:
- TODOs in DB layer for `weight`, `attachments`, and `gitlab_issues` persistence gaps ([sqlite_backend.rs](/home/joe/ai/aida/aida-core/src/db/sqlite_backend.rs#L461), [sqlite_backend.rs](/home/joe/ai/aida/aida-core/src/db/sqlite_backend.rs#L470), [postgres_backend.rs](/home/joe/ai/aida/aida-core/src/db/postgres_backend.rs#L380)).

Impact:
- Potentially inconsistent behavior across storage backends/features.

## Honest Product Positioning
AIDA is strongest as:
- a requirements-centric engineering operating system for teams that care about traceability and structured planning
- a bridge between human planning artifacts and coding agents

AIDA is weaker as:
- a general-purpose coding agent product competing directly on coding-loop speed with IDE-native agents (Cursor/Windsurf/Copilot/Codex)

The right strategy is not to out-Cursor Cursor. The right strategy is to own requirement traceability + agent orchestration + compliance/review workflows, while integrating whichever coding agent teams choose.

## Market Comparison Snapshot (as of 2026-02-28)

### Key competitor signals
- **OpenAI Codex**: cloud coding agent with task environment + PR-style flow, model-level options (`codex-mini-latest`, etc.).
- **Claude Code**: mature terminal-agent workflows, explicit docs around settings, hooks, slash commands, costs.
- **GitHub Copilot**: deeply integrated with GitHub ecosystem and broad pricing tiers.
- **Cursor / Windsurf / Cline / Devin**: varying mixes of IDE-native autonomy, agent mode, and “delegate task” positioning.

### Implication for AIDA
To compete effectively, AIDA should be **agent-provider neutral by architecture**, with Claude and Codex both first-class. Your moat should be orchestration, context, traceability, and project memory, not single-model lock-in.

## Codex-First Adaptation Plan

### Priority A: Abstract AI provider layer (must-do)
Build a provider interface used by `chat`, `evaluate`, `skill_runner`, and scaffolding.

Suggested shape:
- `AiProvider` trait in `aida-core` with capabilities:
  - streaming chat
  - non-streaming structured eval
  - model list / health
  - optional tool-calling support
- provider implementations:
  - `AnthropicProvider`
  - `OpenAIProvider` (Codex + general models)
- config resolution precedence:
  - project settings -> env vars -> defaults

### Priority B: Replace Claude-only scaffolding with multi-agent scaffolding
- Keep `.claude/*` generation as one profile.
- Add `.codex/*` profile (or AGENTS.md profile templates) with equivalent workflows.
- Convert `ai:claude:*` attribution to normalized `ai:<provider>:<confidence>` with provider enum.

### Priority C: Unify API capability across project modes
- Remove the “legacy router as superset” pattern.
- Use one v2 capability map for both single and multi-project.
- Add project scoping uniformly, instead of removing endpoints by mode.

### Priority D: Security baseline for server mode
- Add authn/authz (API key or JWT minimum).
- Restrict CORS by config; actually honor `cors_origins` CLI arg.
- Gate dangerous admin/skill actions behind explicit admin auth and environment allowlists.

## Feature Additions Worth Building Next

### Near-term (2-4 weeks)
1. Provider abstraction + OpenAI/Codex backend (chat + evaluation parity).
2. Multi-project parity for `/api/v2/chat`, `/api/v2/skills/*`, `/api/v2/requirements/:id/evaluate`.
3. Server auth MVP + CORS hardening.
4. Fix known schema TODOs for data fidelity (`attachments`, `weight`, `gitlab_issues`).

### Mid-term (1-2 quarters)
1. Agent-run ledger: every AI action gets durable audit metadata (who, model, prompt hash, changed artifacts).
2. Policy engine: require requirement linkage before merge/close; enforce by repo hooks/CI.
3. Multi-repo program view: cross-project requirement graph and dependency risk dashboard.
4. Native GitHub/Jira connectors (GitLab already present).

### Strategic (2+ quarters)
1. Agent evaluation framework: benchmark providers on requirement quality tasks (duplicate detection, relationship suggestion, requirement rewrite quality).
2. Memory + retrieval layer tuned to spec/history semantics.
3. Enterprise controls: SSO, RBAC, encryption-at-rest options, audit export formats.

## Recommended Execution Sequence
1. **Stabilize platform core**: mode parity + security baseline.
2. **De-risk provider dependence**: implement provider abstraction and Codex support.
3. **Productize differentiation**: traceability governance and agent auditability.
4. **Scale adoption**: integration ecosystem (GitHub/Jira) and team admin controls.

## Practical Risks to Track
- API drift between frontend expectations and server mode-specific routes.
- Security incidents if server is exposed without auth/CORS restrictions.
- Migration complexity if provider abstraction is delayed and Claude assumptions keep spreading.
- Maintenance burden from parallel UI stacks (egui desktop + React) without strict boundary ownership.

## Quality Notes from Local Verification
- `aida-core` tests: 68 passed.
- `aida-web-react` TS check: passed.
- Warnings remain in `aida-core` (unused code/vars); not release-blocking but should be cleaned.

## Bottom Line
AIDA already has the hardest part done: a strong requirements data model with working multi-interface implementations. The next step is architectural hardening:
- unify mode capabilities
- secure server operations
- make AI provider integration pluggable

If you do those three, AIDA can become a durable control plane for agentic software delivery across Claude, Codex, and whatever comes next.

---

## External Sources
- OpenAI API pricing: https://openai.com/api/pricing/
- OpenAI model docs (Codex model docs): https://platform.openai.com/docs/models
- OpenAI cookbook codex examples: https://cookbook.openai.com/examples/gpt-5/codex_sdk_
- Anthropic Claude Code docs hub: https://docs.anthropic.com/en/docs/claude-code
- Anthropic Claude Code costs: https://docs.anthropic.com/en/docs/claude-code/costs
- Anthropic Claude Code settings: https://docs.anthropic.com/en/docs/claude-code/settings
- GitHub Copilot plans/pricing: https://github.com/features/copilot/plans
- Cursor docs (Agent): https://docs.cursor.com/agent
- Windsurf pricing: https://windsurf.com/pricing
- Cline (README): https://github.com/cline/cline
- Cognition (Devin): https://www.cognition.ai/
