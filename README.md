# AIDA — ask your codebase *why*

**Point at any line of code and get the decision behind it.**

```text
$ aida why src/auth.py:42
▸ src/auth.py:42

  this code exists because of STORY-118 (completed)
    Rate-limit the login endpoint
    why: We were seeing credential-stuffing attacks — throttle to 5/min per IP.
    spec: specs/STORY-118.md
```

A trace comment links the line to a spec; `aida why` reads the intent back to you, with a citation. **No database, no server, no account** — it works on a plain folder of markdown + trace comments with **zero setup**: the answer lands whether or not you've run `aida init`.

Karpathy's *LLM Wiki* showed that a folder of markdown you can **ask** beats a heavyweight app. AIDA adds the one thing a notes-wiki can't: your intent is **wired to the code**, so you can also ask *"what breaks if I change this?"* — `aida graph <id> --impact`.

### The whole thing in 60 seconds

1. Write a spec as markdown — `specs/STORY-1.md`: a title and a sentence of *why*.
2. Drop a trace comment above the code it explains — `# trace:STORY-1`.
3. `aida why <file>:<line>` — intent, instantly.

```rust
// trace:STORY-1 | ai:claude
fn validate_token(token: &str) -> Result<Session> { ... }
```

Everything else in AIDA — the typed spec graph, stable IDs that never rot, an MCP server that serves the graph to any agent, the queue and review lifecycle — is **optional depth** for when a folder of markdown isn't enough. Start with the one command above.

