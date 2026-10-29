# AIDA

AIDA is an agent-collaboration substrate for a codebase. It gives humans and
coding agents (Claude Code, Codex CLI, and any harness with a shell) one shared,
project-owned record of what is being built and why, plus the coordination
machinery that lets several agents work in the same repository without
stepping on each other.

It is alpha software, Linux-only, and released under the MIT licence.

## What it provides

- **A git-canonical requirements graph.** Specs (stories, tasks, bugs, spikes,
  epics, decisions) are YAML objects on an orphan `aida-store` branch with
  stable IDs and typed relationships. A local SQLite cache makes reads fast and
  can always be rebuilt from git. Cloning the repository brings the graph with it.
- **Code-to-intent links.** A `// trace:<SPEC-ID>` comment ties code to the spec
  it implements, and commits carry the spec ID in a trailer. `aida why
  <file>:<line>` reads that link back and shows the intent behind a line.
- **Isolated work sessions.** Each piece of work runs in its own git worktree
  (`aida worktree`, `aida agent new`), so parallel agents do not share a checkout.
- **Enforced review and merge gates.** Work lands through pull requests with
  an independent reviewer, required CI checks, and merge-hold markers for
  supervised work (`aida pr`, `aida merge-hold`).
- **Human-controlled mandates.** `aida mandate start` is a time-boxed grant,
  started and stopped only by a human at an interactive terminal, that lets the
  advisor seat ship held PRs with an attested independent approval.
- **Typed dispatch.** `aida do <SPEC>` routes a spec to the right harness based
  on its execution mode (drain, drive, guided, operator, or decide) and tells
  you what will be asked of you before anything starts. Queues, leases, briefs,
  and an inter-agent mailbox coordinate the rest.
- **CLI and MCP surfaces.** The `aida` CLI is the primary tool surface for
  CLI-capable agents and operators. `aida mcp-serve` exposes the same requirements as MCP tools
  over stdio for MCP-native clients.

## Who it is for

Developers and small teams who run one or more AI coding agents on a real
codebase and want the decisions, requirements, and in-flight work to live in
the repository rather than in chat transcripts or a separate tracker.

## Install

AIDA supports Linux only (x86_64 and arm64).

### Prebuilt binary

The install script detects your platform, downloads the matching release, and
installs `aida` into `~/.local/bin/`:

```bash
curl -sSL https://raw.githubusercontent.com/joemooney/aida/main/scripts/install.sh | bash
```

Pass `--version <tag>` to pin a release or `--prefix <dir>` to change the
install directory (`... | bash -s -- --prefix /usr/local/bin`). Make sure the
install directory is on your `PATH`. Later upgrades are `aida upgrade`.

### Build from source

Requires a stable Rust toolchain.

```bash
git clone https://github.com/joemooney/aida.git
cd aida
cargo install --path aida-cli
```

No external `protoc` is needed: protobuf definitions are compiled with the
pure-Rust `protox` crate.

## Quick start

Start with your usual conversational coding agent. Discuss a change, agree on
acceptance criteria, and ask the agent to keep the intent and implementation
linked in AIDA. You develop and review the system; the agent handles the tracking
commands as part of the work.

The [getting-started guide](docs/getting-started.md) explains this journey and
labels the one-time operator setup separately. The [product showcase guide](docs/showcase/getting-started.html)
presents the same source in a reader-friendly layout. Autonomous delivery is optional.

### Cloning an existing AIDA project

The store travels on the `aida-store` branch, so a normal clone is enough to
read it. The first read attaches the store and rebuilds the local cache:

```bash
git clone <repo-url> && cd <repo>
aida list
```

Before creating specs, follow the [operator setup guide](docs/operator-setup.md)
for the project's attachment and clone-local identity. Allocation defaults to
UUID-bound WORK IDs; global allocation requires explicit HOME enrollment. These
policy names do not describe a person's physical workplace or sharing policy.

## Finding your way around

`aida --help` lists every command; `aida <command> --help` documents each one.
Useful starting points:

| Command | What it shows |
|---|---|
| `aida status` | Where you are: session, branch, PR/CI, queue, cache |
| `aida awaiting` | Everything currently waiting on you |
| `aida ps` | Active sessions and agents across the project |
| `aida autonomy` | The autonomy ladder and the knobs for each rung |
| `aida doctor` | Report-only diagnosis of multi-agent state drift |

## Documentation

- [OVERVIEW.md](OVERVIEW.md): vision, architecture, and feature surface in depth
- [docs/what-is-aida.md](docs/what-is-aida.md): What is AIDA? (overview for managers and newcomers; a self-contained `what-is-aida.html` beside it is for offline reading)
- [docs/getting-started.md](docs/getting-started.md): setup and first steps
- [docs/cli/](docs/cli/README.md): CLI reference
- [docs/lifecycle.md](docs/lifecycle.md): the spec lifecycle
- [docs/autonomous-drain.md](docs/autonomous-drain.md): unattended queue drains
- [docs/architecture/autonomy-and-escalation.md](docs/architecture/autonomy-and-escalation.md): autonomy and escalation
- [docs/agents/aida-mcp-install-matrix.md](docs/agents/aida-mcp-install-matrix.md): MCP and client setup
- [docs/commit-trailer-convention.md](docs/commit-trailer-convention.md): commit message convention
- [CLAUDE.md](CLAUDE.md): working on AIDA itself

## License

MIT. See [LICENSE](LICENSE).
