# AIDA — Overview

> **What this is — read first.** AIDA is a **research probe** into how AI agents coordinate — especially across vendors — and an existence-proof for a home-grown coordination substrate. **The artifact is the instrument; the deliverable is knowledge.** Promoting AIDA-the-product is explicitly *not* the goal: the honest verdict on roll-your-own is **open** (it may be "don't build this," or "the right tool isn't AIDA"), and AIDA's own failures are first-class data. The thesis, evidence grades, threats-to-validity, and the open verdict live in [docs/research/2026-06-16-coordinating-multi-vendor-agent-fleets.md](docs/research/2026-06-16-coordinating-multi-vendor-agent-fleets.md) (tracked as EPIC-48); falsifiable experiments are in [docs/research/ablations/](docs/research/ablations/). Everything below describes the *instrument*.

**AIDA is your project's missing index — of intent, not just code.** A hidden kernel that maintains a stable, queryable graph of what exists *and why*, served to AI through MCP and to you through a small CLI. (Auto-derived code-graph tools index what the code *is*; AIDA indexes what it was *for* — and keeps the two linked.)

**Without it**, coding agents start every session cold, re-deriving the same context they had yesterday; humans rediscover and re-debate decisions for years; cross-references between code and intent rot silently. **With it**, *"does this already exist?"*, *"why did we choose X?"*, and *"is this code still tied to a live requirement?"* are one query away — for the agent and for you.

<!-- trace:TASK-885 -->
For day-to-day usage see the [CLI reference](docs/cli/README.md). For project conventions, build commands, and developer workflow see [CLAUDE.md](CLAUDE.md). For getting set up see [docs/getting-started.md](docs/getting-started.md). For *"how does AIDA fit alongside X?"* — neighbor-by-neighbor comparisons against `/ultrareview`, Karpathy-style structured markdown, Linear/Jira, etc. — see [docs/positioning/](docs/positioning/).

---

## Vision

The defensible niche is the **agent-collaboration layer**: stable spec IDs, typed relationships, code-to-spec trace comments, and an MCP server that exposes the requirement graph to coding agents. Karpathy-style "structured markdown queryable by Claude" is the floor; AIDA is the *durable index* on top of it. Its nearest competitor, GitHub Spec Kit, produces structured specs per feature and then freezes them — AIDA's delta is keeping them a maintained, cross-cutting graph (stable IDs + typed relationships + enforced traces + lifecycle, queryable via MCP, portable across vendors because it lives in git) that outlives any single feature. A small invisible kernel that captures what exists, plus optional layered modules for everything else. (See [docs/positioning/vs-spec-kit.md](docs/positioning/vs-spec-kit.md) and the current [competitive synthesis](docs/competitive-analysis/2026-05-31-round2-moat-gaps-moves.md).)

---

## The niche, concretely

> Stated in the probe's voice: this is the niche the *evidence* points to, not a sales claim. Backing: the theory paper [§7 (the apex claim) + §15](docs/research/2026-06-16-coordinating-multi-vendor-agent-fleets.md) and the two ablations cited below.

**What it's for.** AIDA is the neutral, **cross-vendor intent + coordination substrate** for a multi-vendor agent fleet — the shared graph of *why* (specs, typed relationships, code-to-spec traces, an enforced lifecycle) that any vendor's agent (Claude, Codex, …) reads and writes through one CLI/MCP surface, plus the coordination layer that lets those agents and a human share one workspace: leases, queue, mailbox, role/RBAC gates. One mailbox, one role queue, one lease table, one intent graph — vendor-agnostic by construction.

**Why it's defensible.** Neutrality. No single model vendor is incentivized to build *portable* cross-vendor coordination, because portability dissolves the within-vendor lock-in that vendor coordination exists to create (P8a, grade (M) — the apex claim). And the substrate is the lever: the open-brief bake-off ([2026-06-18-open-brief-convergence.md](docs/research/ablations/2026-06-18-open-brief-convergence.md)) found two vendors handed the same open brief **converge on the same design because the shared substrate dictates the shape** — so owning the substrate shapes the fleet's output regardless of which vendor executes. That makes the neutral substrate-owner, not any one vendor, the party positioned to coordinate the fleet.