> **Status — honest about it:** AIDA is alpha, and began as a research probe into cross-vendor agent coordination (that thesis, and exactly what's proven vs. hypothesised, is in [Where this is and isn't proven](#where-this-is-and-isnt-proven) below). The neutral cross-vendor substrate angle — running a spec through N vendors as quality-variance QA (`aida compete`) — is the deeper bet a substrate-owner can make.

## What makes AIDA distinct

<!-- trace:TASK-289 | ai:claude -->

Most agent-collaboration tooling is a queue, a swarm orchestrator, or a one-shot planning prompt. AIDA is the durable graph of *what exists and why* — eight dimensions of a niche no single-purpose tool occupies:

1. **Spec graph as the backbone** — typed relationships (parent, blocks, relates-to) drive coordination, not a flat queue or an agent swarm.
2. **Identity stability** — `TASK-289` is `TASK-289` forever; the ID survives years, releases, and refactors, so trace comments never rot.
3. **Trace-comment enforcement** — code knows its spec and specs know their code; `// trace:TASK-289` is a checked link, not a decorative note.
4. **Discipline-first** — AIDA scaffolds the vocabulary, workflow patterns, and starter memories alongside the tools, so a project inherits the habits.
5. **Lifecycle-based roles** — implementer, reviewer, and advisor are distinct seats with deterministic handoffs, not one agent wearing every hat.
6. **Trojan-horse surface** — simple on first sight (a TUI over Claude Code sessions); the depth — graph, IDs, traces, MCP — compounds through use.
7. **Git-native** — per-session worktree isolation, orphan-branch storage for the graph, trace comments in source; no external database, no SaaS account.
8. **Single-user-first** — built for solo developers and small teams, not enterprise federation; the whole graph lives in your repo.

For deeper positioning vs specific tools, see [`docs/positioning/`](docs/positioning/README.md).

## Where this is and isn't proven

<!-- trace:TASK-403 | ai:claude -->

AIDA is **alpha-quality**. The strongest empirical evidence so far comes from this repository's own multi-agent dogfood on 2026-05-22, where three different coding agents (Claude Code, Codex CLI, Antigravity CLI) coordinated on this project via AIDA's MCP server. In a single 24-hour window:

- 30+ PRs merged through the autonomy keystone plus manual recovery flows.
- Each agent surfaced bugs in shared substrate that the others missed.
- Reliability fixes (auto-clean dormant leases, programmatic queue-done gate, structured-disable for headless AskUserQuestion, transient-error retry layer) shipped *through the system they fix* — dogfood-merge as validation primitive.
- The substrate (spec graph, leases, findings, punts, directives) held under concurrent multi-agent operation without state corruption.

**Proven on this project:**

- Multi-agent coordination via MCP works — three agents, no SaaS, no cross-agent state corruption.
- Spec graph survives refactors, renames, and multi-author commits across months.
- Recovery flows close the known failure modes (orphan leases, missed PRs, transient API errors, worktree-merge friction).
- Substrate holds under unattended overnight drains — measured ~75-85% per-spec success rate, with all failures recoverable in a few mechanical commands.
- AI-assisted code is auditable — every line traces to a spec, every commit names what it implements.

**Not yet proven (hypothesis):**

- Scales to N developers on one project. Today's evidence is single-user multi-agent.
- Production-stable unattended runs. The ~15-25% per-spec recoverable-failure rate is fine with discipline; it's not "set it and forget it" yet.
- Throughput-vs-reliability tradeoff is net-positive vs a human-only baseline. We don't measure this rigorously yet.
- Agent-agnostic positioning works across agents we haven't tested. Verified: Claude Code, Codex CLI, Antigravity CLI. Untested: Cursor, Continue, others.
- Value sustains across project domains. One project (this AI-tooling repo) is the empirical sample.

**Skeptical of AI reasoning?** AIDA's value doesn't require LLMs to reason from first principles. The substrate makes AI-assisted code *auditable*: every line points back to a spec, every commit names what it implements, every spec has a verifiable status. If you treat LLMs as competent pattern-matchers within bounded scope, the substrate is the audit trail that makes the pattern-matching verifiable.

**The honest pitch:** AIDA is worth the overhead when value compounds — multiple agents, multiple sessions, cross-month continuity, multi-developer handoffs. It is plausibly *not* worth the overhead for a solo developer on one-off work with no agents involved. The substrate scales sub-linearly with project size and super-linearly with concurrent collaborators (human or AI).

## Spec lifecycle

<!-- trace:TASK-273 | ai:claude -->

Every spec in AIDA travels the same path, from "filed an idea" to "users have it." First-time users hit this path as implicit knowledge that takes hours to absorb — this section is the map. The deep dive (cluster PRs, parallel pipelining, autonomous drains) lives in [`docs/lifecycle.md`](docs/lifecycle.md).

```
   ┌─────────┐
   │  Draft  │   filed, not yet agreed
   └────┬────┘
        │   aida edit SPEC --status approved
        ▼
   ┌──────────┐
   │ Approved │   agreed — ready to schedule
   └────┬─────┘
        │   (optional) aida edit SPEC --status planned  ·  /aida-plan
        ▼
   ┌─────────┐
   │ Planned │   scheduled into a sprint or cycle
   └────┬────┘
        │   aida queue work SPEC   → spawns a Claude session in a fresh worktree
        ▼
   ┌─────────────┐                ┌─────────────────┐
   │ In Progress │ ──aida punt──► │ Needs Attention │  paused — an agent
   └──────┬──────┘                └─────────────────┘  punted a design-fork
          │   /aida-pr   → push branch + open PR        it couldn't resolve;
          ▼                                             awaits triage
   ┌────────┐
   │  Done  │   PR open on GitHub, awaiting CI + review
   └───┬────┘
       │   /aida-review → approve · gh pr merge --squash · aida pull
       ▼
   ┌───────────┐
   │ Completed │   merged to main — auto-bumped by aida pull
   └─────┬─────┘
         │   make release-minor   → aggregates many completed specs
         ▼
   ┌──────────┐
   │ Released │   version tagged, binaries published
   └──────────┘
```

### What "shipped" actually means

"Ship" is fuzzy across the software industry. AIDA names six distinct steps between a local commit and an installable version — when unsure, use the precise verb. Each links to its command in [`docs/lifecycle.md`](docs/lifecycle.md):

| Verb | Where the work is | Spec status |
|------|-------------------|-------------|
| [**Committed**](docs/lifecycle.md#committed) | Local git history | In Progress |
| [**Pushed**](docs/lifecycle.md#pushed) | On `origin` (the remote) | In Progress |
| [**PR opened**](docs/lifecycle.md#pr-opened) | A GitHub PR exists, awaiting CI + review | Done |
| [**Reviewed**](docs/lifecycle.md#reviewed) | A reviewer rendered a verdict (approved / changes requested) | Done |
| [**Merged**](docs/lifecycle.md#merged) | The PR squashed onto `main` | Completed — via auto-bump on `aida pull` |
| [**Released**](docs/lifecycle.md#released) | A version is tagged and binaries published | cross-spec — many merges aggregate per release |

The default meaning of "shipped" in AIDA's docs is **merged to `main`** — the developer-facing "out the door." For "available to download with a version number," say **released** (e.g. v0.8.0). A merge does not auto-release.

One status sits **off** this mainline: **Needs Attention**. An autonomous drain that hits a design-fork it can't safely resolve runs `aida punt` (the [`/aida-punt`](docs/lifecycle.md#the-off-mainline-state-needs-attention) skill) instead of guessing — the spec pauses with a structured reason and surfaces in `aida findings` for triage, rather than landing a silent wrong implementation. Under a fully-headless drain (`--no-human=both`) that punt first reaches a headless **advisor** (STORY-306): it resolves the fork where the answer is grounded in a recorded principle or preference, and escalates only what genuinely needs a person — shrinking the morning queue without ever guessing past a fork. Run `aida advisor register` from your live advisor session to opt into **fork-from-live** (STORY-360) — the headless advisor boots from a copy of your live transcript instead of cold-booting on substrate alone, so the overnight judgement carries the in-flight context you have been building up. See [`docs/autonomous-drain.md`](docs/autonomous-drain.md#fork-from-live-full-in-flight-context-for-the-headless-advisor-story-360) for the trade-offs.

### Commands at each stage

| Transition | Command | What it does |
|------------|---------|--------------|
| File a spec | `aida add --title "..." --type task` | Creates the spec in Draft (or Approved with `--status approved`) |
| Queue it for a role | `aida queue add SPEC --for implementer` | Routes the spec to a role's work queue |
| Pick it up | `aida queue work SPEC` | Spawns a Claude session in a fresh git worktree |
| Open a PR | `/aida-pr` *(inside the session)* | Pushes the branch, opens the PR, queues the reviewer |
| Review it | `/aida-review` *(inside a reviewer session)* | Reads the diff, renders an approve / request-changes verdict |
| Merge it | `gh pr merge N --squash --delete-branch` | Lands the work on `main` |
| Promote the status | `aida pull` | Detects the merge, auto-bumps the spec Done → Completed |
| Release | `make release-minor` | Bumps the version, tags, pushes, publishes binary tarballs |
| Do all of it at once | `aida queue work SPEC --auto-complete` | Runs the whole lifecycle — implement → CI → review → merge → pull → build — as a single command |

## Quick start

### Install

Two paths: a **prebuilt binary** (the primary alpha path — no Rust toolchain needed) or a **build from source** (for platforms outside the release matrix, or for local development).

#### Prebuilt binary

The install script auto-detects your platform, downloads the matching release tarball, and drops `aida` into `~/.local/bin/`:

```bash
curl -sSL https://raw.githubusercontent.com/joemooney/aida/main/scripts/install.sh | bash
```

Pin a specific release or change the install prefix:

```bash
curl -sSL https://raw.githubusercontent.com/joemooney/aida/main/scripts/install.sh | bash -s -- --version v0.4.0
curl -sSL https://raw.githubusercontent.com/joemooney/aida/main/scripts/install.sh | bash -s -- --prefix /usr/local/bin
```

Prefer to do it by hand? Download the tarball for your platform straight from the [releases page](https://github.com/joemooney/aida/releases). Each tarball unpacks to `aida` and `aida-server` in the current directory.

**Linux** (Tier 1 — primary alpha target):

```bash
curl -L https://github.com/joemooney/aida/releases/latest/download/aida-linux-x86_64.tar.gz | tar xz
./aida --version            # verify it runs
mv aida ~/.local/bin/       # or: sudo mv aida /usr/local/bin/
```

On arm64, swap `aida-linux-x86_64` → `aida-linux-arm64`.

**macOS** (Tier 2 — nightly-validated):

```bash
curl -L https://github.com/joemooney/aida/releases/latest/download/aida-darwin-arm64.tar.gz | tar xz
./aida --version
mv aida ~/.local/bin/
```

On Intel Macs, swap `aida-darwin-arm64` → `aida-darwin-x86_64`.

**Windows** (Tier 2): no prebuilt tarball ships yet — [build from source](#build-from-source) below.

Tier 1 vs Tier 2 is the [platform support](#platform-support) story: Linux is exercised on every PR, macOS and Windows by a nightly cross-platform CI run. After install, `aida upgrade` is the one-command path to future versions. Make sure your chosen install prefix (e.g. `~/.local/bin`) is on your `PATH`.

#### Build from source

Requires a recent stable Rust toolchain (the workspace uses edition 2021). `protoc` (the Protocol Buffers compiler) is needed **only** for the server / gRPC features — `apt install protobuf-compiler` or `brew install protobuf`; the default CLI build does not need it.

```bash
git clone https://github.com/joemooney/aida.git
cd aida
make build-release
./target/release/aida --version
```

To install just the CLI straight onto your `PATH` instead — server features are off by default, so no `protoc` required:

```bash
cargo install --path aida-cli                                        # from a clone
cargo install --git https://github.com/joemooney/aida.git aida-cli   # without cloning
```

> Optional integrations (PostgreSQL, GitHub/GitLab/Jira sync) are off by default. Add `--features postgres,github,gitlab,jira` to the `cargo install` line if you need them.

**Working on AIDA itself?** See [CLAUDE.md](CLAUDE.md) for the full developer workflow — `aida dev shell-init --install` installs the `aida()` shell wrapper, and `aida dev activate` then activates your in-repo build pyenv-style (the wrapper auto-evals it) with no released binary needed.

### Bootstrap a project

```bash
cd my-project
git init                                 # if not already a git repo
aida init                                # one command: store + cache + skills + MCP

aida add --title "User auth" --type story
aida list
aida show FR-1-001
```

`aida init` creates the orphan-branch git store, a SQLite cache for fast queries, the MCP server config, Claude Code skills + commands + hooks, and a docs/plans/ archive — in one command.

After `aida init`, your first spec goes through [the lifecycle above](#spec-lifecycle): it starts in Draft, you approve it, queue it, and `aida queue work` carries it to a merged PR. The [Getting started](#getting-started-in-5-minutes) walkthrough below runs one real spec through it, end to end.

### Cloning an existing AIDA project

The requirement store rides along on the orphan `aida-store` branch, so a clone has everything — clone as usual and **reads just work**:

```bash
git clone <repo-url> && cd <repo>
aida list                                # backlog, findings, queue — all visible
```

The first read **auto-attaches** the store: it materializes the gitignored `.aida-store/` worktree from the `aida-store` branch and rebuilds the local cache. No setup step.

**Before issuing new spec ids** (i.e. before writing — `aida add`), claim a collision-free node id for your clone so your ids don't clash with other clones:

```bash
aida init                                # full setup; prompts to acquire a node id
# — or, if the store is already attached and you just need the id —
aida node acquire
```

(`.aida-store/` and `.aida/cache.db` are per-clone and gitignored; only the `aida-store` branch is shared. See [Architecture](#architecture-one-paragraph).)

## Getting started in 5 minutes

<!-- trace:TASK-274 | ai:claude -->

The [Spec lifecycle](#spec-lifecycle) above is the map. This is the same trip run for real — one concrete spec, **"User can log in with email and password,"** carried from a blank idea to merged-and-Completed. It assumes AIDA is installed and you've run `aida init` in your project ([Quick start](#quick-start) above).

> Reading time: ~4 min. Following along on your own project: ~10–15 min.

<!-- trace:BUG-597 -->

**1. File the spec.** Everything in AIDA starts as a spec.

```
$ aida add --title "User can log in with email and password" --type story
Added: STORY-1 - User can log in with email and password
```

A fresh spec lands in **Draft**. `aida add` prints one line: the new spec id and its title.

**Approving and queueing need advisor authority.** The next two steps — promoting a spec out of Draft and routing it to a work queue — are gated to the **advisor** role (or an interactive session). The role you start in by default is **implementer**, which files and implements work but doesn't approve or queue it; running these as the implementer is refused or silently downgraded, on purpose. Working solo? Prefix those commands with `AIDA_SESSION_ROLE=advisor` to wear both hats (the prefix also prints a one-line `ℹ You're operating as advisor…` reminder — harmless, just informational):

```
$ AIDA_SESSION_ROLE=advisor aida edit STORY-1 --status approved
Updated: STORY-1
```

That moves STORY-1 out of Draft — you've agreed this should happen. STORY-1 is now **Approved**: agreed, ready to schedule.

**2. Route it to a work queue.** Queues are per-role; send STORY-1 to whoever does the coding.

```
$ AIDA_SESSION_ROLE=advisor aida queue add STORY-1 --for implementer
✓ Added STORY-1 (User can log in with email and password) to queue [for:implementer]
```

**3. Pick it up.** `aida queue work` collapses pull + worktree + session + role into one command.

```
$ aida queue work STORY-1
✓ pulled aida-store (up to date)
✓ worktree   ../your-project-story-1   ·   branch story-1
✓ session    019e3a7c · role implementer · scope STORY-1
↳ launching Claude Code…
```

This drops you into a Claude Code session in a fresh git worktree — your main checkout is untouched. Inside, run the `/aida-pickup` skill: Claude reads STORY-1, writes the code with `// trace:STORY-1` comments linking each change back to the spec, and commits. STORY-1 flips to **In Progress**.

**4. Open the PR.** Still inside the session, run `/aida-pr`:

```
> /aida-pr
✓ pushed story-1 → origin
✓ opened PR #42 — https://github.com/you/your-project/pull/42
✓ queued a reviewer for PR #42
```

STORY-1 is now **Done** — *work finished on a branch*, PR open, awaiting CI and review. Done is not merged; the precise distinction is in [what "shipped" actually means](#what-shipped-actually-means) above.

**5. Review it.** Reviewing is its own session. Pick up the reviewer item the same way — `aida queue work` with no argument takes the head of your queue — then run `/aida-review` inside it:

```
$ aida queue work          # no arg = next item routed to you
✓ session    019e4b13 · role reviewer · scope PR-42
↳ launching Claude Code…

> /aida-review
✓ reviewed PR #42 — verdict: approve
  trace comments present · tests cover the new path · no findings
```

**6. Merge it.**

```
$ gh pr merge 42 --squash --delete-branch
✓ Squashed and merged pull request #42
```

**7. Watch the status land.** `aida pull` refreshes both the code and the spec store — and notices the merge:

```
$ aida pull
✓ code    fast-forwarded main (1 commit)
✓ store   up to date
↳ auto-bumps STORY-1 → Completed

$ aida show STORY-1
ID: STORY-1   Status: ✓ Completed
```

That is the whole loop: **Approved → In Progress → Done → Completed.** You never set "Completed" by hand — the merge earned it.

### Once you're comfortable: collapse it to one command

Steps 3–7 are a chain — implement, CI, review, merge, pull. Once you've run it by hand enough times to trust each stage, `--auto-complete` runs the entire chain from a single command:

```
$ aida queue work STORY-1 --auto-complete
```

It drives the implementer session, waits for CI, runs the reviewer, merges the PR, pulls, and bumps STORY-1 to Completed — no further input. Run the loop manually first so you can see each stage; reach for `--auto-complete` once the rhythm is familiar. The trade-off (interactive = better decisions, autonomous = better throughput) and the headless overnight-drain variants are covered in [`docs/lifecycle.md`](docs/lifecycle.md#autonomous-drains-and---auto-complete) and [`docs/autonomous-drain.md`](docs/autonomous-drain.md).

> **New here and want the short, friendly tour of the autonomy commands?** [`docs/aida-power-features.md`](docs/aida-power-features.md) distills the whole thought-to-merged taxonomy — `aida ship`, `aida zen`, `aida burndown run`, `aida queue integrate --watch` — into one skimmable page.

Concretely, `--auto-complete` is a process tree — the orchestrator process spawns short-lived Claude sessions for the two phases that need judgment (implementer, reviewer) and runs the deterministic steps itself:

```
   $ aida queue work SPEC --auto-complete       ← orchestrator process (your terminal)
   │
   ├─▶ Phase 1: spawn implementer Claude  ─────►  [Claude session — implements SPEC, ─┐
   │   (waits for it to exit)                     runs /aida-pr, exits]               │
   │◀──── detects exit ───────────────────────────────────────────────────────────────┘
   │
   ├─▶ Phase 2: end session + wait for CI       (deterministic — no Claude session)
   │
   ├─▶ Phase 3: spawn reviewer Claude  ────────►  [Claude session — reviews PR,  ─────┐
   │   (waits for it to exit)                     writes verdict, exits]              │
   │◀──── detects exit ───────────────────────────────────────────────────────────────┘
   │
   ├─▶ Phase 4: gh pr merge                     (deterministic)
   ├─▶ Phase 5: aida pull + auto-bump           (deterministic)
   └─▶ Phase 6: cargo build verify              (deterministic)
```

Two Claude sessions get spawned (phase 1 + phase 3), each in its own worktree; the orchestrator process itself does phases 2, 4, 5, 6 directly. `--zen` and `--no-human` change *which prompts pause for input vs auto-resolve*, not the process shape.

> **Want to see AIDA on a real project?** This walkthrough carries *one* spec. [`docs/first-project.md`](docs/first-project.md) builds a whole tiny project — a TODO CLI, six specs, a real parent/child graph driven to merged — so you can see what the graph is *for*, not just how each command works.

## What you get

- **CLI (`aida`)** — daily-driver command-line interface
- **MCP server** — Claude Code (and any MCP client) queries requirements natively
- **MCP coordination surface** — any MCP-speaking agent (Codex, Cursor, …) can participate in AIDA drains via the coordination tools (punts, findings, leases, directives). See [`docs/architecture/mcp-coordination-surface.md`](docs/architecture/mcp-coordination-surface.md).
- **Claude Code skills** — `/aida-req`, `/aida-implement`, `/aida-commit`, `/aida-capture`, `/aida-search`, `/aida-plan`, and more
- **Trace comments + commit hooks** — code-to-requirement linkage enforced at commit time
- **React web dashboard** — kanban, sprint planning, my-queue inbox, AI chat (start with `aida-server` + `cd aida-web-react && npm run dev`)
- **Stable IDs** — `FR-1-001` (node-namespaced) and `FR-1` (agreed short ID, assigned at merge-to-trunk)
- **Distributed by default** — offline-capable, multi-node, conflict-detecting via HLC timestamps + git
- **Optional integrations** — GitHub / GitLab / Jira sync, PostgreSQL backend (compile with the corresponding feature flags)

### Scripting AIDA output — the format switches when you pipe

Heads-up for shell scripts: `aida` picks its output format from whether **stdout is a terminal**. At an interactive TTY you get the human table; the moment output is **piped or captured** — `aida list | grep …`, `$(aida list …)`, a redirect to a file — it switches to the token-efficient **TOON** format (a compact `name[N]{fields}:` table). A script written against the interactive table will silently parse a different shape.

Pin the format so your script is stable regardless of TTY:

```bash
aida list --format human | awk '{print $1}'   # force the human table
aida list --format toon                        # force TOON even at a TTY
aida list --format json | jq '.[].spec_id'     # force JSON (list/search/status)
export AIDA_OUTPUT_FORMAT=human                 # or pin it for the whole script
```

Precedence: `--format` > `AIDA_OUTPUT_FORMAT` > `AIDA_AGENT_OUTPUT` > the TTY default. The first time a session auto-switches to TOON without a pin, `aida` prints a one-line reminder to **stderr** (so it never corrupts piped stdout). Full details: [`docs/environment-variables.md`](docs/environment-variables.md) (`AIDA_OUTPUT_FORMAT`).

## With Claude Code

After `aida init`, the most-used skills:

| Skill | Purpose |
|-------|---------|
| `/aida-req` | Add a requirement with AI quality feedback |
| `/aida-implement` | Implement a requirement with traceability |
| `/aida-commit` | Commit with automatic requirement linking |
| `/aida-capture` | End-of-session safety net for un-traced work |
| `/aida-search` | Unified search across requirements + code |
| `/aida-plan` | Plan implementation (vertical slice decomposition) |
| `/aida-onboard` | Interactive project walkthrough |

Run `aida` (no args) for the full CLI surface.

## How AIDA compares

<!-- trace:TASK-276 | ai:claude -->

You've tried AIDA — so *what is it actually for, next to the tools you already reach for?* [`docs/positioning/`](docs/positioning/README.md) carries one focused comparison per neighbouring tool, each answering *"why AIDA instead of X?"* — always honestly, including when X is the better call. **Start with the two nearest competitors:**

- [**AIDA vs GitHub Spec Kit**](docs/positioning/vs-spec-kit.md) — *(nearest competitor)* Spec Kit scaffolds a great first-feature `spec → plan → tasks`; AIDA keeps specs a maintained, cross-cutting graph — stable IDs, typed relationships, trace enforcement, lifecycle — for the project's whole life. They compose.
- [**AIDA vs AWS Kiro**](docs/positioning/vs-kiro.md) — *(nearest competitor)* Kiro's polished agentic IDE with EARS requirements + per-feature traceability vs AIDA's vendor-neutral, git-canonical graph readable by *any* agent over MCP, independent of the editor that produced the specs.
- [**AIDA vs Claude Code Agent Teams**](docs/positioning/vs-agent-teams.md) — native *within-session* multi-agent coordination vs AIDA's *cross-session, cross-vendor* graph + lifecycle. The closest provider overlap yet — and why the gap persists on incentive, not capability.
- [**AIDA vs Claude Code subagents / workflows**](docs/positioning/vs-claude-code-subagents.md) — within-conversation primitives vs AIDA's cross-conversation lifecycle layer; AIDA composes them inside its roles. <!-- trace:TASK-337 | ai:claude -->
- [**AIDA vs `/ultraplan` & `/ultrareview`**](docs/positioning/vs-ultraplan.md) — AIDA layers persistent requirement structure on top of Claude's planning/review primitives, which it composes with rather than replaces.
- [**AIDA vs Karpathy-style markdown**](docs/positioning/vs-karpathy-md.md) — structured markdown queryable by Claude is the floor; AIDA adds the relationship graph, stable IDs, MCP, queue, and lifecycle.
- [**AIDA vs SaaS PM tools**](docs/positioning/vs-saas-pm.md) — Linear/Jira-style PMs assume humans drive tickets; AIDA is built for agent collaboration with humans in the loop.
- **AI code-editor neighbors** — [Aider](docs/positioning/vs-aider.md), [Continue](docs/positioning/vs-continue.md) — a different layer entirely: they *edit*, AIDA is the spec graph + lifecycle *above* the editing.

**Deciding whether to adopt at all?** Two cross-cutting guides cut through it:

- [**When *not* to use AIDA**](docs/positioning/when-not-to-use-aida.md) — the honest scope limits: six cases where a neighbour tool alone is the right call, full stop.
- [**Composition recipes**](docs/positioning/composition.md) — when the answer is *"use AIDA **with** X"* (Spec Kit, Agent Teams, MCP editors, GitHub Issues…), with the concrete seam for each.

For the broader problem statement behind all of it, see [Why AIDA?](docs/WHY-AIDA.md).

## Architecture (one paragraph)

Git is the canonical store: one YAML file per requirement on the orphan `aida-store` branch, sharded as `objects/TYPE/000/SPEC-ID.yaml`. A SQLite cache at `.aida/cache.db` (gitignored, auto-rebuilt) projects summary fields for fast list/filter/search. An FTS5 virtual table backs full-text search. Writes go to git first, then the cache (write-through). The cache stale-detects via the orphan branch's HEAD SHA and rebuilds when needed. PostgreSQL is opt-in via feature flag for teams wanting a server-backed shared projection. See `docs/admin-guide.md` for the full storage details and `docs/plans/2026-05-02-git-canonical-storage.md` for the design rationale.

## Platform support

Linux is the **primary platform during the alpha** ("Tier 1") — PR CI runs Linux-only for a fast ~3-5 min cycle. macOS and Windows are **Tier 2**: supported, but validated by a nightly [cross-platform CI run](https://github.com/joemooney/aida/actions/workflows/cross-platform.yml) rather than on every change, so cross-platform regressions surface within ~24h. Pre-built macOS tarballs ship with each [release](https://github.com/joemooney/aida/releases); Windows builds from source (`cargo install --git https://github.com/joemooney/aida.git aida-cli`). Releases are gated on a green cross-platform run.

## Documentation

The full, journey-ordered index lives at [`docs/`](docs/README.md) (start there if you're browsing). The highlights:

| Doc | What it covers |
|-----|----------------|
| [Getting Started](docs/getting-started.md) | Install, init, first requirement |
| [First Project](docs/first-project.md) | Follow-along walkthrough — a TODO CLI from zero to a merged spec graph |
| [Spec Lifecycle](docs/lifecycle.md) | Draft → Released, the verb vocabulary, and edge cases |
| [Administrator's Guide](docs/admin-guide.md) | Storage backends, migration, multi-user setup |
| [User Guide](docs/user-guide.md) | Daily-use reference for the CLI and dashboard |
| [Why AIDA?](docs/WHY-AIDA.md) | Problem statement and competitive positioning |
| [Future Vision](docs/future-vision.md) | AIDA in the agentic coding era |
| [Skills vs Commands](docs/UNDERSTANDING_SKILLS.md) | How Claude Code skills and commands differ |
| [Claude Code Plugin Package](docs/agents/claude-plugin-package.md) | Marketplace/package skeleton for installing AIDA's Claude Code-facing setup |
| [`docs/plans/`](docs/plans/) | Implementation plan archive (chronological) |

## License

MIT
