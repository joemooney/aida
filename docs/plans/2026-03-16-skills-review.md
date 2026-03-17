# Skills Review: mattpocock/skills Repository Analysis

**Date**: 2026-03-16
**Source**: https://github.com/mattpocock/skills/tree/main
**Purpose**: Identify patterns, ideas, and skills worth adapting for AIDA

## Related Requirements

- General AIDA skill system improvement
- Template architecture in `aida-core/templates/skills/`

## Status

Completed (analysis only, no implementation)

---

## 1. Inventory of Skills Found

The repository contains **17 skills** organized into categories. Each skill follows a consistent structure: a `SKILL.md` file with YAML frontmatter (`name`, `description`) and step-by-step workflow instructions. Some skills include supplementary reference files.

### Planning & Design (5 skills)

#### write-a-prd
Creates product requirement documents through an interactive interview process. The key workflow:
1. Get a detailed problem description from the user
2. Explore the codebase to verify assertions
3. **Interview relentlessly** about every aspect, walking each branch of the design tree
4. Sketch major modules, looking for "deep modules" (small interface, large implementation)
5. Write PRD using a structured template and submit as a GitHub issue

The PRD template includes: Problem Statement, Solution, User Stories (extensive numbered list), Implementation Decisions, Testing Decisions, Out of Scope, and Further Notes. Notably, it explicitly says "Do NOT include specific file paths or code snippets. They may end up being outdated very quickly."

#### prd-to-issues
Breaks a PRD into independently-grabbable GitHub issues using **vertical slices** (tracer bullets). Key concepts:
- Each issue is a thin vertical slice cutting through ALL layers end-to-end (schema, API, UI, tests)
- Slices classified as **HITL** (human-in-the-loop) or **AFK** (can be done autonomously)
- Structured issue template with: Parent PRD reference, What to Build, Acceptance Criteria, Blocked By, User Stories Addressed
- Iterative user approval of the breakdown before creating issues

#### prd-to-plan
Similar to prd-to-issues but outputs a Markdown plan file to `./plans/` instead of GitHub issues. Emphasizes "durable decisions" (routes, schema shapes, data models) over implementation specifics.

#### grill-me
Minimal but powerful: interviews the user relentlessly about a plan or design until reaching shared understanding. Walks each branch of the decision tree. If a question can be answered by exploring the codebase, it explores instead of asking.

#### design-an-interface
Based on "Design It Twice" from John Ousterhout's "A Philosophy of Software Design." Spawns 3+ parallel sub-agents, each producing a **radically different** interface design under different constraints (minimize methods, maximize flexibility, optimize for common case, ports & adapters). Compares designs and gives an opinionated recommendation.

### Development (3 skills)

#### tdd
Test-driven development with strict red-green-refactor vertical slices. Includes 5 supplementary reference files:
- `tests.md` - Test examples
- `mocking.md` - Mocking guidelines
- `deep-modules.md` - Deep vs shallow module concepts
- `interface-design.md` - Interface design for testability
- `refactoring.md` - Refactoring patterns

Key anti-pattern called out: **horizontal slicing** (writing all tests first, then all implementation). Correct approach is vertical: one test, one implementation, repeat. Each cycle responds to what was learned in the previous cycle.

Planning phase asks user to confirm which behaviors to test -- "You can't test everything."

#### triage-issue
Systematic bug investigation workflow:
1. Get brief problem description
2. Deep codebase exploration (source, deps, tests, recent changes, error handling)
3. Root cause analysis (regression vs missing feature vs design issue)
4. TDD fix plan with ordered RED-GREEN cycles
5. Creates a GitHub issue with the analysis and fix plan **without asking for review first**

#### improve-codebase-architecture
Explores codebase organically looking for architectural friction, then proposes module-deepening refactors. Key innovation:
- Uses the concept of **dependency categories**: In-process, Local-substitutable, Remote-but-owned (Ports & Adapters), True external (Mock)
- Spawns parallel sub-agents for competing interface designs
- Issues created as RFC-style GitHub issues
- Philosophy: "The friction you encounter IS the signal"
- Testing strategy: "replace, don't layer" -- delete old shallow tests when boundary tests exist

