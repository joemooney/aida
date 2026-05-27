# AI Coding Agents: A Framework Lens

**Date**: 2026-05-26
**Scope**: Single-agent CLI/IDE coding tools — Cline / Aider / Plandex / Goose / Continue. *Adjacent neighbors to AIDA, NOT direct competitors* (different layer in most cases).
**Refresh trigger**: any of the five ships a structured task/spec system, a code-to-task linkage primitive, or unattended drain semantics. Or a new tool enters the top tier (current cutoff: 30K+ GitHub stars).
**Companion to**: [2026-03-17-landscape-scan.md](2026-03-17-landscape-scan.md) (broader landscape, lighter detail) and [2026-05-26-agent-memory-libraries.md](2026-05-26-agent-memory-libraries.md) (memory primitives).

---

## TL;DR

AI coding agents in 2026 mostly fall on one axis (chat-driven coding) and vary along five orthogonal-ish dimensions: **task/spec model**, **state substrate**, **code↔task linkage**, **autonomy gradient**, **multi-agent coordination**. None of the five reviewed here ship anything close to AIDA's combination of *structured spec graph + git-canonical substrate + trace-comment linkage + queue+lease coordination*.

This isn't a "AIDA wins on every axis" framing — most of these tools optimize for **lower friction at single-agent pair-programming**, where AIDA optimizes for **multi-agent coordination + structured memory + lifecycle enforcement**. The honest framing is *different scopes; sometimes both*.

Where AIDA can learn from this layer:
- **Aider's automatic git integration** is the gold standard for "every change becomes a commit" — AIDA partially has this via the `(SPEC-ID)` trailer convention but doesn't auto-commit per turn the way Aider does.
- **Plandex's branched plan + sandbox diff review** maps cleanly onto AIDA's docs/plans/ + worktree model, with one piece missing: AIDA doesn't sandbox the diff before apply.
- **Continue's `.continue/checks/` markdown-as-CI** is a clean pattern AIDA could adopt for the reviewer phase (instead of `claude -p` invocations, declarative check files).

---

## The framework

Five axes describe any AI coding agent:

| Axis | Spectrum |
|---|---|
| **Task/spec model** | none (chat-only) → flat list (todo) → hierarchical (epic/story/task) → typed graph (DAG with edges) |
| **State substrate** | ephemeral conversation → project-local files (`.clinerules`, `.aider.conf.yml`) → git-canonical (substrate-of-truth in version control) |
| **Code↔task linkage** | none → commit messages → conventional commits with REQ-ID → inline trace comments + ID-stable spec graph |
| **Autonomy gradient** | suggest-only → approve-each-action → batched plan w/ checkpoint → unattended drain w/ escalation |
| **Multi-agent coordination** | single-agent → pair (agent + human) → coordinator+specialists → shared substrate with leases + handoffs |

A tool's "shape" in this space is what its design optimizes for. Specs are derived properties.

---

## Tool-by-tool

### Cline (github.com/cline/cline)

VS Code / JetBrains / CLI extension, 59K+ stars, 5M+ installs (March 2026 numbers, likely higher now).

| Axis | Cline's position |
|---|---|
| Task/spec model | **None** (chat-only). `.clinerules` files hold project conventions but aren't tasks. |
| State substrate | Project-local `.clinerules` + checkpoint snapshots (for undo). |
| Code↔task linkage | None — commits are auto-generated, not bound to specs. |
| Autonomy gradient | **Approve-each-action** (per-file edit, per-terminal-command approval) + Plan/Act mode toggle. |
| Multi-agent | Recently added coordinator + specialist subagent pattern. "Team state persists across sessions" per recent docs. |

**Best at**: low-friction pair-programming with strict approval gates. The "every edit needs OK" model is the *most* careful single-user-flow approach.

