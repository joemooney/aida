# Terminal User Interface Prior-Art Study

This competitive analysis study examines the terminal user interface (TUI) designs, workspace hosting models, session state management, and multi-agent workflows of six prominent AI coding agent orchestrators. The findings from this research directly inform the upcoming TUI architecture pivot, providing empirical validation for design decisions and sharpening the differentiation framing of the agent coordination substrate.

---

## 1. Project-by-Project Competitive Analysis

### Claude Squad

*   **Hosting Model**: Tmux-backed PTY hosting. It spawns persistent background `tmux` sessions for each agent to run their command-line interface.
*   **Layout**: Text-based terminal interface. Features a split-pane layout: a vertical session listing on the left, a main viewer on the right that toggles between live preview and git diff views via `tab`, and an active keyboard command menu pinned to the bottom.
*   **Keybindings**: Direct single-key hotkeys (bypassing tmux prefix keys). Uses `n`/`N` to spin up sessions, `D` to terminate, `↵`/`o` to attach, `ctrl-q` to detach, `c` to commit and pause, `r` to resume, and `↑/j`/`↓/k` for navigation.
*   **Session State**: Process state persists in background tmux sessions. Session metadata and profile configurations are stored in a simple local JSON file at `~/.claude-squad/config.json`.
*   **Multi-Agent Management**: Users navigate an active sidebar list of sessions, attaching/detaching at will. Supports distinct configurations (profiles) to switch between different agent CLIs (Claude, Aider, Codex, Gemini) from a single selection overlay.
*   **What's GOOD**:
    *   **Process survivability**: Using `tmux` as a background layer ensures that agent tasks do not crash if the user's terminal emulator or SSH connection disconnects.
    *   **Agent profile switching**: Simple, modular launch profiles in JSON make switching between different underlying LLM runners highly accessible.
    *   **Resource suspension**: The checkout/pause (`c`) and resume (`r`) flow is a clean way to freeze an agent's process lifecycle while storing progress.
*   **What's MISSING**: Complete lack of a structured, program-addressable state substrate. Agents operate in silos on independent git branches; there is no machine-level coordination or dependency mapping.

---

### Crystal (Nimbalyst)

*   **Hosting Model**: Electron desktop application. Integrates `node-pty` in a TypeScript main process to run shell/agent PTY instances, streaming terminal output to a React renderer.
*   **Layout**: Rich multi-pane desktop UI. Sidebar houses active projects, a hierarchical session folder tree, and prompt histories. The main display uses a dual-tab architecture: main workspace views (Output | Diff | Logs | Editor) on top, and a sub-tab tool panel bar below supporting multiple terminal panels per session.
*   **Keybindings**: Primarily mouse-driven with standard GUI controls, featuring keyboard shortcuts (e.g., `Cmd/Ctrl+Enter` to submit prompts) and a searchable keyboard shortcut help modal.
*   **Session State**: Fully structured SQL database. Backed by a local SQLite database (`better-sqlite3`) tracking projects, sessions, raw outputs, conversation messages, diffs, folders, and prompt navigation markers under `~/.crystal/`.
*   **Multi-Agent Management**: Parallel agent execution in dedicated git worktrees. Sessions are organized into a nested folder structure. Color-coded status badges stream active states (Initializing, Running, Waiting, Completed, Error) in real time.
*   **What's GOOD**:
    *   **Multi-instance tool panels**: Allowing multiple independent terminal panels per session enables running tests, tailing logs, and executing the agent concurrently.
    *   **Structured persistence**: SQLite database ensures structured histories, conversation trees, and tool call logs survive client restarts.
    *   **Folder hierarchy**: The ability to organize dozens of active sessions into nested directory structures scales gracefully.
*   **What's MISSING**: Lacks a collaborative coordination protocol. The workspace is a GUI wrapper over isolated processes; agents cannot interact with each other or resolve dependencies programmatically.

---

### Vibe Kanban

*   **Hosting Model**: Web Application. Spawns a Rust (`axum`) backend backed by SQLite and serves a React frontend on a local port. Can be hosted in a Docker container or accessed remotely with SSH editor integrations.
*   **Layout**: Kanban-based task interface. Organizes work visually into Kanban board columns (Backlog, Todo, In Progress, Review, Done). Selecting a card opens a dedicated workspace pane containing the terminal, a side-by-side git diff viewer, and an embedded browser preview panel with devtools.
*   **Keybindings**: Interactive web controls, drag-and-drop cards, and keyboard shortcuts for form submissions.
*   **Session State**: Persisted in a SQLite database managed by the Rust backend. Project configurations and connections are defined via environment variables.
*   **Multi-Agent Management**: Bridges task planning with execution. Starting a Kanban card automatically spawns a dedicated git worktree and branch for the agent. Users can select from 10+ pre-configured coding agent profiles.
*   **What's GOOD**:
    *   **Kanban task alignment**: Mapping agent workspaces directly to discrete task tickets on a board provides excellent structural alignment for software workflows.
    *   **Inline diff feedback**: Developers can write comments directly on lines of code in the built-in diff viewer. These comments are compiled and fed back into the agent's prompt context as corrective feedback.
    *   **Embedded app preview**: Spawning an iframe browser window alongside the terminal accelerates visual validation.