### Tooling & Setup (2 skills)

#### setup-pre-commit
Opinionated setup for Husky + lint-staged + Prettier pre-commit hooks in Node.js projects. Detects package manager, installs deps, creates config files, verifies, and commits.

#### git-guardrails-claude-code
Installs a Claude Code PreToolUse hook that blocks dangerous git commands (`git push`, `git reset --hard`, `git clean -f`, `git branch -D`, `git checkout .`). Includes a bash script that intercepts tool calls via JSON/jq parsing.

### Writing (2 skills)

#### write-a-skill
Meta-skill for creating new skills. Provides a template structure (`SKILL.md`, `REFERENCE.md`, `EXAMPLES.md`, `scripts/`). Key insight: the description field is "the only thing your agent sees" when deciding which skill to load -- it must include triggers. Rules: SKILL.md under 100 lines, split files for distinct domains, add scripts for deterministic operations.

#### edit-article
Treats article content as a directed acyclic graph of information dependencies. Restructures sections respecting dependency order, then rewrites each section with a 240-character paragraph limit.

### Domain-Specific (5 skills)

#### ubiquitous-language
Extracts domain terminology from conversations into a structured `UBIQUITOUS_LANGUAGE.md` glossary. Picks canonical terms and relegates alternatives to an "avoid" list. On re-invocation, reads existing file and marks changes as "(updated)" or "(new)".