**AIDA contrast**: Cline's task model is *conversational* — what you're working on is "whatever this chat is about". AIDA's task model is *structured* — what you're working on has a SPEC-ID, a parent, children, status, and survives the chat ending. Different optimizations: Cline minimizes setup friction; AIDA minimizes drift across sessions / agents / time.

### Aider (github.com/paul-gauthier/aider)

Terminal-only pair programming. Mature, focused.

| Axis | Aider's position |
|---|---|
| Task/spec model | **None** (chat-only). |
| State substrate | **Git itself** — every change is auto-committed with a sensible message. Plus the codebase map (LLM-built index). |
| Code↔task linkage | Commit messages are LLM-generated descriptions of the diff — no link back to a spec. |
| Autonomy gradient | Auto-commit per turn (configurable). |
| Multi-agent | Single-agent. |

**Best at**: friction-free chat-to-git. *"Just edit the code and commit"* — the cleanest expression of pair-programming UX on the market.

**AIDA contrast**: Aider's **automatic commit-per-turn** is a strong pattern AIDA partially has via the `(SPEC-ID)` trailer convention, but AIDA expects the operator/agent to write the commit message + tag. Aider proves the auto-commit-per-turn model works without losing operator control. **Opportunity for AIDA**: `aida implement --auto-commit` (or equivalent) that auto-generates commit messages from the diff + appends the active `(SPEC-ID)` trailer. The substrate already knows the scope — adding the trailer is mechanical.

### Plandex (github.com/plandex-ai/plandex)

Terminal-based, 2M token effective context, sandbox diff review, configurable autonomy.

| Axis | Plandex's position |
|---|---|
| Task/spec model | **Plan-centric** — each plan is a structured multi-step task. Plans have version control + branches. |
| State substrate | Local plan directory + git-versioned plan history. |
| Code↔task linkage | Plan → diffs (sandboxed before apply) → optional commit. No explicit spec-ID linkage. |
| Autonomy gradient | **"Configurable autonomy from full automation to granular oversight"**. Sandbox keeps changes uncommitted until approved. |
| Multi-agent | Single-agent, but plan-branches let one operator explore multiple paths. |

**Best at**: large, multi-file changes with reviewable diff before commit. The closest tool reviewed here to AIDA's *"structured artifact + branched exploration"* pattern.

**AIDA contrast**: AIDA's [docs/plans/](../plans/) directory + `aida plan verify` + `docs/plans/_TEMPLATE.md` is the same pattern Plandex implements in-product. AIDA puts plans in version-controlled markdown that travels with the repo; Plandex puts them in a local-cached plan store. **Plandex's sandbox-diff-before-commit is something AIDA doesn't have** — `aida queue work` writes commits directly. **Opportunity**: `aida queue work --dry-run-diff` that surfaces the planned diff before any file edit lands on the worktree. Useful for the headless drain path where the operator can't watch in real time.

### Goose (github.com/block/goose)

Block's open-source agent. Rust-built. CLI + native desktop. 15+ LLM providers.

| Axis | Goose's position |
|---|---|
| Task/spec model | **None** at the CLI level. Recipes (`workflow_recipes/`) are reusable workflow templates, not tasks. |
| State substrate | Per-session, ephemeral by default. Recipes persist. |
| Code↔task linkage | None. |
| Autonomy gradient | Agent-style — installs, executes, edits, tests with operator approval gates. |
| Multi-agent | Single-agent. |

**Best at**: cross-tool orchestration with Rust performance. Goose is more *general-purpose agent* than *coding agent* — its strength is being a thin agent runtime that can execute against many backends.

**AIDA contrast**: Goose recipes are stateless workflow templates — *"how to do X"* in declarative form. AIDA skills (in `.claude/skills/`) are conceptually similar but tied to Claude Code and to AIDA's spec graph. **AIDA could compile to Goose recipes** as a portability layer (per `signals-to-watch.md` already), which would let AIDA's discipline ride on top of Goose for non-Claude operators.

