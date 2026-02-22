# Why AIDA?

## The Problem Nobody Talks About

AI coding assistants are extraordinary. Claude Code can implement a feature in minutes that would take hours by hand. But there's a dirty secret that becomes obvious after a few weeks of real use: **the code gets written, but nobody knows why it was written.**

Ask yourself:
- After a month of AI-assisted development, can you explain what your system does and why each piece exists?
- If a new team member joins, can they trace a line of code back to a business decision?
- When you open a chat with Claude Code, how much context do you spend re-explaining before you can start working?
- When the AI suggests a change, how do you evaluate whether it aligns with what the system *should* do?

These aren't hypothetical problems. They're the lived experience of every developer who has used AI coding tools seriously.

---

## Who Is This For?

### The Lone Developer

You're building something. Maybe a side project, maybe a startup. Claude Code is your co-pilot and you're moving fast. But you've noticed a pattern:

1. You start a session with a vague idea
2. Claude builds something impressive
3. Two days later you can't remember why a component works the way it does
4. You spend the first 10 minutes of every session re-explaining context
5. Features accumulate but there's no coherent picture of what the system *is*

You don't need Jira. You don't want to write formal requirements documents. But you do need *something* between "it's all in my head" and "enterprise project management."

AIDA gives you that something. A lightweight, file-based requirements database that lives in your repo, speaks the same language as your AI tools, and grows with your project without adding ceremony.

### The Program/Project Manager

You're responsible for a product. You need to know what's been built, what's in progress, and what's planned. Your developers are using AI assistants and shipping fast - which is great, except:

- You can't reliably answer "what changed and why?"
- Sprint planning feels disconnected from what actually gets built
- There's no audit trail connecting business decisions to code
- When someone says "the AI built it," you have no way to evaluate whether it built the *right* thing

AIDA bridges the gap between product intent and code reality. Requirements link to code through trace comments. Commits reference spec IDs. Sprint planning and progress tracking happen in the same system that the AI reads when it writes code.

### The Team That's Scaling

You started with one developer and Claude Code. Now you're three, or five, or ten. The CLAUDE.md file that worked for one person is a bottleneck. People are duplicating work, building inconsistent features, and losing track of decisions.

AIDA provides the shared context layer that scales. A SQLite or PostgreSQL-backed requirements database with a REST API, web dashboard, concurrent access with optimistic locking, and multi-project support. The same tool that helped one developer stay organized now helps a team stay aligned.

---

## What AIDA Actually Does

At its core, AIDA is a **requirements management system built for AI-assisted development**. But that undersells it. Here's what it means in practice:

### 1. Requirements as Living Context

When Claude Code starts a session in an AIDA project, it doesn't just read a static CLAUDE.md. Through MCP integration and skills, it can query the requirements database directly:

- "What are the approved requirements for the auth feature?"
- "Show me the children of EPIC-0365"
- "What's the status of sprint 4?"

Requirements aren't documents that rot in a wiki. They're queryable, structured data that the AI uses to make better decisions.

### 2. Bidirectional Traceability

When you implement a feature, AIDA adds trace comments to your code:

```rust
// trace:FR-0042 | ai:claude
fn validate_user_input(input: &str) -> Result<()> {
```

When you commit, the commit message links back:

```
[AI:claude] feat(auth): add input validation (FR-0042)
```

This means you can go from a business requirement to every line of code that implements it, and from any line of code back to the business requirement that justified it. This is table stakes in regulated industries. It's also just good engineering.

### 3. AI-Native Workflow

AIDA ships 15 Claude Code skills that encode a requirements-driven development workflow:

- `/aida-req` - Capture a requirement before you build (not after)
- `/aida-plan` - Create an implementation plan from requirements
- `/aida-implement` - Build with traceability baked in
- `/aida-commit` - Commit with automatic requirement linking and untraced code detection
- `/aida-capture` - End-of-session safety net for anything you discussed but didn't formalize

This isn't about adding process for its own sake. It's about making the AI *better* by giving it structured context to work with.

### 4. Full-Stack Tool

AIDA isn't just a CLI bolted onto a YAML file. It's a complete system:

- **CLI** for quick operations and scripting
- **React dashboard** with kanban, list, sprint planning, timeline, search, and AI chat
- **REST + gRPC APIs** for integration
- **Three storage backends** (YAML, SQLite, PostgreSQL) with migration between them
- **GitLab integration** with bidirectional sync
- **MCP server** for native Claude Code tool integration

You can start with `aida init` and a YAML file, and grow into a multi-project PostgreSQL deployment without changing tools.

---

## The Honest Competitive Landscape

AIDA doesn't exist in a vacuum. Here's how it compares to the alternatives, honestly.

Before diving in, an important nuance: nearly every tool on this list now has *some* form of AI integration. MCP servers exist for Jira, GitHub, Linear, Notion, and more. Claude Code can talk to all of them. The question isn't "can AI access this tool?" - it can. The question is **whether the tool was designed around the AI-assisted development workflow, or whether AI was bolted onto a product designed for humans filling out forms in a browser.**