**The concrete embodiment.** `aida compete` runs a spec through N vendors in isolated worktrees and lets an objective gate + a judge pick the best. The same bake-off found designs converge but *execution quality varies* — so multi-vendor competition pays as **quality-variance QA** (selection + verification + regression-catching), not as design diversity, and only a neutral substrate-owner can offer it. Around it sits the spec-quality loop (`aida spec dryrun` / `aida spec interview`, which pre-check and resolve a spec's implementer-readiness) and code-to-spec inline traces (`// trace:SPEC-ID`, machine-checkable from either direction) — the latter the uncontested wedge ([§15 Tier 1](docs/research/2026-06-16-coordinating-multi-vendor-agent-fleets.md): found in *none* of the dozen-odd tools surveyed).

**Honest scope.** The roll-your-own verdict stays open (§12) and AIDA is one member of a small family (Gas Town/Beads, GNAP) — not a singular insight. Selective gating, not blanket: a programmatic gate beats a stated rule only when the invariant sits *far from the point of action* (attention-distance; [2026-06-18-gate-vs-rule-pilot.md](docs/research/ablations/2026-06-18-gate-vs-rule-pilot.md) falsified the blanket form). Every claim above traces to a finding or a shipped command.

---

## Public face: the TUI is the product (the platform is what makes it work)

The visible product — what users install, what they look at, what they tell their friends about — is **a TUI that wraps Claude Code sessions** ([EPIC-26](docs/positioning/) for the implementation track). It hosts Claude Code as a child process. Drop out to a status overlay. Drop back in to the same conversation. Quick-action review, queue, merge, pull. List and switch between multiple Claude sessions. *That's the visible product.*

The intended reaction on first sight is **"so what? I could write that in a 20-line bash script."**

That reaction is a feature, not a bug. Three reasons:

1. **Low barrier to adoption.** Anyone who looks at AIDA's TUI sees a tool that does what they could imagine doing themselves. No learning curve to be convinced it's worth trying. They install it because it's *obviously easy*, not because they've been sold on a platform vision.

2. **The depth is what they discover after.** Once installed, the TUI's status overlay surfaces stable spec IDs, MCP-served requirement graphs, typed relationships, auto-bump lifecycle, queue routing, role-pure sessions, plan templates, integration recommendations for `/ultraplan` and `/ultrareview`, telemetry-informed deprecation hygiene, the auto-queued reviewer hand-off, the worktree-isolated implementer sessions, the orphan-store provenance trail. None of that is visible up front. It's all underneath the "trivial wrapper."

3. **The platform is the durable value; the TUI is just the surface that exposes it.** If a competing tool ships a similar TUI tomorrow, they'd need to also ship: the YAML-canonical store with serializable IDs, the cache + projection model, the node-aware identity scheme with merge-gate promotion, the MCP server, the trace-comment convention, the relationship graph, the role/session/worktree model, the scaffolded skill set, the auto-bump lifecycle, the integration framework. Months of foundational work. The TUI on top of all that is the easy part; building all that without the TUI is what people who try the bash-script version will discover they're now signed up for.

This is the **Trojan-horse positioning**: ship the visible product as humble. Let people install it because it looks simple. Let them discover, over weeks of use, that they're now using a platform with no equivalent. Don't try to convince anyone of the platform upfront — convince them through their own experience.

> *The TUI is what people will think AIDA is. The platform is what AIDA actually is.*

This framing intentionally shapes documentation, marketing, and prioritization: when adding features, the test isn't "is this visible in the TUI?" — it's "does this make the TUI's quiet depth stronger when someone digs in?" See our standalone [Strategic Positioning Statement](docs/competitive-analysis/positioning.md) for the 8 core pillars of this defensible niche.

For the implementation track + child STORYs see EPIC-26 in the requirements DB (`aida show EPIC-26`). For the TUI itself — hosting model, keybindings, status overlay, autonomous drains, crash recovery — see [`docs/tui/README.md`](docs/tui/README.md). Launch it with `aida tui` (shipped default-on as of STORY-137).

---

## AIDA in the Claude ecosystem: vertical depth on horizontal ground

A newcomer evaluating AIDA in mid-2026 has reasonable confusion to resolve: *Anthropic ships a lot. Claude Code already has so many capabilities. Why does AIDA exist?* This section names the breadth, names the structural relationship, and names the bet AIDA is making.

### The Claude ecosystem in 2026

Anthropic has shipped a striking density of primitives in the last six months. As of 2026-05-14, an incomplete inventory:

| Primitive | What it provides |
|---|---|
| **Claude Code** (CLI + web + IDE extensions) | The substrate — a coding agent that runs in your terminal, in the cloud, or in your editor |
| **`/ultraplan`** (research preview) | Cloud-based multi-agent plan generation (3 explorers + 1 critic); browser review surface; teleport-back-to-terminal |
| **`/ultrareview`** (3 free uses then quota) | Cloud-based multi-agent code review |
| **`/goal`** (2.1.139, 2026-05-12) | Set completion condition; loop until met; small evaluator decides; exits when done |
| **`/schedule`** | Cadence-driven Claude invocations (nightly, morning, weekly) |
| **Auto mode** (Shift+Tab in CLI) | Permission posture for long-running autonomous work |
| **Agent view** | Visual observability for autonomous runs |
| **Remote Control** | Browser-driven control of local Claude Code |
| **MCP (Model Context Protocol)** | Open protocol for exposing tools + resources to Claude |
| **Claude Code on the Web** | Cloud-hosted Claude Code sessions, github-integrated |

The cadence is real and the surface is genuinely useful. A reasonable first impression: *"Anthropic is shipping the platform; do I need anything else?"*

### Horizontal vs vertical: where AIDA sits

The Claude ecosystem's primitives are deliberately **horizontal** — generic, composable, workflow-agnostic. `/goal` works for "all tests pass" the same way it works for "all dependencies upgraded." `/schedule` runs whatever you point it at. MCP exposes any tool you can describe in a schema. The substrate accommodates many workflows by holding no opinion about any specific one.

AIDA goes **vertical** — opinionated about *one* domain: agent-collaboration on project intent. Stable spec IDs, typed relationships, code-to-spec trace comments, an MCP server that exposes a requirement graph, a queue + role + session model for human-agent workflow, a lifecycle for shipping. The horizontal primitives are the ground AIDA stands on; the vertical depth is what AIDA contributes.

The composition is symbiotic, not competitive:

| Anthropic provides (horizontal) | AIDA provides (vertical) |
|---|---|
| `/goal` — autonomous completion loop | The **vocabulary** for machine-checkable conditions (`/goal all specs tagged batch:X are Completed` is precise; `/goal make the queue empty` is vague and loops forever) |
| `/ultraplan` — dense plan generation | The **persistence + graph linkage** (plans live in `docs/plans/`, pinned to specs, surfaced in session manifests, verified by `aida plan verify`) |
| `/ultrareview` — multi-agent review | The **lifecycle hooks** (`/aida-review` walks each linked spec's acceptance; queue done flips status; auto-bump fires on merge) |
| MCP — tool/resource protocol | The **content** served over MCP (requirements, relationships, history, comments) |
| Claude Code — agent runtime | The **shared workspace** the agent collaborates on (graph + IDs + traces) |

### Why this composition is likely to remain stable

AIDA's bet is that **Anthropic has structural reasons to stay horizontal**, leaving vertical territory for tools like AIDA. Three reinforcing forces:

1. **Verticals shrink the market.** A "simple project tracker built into Claude Code" would lock users into Anthropic's specific opinions about how to track work. Many users have existing tools (Linear, Jira, GitHub Issues, Notion); a built-in vertical would compete with all of them, reducing Claude Code's appeal as a substrate that fits any workflow. Horizontal primitives compose with whatever the user already uses.

2. **The reusable primitives compound; the verticals don't.** `/goal` works for coding, ops, content review, data work, research workflows. A "Claude requirements graph" would only matter to teams using requirements graphs. The horizontal investment has higher ROI per Anthropic engineer-hour.

3. **Competing with integration partners is a known platform anti-pattern.** Anthropic benefits from a rich ecosystem of integrations. Going too vertical means competing with the people building the ecosystem — which historically leads to platform decline. Anthropic's published interest in MCP (an open protocol explicitly for external integrations) signals they understand this.

The exceptions are interesting: `/ultraplan` and `/ultrareview` ARE semi-vertical (they're opinionated about phases of work). But they're opinionated about *universal* phases (planning, reviewing) — every project plans and reviews. They're not opinionated about *what* you track or *how* your team structures intent. That stays open.

### What this means for AIDA's roadmap

The strategic implication: AIDA should keep doing the vertical depth that the horizontal primitives can't reach. Concretely:

- **Don't compete with `/goal`** — compose with it via `aida goal` ([TASK-242](docs/plans/)), which derives machine-checkable conditions from the requirement graph
- **Don't compete with `/ultraplan`** — compose with it via `aida ultraplan SPEC` ([TASK-113](docs/plans/)) for prompt assembly + `/aida-import-plan` ([TASK-114](docs/plans/)) for output persistence
- **Don't compete with `/ultrareview`** — compose with it via `/aida-review`'s spec-walk + adversarial-pass discipline (STORY-109, shipped) that uses requirement metadata `/ultrareview` doesn't know about
- **Don't compete with Claude Code's session model** — extend it via worktree-isolated implementer sessions, role-pure boundaries, queue routing, the (planned) TUI that hosts Claude Code itself ([EPIC-26](docs/positioning/))
- **Compete vertically only where the platform structurally won't go** — the graph, the IDs, the trace network, the queue/role/session opinions, the requirement lifecycle

### The risk + how AIDA mitigates

Two risks worth naming explicitly:

**Risk 1: Anthropic ships a vertical that overlaps AIDA's core (a built-in requirement graph, a `/track` command, etc.).** Mitigation: AIDA's core value is the *composition* of graph + IDs + traces + MCP + queue + lifecycle. A built-in graph alone wouldn't replicate the trace network or the lifecycle. A built-in lifecycle alone wouldn't have the graph. AIDA's moat is the multi-layer stack, not any one feature.

**Risk 2: AIDA's CLI surface keeps growing as Anthropic adds primitives, leading to confused users.** Mitigation: the Trojan-horse TUI positioning ([EPIC-26](docs/positioning/)) collapses the surface back into one coherent visible product. *"You see a TUI wrapping Claude Code. The platform is what you discover."*

### Summary

AIDA's bet is **vertical depth on horizontal ground**: Anthropic ships the substrate; AIDA composes the substrate into a specific workflow domain (agent-collaboration on project intent). The bet stays sound as long as Anthropic stays horizontal, and Anthropic's structural incentives push them to stay horizontal. The TUI ([EPIC-26](docs/positioning/)) is the visible product; the platform is the vertical depth; the Claude ecosystem is the ground both stand on.

---

## Architecture

### Storage (EPIC-1-001) — git-canonical by default

The orphan branch `aida-store` is the writer of record. Each requirement is one YAML file at `objects/<TYPE>/000/<SPEC-ID>.yaml`, committed to that branch. Writes go to git first, then to a local SQLite cache (`.aida/cache.db`, gitignored, rebuildable).

- **Live worktree:** `.aida-store/` (gitignored on the main branch; populated by `aida init`)
- **Branch:** `aida-store` on origin
- **Cache:** `.aida/cache.db` — read projection for fast `list / search / filter`; auto-rebuilt when the cache's recorded HEAD doesn't match the orphan's HEAD
- **Sync:** `aida db sync --pull --push` (uses `git pull --rebase` under the hood for linear orphan history)

The legacy centralized SQLite path (`aida init --centralized`) still exists but prints a deprecation warning. PostgreSQL is opt-in via the `postgres` feature flag for teams wanting a server-backed shared projection.

### Distributed identity (EPIC-1-052)

Each clone of an AIDA-using project gets a unique **node id** and writes its identity to a per-clone, gitignored `.aida-store/.aida/node.toml`. The shared `.aida-store/registry/nodes.toml` is the source of truth.

- **Node-aware ids** look like `FR-1-052` (`<TYPE>-<NODE>-<SEQ>`) — issued offline, never collide across clones.
- **Agreed ids** look like `FR-052` — promoted from node-aware ids by `aida db merge-gate` once the work merges into trunk.
- **Pre-allocated blocks** (`FR-2-005`) let a clone reserve a contiguous range of agreed ids up front so trace comments can use the short form immediately, even offline. `aida node acquire` auto-allocates the first FR block.
- **`[id_format]` policy** in `.aida/config.toml`: `node-aware-only` | `blocks-then-fallback` (default) | `blocks-only`.
- **`aida init` post-clone bootstrap**: when origin already has the `aida-store` branch, `aida init` fetches it, sets up the worktree, and prompts for node-id acquisition.

### Surfaces

- **CLI (`aida`)** — primary work surface. Embeds the MCP server (`aida mcp-serve`).
- **MCP server** — exposes requirements as native Claude Code tools over JSON-RPC 2.0 stdio: the **typed, structural** surface for MCP-native clients. (AIDA's own agent-surface benchmark found the token-efficient CLI path — `AIDA_AGENT_OUTPUT`/TOON — costs roughly **half** of MCP for identical agent tasks, so the CLI is the primary agent surface and MCP is the typed option, not the cost-winner. See `bench/agent-surface/`.)
- **REST + gRPC server (`aida-server`, port 8080)** — backs the React dashboard and provides a public API.
- **React dashboard (`aida-web-react/`, port 5173 dev)** — kanban / sprint / queue / chat UI; Vite proxies `/api` to the server.

The native desktop and WASM clients (egui-based) were extracted to a separate repo on 2026-05-02. The React dashboard's keyboard navigation (`j/k`, `g+key` chords, quick pickers) covers the vi-feel use case.

### Autonomy & escalation

The autonomous-collaboration layer — the three-mode autonomy ladder (default / `--zen` / `--no-human`), the implementer → advisor → human escalation cascade, the advisor's Type A/B/C resolve-vs-escalate calibration, and the file-based handshakes that coordinate the tiers — is described in [docs/architecture/autonomy-and-escalation.md](docs/architecture/autonomy-and-escalation.md). For the practical user guide to `--auto-complete` and `--no-human` see [docs/autonomous-drain.md](docs/autonomous-drain.md); for the MCP transport layer over the same filesystem substrate see [docs/architecture/mcp-coordination-surface.md](docs/architecture/mcp-coordination-surface.md). The **resilient-drain primitive** is EPIC-28 (`docs/autonomous-drain.md` → "Shelving on failure"): a shelvable phase failure parks the spec in `NeedsAttention` with a structured `FailureReason` and the batch drain continues past it, with dependents skipped automatically via the `BlockedBy` pickability gate (STORY-333) — exit 2 + `aida findings list` for triage.

---

## Workspace layout

```
aida/
├── aida-core/             Engine — models, storage, cache, dispenser, HLC, conflict
├── aida-cli/              `aida` binary + MCP server
├── aida-crate/            Published `aida` crate metadata
├── aida-server/           REST + gRPC server (port 8080)
├── aida-generate-types/   Rust → TypeScript types (ts-rs)
├── aida-tui/              `aida tui` terminal shell — the public face (EPIC-26)
├── aida-web-react/        React 19 + Vite + Tailwind dashboard (port 5173 dev)
├── proto/                 Protocol Buffers definitions
├── docs/                  Markdown docs (incl. plans/ archive)
├── tests/                 Integration test scripts
└── (orphan branch:        aida-store — canonical YAML store)
```

---

## Feature surface (high level)

For details on any of these see the [CLI reference](docs/cli/README.md).

### Requirements

- **Types** (19): functional, non-functional, system, user, change-request, bug, epic, story, task, spike, sprint, folder, meta, principle, vision, constraint, decision, term, doc
- **Status workflows** are type-specific (e.g., the standard `draft → approved → in-progress → completed | rejected`)
- **Relationships** with typed cardinalities: parent/child, verifies/verified-by, references, duplicate, plus user-defined types via `aida rel-def`
- **Comments** with threaded replies and configurable emoji reactions
- **Custom fields** per type definition
- **Meta requirements** (META-002…006) store the AI prompt templates as editable requirements; `aida edit META-002 --description …` customizes evaluation/duplicates/relationships/improve/generate-children prompts

### Identity & traceability

- Stable node-aware ids (`FR-1-052`) and agreed-id promotion at merge-gate
- Pre-allocated blocks for offline-safe agreed ids (`aida db block claim`)
- Inline trace comments: `// trace:FR-1-052 | ai:claude[:confidence]`
- Commit message format: `[AI:tool] type(scope): description (REQ-ID)` — validated by the scaffolded commit-msg hook (`AIDA_COMMIT_STRICT=true` to reject non-conforming commits)

### AI / agent integration

- **Claude Code skills** scaffolded by `aida init` under `.claude/skills/` and `.claude/commands/` (47 skills, 48 commands as of writing): daily drivers `/aida-req`, `/aida-implement`, `/aida-commit`, `/aida-capture`, `/aida-search`, `/aida-plan`, `/aida-onboard`, `/aida-pickup`
- **Codex (AGENTS.md)** profile scaffolded in parallel; `aida init --agent codex` for Codex-only, `--agent both` (default) for both
- **MCP tools**: `list_requirements`, `show_requirement`, `add_requirement`, `update_requirement`, `search_requirements`, `add_comment`, `list_features`
- **MCP resources**: `aida://project/summary`, `aida://requirements/tree`
- **Hooks**: `aida-stop-check.sh` warns about untraced edits; `aida-session-context.sh` injects role + project context at session start
- **Roles & sessions**: `aida role` manages persistent named contexts (architect, implementer, reviewer, and **advisor** — "trusted counsel" across the project's lifetime; `dialog` is a deprecated alias for it) with per-role scope filters and Claude Code system-prompt addenda; `aida session list/resume/new` enriches Claude Code's session list with the active role and most-recent spec id
- **Statusline**: `aida statusline` is sub-50ms and suitable for `~/.claude/settings.json`'s `statusLine.command`

### Web dashboard highlights

- Kanban + list + sprint planning with drag-and-drop (@dnd-kit)
- Advanced query builder (`react-querybuilder` with json-logic), URL-persisted via `?aq=`
- Markdown rendering with auto-linked spec ids, `::color[text]` syntax, and Prism syntax highlighting
- Personal work queue (drag-to-reorder, dashboard widget, cross-user assignment via `added_by`)
- Skills browser — view/edit skills, run executable ones (e.g., compiler-warnings) with SSE streaming output
- AI Evaluate — one-click quality evaluation per requirement (Sparkles button) using the META-002 prompt
- Chat — Claude-powered Q&A with full requirements context (`ANTHROPIC_API_KEY` or runtime key via Admin)

### GitLab integration

Bidirectional sync with GitLab issues: configurable type/priority/status label mappings, content-hash-based drift detection (`aida gitlab status --diverged`), background polling, dashboard status indicators. See the GitLab use case in the user guide.

---

## Developer workflow (working *on* AIDA)

```bash
# One-time: install the `aida()` shell wrapper into ~/.bashrc or ~/.zshrc
aida dev shell-init --install

# Per-shell: activate the in-repo build (pyenv-style)
aida dev activate                      # the `aida()` wrapper auto-evals this — no `eval $(...)` needed
aida dev status                        # confirm activation, show binary mtime
aida dev serve                         # foreground supervisor: aida-server (8080) + vite (5173)
aida dev deactivate                    # the wrapper auto-evals this too
```

`aida dev activate` prepends `target/{release,debug}/` (whichever is more recently built) to PATH and prefixes the shell prompt with `(aida-debug)` or `(aida-release)`. For releases, `scripts/release.sh {major|minor|patch}` bumps the workspace version, generates tag notes, commits, tags, and pushes — triggering the release workflow that builds and publishes binary tarballs.

For project conventions (commit format, scaffold/template architecture, CLI reference) see [CLAUDE.md](CLAUDE.md).

---

## Documentation map

| File | Purpose |
|---|---|
| [README.md](README.md) | Quick start and project structure |
| [CLAUDE.md](CLAUDE.md) | Conventions, build commands, scaffold/template architecture, CLI reference |
| [docs/getting-started.md](docs/getting-started.md) | First-time setup walkthrough |
| [docs/cli/](docs/cli/README.md) | The CLI reference manual — when and why to use every command (12 chapters) |
| [docs/admin-guide.md](docs/admin-guide.md) | Storage administration, multi-user configuration |
| [docs/storage-modes.md](docs/storage-modes.md) | Deeper dive on storage modes and migration paths |
| [docs/architecture/autonomy-and-escalation.md](docs/architecture/autonomy-and-escalation.md) | The autonomy modes, the implementer → advisor → human escalation cascade, the advisor's Type A/B/C calibration, the inter-agent comms substrate |
| [docs/architecture/mcp-coordination-surface.md](docs/architecture/mcp-coordination-surface.md) | The MCP transport layer over the filesystem-canonical coordination substrate |
| [docs/autonomous-drain.md](docs/autonomous-drain.md) | Practical user guide to `--auto-complete` and `--no-human` (paired with the autonomy architecture doc above) |
| [docs/plans/](docs/plans/) | Archived implementation plans (one per `YYYY-MM-DD-<slug>.md`) |