### Continue (github.com/continuedev/continue)

CLI + VS Code / JetBrains extension. **Pivoted to CI-native** in 2026.

| Axis | Continue's position |
|---|---|
| Task/spec model | **Markdown checks** in `.continue/checks/`. Each check is a file. Source-controlled, reviewable. |
| State substrate | The check files themselves — version-controlled in the repo. |
| Code↔task linkage | Implicit via the check definition (a check describes what should be true after work lands). |
| Autonomy gradient | **CI-native** — checks run on every PR as GitHub status checks. Not interactive coding. |
| Multi-agent | N/A — agent-per-check, but no coordination between them. |

**Best at**: enforceable governance — checks become part of the codebase, reviewed like code, run automatically. Different layer from the other four tools.

**AIDA contrast**: Continue's pivot to *"markdown checks as CI"* is a pattern AIDA's reviewer phase could adopt. Currently `aida queue work --auto-complete` invokes `claude -p` for the reviewer phase — a fresh model call each time, with the reviewer skill loaded as a system-prompt addition. **Continue's approach**: ship the check definition AS A FILE in the repo. **AIDA equivalent**: `.aida/reviewers/<spec-pattern>.md` files that describe per-spec-pattern review behavior, scaffolded by `aida init` for common shapes. Less per-PR cost (cache the system prompt + check definition), more declarative governance.

---

## AIDA's position in the framework

| Axis | AIDA |
|---|---|
| **Task/spec model** | Typed graph — EPIC/STORY/TASK/BUG/SPIKE/FOLDER/META/DOC with stable SPEC-IDs, parent/child/subsumes/depends-on edges, status lifecycle (Approved → In Progress → Done → Completed → Released), severity + priority + tags. |
| **State substrate** | **Git-canonical**: YAML files per spec under orphan `aida-store` branch (substrate-of-truth) + SQLite cache (rebuildable read projection). |
| **Code↔task linkage** | Inline `// trace:SPEC-ID \| ai:tool[:confidence]` comments + `(SPEC-ID)` commit-subject trailer. Bidirectional: `aida show SPEC-ID` lists every file referencing it; `grep trace:SPEC-ID` finds the code from the spec side. |
| **Autonomy gradient** | Four-mode ladder: interactive default → `--zen` advisor-on-standby → `--auto-complete` orchestrator drain → `--no-human=both` unattended with advisor escalation. Each phase (impl/CI/review/merge/pull/build) is configurable. |
| **Multi-agent coordination** | Queue + lease + role routing + briefs + handoffs (punt → advisor → resume). Designed for Claude + Codex + Antigravity + Goose sharing the same backlog. |

The differentiator isn't any single axis — every axis has an existing tool that does that ONE thing well. It's the **combination**: structured spec graph + git substrate + trace linkage + queue/lease + lifecycle enforcement, all riding on top of the underlying agent (Claude Code, in current scaffolding).

This is *"different scopes"* in action. AIDA isn't replacing Cline / Aider / Plandex / Goose / Continue. It's the **coordination + memory + lifecycle layer** that runs above whichever single-agent tool is doing the actual editing. AIDA's `aida queue work` invokes `claude` — but the same pattern works with `goose`, `aider`, or `cline` as the implementer, given a thin scaffold.

---

## Opportunities AIDA could pull from this layer

Ranked by leverage × cost:

### 1. Aider-style auto-commit-per-turn

**Pattern**: every implementer turn ends with `git add . && git commit -m "<LLM-generated summary> (TASK-N)"`. Lower friction than the current expect-the-agent-to-commit-explicitly model.

**Scope**: small. Add `aida queue work --auto-commit` flag. Default off; opt-in for "I trust the implementer to commit reasonably".

**Risk**: low. Existing pre-commit hook gates would still fire.

### 2. Plandex-style sandbox diff before apply

**Pattern**: agent proposes a diff; operator reviews + approves before the file actually changes.

