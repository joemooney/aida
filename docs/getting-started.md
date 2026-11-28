# Build as you already work

<!-- trace:TASK-B6B1278890774EF993D2EBC327862470-106 | ai:codex -->
<!-- trace:STORY-2839 | ai:claude -->

Keep working with your conversational coding agent. Explain the change, discuss tradeoffs, inspect the implementation and decide what should happen next. With AIDA configured in the project, your agent can use its CLI or MCP tools to maintain a durable record of requirements, decisions and the work that fulfills them.

The result is the system you developed and a record of its intent: why it behaves that way, what was agreed and where the implementation came from. That record helps you maintain and enhance the system and work with other people and agents.

## 1. Install AIDA

AIDA is alpha software and supports **Linux (`x86_64` and `arm64`)**. Choose whichever installation path fits your workflow: hand a prompt to your coding agent, install a prebuilt binary from a release host, or build from source code.

### Option A: Give your coding agent an install prompt

If you are already inside Claude Code, Codex CLI, or Antigravity, paste this prompt so your agent follows the [installation instructions](../README.md#install) and [operator setup guide](operator-setup.md) for you:

> Install AIDA on this Linux machine by following https://github.com/joemooney/aida#install (`README.md#install` and `docs/operator-setup.md`). Inspect `scripts/install.sh` first, install the latest release binary into `~/.local/bin` (or build from source if we have a local checkout), verify `aida --version` is on `PATH`, and then run `aida init --help` and `aida init` in this repository. Ask me before changing shell profile files or enabling optional `--with-mcp` registration.

### Option B: Install the prebuilt binary from a release location

For end users and CI runners (no Rust toolchain required), the [release installer](../scripts/install.sh) detects your Linux architecture (`x86_64` or `arm64`), downloads the release tarball (`aida-linux-x86_64.tar.gz` or `aida-linux-arm64.tar.gz`), and installs `aida`, `aida-server`, `aida-porcelain`, and the `aida-flow` symlink into `~/.local/bin/`:

```bash
# Install the latest release from GitHub Releases into ~/.local/bin
curl -sSL https://raw.githubusercontent.com/joemooney/aida/main/scripts/install.sh | bash

# Pin a specific release tag or choose a custom install directory
curl -sSL https://raw.githubusercontent.com/joemooney/aida/main/scripts/install.sh | bash -s -- --version v0.16.0 --prefix ~/.local/bin

# Or install from a GitLab release host (or set AIDA_RELEASE_HOST=gitlab)
curl -sSL https://raw.githubusercontent.com/joemooney/aida/main/scripts/install.sh | bash -s -- --host gitlab
```

Ensure `~/.local/bin` is on your `PATH`, then verify and update later with:

```bash
aida --version
aida upgrade
```

### Option C: Build and install from source code

If you have a stable Rust toolchain and want to build directly from source (no external `protoc` binary is needed — protobuf definitions compile with the pure-Rust `protox` crate):

```bash
git clone https://github.com/joemooney/aida.git
cd aida

# Build and install aida + aida-porcelain + aida-flow into ~/.cargo/bin
make install-cli
# (or run `make install` to also build and install aida-server)
```

You can also run the underlying Cargo commands directly:

```bash
cargo install --path aida-cli
cargo install --path aida-porcelain
ln -sfn aida-porcelain "${CARGO_INSTALL_ROOT:-${CARGO_HOME:-$HOME/.cargo}}/bin/aida-flow"
aida --version
```

## 2. Initialize or join a repository

Once `aida` is on your `PATH`, initialize AIDA inside your Git repository:

```bash
aida init --help
aida init
```

The full footprint scaffolds project guidance and agent skills for selected profiles; `--agent` chooses profiles explicitly. The CLI is the default tool surface for agents with shell access. MCP registration is opt-in with `--with-mcp`; `aida mcp-serve` provides the stdio server for MCP clients. Check the [client setup matrix](agents/aida-mcp-install-matrix.md) for your harness. Agent permissions, project policy and review authority still need deliberate configuration.

Operators can follow the [supported setup and storage guide](operator-setup.md).

Before creating specs in a fresh clone of an [existing AIDA project](../README.md#cloning-an-existing-aida-project), run `aida init` (or `aida node acquire`) in the clone to establish its local identity. Follow the project's onboarding guidance and preserve existing adoption/configuration choices. Operators can consult the [setup reference](cli/07-project-setup.md) and [storage reference](cli/10-storage.md). The [first-project walkthrough](first-project.md) is a detailed command-oriented exercise for operators and agents; ordinary users can start with the steps below.

## 3. Record your first spec

<!-- trace:TASK-3960 | ai:claude -->
<!-- The bash blocks between the doc-test markers run in a fresh repository in
     tests/test_getting_started_commands.py. Keep them working as written. -->
<!-- doc-test:first-run begin -->

A spec records one piece of intended behaviour and how you will know it is done. Your agent normally writes specs for you, but you can add one yourself:

```bash
aida add "Greet by name with a configurable greeting" --description "## Acceptance
- greet() takes an optional greeting and defaults to Hello"
aida list
```

`aida add` prints `Added: <ID> - <title>`. Copy that ID: a new project with no shared remote yet issues long IDs such as `TASK-905719A8BC5840788657098B0B860D11-001`. Then read the spec back:

```bash
aida show <ID>
```

Replace `<ID>` with the ID that `aida add` printed. The `## Acceptance` section is what a review checks against.

## 4. Prepare your first review

Make a small change on a branch and commit it. For example, after editing a tracked file (run `git add` first for a new file):

```bash
git switch -c greeting
git commit -am "feat: configurable greeting"
aida review prepare --from main --specs <ID>
```

`aida review prepare` runs locally and sends nothing to an agent. It prints a report:

- **Mode**: `range` when you pass `--from main` (the commits on your branch; use your default branch's name if it is not `main`). Without `--from`, it reviews your uncommitted changes instead.
- **Linked AIDA Requirement Acceptance Criteria**: the acceptance criteria of the specs you named with `--specs`.
- **Reviewable File Inventory**: every changed file. A review must account for each one.
- **Token-Budgeted Semantic Review Batches**: the files grouped into pieces small enough for one reviewer pass.
- **Active Path-Scoped Review Playbooks**: the checklist that applies to each kind of file, such as Python or general code.

<!-- doc-test:first-run end -->

To have an agent review the batches, add `--run-review`: `aida review prepare --run-review --from main`. It uses Claude Code by default (`--agent codex` selects the Codex CLI), so that agent's CLI must be installed, on your `PATH` and signed in.

## 5. Start with a conversation

Tell your AIDA-aware agent what you want to build. For example:

> Add a way to export invoices. Before implementing it, record the agreed behavior and acceptance criteria in AIDA. Ask me about choices that affect customers.

You remain the developer. Explore alternatives, change your mind and review the result in your usual editor and pull-request workflow. The agent uses AIDA commands to read existing intent, capture agreed requirements, record decisions and connect changes to the relevant specs. You do not need to memorize requirement-mutation commands to begin.

## 6. Make the record part of the work

Ask the agent to show what it recorded before implementation. Correct missing constraints or misunderstood intent. As implementation reveals new facts, ask it to update the record and explain material changes.

At a stopping point, ask: "What intent did we capture, what did we implement, and what is still unresolved?" The capture skill can help an agent sweep the conversation for missed requirements. Capture depends on the configured agent following the project guidance and invoking the tools; AIDA does not automatically record every utterance or observe work outside those tools.

## 7. Return with context

Later, ask the agent to find the requirement or decision behind a behavior before changing it. Specs, comments, relationships and code traces provide starting points for maintenance and enhancement. A colleague can use the same project record to understand the agreed behavior and contribute a change.

The record needs care: check that acceptance criteria match the intended outcome and that the implementation and links remain current. Unrecorded decisions cannot be recovered from the store by assumption.

## 8. Choose what to delegate

You can work conversationally throughout. Autonomous delivery is an option for suitable authorized work, not a prerequisite for onboarding. People can develop, review and collaborate while agents handle tracking and selected implementation steps. Execution mode and authority are separate: selecting a mode does not grant permission to approve or merge.

Read the [AIDA overview](what-is-aida.md) for selectable autonomy and current limitations. AIDA is alpha software and supports Linux only.

## Uninstall

Back up and publish any intent history you need before removing AIDA. Remove the binary from the location where you installed it, and remove AIDA's client registration and generated guidance only after checking for local customizations. Keep your source repository and any store history you still need. For a live dev installation, inspect `aida dev status` first so you identify the active links and binaries correctly.