#### request-refactor-plan
Creates detailed refactor plans broken into tiny commits (Martin Fowler's advice). Interviews user, explores codebase, checks test coverage, then creates a GitHub issue with: Problem Statement, Solution, Commits (tiny incremental plan), Decision Document, Testing Decisions, Out of Scope.

#### migrate-to-shoehorn
TypeScript-specific: migrates test files from `as` type assertions to `@total-typescript/shoehorn`. Domain-specific, not relevant to AIDA.

#### scaffold-exercises
Creates exercise directory structures for a training platform. Domain-specific, not relevant to AIDA.

#### obsidian-vault
Manages notes in an Obsidian vault. Domain-specific, not relevant to AIDA.

---

## 2. Patterns and Ideas Worth Adopting

### Pattern A: Relentless Interviewing / Decision Tree Walking

The `grill-me` and `write-a-prd` skills share a powerful pattern: **interview the user relentlessly**, walking each branch of the design tree and resolving dependencies between decisions one-by-one. This is more structured than simply "ask clarifying questions." It treats the design space as a tree to be exhaustively explored.

**Current AIDA gap**: `/aida-req` adds requirements but does not deeply interrogate the user about the design space. `/aida-plan` decomposes but does not stress-test assumptions.

### Pattern B: Vertical Slices / Tracer Bullets

Multiple skills (`prd-to-issues`, `tdd`, `triage-issue`) consistently use vertical slicing -- thin end-to-end cuts through all layers rather than horizontal layer-by-layer work. The HITL/AFK classification for slices is particularly useful for AI-assisted workflows (knowing which slices the agent can do autonomously vs which need human decisions).

**Current AIDA gap**: `/aida-plan` decomposes into child requirements but does not enforce vertical slice thinking. Child requirements could easily become horizontal slices (e.g., "build the data model", "build the API", "build the UI") rather than vertical ones.

### Pattern C: Parallel Sub-Agent Interface Design

`design-an-interface` and `improve-codebase-architecture` spawn 3+ sub-agents in parallel with different design constraints. This "Design It Twice" pattern produces genuinely different solutions rather than variations on the first idea.

**Current AIDA gap**: No skill uses parallel competing approaches for design exploration.

### Pattern D: Deep Module Philosophy

The consistent thread of "deep modules" (small interface hiding complex implementation) from Ousterhout's book runs through `tdd`, `improve-codebase-architecture`, and `design-an-interface`. This provides a concrete, opinionated framework for evaluating architecture quality.

**Current AIDA gap**: `/aida-review` checks trace coverage but does not evaluate architectural quality of the implementation.

### Pattern E: HITL/AFK Classification

The `prd-to-issues` skill classifies each work item as requiring human interaction (HITL) or being doable autonomously (AFK). This is directly relevant to AI-native tools where some tasks can be delegated to agents.

**Current AIDA gap**: Requirements have no field indicating whether they can be autonomously implemented by an AI agent.

### Pattern F: Structured Issue/Plan Templates with "No File Paths"

The PRD template explicitly says "Do NOT include specific file paths or code snippets. They may end up being outdated very quickly." Implementation decisions are captured at the architectural level, not the file level. This produces more durable documentation.

**Current AIDA gap**: `/aida-plan` records file paths in comments. These become stale quickly.

### Pattern G: Codebase Exploration Before Planning

Every planning/design skill explores the codebase first to verify assumptions and understand current state. The `improve-codebase-architecture` skill treats the friction encountered during exploration as the primary signal.

**Current AIDA gap**: `/aida-plan` does scope analysis but does not mandate codebase exploration first.

### Pattern H: Git Safety Guardrails via PreToolUse Hooks

The `git-guardrails-claude-code` skill uses Claude Code's hook system to prevent destructive git operations. This is a simple bash script that parses tool input JSON and blocks dangerous patterns.

**Current AIDA gap**: AIDA's `.claude/hooks/` focuses on commit validation, not on preventing destructive operations.

### Pattern I: Ubiquitous Language Extraction

The `ubiquitous-language` skill formalizes domain terminology into a structured glossary, picking canonical terms and tracking what to avoid. On re-invocation it incrementally updates rather than rebuilding from scratch.

**Current AIDA gap**: No skill manages domain terminology. For a requirements tool, consistent terminology across requirements is critical.

---

## 3. Concrete Proposals

### Proposal 1: New Skill -- `/aida-grill` (Decision Tree Interrogation)

**Inspiration**: `grill-me`

**Purpose**: Stress-test a requirement, plan, or design decision by walking every branch of the decision tree. Unlike `/aida-evaluate` (which scores quality), this skill conducts an interactive adversarial interview.

**Workflow**:
1. Load the requirement or accept a free-form plan description
2. Interview the user about every assumption, dependency, and edge case
3. When a question can be answered by exploring the codebase, explore instead of asking
4. Resolve each branch before moving to the next
5. When all branches are resolved, summarize decisions and optionally update the requirement or add comments

**Value**: Catches design gaps before implementation begins. The relentless interview format forces the user to think through edge cases they would otherwise skip.

**Implementation effort**: Low -- primarily a prompt-engineering skill with no CLI changes needed.

### Proposal 2: New Skill -- `/aida-decompose` (Vertical Slice Breakdown)

**Inspiration**: `prd-to-issues`, `prd-to-plan`

**Purpose**: Decompose a requirement into vertical slices (tracer bullets) rather than horizontal layers. Each slice cuts through all layers end-to-end and is independently demoable.

**Workflow**:
1. Load the parent requirement
2. Explore the codebase to understand affected layers
3. Draft vertical slices, each with: title, HITL/AFK classification, dependencies on other slices, acceptance criteria
4. Present to user for approval (granularity check, dependency check, HITL/AFK accuracy)
5. Create child requirements in AIDA with appropriate relationships and tags
6. Optionally tag children as `ai:autonomous` (AFK) or `ai:needs-review` (HITL)

**How it differs from `/aida-plan`**: The current plan skill decomposes into child requirements but does not enforce vertical slice thinking or HITL/AFK classification. `/aida-decompose` would be invoked *during* planning as a decomposition strategy.

**Implementation effort**: Medium -- new skill file, possibly new tags for HITL/AFK classification.

### Proposal 3: New Skill -- `/aida-architecture` (Codebase Health Assessment)

**Inspiration**: `improve-codebase-architecture`

**Purpose**: Explore the AIDA codebase (or any project using AIDA) organically, surface architectural friction, and propose module-deepening refactors filed as requirements.

**Workflow**:
1. Explore the codebase looking for friction (shallow modules, tight coupling, untestable code, files that must be read together)
2. Present numbered list of deepening opportunities with dependency categories
3. User picks a candidate
4. Frame the problem space with constraints
5. (Optionally) spawn parallel approaches for interface design
6. Create a requirement of type `task` or `spike` with the refactor proposal

**Adaptation for AIDA**: Instead of GitHub issues, this creates AIDA requirements. The dependency categories (in-process, local-substitutable, ports & adapters, true external) can be recorded as tags or comments.

**Implementation effort**: Medium -- new skill file, reference material.

### Proposal 4: Enhance `/aida-plan` with Vertical Slice Enforcement

**Inspiration**: `prd-to-issues` vertical slice rules

**Changes to existing `/aida-plan`**:
1. Add a "Slice Check" step after decomposition: verify each child requirement is a vertical slice (crosses multiple layers) rather than a horizontal slice (one layer only)
2. Warn if children look like horizontal slices (e.g., all children are backend-only or all are frontend-only)
3. Add HITL/AFK classification to each child
4. Avoid recording file paths in comments (they go stale); instead record architectural decisions at the module/interface level
5. Add a "durable decisions" section that captures routes, schema shapes, data models, and integration boundaries

**Implementation effort**: Low -- edit existing `aida-plan.md` template.

### Proposal 5: Enhance `/aida-review` with Architectural Quality Check

**Inspiration**: `improve-codebase-architecture`, deep module philosophy

**Changes to existing `/aida-review`**:
1. Beyond trace coverage, assess whether the implementation creates deep or shallow modules
2. Check if new interfaces are testable (accept dependencies, return results, small surface area)
3. Flag if new code introduces tight coupling between modules
4. Suggest interface improvements if modules are too shallow

**Implementation effort**: Low -- add steps to existing `aida-review.md` template.

### Proposal 6: New Skill -- `/aida-glossary` (Ubiquitous Language)

**Inspiration**: `ubiquitous-language`

**Purpose**: Extract and maintain canonical domain terminology across all requirements. Ensures consistent language in requirement titles, descriptions, and acceptance criteria.

**Workflow**:
1. Scan all requirements for domain-relevant terms
2. Identify problems: synonyms used for same concept, ambiguous terms, inconsistent naming
3. Propose canonical glossary with opinionated choices
4. Save as `GLOSSARY.md` or as a `meta` requirement in the database
5. On re-invocation, read existing glossary and incrementally update with "(new)" and "(updated)" markers
6. Optionally flag requirements that use non-canonical terms

**Value for AIDA**: As a requirements management tool, terminology consistency is critical. Requirements that use "item" in one place and "requirement" in another for the same concept create confusion. This skill would surface and resolve those inconsistencies.

**Implementation effort**: Medium -- new skill file, optionally a new CLI command for glossary management.

### Proposal 7: New Skill -- `/aida-triage` (Bug Investigation)

**Inspiration**: `triage-issue`

**Purpose**: Systematically investigate a reported bug, analyze root cause, and create a bug requirement with a TDD fix plan.

**Workflow**:
1. Get brief problem description from user
2. Deep codebase exploration (source, deps, tests, recent changes, error handling)
3. Root cause analysis: regression, missing feature, or design issue
4. Create a bug requirement in AIDA with: problem description, root cause, TDD fix plan (ordered RED-GREEN cycles), acceptance criteria
5. Link to related requirements if the bug stems from an incomplete implementation

**How it differs from manually adding a bug**: The triage skill does the investigation work and produces a structured analysis, not just a bug report.

**Implementation effort**: Medium -- new skill file.

### Proposal 8: Adopt Git Safety Guardrails in `aida init`

**Inspiration**: `git-guardrails-claude-code`

**Purpose**: When `aida init` sets up a project, optionally install a Claude Code PreToolUse hook that blocks dangerous git operations.

**Changes**:
1. Add a `block-dangerous-git.sh` script to `aida-core/templates/hooks/`
2. During `aida init`, create `.claude/settings.json` (or update it) with a PreToolUse hook entry
3. Add `--no-guardrails` flag to skip this during init

**Value**: Prevents AI agents from accidentally force-pushing, hard-resetting, or deleting branches. This is especially important in AI-native workflows where the agent has broad tool access.

**Implementation effort**: Low -- one shell script and a settings.json template update.

### Proposal 9: Enhance `/aida-test` with TDD Vertical Slice Workflow

**Inspiration**: `tdd` skill

**Changes to existing `/aida-test`**:
1. Enforce vertical slice testing: one test at a time, implement to pass, repeat
2. Add anti-pattern warning: do not write all tests first (horizontal slicing)
3. Add planning phase where user confirms which behaviors to test (prioritize, don't test everything)
4. Include refactoring step after all tests pass
5. Add checklist per cycle: "Test describes behavior, not implementation", "Test uses public interface only", "Test would survive internal refactor"

**Implementation effort**: Low -- edit existing `aida-test.md` template.

### Proposal 10: Adopt "Write a Skill" Meta-Skill Pattern

**Inspiration**: `write-a-skill`

**Purpose**: `/aida-new-skill` would help users create custom skills for their AIDA-managed projects. This is useful when teams want project-specific workflows beyond the standard AIDA skills.

**Workflow**:
1. Interview user about: what task/domain, specific use cases, need for scripts, reference materials
2. Draft the skill following AIDA's template structure
3. Review with user
4. Save to `aida-core/templates/skills/` (for AIDA development) or `.claude/skills/` (for user projects)

**Value**: Makes the skill system self-extending. Teams can create domain-specific skills without understanding the template architecture.

**Implementation effort**: Low -- new skill file.

---

## 4. Priority Ranking

| Priority | Proposal | Effort | Impact |
|----------|----------|--------|--------|
| 1 | P4: Enhance `/aida-plan` with vertical slices | Low | High -- fixes a fundamental decomposition weakness |
| 2 | P1: `/aida-grill` (decision tree interrogation) | Low | High -- catches design gaps early, low effort |
| 3 | P8: Git safety guardrails in `aida init` | Low | High -- prevents destructive operations in AI workflows |
| 4 | P9: Enhance `/aida-test` with TDD vertical slices | Low | Medium -- improves test quality guidance |
| 5 | P5: Enhance `/aida-review` with architecture check | Low | Medium -- adds depth to code review |
| 6 | P2: `/aida-decompose` (vertical slice breakdown) | Medium | High -- but overlaps with enhanced P4 |
| 7 | P7: `/aida-triage` (bug investigation) | Medium | Medium -- structured bug analysis |
| 8 | P6: `/aida-glossary` (ubiquitous language) | Medium | Medium -- terminology consistency |
| 9 | P3: `/aida-architecture` (codebase health) | Medium | Medium -- architectural quality |
| 10 | P10: `/aida-new-skill` (meta-skill) | Low | Low -- nice to have for extensibility |

---

## 5. Key Takeaways

1. **Vertical slices over horizontal layers** is the single most impactful pattern to adopt. It affects planning, testing, and implementation. AIDA's current decomposition approach does not enforce this.

2. **Interactive interrogation** (grill-me pattern) is fundamentally different from quality scoring (aida-evaluate). Both are valuable but serve different purposes. AIDA should have both.

3. **HITL/AFK classification** is uniquely relevant to AI-native tools. Knowing which tasks an agent can do autonomously vs which need human review is essential for effective AI-assisted development.

4. **Durable documentation** that avoids file paths and code snippets (focusing on architectural decisions, interfaces, and module responsibilities) ages much better than file-level implementation notes.

5. **Git safety guardrails** via PreToolUse hooks are a simple, high-impact addition that any AI-native tool should ship by default.

6. **Supplementary reference files** (as seen in the TDD skill with 5 additional .md files) allow keeping the main SKILL.md concise while bundling deep reference material. AIDA skills currently use single files; some could benefit from this pattern.