*   **What's MISSING**: No decentralized or program-addressable dependency graph. The task board structure is purely visual for humans; agents cannot programmatically create, link, or resolve card dependencies themselves.

---

### Vibe Tree

*   **Hosting Model**: Hybrid Desktop & Web. Structured as a monorepo consisting of an Electron app, a Node WebSocket/REST server, and a Progressive Web App (PWA). Utilizes the Adapter Pattern to route commands via IPC (desktop) or WebSockets (web).
*   **Layout**: Tabbed interface for multi-project support. The sidemenu manages git worktrees (dev branches), while the active workspace displays a persistent, touch-responsive terminal running the agent CLI.
*   **Keybindings**: GUI controls, standard terminal inputs, and touch-optimized gestures for PWAs on mobile browsers.
*   **Session State**: Session history, terminal outputs, and git worktree mappings are managed dynamically by the WebSocket server and configured via environment files.
*   **Multi-Agent Management**: Spawns parallel git worktrees automatically. Multi-platform access features QR-code pairing: scanning a desktop QR code from a mobile device establishes a JWT-authenticated WebSocket session to monitor and control agents remotely.
*   **What's GOOD**:
    *   **Adapter pattern abstraction**: Decoupling the UI from the transport layer (IPC vs. WebSockets) allows the same terminal and git components to run on desktop or mobile web.
    *   **QR-code pairing and mobile PWA**: Monitoring long-running agent tasks from a mobile device using a paired WebSocket connection is highly practical for developers.
*   **What's MISSING**: Primarily functions as a remote terminal multiplexer over worktrees. It does not parse semantic agent logs, track structured plan completions, or enable machine-to-machine coordination.

---

### CMux

*   **Hosting Model**: Native Shell Integration. Sourced directly into the active shell (`bash`/`zsh`) with zero external runtime dependencies.
*   **Layout**: No graphical interface. Operates directly in the terminal shell, switching active directory paths between the repository root and `.worktrees/<branch>/` subdirectories.
*   **Keybindings**: Standard terminal shell inputs, supplemented by native command-line arguments and built-in tab completion for subcommands and worktree branches.
*   **Session State**: Stored entirely in the local filesystem under the `.worktrees/` directory using native git worktree and branch data structures.
*   **Multi-Agent Management**: Spin up sessions with `cmux new <branch>` (creates branch and worktree, runs setup hooks, and invokes Claude). Switch workspaces using `cmux cd [branch]` and resume with `cmux start <branch>`. Clean up using `cmux merge` and `cmux rm`.
*   **What's GOOD**:
    *   **Bootstrapping setup hooks**: The `.cmux/setup` hook automatically handles workspace initialization (such as symlinking env secrets, installing dependencies, or running codegen) when a worktree is created.
    *   **Minimalist footprint**: Extremely fast, lightweight, and zero-dependency shell script that does not introduce background memory overhead.
    *   **Tab completion**: Native shell completion for branches makes workspace hopping fast.
*   **What's MISSING**:
    *   **High cognitive load**: Lacks a visual dashboard, making it difficult to track state, uncommitted changes, or task progress across numerous active agents.
    *   **No plan or state tracking**: Relies entirely on terminal scrollback for context.

---

### Conductor

*   **Hosting Model**: CLI Assistant Plugin. Runs inside Claude Code, Gemini CLI, or Codex as a plugin/extension that hooks into the assistant's runtime.
*   **Layout**: Non-graphical. Stored as a version-controlled directory structure inside the repository under `/conductor`.
*   **Keybindings**: Custom slash commands (e.g., `/conductor:setup`, `/conductor:new-track`, `/conductor:implement`, `/conductor:status`, `/conductor:revert`).
*   **Session State**: Persisted entirely in git-versioned markdown and JSON files (e.g., `conductor/product.md`, `tracks.md`, `tracks/<track_id>/spec.md`, `plan.md`).
*   **Multi-Agent Management**:
    *   The master file `tracks.md` registers all units of work.
    *   Enforces a strict planning protocol: setup context, draft specifications, review implementation plans, and execute tasks via `/conductor:implement`.
    *   Logical git-aware reverts: `/conductor:revert` selectively rolls back entire tracks, phases, or tasks by mapping git history back to task IDs.
