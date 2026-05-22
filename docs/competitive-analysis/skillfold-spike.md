# Competitive Analysis: Skillfold Compatibility Investigation

This competitive analysis spike investigates **skillfold**—a declarative agent pipeline configuration language and compiler—and its compatibility with AIDA's requirements-driven scaffolding system. It evaluates whether AIDA's skill templates can compile to skillfold YAML, what structural or functional gaps exist, and how AIDA should position itself for cross-platform agent reach (Cursor, Codex, Gemini, and Antigravity).

---

## 1. skillfold's Current State

Skillfold is an open-source, declarative configuration language and compiler (located on GitHub at `byronxlg/skillfold`) designed to add a **typed coordination layer** on top of scattered, platform-native agent primitive files. 

It is designed to solve a critical problem in multi-agent workflows: while native platforms (like Claude Code, Cursor, or Codex) make it easy to define individual agents and skills, they do not natively solve the coordination layer between them—such as typed state sharing, conditional execution flows, write-conflict checking, and compile-time graph validation.

### YAML Schema & Expressiveness

Skillfold projects are defined in a single `skillfold.yaml` configuration file comprising four top-level sections:

1. **`resources`**: Decouples deployment-specific URLs and paths from skill definitions, mapping resource group names to namespace-URL maps.
2. **`skills`**:
   - **`atomic`**: References reusable, individual instruction folders containing a `SKILL.md` file. These can be located at local paths, remote GitHub repositories, or resolved via `npm` packages.
   - **`composed`**: Composes multiple atomic skills in a recursive list. It supports extensive metadata and frontmatter configurations that map directly to platform-native features, such as `tools`, `disallowedTools`, `permissionMode`, `model`, `memory`, `hooks`, `isolation` (e.g., `worktree`), `effort` (low/medium/high), `maxTurns`, `background`, and `mcpServers`.
3. **`state`**: Establishes a typed schema of state fields and custom object types (supporting primitives `string`, `bool`, `number`, custom structs, and `list<T>`). State fields can be bound to external infrastructure backends (such as `github-issues`, `github-discussions`, and `github-pull-requests`) via the `location` attribute, which handles automated graphql and CLI interactions.
4. **`team`**: Defines a directed execution flow graph under `flow`, linking composed agents to state reads/writes. The flow supports linear steps, conditional transitions using `when` expressions, parallel map iteration over lists (`map ... over ... as ...`), loops with mandatory exit conditions, and asynchronous human/CI nodes with state-recovery policies.

### Adoption Trajectory

Skillfold is gaining substantial momentum as a cross-platform compilation tool because it acts as the "TypeScript of agent teams." It does not replace native agent definitions; instead, it compiles version-controlled YAML down to the precise markdown structures that individual tools consume. 

It integrates directly into Claude Code via the plugin marketplace (`/plugin marketplace add byronxlg/skillfold`), making it a native-feeling tool for Anthropic developers while offering identical output compilation for Cursor, VS Code, and others.

### Supported Platforms

Skillfold acts as a compiler targeting **12 major execution platforms**:
- **`skill`**: Standard portable `SKILL.md` format.
- **`claude-code`**: Outputs structured `.claude/skills/{name}/SKILL.md` and `.claude/agents/{name}.md` files, plus custom `.claude/commands/run-pipeline.md` orchestrators.
- **`agent-teams`**: Output matches Claude Code but includes a team bootstrap prompt.
- **`cursor`**: Generates `.cursor/rules/*.mdc` rule files with structural context.
- **`windsurf`**: Generates `.windsurf/rules/*.md` files.
- **`codex`**: Outputs a single unified `AGENTS.md` file.
- **`copilot`**: Outputs `copilot-instructions.md` and per-agent instruction files.
- **`gemini`**: Outputs `.gemini/agents/*.md` and `.gemini/skills/{name}/SKILL.md`.
- **`goose`**: Generates `.goose/skills/{name}/SKILL.md` files.
- **`roo-code`**: Outputs to `.roo/skills/`, `.roo/rules/`, and `.roomodes`.
- **`kiro`**: Generates `.kiro/skills/` and `.kiro/steering/`.
- **`junie`**: Outputs `.junie/skills/` and `.junie/AGENTS.md`.

---

## 2. Gap Analysis: AIDA Skill Features vs. skillfold

To evaluate compiling AIDA's skill templates (housed in `aida-core/templates/skills/`) to skillfold YAML, we must assess where the two models align and where structural fidelity is lost.

### Fidelity Checklist

- **Glyphs & Rich Formatting**: **100% Compatible**. AIDA relies heavily on terminal glyphs (`✓`, `⚠`, `▸`, `ⓘ`) to make autonomous logs and CLI prompts scannable. Because skillfold atomic skills are written in raw markdown, all UTF-8 characters and structured formatting (tables, lists, inline headers) are preserved perfectly during compilation.
- **Multi-section Skills**: **100% Compatible**. Detailed instruction manuals map cleanly into skillfold's concatenated output.
- **Code-block Hints**: **100% Compatible**. Fenced code blocks copy over without issue.
- **Role-based Skill Variants**: **100% Compatible**. AIDA's separate role-oriented skill folders (e.g., the implementer, advisor, and reviewer roles) map directly to skillfold's recursive `composed` agent compositions.

