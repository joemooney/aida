# AIDA — Your project's missing index

**A hidden kernel that maintains a stable, queryable graph of what exists, served to AI through MCP and to you through a small CLI.**

**Without it**, coding agents start every session cold, re-deriving the same context they had yesterday; humans rediscover and re-debate decisions for years; cross-references between code and intent rot silently. **With it**, *"does this already exist?"*, *"why did we choose X?"*, and *"is this code still tied to a live requirement?"* are one query away — for the agent and for you.

```rust
// trace:FR-042 | ai:claude
fn validate_token(token: &str) -> Result<Session> { ... }
```

```
[AI:claude] feat(auth): add token validation (FR-042)
```

Every line of code links back to a requirement. Every commit references what it implements. The MCP server exposes the whole graph to any agent.

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
   ┌─────────────┐
   │ In Progress │   Claude implements · git commit
   └──────┬──────┘
          │   /aida-pr   → push branch + open PR
          ▼
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

**Working on AIDA itself?** See [CLAUDE.md](CLAUDE.md) for the full developer workflow — `aida dev shell-init --install` wires the `aida-on` / `aida-off` aliases into your shell, and `aida-on` then activates your in-repo build pyenv-style with no released binary needed.

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

After `aida init`, your first spec goes through [the lifecycle above](#spec-lifecycle): it starts in Draft, you approve it, queue it, and `aida queue work` carries it to a merged PR. See [`docs/lifecycle.md`](docs/lifecycle.md) for the full walkthrough.

## What you get

- **CLI (`aida`)** — daily-driver command-line interface
- **MCP server** — Claude Code (and any MCP client) queries requirements natively
- **Claude Code skills** — `/aida-req`, `/aida-implement`, `/aida-commit`, `/aida-capture`, `/aida-search`, `/aida-plan`, and more
- **Trace comments + commit hooks** — code-to-requirement linkage enforced at commit time
- **React web dashboard** — kanban, sprint planning, my-queue inbox, AI chat (start with `aida-server` + `cd aida-web-react && npm run dev`)
- **Stable IDs** — `FR-1-001` (node-namespaced) and `FR-1` (agreed short ID, assigned at merge-to-trunk)
- **Distributed by default** — offline-capable, multi-node, conflict-detecting via HLC timestamps + git
- **Optional integrations** — GitHub / GitLab / Jira sync, PostgreSQL backend (compile with the corresponding feature flags)

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

## Architecture (one paragraph)

Git is the canonical store: one YAML file per requirement on the orphan `aida-store` branch, sharded as `objects/TYPE/000/SPEC-ID.yaml`. A SQLite cache at `.aida/cache.db` (gitignored, auto-rebuilt) projects summary fields for fast list/filter/search. An FTS5 virtual table backs full-text search. Writes go to git first, then the cache (write-through). The cache stale-detects via the orphan branch's HEAD SHA and rebuilds when needed. PostgreSQL is opt-in via feature flag for teams wanting a server-backed shared projection. See `docs/admin-guide.md` for the full storage details and `docs/plans/2026-05-02-git-canonical-storage.md` for the design rationale.

## Platform support

Linux is the **primary platform during the alpha** ("Tier 1") — PR CI runs Linux-only for a fast ~3-5 min cycle. macOS and Windows are **Tier 2**: supported, but validated by a nightly [cross-platform CI run](https://github.com/joemooney/aida/actions/workflows/cross-platform.yml) rather than on every change, so cross-platform regressions surface within ~24h. Pre-built macOS tarballs ship with each [release](https://github.com/joemooney/aida/releases); Windows builds from source (`cargo install --git https://github.com/joemooney/aida.git aida-cli`). Releases are gated on a green cross-platform run.

## Documentation

| Doc | What it covers |
|-----|----------------|
| [Getting Started](docs/getting-started.md) | Install, init, first requirement |
| [Spec Lifecycle](docs/lifecycle.md) | Draft → Released, the verb vocabulary, and edge cases |
| [Administrator's Guide](docs/admin-guide.md) | Storage backends, migration, multi-user setup |
| [User Guide](docs/user-guide.md) | Daily-use reference for the CLI and dashboard |
| [Why AIDA?](docs/WHY-AIDA.md) | Problem statement and competitive positioning |
| [Future Vision](docs/future-vision.md) | AIDA in the agentic coding era |
| [Skills vs Commands](docs/UNDERSTANDING_SKILLS.md) | How Claude Code skills and commands differ |
| [`docs/plans/`](docs/plans/) | Implementation plan archive (chronological) |

## License

MIT