*   **What's GOOD**:
    *   **Git-versioned context**: Storing specifications and task plans as human-readable markdown directly in the repo ensures they are subject to peer code review and act as a persistent source of truth.
    *   **Planning protocol enforcement**: Requiring the agent to generate and align on a detailed specification and checklist before writing code dramatically increases task success rates.
    *   **Task-to-commit mapping**: The ability to revert high-level tracks or phases rather than raw commit hashes is extremely powerful.
*   **What's MISSING**: Does not handle process orchestration or environment isolation (no worktree or PTY runners). Lacks a program-addressable dependency graph or inter-agent messaging protocol.

---

## 2. Competitive Matrix

| Project | Hosting Model | State Persistence | Workspace Isolation | Multi-Agent Orchestration | Primary Interface | Unique Advantage |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **Claude Squad** | Tmux / Go PTY | local JSON & Tmux | Git Worktrees | Manual session list | TUI (Terminal) | Process survivability via Tmux |
| **Crystal** | Electron PTY | local SQLite | Git Worktrees | Hierarchical project tree | GUI (Desktop App) | Rich dual-tab panel framework |
| **Vibe Kanban** | Rust Backend | local SQLite | Git Worktrees | Board cards to workspaces | GUI (Web App) | Inline diff review comments |
| **Vibe Tree** | Node Server | local WebSocket | Git Worktrees | QR-code paired remote sessions | GUI (Desktop & Web/PWA) | Transport-agnostic Adapter Pattern |
| **CMux** | Shell script | local Filesystem | Git Worktrees | CLI directory hopping | CLI (Shell) | Lightweight setup hooks |
| **Conductor** | Agent Plugin | Git Markdown / JSON | None (process-level) | Tracks checklist | CLI (Agent plugin) | Git-versioned spec & plan artifacts |

---

## 3. Concrete Design Recommendations

These design patterns, validated by the prior art, are recommended for implementation:

### 1. Persistent Session Isolation via tmux Backgrounding
*   **Pattern**: Run all underlying agent PTY processes in backgrounded, named `tmux` sessions. The frontend CLI/TUI connects to these processes via socket descriptors or standard stream capturing rather than running them directly in-process.
*   **Validation**: Claude Squad demonstrates that decoupling terminal rendering from process execution protects long-running agent tasks from unexpected terminal crashes, SSH dropouts, or UI refreshes.

### 2. Visually Integrated Inline Review Comments
*   **Pattern**: In the TUI/GUI code-diff viewer, allow users to select lines of code and write feedback comments. These comments should be compiled and automatically injected as high-priority corrective instructions into the agent's active prompt loop.
*   **Validation**: Vibe Kanban successfully uses this human-in-the-loop loop to drastically reduce agent correction cycles without forcing context switches.

### 3. Native Workspace Bootstrapping Setup Hooks
*   **Pattern**: When spawning a new workspace or worktree, automatically execute a project-defined initialization hook (e.g., `.aida/setup`). This hook should automate environment variables setup, dependency installation, and local secrets mapping.
*   **Validation**: CMux proved that setup hooks are essential for parallel worktree development, preventing agents from breaking on uncompiled code, missing locks, or unconfigured environment profiles.

### 4. Git-Versioned Planning and Specification Artifacts
*   **Pattern**: Maintain the plan, specs, and status of active tasks in human-readable, git-versioned markdown files under a canonical folder in the repository. The agent reads and updates these files incrementally, and changes are committed directly to the worktree branch.
*   **Validation**: Conductor validated that keeping plans and specs in git ensures the planning history is collaborative, easily reviewable by humans, and acts as a robust source of truth.

---

## 4. What AIDA Does Differently

While the prior art offers excellent visual multiplexing (Crystal, Vibe Kanban) and workspace isolation (CMux, Claude Squad), they all operate as **uncoordinated wraps** over independent agent processes. 

AIDA differentiates itself by acting as a **cooperative agent substrate** driven by a decentralized **spec-graph backbone**:

1.  **Decentralized Spec-Graph**: Rather than relying on simple text checklists (Conductor) or human-organized boards (Vibe Kanban), AIDA models the entire codebase lifecycle as a program-addressable Directed Acyclic Graph (DAG) of requirements, specifications, and tasks. Agents do not merely run side-by-side; they programmatically query the DAG, detect upstream blockers, and coordinate execution.
2.  **Machine-to-Machine Directives**: In AIDA, agents can issue structured directives, register local worker channels, and spawn sub-agents to claim adjacent branches of the spec-graph. The prior art treats multi-agent work as a series of isolated terminals managed by a human; AIDA enables autonomous, peer-to-peer delegation and status synchronization.
3.  **Substrate-Level Identity Stability**: Because AIDA maintains an underlying state database (the `.aida-store`), session states, leases, and coordination channels remain stable and consistent even as agents transition between parallel git worktrees, remote servers, or headless background runs. AIDA's agents are deeply integrated into a shared, version-controlled coordination medium.
