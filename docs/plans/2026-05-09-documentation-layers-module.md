# Documentation Layers: a proposed `aida-docs` module

## What the research says

Three established frameworks compose, each addressing a different axis. None of them alone is enough; together they describe how mature engineering orgs (and the recent AI-agent-aware extensions of those orgs) organize project understanding.

### 1. Diátaxis (the *user need* axis)

Daniele Procida's framework — adopted by Canonical, Python, Django, Cloudflare. Documentation is organized by the **need the reader brings**, not by the topic:

| Quadrant | User need | Form |
|---|---|---|
| **Tutorials** | Learning | Hand-held, do-this-and-see-this |
| **How-to guides** | Task accomplishment | "I have a goal, give me steps" |
| **Reference** | Information lookup | Facts, schemas, API surface |
| **Explanation** | Understanding | "Why is this the way it is?" |

Diátaxis is the *navigation* layer — it tells you what TYPE of doc to write for what reader. It says nothing about what content goes inside.

### 2. arc42 + C4 (the *structural* axis)

arc42 is the de-facto template for *architecture* documentation; C4 is the de-facto *visual* model. They compose: arc42 sections 5–7 are where C4 diagrams live.

**arc42's 12 sections, in order:**

| # | Section | What it captures |
|---|---|---|
| 1 | Introduction & Goals | Fundamental requirements, quality goals, target audience |
| 2 | Constraints | Regulatory / technical / organizational constraints |
| 3 | Context & Scope | External systems, interfaces, system boundaries |
| 4 | Solution Strategy | Core ideas + fundamental approach |
| 5 | Building Block View | Static structure (modules, sub-systems) |
| 6 | Runtime View | Important runtime scenarios |
| 7 | Deployment View | Hardware, infrastructure |
| 8 | Crosscutting Concepts | Patterns applied across blocks |
| 9 | Architectural Decisions | Important decisions (often ADR-formatted) |
| 10 | Quality Requirements | Quality tree, quality scenarios |
| 11 | Risks & Technical Debt | Known problems, mitigation status |
| 12 | Glossary | Domain terms, ubiquitous language |

Sections 1–4 = problem space ("what / why"). Sections 5–12 = solution space ("how"). The split matters: it's the same split between vision/principles and architecture/implementation.

**C4's four levels** (zoom hierarchy):

| Level | Audience | Shows |
|---|---|---|
| 1 — System Context | Anyone | The system as a black box + external actors |
| 2 — Container | Engineers | Apps / services / data stores |
| 3 — Component | Engineers | Internal building blocks per container |
| 4 — Code | Engineers | Class / function structure (rarely used) |

### 3. ADRs + Constitution + Living Architecture (the *time / governance* axis)