**Scope**: medium. Need a buffered-edit primitive (`aida queue work --buffer-edits`) that holds diffs in `.aida/pending-edits/` until acknowledged.

**Risk**: medium — changes the implementer's tool-use shape.

### 3. Continue-style declarative reviewer checks

**Pattern**: per-spec-pattern reviewer behavior defined in markdown files (`.aida/reviewers/<glob>.md`), not via fresh `claude -p` invocations.

**Scope**: medium. Reviewer-phase refactor — load the matching reviewer file as system-prompt context instead of running the generic reviewer skill.

**Risk**: medium — touches the orchestrator's phase 3.

### 4. Compile AIDA's discipline to Goose recipes

**Pattern**: scaffolded `.goose/skills/` files generated from `aida-core/templates/skills/` — port AIDA's session conventions to Goose's recipe format.

**Scope**: medium-to-large. Templated translation per skill. Per `signals-to-watch.md`, this was already on the radar — would unlock AIDA's discipline for non-Claude-Code operators.

**Risk**: low for the build, medium for ongoing maintenance (skill-format drift between the two tools).

### 5. Cline-style coordinator+specialist within a single AIDA session

**Pattern**: implementer agent delegates to specialist subagents (e.g. "have the schema-specialist agent review this YAML"). Cline ships this in 2026.

**Scope**: large. Touches agent-launching, MCP routing, lease semantics, advisory-claim ergonomics.

**Risk**: high. EPIC-shaped — would need master sign-off before scoping.

---

## What's MISSING from this layer (for AIDA to claim)

Three properties AIDA has that none of these tools have:

1. **Stable spec IDs** — Cline / Aider / Plandex / Goose all key tasks by ephemeral identifiers (chat session ID, plan name, recipe slug). AIDA's `SPEC-ID` is stable across refactors, agent handoffs, and years (`(STORY-1)` from 2024 still resolves in 2026).

2. **Cross-agent backlog** — none of these tools assume multiple AI agents share the same backlog. AIDA assumes this from day one: queue routing, role-keyed leases, briefs, punt→advisor→resume.

3. **Lifecycle-as-substrate** — Cline / Aider / Plandex have *workflow state* (working on this plan, editing that file). None have *spec lifecycle* (Approved → In Progress → Done → Completed → Released) as substrate-of-truth that drives behavior. AIDA's auto-bump scanner closes the lifecycle without anyone telling it to.

---

## Refresh signals

- A neighbor ships **structured task tracking** (Cline adding `.cline/tasks/`, Aider growing a backlog primitive, Plandex exposing plans as queryable specs). Refresh: re-categorize on the task/spec model axis.
- A neighbor ships **code↔task linkage** (trace-comment equivalent, or commit-trailer convention adoption). Refresh: AIDA's linkage differentiator narrows.
- A neighbor ships **multi-agent coordination primitives** beyond Cline's coordinator+specialist (lease equivalents, queue routing). Refresh: AIDA's coordination differentiator narrows.
- **Anthropic ships an opinionated workflow** through Claude Code (`/aida-pickup` equivalent, scaffolded skills directory) — this would be the largest signal. AIDA's discipline-pack-as-scaffolding may need to compose rather than compete.

---

## Related

- [`positioning.md`](positioning.md) — AIDA's defensible-niche statement.
- [`2026-03-17-landscape-scan.md`](2026-03-17-landscape-scan.md) — broader landscape with these tools at lighter detail (March 2026 data).
- [`2026-05-26-agent-memory-libraries.md`](2026-05-26-agent-memory-libraries.md) — memory-library category (companion analysis, same date).
- [`../positioning/vs-claude-code-subagents.md`](../positioning/vs-claude-code-subagents.md) — AIDA vs Claude Code's `/agents` catalog.
- [`signals-to-watch.md`](signals-to-watch.md) — already names Goose/Codex skill-registry as a signal worth watching.
