# AIDA and the Future of Agentic Coding

**Last updated**: 2026-03-17

## The Shift Happening Now

Software development is transitioning from **human-writes-code-with-AI-assistance** to **AI-writes-code-with-human-oversight**. The implications for project management and requirements tracking are profound.

### What's Changed (2024-2026)

- **Claude Code, Cursor, Windsurf, Cline** — AI writes most of the code in a session
- **Devin, SWE-Agent, OpenHands** — autonomous agents that take a ticket and produce a PR
- **MCP (Model Context Protocol)** — standardized way for AI agents to access external tools
- **Multi-agent systems** — agents spawning sub-agents, planning and executing in parallel
- **Context windows growing** — 200K → 1M+ tokens, enabling entire codebases in context

### The Problem This Creates

When AI writes 80%+ of the code, three things break:

1. **Traceability vanishes.** Who decided this function should exist? What business requirement does it serve? The human "just knows" — but the knowledge is in a chat session that disappears.

2. **Context evaporates between sessions.** Each new Claude Code session starts cold. The first 10 minutes are spent re-explaining what the system does and what was decided.

3. **Quality becomes hard to verify.** The AI built it fast, but did it build the *right* thing? Without structured requirements, there's no specification to verify against.

## Where This Is Going (2026-2027)

### Near-Term: Agents Get Task Queues

Autonomous agents (Devin-like) will increasingly:
- Pick tasks from a backlog
- Implement them with full PR workflow
- Run tests, request review, iterate

This requires **structured task definitions** — not prose in a CLAUDE.md, but queryable, typed, relational data that an agent can understand programmatically.

### Mid-Term: Multi-Agent Workflows

Teams will run multiple agents concurrently:
- Agent A implements feature X
- Agent B fixes bug Y
- Agent C writes tests for Z
- Human reviews PRs and makes design decisions

This requires **coordination** — agents need to know what other agents are working on, what's already been decided, and what the system is supposed to do. A shared requirements database provides this coordination layer.

### Long-Term: Requirements as the Primary Artifact

The shift is from **code as the primary artifact** to **requirements as the primary artifact**:

- Humans define *what* the system should do (requirements)
- AI agents figure out *how* to build it (code)
- Code becomes a derived artifact that can be regenerated

In this world, the requirements database is more important than the code. The code is disposable; the requirements are the intellectual property.

## How AIDA Fits

### For the Current Era (Human + AI Pair Programming)

AIDA's value today:

- **Context persistence** — requirements survive across chat sessions. No re-explaining.
- **Traceability** — trace comments link code to requirements. You can always answer "why does this code exist?"
- **Quality verification** — AI evaluation scores requirements for clarity, testability, completeness.
- **Workflow skills** — `/aida-req`, `/aida-implement`, `/aida-commit` encode a development methodology that works with AI.

### For the Agent Era (Autonomous Agents)

AIDA's value in an agent-driven world:

| Agent Need | How AIDA Helps |
|---|---|
| "What should I build?" | `aida list --status approved` → agent's task queue |
| "What does the system do?" | `aida show FR-042` → structured context, not prose |
| "Is this already built?" | `aida search "authentication"` → avoid duplicate work |
| "How does this relate to X?" | `aida rel` → relationship graph between requirements |
| "Am I building the right thing?" | `aida show FR-042` description + acceptance criteria |
| "I'm done, what's next?" | `aida edit FR-042 --status completed && aida list --status approved` |

The MCP server (`aida mcp-serve`) makes all of this available to any agent that speaks MCP — not just Claude Code.

### What AIDA Does NOT Do (Honestly)

- **AIDA does not write code.** It provides context to agents that write code.
- **AIDA does not replace Jira** for large enterprises with established processes.
- **AIDA does not auto-generate requirements** from code (yet).
- **AIDA does not track velocity or predict delivery** (telemetry module is a foundation, not a dashboard).
- **AIDA requires buy-in** — you have to actually create requirements for it to be useful. There's no value if the database is empty.

## The Measurement Challenge

The biggest objection to any project management tool: **"does it actually help, or does it just add process?"**

AIDA's answer: **measure it.**

The telemetry module tracks:
- Requirements created, modified, completed per week
- Skills invoked (which workflows are actually used?)
- Traceability coverage (what % of commits reference requirements?)
- Cycle time (how long from requirement creation to completion?)
- Search usage (is the AI actually querying the database?)

These metrics are stored locally and can be reported:
```bash
aida db usage    # show usage summary
```

If AIDA isn't being used, the numbers prove it — and that's valuable information too. The tool should justify its own existence with data, not promises.

## Adoption Case Studies

### Case 1: Small Open-Source CLI Tool (e.g., ripgrep — 50K stars)

**Current state**: README + GitHub Issues for bugs. No formal requirements.

**With AIDA**:
```bash
cargo install --git https://github.com/joemooney/aida.git aida-cli
cd ripgrep && aida init
aida add --title "Support PCRE2 regex" --type functional --status approved
aida add --title "Respect .gitignore by default" --type functional --status completed
```

**Value**: When a contributor opens a PR, they can reference `FR-042` and the maintainer can see exactly which requirement it addresses. AI assistants helping contributors can query the requirements for context.

**Effort**: 30 minutes to set up, 2 minutes per requirement.

### Case 2: Medium Startup Backend (e.g., 5 developers, ~100K LOC)

**Current state**: Linear for tickets, CLAUDE.md for AI context, Notion for specs.

**With AIDA**:
```bash
aida init --distributed
aida github pull --labels "enhancement"  # import from GitHub Issues
```

**Value**: The requirements database gives AI assistants structured context instead of parsing Notion prose. Trace comments in code create bidirectional links. Sprint planning uses real requirement data.

**Effort**: 2 hours to import existing issues and set up.

### Case 3: Enterprise Monorepo (e.g., 50+ developers, multiple services)

**Current state**: Jira for PM, scattered documentation, AI adoption growing but uncoordinated.

**With AIDA**:
```bash
# Workspace mode — shared store across services
aida db workspace-init --name "platform"
```

**Value**: AIDA complements Jira — it's the developer-side layer between Jira tickets and code. AI agents query AIDA for technical context while PMs use Jira for workflow. GitHub/GitLab integration keeps both in sync.

**Effort**: 1 week to set up, ongoing maintenance.

## Differentiation

### Why Not Just Use GitHub Issues + MCP?

GitHub Issues + an MCP server gives AI basic access to your backlog. But:
- Issues are unstructured prose — the AI parses text, not typed data
- No traceability — no link from code back to the issue that justified it
- No relationship graph — can't query "all requirements that depend on auth"
- No offline capability — requires GitHub connectivity
- No AI evaluation — no quality scoring on requirements

AIDA provides **queryable, typed, relational data with bidirectional traceability**. That's the difference between "AI can read your tickets" and "AI understands your system."

### Why Not Just Use CLAUDE.md?

CLAUDE.md works for small projects. At scale:
- It doesn't scale past ~500 lines without consuming context window
- It's not queryable — AI reads the whole file every time
- It has no structure — status, priority, relationships are all prose
- It has no history — when you change a requirement, the old version is gone
- It can't be filtered — sprint planning, feature filtering, owner assignment are all manual

AIDA generates and maintains the CLAUDE.md as part of its workflow. You get both.

### The Unique Position

AIDA occupies a space no other tool fills: **the structured context layer between humans who decide what to build and AI agents that build it.**

```
Humans (what)  →  AIDA (structured context)  →  AI Agents (how)
                       ↕                            ↕
                  Traceability              Code + Tests
```

Every other tool is designed for humans coordinating with humans. AIDA is designed for humans coordinating with AI.