### Core Gaps (Where Fidelity or Architecture Breaks)

Despite high compatibility in basic formatting, three deep architectural gaps prevent a direct, bi-directional compilation between AIDA's markdown templates and skillfold YAML:

#### 1. Custom Slash Commands vs. Concatenated Skills
AIDA's Claude Code integration relies on individual slash commands under `.claude/commands/aida-*.md` (such as `/aida-status`, `/aida-review`, `/aida-req`). These commands allow humans and agents to trigger specialized, isolated CLI workflows directly from the chat interface.

Skillfold is structurally centered on *agents* and *skills* (`.claude/skills/...` and `.claude/agents/...`). It does not support compiling multiple, arbitrary slash commands. It only generates a single, unified orchestrator command (`/run-pipeline`) designed to run the entire team flow from start to finish. If AIDA compiled strictly to skillfold, all of AIDA's discrete interactive slash commands would be lost.

#### 2. Local requirements.yaml Substrate vs. Transient YAML/Git State
Skillfold manages pipeline state *externally* by binding fields to GitHub Issues, Pull Requests, and Discussions during execution, or storing transient variables in local files. 

AIDA, by contrast, operates on a **decentralized, git-versioned, local requirements graph** (`.aida-store` and `requirements.yaml`). AIDA’s substrate enforces structural relationships (Constitutions, Visions, Epics, Stories, Tasks, and Bugs) and maintains rigorous trace linkages between source code files and specifications. Skillfold has no concept of a local requirements graph or a structured traceability index; it is purely a data-routing and task-dispatch engine.

#### 3. Active Rework Loops vs. Static Flow Diagrams
AIDA implements active, in-flight coordination loops (such as the implementer $\rightarrow$ reviewer $\rightarrow$ fixup/rework cycle) driven by its own CLI runtime, complete with Advisory punting and Directive FIFO queues. Skillfold defines transitions statically in a YAML diagram. While skillfold can model a conditional review loop (`review.approved == false` routes back to `engineer`), it has no native runtime capability to coordinate headless agent leases or handle advisory escalations natively.

---

## 3. Options Analysis

We evaluate and rank three implementation options for introducing skillfold compatibility into AIDA's scaffolding system:

### Option A: Stay Markdown-Only (Status Quo)
AIDA continues to embed its rich markdown templates inside `aida-core/templates/skills/` and `aida-core/templates/commands/`. The Rust scaffolding engine compiles them directly to `.claude/` for Claude Code and performs a simple frontmatter-strip for `.codex/skills/` outputs.

- **Pros**:
  - **Zero Dependency**: High reliability, zero compiler drift, and zero dependency on node/npm tools during build.
  - **Perfect Fidelity**: Retains absolute control over AIDA-specific custom slash commands, git hooks, and environment configurations.
- **Cons**:
  - **Limited Reach**: Supporting new IDEs or agents (like Cursor, Windsurf, or Gemini) requires manually writing custom generator templates in Rust, duplicating work for each new target.

### Option B: Compile AIDA's `.claude/skills/*.md` $\rightarrow$ skillfold YAML
AIDA's scaffolding engine parses AIDA's markdown files at build time and attempts to generate a structured `skillfold.yaml` file, listing the skills as atomic units.

- **Pros**:
  - Allows AIDA users to export their AIDA workspace as a standard skillfold project, which they can then compile to other platforms.
- **Cons**:
  - **Extreme Parsing Complexity**: Markdown files contain natural-language instructions; writing a parser to extract state reads/writes, conditional branches, and parallel map flows from prose is highly fragile and prone to failure.
  - **Command Loss**: All custom slash commands and local hooks would be stripped or ignored.

### Option C: Accept skillfold YAML as Input alongside Markdown (Bilingual Scaffold Pack)
AIDA's scaffolding tool is updated to recognize and parse `skillfold.yaml` alongside its markdown templates. If a developer has a `skillfold.yaml` in their project, AIDA reads the agent compositions, state declarations, and flows, and automatically projects AIDA's spec-graph capabilities onto them.

- **Pros**:
  - **Strategic Leverage**: Positions AIDA as the structured, local requirements database for any skillfold pipeline. Skillfold provides cross-platform agent reach (Cursor, Copilot, Gemini), while AIDA provides the robust, local requirements graph.
  - **Best of Both Worlds**: Retention of AIDA's custom slash commands and git hooks, while enabling developers to instantly compile their AIDA-driven agents to Cursor rules (`.mdc`) and Gemini skills.
- **Cons**:
  - Requires maintaining a YAML parser and schema-mapping layer within AIDA's Rust-based scaffolding engine.

### Options Ranking