AIDA was born into the AI era. It evolved alongside Claude Code, not before it. That shapes everything - from the data model to the workflow to the interface.

### Traditional Project Management (Jira, Linear, Shortcut, Azure DevOps)

**What they do well:**
- Mature, battle-tested, team-oriented
- Rich ecosystem of integrations (including MCP servers for AI access)
- Proven workflows (scrum, kanban, SAFe)
- Enterprise features (permissions, audit, compliance)

**Where AIDA differs:**
- These tools were designed for humans coordinating with humans. AI integration is an add-on - you can query Jira through MCP, but the data model, workflows, and UI were all designed around a human clicking through a web form. The AI is a consumer of the tool, not a participant in the workflow.
- They're external to your codebase. Requirements live in a browser tab, not in your repo.
- There's no concept of code traceability. A Jira ticket doesn't know which lines of code implement it.
- They're heavy. A solo developer doesn't need Jira, and adding an MCP server to Jira doesn't change that.

**Honest assessment:** For large teams with established processes, Jira/Linear aren't going away - and they shouldn't. AIDA's opportunity is as a **complement** - the developer-side layer between the project management tool and the code, where AI needs structured context that's shaped for development, not project management. For smaller teams and solo developers, AIDA can *replace* these tools entirely.

### GitHub/GitLab Issues + Projects

**What they do well:**
- Free, lightweight, close to the code
- Good enough for many projects
- Native to the development workflow
- MCP servers exist for AI access to issues, PRs, and projects

**Where AIDA differs:**
- Issues are flat text with labels. Even with MCP access, the AI is reading unstructured prose - there are no typed requirements, no relationship graphs, no queryable custom fields.
- AI can *read* issues through MCP, but the tool wasn't designed to *guide* AI development. There's no concept of trace comments, implementation plans, or commit validation against requirements.
- No automated traceability. You can manually reference issue numbers in commits, but there's no system enforcing or verifying it.
- No requirement relationships (parent/child, verifies, references) beyond basic labels and milestones.

**Honest assessment:** GitHub Issues with an MCP server gives AI basic access to your project backlog. AIDA gives AI a structured development context with typed requirements, relationships, traceability, and workflow skills. They serve different depths of the same need. They can coexist - AIDA already has GitLab integration, and GitHub integration would be natural.

### Notion / Confluence / Wiki-based Approaches

**What they do well:**
- Flexible, rich formatting, good for documentation
- Low barrier to entry
- Good for non-technical stakeholders
- MCP/API integrations make content accessible to AI

**Where AIDA differs:**
- Even with MCP access, wikis are unstructured documents. The AI can read them, but it can't query "all approved requirements for the auth feature" - it has to parse prose.
- No traceability to code. A Notion page describing a feature doesn't link to the code implementing it.
- They drift. Requirements documents in Notion become stale within weeks. There's no workflow to keep them current as code evolves.
- They're designed for humans reading documents, not for AI querying structured data.

**Honest assessment:** Notion is great for high-level product documents and stakeholder communication. AIDA is for the structured, queryable, traceable layer underneath. An ideal setup might use Notion for product strategy and AIDA for development-level requirements.

### AI Coding Assistants Alone (Claude Code, Cursor, Windsurf, Copilot)

**What they do well:**
- Write code fast
- Understand context from CLAUDE.md, open files, and MCP tools
- Getting better every month
- Can access external tools through MCP, extensions, and APIs

**Where AIDA differs:**
- AI assistants are stateless between sessions. Claude Code doesn't remember what you discussed yesterday unless you write it down somewhere.
- They're general-purpose. They can talk to any MCP server, but they don't impose a *development workflow*. AIDA's skills encode opinions about how to develop (requirements first, trace your code, validate your commits).
- Without structure, the AI builds what you ask for in the moment, not what the system needs. Context from an MCP-connected Jira is better than no context, but it's still context designed for human project managers, not for AI writing code.

**Honest assessment:** AI coding assistants are the engine. AIDA is the steering wheel. You need both. Claude Code without AIDA is powerful but undirected. AIDA without Claude Code is just another requirements tool. The unique value is in the combination.

### Plain CLAUDE.md / Markdown Convention Files

**What they do well:**
- Zero setup, zero dependencies
- Claude Code reads them natively
- Version controlled
- Immediate, no learning curve

**Where AIDA differs:**
- A CLAUDE.md file doesn't scale. At 500 lines it's already unwieldy. At 1000 lines it's consuming context window for information the AI doesn't need right now.
- It's not queryable. Claude reads the whole thing every time. AIDA's MCP server lets the AI fetch only what's relevant.
- No structure. You can't filter by status, type, or sprint.
- No history. When you change a requirement, the old version is gone (unless you diff git history).

**Honest assessment:** CLAUDE.md is where every project should start. AIDA is where you go when CLAUDE.md isn't enough - when you need to query, filter, track status, and give the AI structured data instead of a wall of text. AIDA generates and maintains the CLAUDE.md as part of its workflow, so you get both.

### Emerging AI-Native Dev Tools (Devin, SWE-Agent, OpenHands)