| Pattern | Captures | Lifetime |
|---|---|---|
| **Constitution** (Spec Kit, CLAUDE.md, *Anthropic's [Claude's Constitution](https://www.anthropic.com/constitution)*) | Non-negotiable principles | Immutable; rarely changes |
| **ADRs** (Michael Nygard, 2011) | A single decision + rationale + status | Append-only; superseded, not edited |
| **Living Architecture** ([ceaksan.com](https://ceaksan.com/en/living-architecture-ai-architectural-documentation)) | Project's *current* structural reality | Updated continuously |

Constitution = the rules that govern HOW you decide. ADRs = the trail of WHAT you decided. Living Architecture = a snapshot of WHERE you are now. They're complementary and load-bearing for AI agents specifically — agents that don't see them re-derive every decision and re-rebuild every mental model on every session.

---

## The synthesis: layered understanding

Stack the three axes and you get a single hierarchy that maps to how readers (human and agent) actually need to understand a project:

```
┌─────────────────────────────────────────────────────┐
│  CONSTITUTION (non-negotiable principles)           │  ← rarely changes, immutable
├─────────────────────────────────────────────────────┤
│  VISION & GOALS (what we're building, for whom)     │  ← changes with strategy
├─────────────────────────────────────────────────────┤
│  CONSTRAINTS (regulatory, technical, external)      │  ← changes with environment
├─────────────────────────────────────────────────────┤
│  CONTEXT & SCOPE (boundaries, external systems)     │  ← changes with integrations
├─────────────────────────────────────────────────────┤
│  ARCHITECTURE                                       │
│   ├─ Building blocks (modules, components)         │
│   ├─ Runtime (key scenarios, data flow)            │
│   ├─ Deployment (where it runs)                    │  ← living, current truth
│   ├─ Crosscutting (patterns)                       │
│   └─ Quality (perf, security, reliability targets) │
├─────────────────────────────────────────────────────┤
│  DECISIONS (ADRs — the audit trail of how we got   │  ← append-only, temporal
│  to today's architecture)                          │
├─────────────────────────────────────────────────────┤
│  REQUIREMENTS / FEATURES / WORK ITEMS              │  ← AIDA's existing graph
│  (what's planned, in progress, done)               │
├─────────────────────────────────────────────────────┤
│  RISKS & TECH DEBT (known issues)                  │
├─────────────────────────────────────────────────────┤
│  REFERENCE (API, schemas, contracts)               │  ← facts
├─────────────────────────────────────────────────────┤
│  GLOSSARY (ubiquitous language, domain terms)      │  ← shared vocabulary
└─────────────────────────────────────────────────────┘
```

The same content also has a **Diátaxis projection**: each layer can render as Tutorial / How-to / Reference / Explanation depending on the reader's need.

---

## Where AIDA stands today

The kernel's graph **already captures most of this** — just under generic types:

| Layer | AIDA today |
|---|---|
| Constitution | ❌ no first-class type (CLAUDE.md is the closest, but it's flat markdown) |
| Vision & Goals | ❌ implicit in EPIC titles + descriptions, not first-class |
| Constraints | ❌ no first-class type |
| Context & Scope | ❌ no first-class type |
| Architecture | ❌ no first-class type (some lives in OVERVIEW.md as prose) |
| Decisions / ADRs | ❌ no first-class type |
| Requirements | ✅ first-class (functional / non-functional / system / user) |
| Features | ✅ folders + feature-prefixed IDs |
| Work items | ✅ epic / story / task / bug / spike / sprint |
| Risks & Tech Debt | ⚠️ implicit via tags (e.g., `tech-debt`) |
| Reference | ⚠️ via custom_fields, ad-hoc |
| Glossary | ❌ no first-class type |
| Relationships | ✅ first-class (parent/child, verifies, references, custom) |

**Five layers are missing as first-class entities**: constitution, vision, constraints, decisions/ADRs, glossary. Architecture is partially modelable from the existing relationship graph (parents + verifies links can rebuild a building-block view).

---

## The proposed `aida-docs` module

Two ideas, in order of ambition.

### Idea A — Add the missing entity types to the kernel, then `aida-docs` projects

The kernel gains five new requirement types (or meta subtypes):

| Type | Purpose | Statelessness |
|---|---|---|
| `principle` | A constitution clause | Stateless (always active) |
| `vision` | A goal / target outcome | Stateful (active / achieved / abandoned) |
| `constraint` | An external/regulatory bound | Stateful (active / lifted) |
| `decision` | An ADR-style record | Stateful (proposed / accepted / superseded) |
| `term` | A glossary entry | Stateless |

These are cheap to add — small `RequirementType` enum extension + minor scaffolder updates. They don't bloat the kernel; they make the existing graph richer.

`aida-docs` is a separate module repo. It does:

1. **Project the graph into a layered docs tree** matching the synthesis hierarchy:
   ```
   docs/
   ├── 00-constitution.md     ← from req_type=principle
   ├── 01-vision.md           ← from req_type=vision (top-level epics)
   ├── 02-constraints.md      ← from req_type=constraint
   ├── 03-context.md          ← derived: external_system tags + interface relationships
   ├── 04-architecture/
   │   ├── building-blocks.md ← derived: folders + parent/child + module relationships
   │   ├── runtime.md         ← derived: scenarios linked to features
   │   ├── deployment.md      ← from req_type=deployment-fact (custom)
   │   └── crosscutting.md    ← from tagged reqs
   ├── 05-decisions/
   │   └── ADR-NNN-*.md       ← one file per req_type=decision
   ├── 06-features/
   │   └── <folder>.md        ← per-folder rollup of children
   ├── 07-quality.md          ← from req_type=non-functional
   ├── 08-risks.md            ← from tagged reqs (tag=risk)
   ├── 09-reference/          ← from custom_fields containing schemas/contracts
   └── 10-glossary.md         ← from req_type=term
   ```

2. **Round-trip support** — auto-generated sections marked with HTML comments (`<!-- AIDA-SOURCE: req=FR-42 -->`); manual prose between them is preserved on regeneration. Same pattern as `AIDA-AUTOGEN` blocks in AGENTS.md.

3. **Living updates** — `aida add` / `aida edit` with side-effect to docs pages where that req appears. Feature-flagged so it's only "live" when `aida-docs` is installed.

4. **Diátaxis lenses** — each generated page has `--as tutorial|howto|reference|explanation` to re-render the same source data for different audiences. Mostly affects ordering + voice.

5. **AI-friendly index** — `docs/INDEX.md` and `docs/README.md` are auto-generated; the MCP server exposes `docs://layer/<name>` resources so an agent can pull "give me the constitution" or "give me the architecture" with one call.

### Idea B — Lighter: a docs *generator* without new types

Skip the new entity types. Use existing tags + folders + custom_fields conventions:

- Tag `principle` ↔ Constitution
- Tag `decision` + custom_field `decision_status` ↔ ADRs
- Tag `term` ↔ Glossary
- Folder `Vision` ↔ Vision section
- Etc.

The module does the projection but the kernel stays unchanged. Less expressive (relationships between Decisions are weaker; querying "all active principles" is a tag scan instead of a type query) but zero kernel surface change.

### My recommendation: Idea A

The Idea-A entity types are tiny additions but they unlock:
- **Stronger queries** (`aida list --type decision --status superseded` is a real workflow)
- **Stronger MCP tools** (an agent can ask "what principles govern this codebase?" and get a typed result, not a tag scan)
- **Stronger projections** (Idea A's docs tree is unambiguous; Idea B's depends on tag conventions that drift)

The cost is ~1 day of plumbing in the kernel + the module. Worth it.

---

## What this changes for the pitch

The expanded pitch becomes provably true on a deeper level. AIDA isn't just an index that *queryable* — it's an index that *renders*. The same graph that answers "does this exist?" via MCP also generates the project's living architecture document, the constitution, the ADR log, and the glossary.

Spec Kit produces specs (one shot, written by you with AI). ADR tools produce ADRs (one shot, manual). Living Architecture is a fill-in template. None of them maintain a graph that's the SOURCE for all of those views.

**The differentiating claim:** *"Documentation is the projection. Update the graph, the docs update."*

---

## Extraction order — fits into the kernel/module audit

This is a NEW module (`aida-docs`) added to the audit's Phase list. Suggested ordering:

1. Phases 1–3 of the kernel/module audit ship first (drop legacy, extract `aida-scaffold`, extract `aida-roles`)
2. **Then add the 5 new entity types to the kernel** (`principle`, `vision`, `constraint`, `decision`, `term`). Small change. ~1 day.
3. **Then build `aida-docs`** as a fresh module. Single repo, depends only on the kernel. Can iterate independently.
4. Continue with Phases 4–8 (integrations, web, ai, reports)

---

## Open questions for mark-up

1. **Idea A vs Idea B?** I recommend A; you may have reasons to prefer B (less kernel churn).
2. **Should `principle` and `term` be requirement subtypes or a new top-level entity?** Today's `Meta` type already has `MetaSubtype` (Prompt/Skill/Command/Template/Config). Adding `Principle` and `Term` as Meta subtypes is cheaper than new top-level types. Probably the right call.
3. **ADR semantics.** Spec Kit uses one folder per feature with multiple files (spec.md, plan.md, tasks.md). AIDA's natural unit is a single requirement. Do we model an ADR as one `decision` requirement with structured custom_fields (consequence / alternatives / superseded_by), OR as a folder containing multiple smaller reqs? Recommend single-req for now; promote to folder if a single ADR routinely needs sub-sections.
4. **Round-trip granularity.** When the user edits the rendered `02-constraints.md`, do we want to parse it back? My instinct: no. One-way from graph → docs. The CLI is the editor. The docs are the projection.
5. **Diátaxis lenses — worth it?** Multi-rendering increases module complexity. Probably defer; ship reference/explanation rendering first.
6. **Visualization (C4)** — do we want SVG / mermaid diagram generation as part of `aida-docs`? Or punt to a separate `aida-diagrams` module?
7. **What's the dev workflow?** Does `aida-docs` re-render on every commit (git hook), every save (filesystem watcher), every `aida db sync`, or only on `aida docs build`? Recommend: `aida docs build` explicit + optional pre-commit hook.

---

## Why this is the right move

1. **It strengthens the differentiating pitch.** The "missing index" claim becomes more concrete: the index isn't just for lookups; it generates the project's documentation surface. No competitor has this.
2. **It's additive to existing users.** People who don't want it just don't install `aida-docs`. The kernel adds 5 enum variants and gains nothing in surface area.
3. **It compounds with MCP.** An agent reading `docs://constitution` is asking the same source that humans browse; both audiences agree on what's true.
4. **It validates the kernel architecture.** If extracting `aida-docs` works cleanly, the kernel/module split is real — not just bookkeeping.

The cost is ~1 day for the kernel additions + ~1 week for an MVP module. Decision is yours.