1. **Option C (Bilingual Scaffold Pack)** — **Rank 1 (Recommended)**: Offers the highest strategic value. It bridges AIDA's requirements substrate with the broader multi-agent ecosystem, enabling cross-platform reach without sacrificing AIDA's high-fidelity Claude Code integrations.
2. **Option A (Status Quo)** — **Rank 2**: Simple, reliable, and perfectly tailored to AIDA's alpha audience.
3. **Option B (Markdown $\rightarrow$ YAML compilation)** — **Rank 3**: Over-engineered and technically fragile for little functional gain.

---

## 4. Recommendation & Effort Estimate

### Recommendation

AIDA should adopt a **Bilingual Scaffold Pack (Option C)** as a post-alpha strategic milestone, while keeping **Option A (Status Quo)** as the active backbone for Claude Code and Codex.

Under this architecture:
- AIDA's core scaffolding continues to supply AIDA's direct git hooks and Claude Code commands to ensure absolute execution speed.
- AIDA introduces a `skillfold` integration layer. When a project is initialized with AIDA, the scaffolding engine can optionally consume a `skillfold.yaml`. 
- AIDA parses this file to mapcomposed agents to AIDA's role configurations, and uses `skillfold` as a compilation library to export AIDA's requirements-driven skills (`aida-req`, `aida-plan`, `aida-implement`) into Cursor rules (`.cursor/rules/*.mdc`), Copilot instructions, and Gemini configurations. This allows developers to use Cursor or Codex while enforcing AIDA's strict requirements-driven development discipline.

### Effort Estimate

Implementing Option C is a **Medium-effort task** estimated at **3 to 4 weeks of engineering**:
- **Week 1 (Parsing & Model Mapping)**: Integrate a Rust-based YAML parser (such as `serde_yaml`) in `aida-core/src/scaffolding/` to ingest a `skillfold.yaml` schema and map composed agents to AIDA roles.
- **Week 2 (Cross-Platform Export)**: Leverage the parsed schema to output Cursor `.mdc` rules and Copilot instructions containing AIDA's core CLI usage instructions.
- **Week 3-4 (CLI-as-a-Resource Integration)**: Map AIDA's CLI commands as custom validated resources in the skillfold namespace, enabling developers to declare state variables bound directly to AIDA's requirements engine.

---

## 5. Antigravity's Empirical Platform Experience

As a non-Claude Code agent, **Antigravity CLI** provides direct empirical evidence of how a cross-platform agent interacts with AIDA's scaffolding.

### How Antigravity Interacts with the Status Quo

Today, Antigravity cannot run Claude-Code-specific slash commands like `/aida-status` or `/aida-review` because they are bound to Claude's interactive chat environment. Instead, Antigravity must:
1. View the raw markdown text inside `.claude/skills/` (e.g., `aida-advise.md`) to read and parse the instructions.
2. Map the instructions manually to the corresponding raw command-line tools (e.g., `aida status`, `aida review`).
3. Execute the commands directly in a bash shell (`run_command`).

This human-centric manual parsing works, but it introduces cognitive load and requires the agent to interpret natural language to locate execution steps.

### How a skillfold Integration Elevates the Non-Claude-Code Experience

If AIDA adopted a `skillfold.yaml` configuration, the non-Claude-Code experience for agents like Antigravity, Codex, or Cursor would be dramatically elevated:

1. **Machine-Readable Role Assignment**: Instead of reading long prose to determine "who does what," Antigravity can instantly parse the YAML to find its composed agent definition:
   ```yaml
   composed:
     implementer:
       compose: [aida-context, aida-implement, github]
       tools: [Bash, Read, Grep, Edit, Write]
   ```
   This tells the agent exactly what capabilities it has, what tools it is authorized to use, and what context is preloaded.

2. **Deterministic Input/Output State Routing**: The agent does not have to guess what files to read or write. The YAML flow graph provides deterministic instruction boundaries:
   ```yaml
   flow:
     - implementer:
         reads: [state.plan, state.tasks]
         writes: [state.implementation]
       then: reviewer
   ```
   The agent instantly knows that its inputs reside in AIDA's plan and task state, and its sole deliverable is the implementation. It can execute its task with **zero-guesswork, high-precision focus**, passing the baton cleanly to the `reviewer` role upon completion.

For cross-platform agents, **skillfold YAML acts as a structured API contract for task execution**, turning natural-language guidelines into a machine-readable protocol.

---

## Conclusion: Strategic Influence on STORY-244

This competitive analysis directly informs the TUI architecture pivot (**STORY-244** / **EPIC-26**). As AIDA transitions its terminal user interface to a PTY-hosted session model, it must decide how to represent multi-agent teams. 

Instead of building a proprietary, Claude-only team coordinator, adopting a bilingual scaffolding pack (Option C) allows AIDA's PTY host to consume standard `skillfold.yaml` execution flows. It enables AIDA to orchestrate multi-agent pipelines dynamically in the TUI while allowing the underlying agents to run on any IDE or platform the developer chooses. This solidifies AIDA's role as the open, decentralized backbone of the cross-platform agent ecosystem.