**What they do well:**
- Autonomous agents that can plan and execute
- End-to-end task completion
- Getting significant investment and attention

**Where AIDA differs:**
- These tools focus on *doing the work*, not *managing what work should be done*.
- They need a source of truth for what to build. AIDA can be that source.
- They don't solve the traceability problem - they make it worse, because even more code is being written without a human in the loop.

**Honest assessment:** Autonomous agents are complementary to AIDA, not competitive. An agent that can query AIDA for its next task, implement it with traceability, and update the requirement status is the dream workflow. AIDA's MCP server already makes this possible.

### The Real Differentiator

The common thread across all these comparisons isn't "they lack AI integration" - most of them have it or are adding it. The differentiator is **where AI sits in the design**.

In traditional tools, AI is a feature you add. An MCP server that lets Claude read your Jira tickets. A Copilot plugin that suggests code. The tool was designed for humans, and AI was invited in later.

In AIDA, AI is a co-designer of the workflow. The data model was shaped by what AI needs to write better code. The skills encode workflows that assume AI is doing the implementation. The traceability system exists because AI-written code needs *more* traceability, not less, since there's no human who "just knows" why that function exists.

This is a meaningful architectural difference, not a marketing claim. It's the difference between a car designed for electric from the ground up versus a gas car with a battery bolted on. Both drive. One was designed around the new reality.

---

## Where AIDA Is Strong Today

1. **Claude Code integration depth.** 15 skills, MCP server, trace comments, commit validation. No other requirements tool comes close to this level of AI-native integration.

2. **Developer-first design.** CLI, file-based storage, git-friendly. It doesn't feel like enterprise software forced onto a developer workflow.

3. **Full vertical stack.** From YAML files to PostgreSQL, from CLI to web dashboard, from solo to team. You can grow without switching tools.

4. **Requirement-to-code traceability.** Trace comments + commit messages + spec IDs create a bidirectional link that most teams don't have at any price point.

5. **Customizable AI behavior.** Meta requirements let you tune AI prompts per project. A medical device project evaluates requirements differently than a web app.

---

## Where AIDA Needs to Grow

Being honest about gaps is how you build something great.

1. **Adoption friction.** AIDA requires `aida init`, learning skills, and buying into a workflow. Compare this to "just open Jira" or "just write a CLAUDE.md." The onboarding path needs to be smoother, with a more gradual on-ramp.

2. **Team collaboration features.** Multi-user support exists (PostgreSQL, optimistic locking, multi-project) but hasn't been battle-tested at scale. Real-time collaboration, notifications, and access control are missing.

3. **Integration breadth.** GitLab integration exists, but GitHub, Slack, CI/CD webhooks, and other ecosystem integrations are absent. In a world where everything connects to everything, AIDA is still relatively isolated.

4. **Reporting and analytics.** Sprint charts exist but deeper analytics - velocity trends, requirement churn, AI contribution metrics, quality score trends - would make the PM story much stronger.

5. **Non-Claude AI support.** The skills and MCP integration are Claude Code specific. As AI assistants proliferate, supporting Cursor rules, Windsurf configurations, or generic MCP clients would broaden the audience.

6. **Documentation and marketing.** The tool is sophisticated but not well-explained to outsiders. There's no landing page, no "getting started in 5 minutes" tutorial, no comparison charts.

---

## The Vision

AIDA's thesis is simple: **AI-assisted development needs a structured context layer.**

Today, developers give AI assistants context through README files, inline comments, and conversation. This works for small projects. It breaks down as projects grow, teams scale, and the distance between business intent and code reality widens.

AIDA is that structured context layer. It sits between the people who decide what to build and the AI that helps build it. It makes the AI smarter by giving it queryable, typed, relational data instead of prose. It makes the humans more confident by providing traceability from decision to implementation. It makes the project more resilient by maintaining a single source of truth that doesn't decay.

The long-term architecture should evolve toward:

- **Universal AI integration** - not just Claude Code, but any AI assistant that speaks MCP or can query an API
- **Event-driven workflows** - requirement status changes trigger CI/CD, notifications, AI evaluations automatically
- **Intelligence layer** - AIDA doesn't just store requirements, it understands them: detecting gaps, suggesting decompositions, identifying risks, predicting delivery
- **Ecosystem connectors** - bidirectional sync with GitHub, Linear, Slack, CI systems, test frameworks
- **Governance without bureaucracy** - traceability and audit trails that emerge naturally from the development workflow, not from forms and approvals

The goal isn't to replace Jira for Fortune 500 companies. It's to be the tool that developers *actually want to use* - one that makes AI-assisted development more intentional, traceable, and effective.

---

## A Final Thought

The best tools encode an opinion about how work should be done. Git encodes the opinion that code changes should be distributed, branching, and mergeable. Jira encodes the opinion that work should be ticketed, assigned, and tracked through stages.

AIDA encodes the opinion that **what you build should be traceable to why you're building it, and your AI assistant should have access to both.** That's a small opinion, but in a world where AI is writing an increasing share of production code, it might be the most important one.
