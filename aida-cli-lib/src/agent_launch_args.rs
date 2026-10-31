//! `aida agent_launch` clap types, owned by `agent_launch.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

// `aida session seat` subcommands.
// trace:TASK-1607 | ai:codex
#[derive(Subcommand, Debug, Clone)]
pub enum SeatCommand {
    /// Claim an empty seat, or one whose holder has provably exited. Holding
    /// it already is fine. `--force` asks a live holder to hand it over.
    Claim {
        #[clap(long, default_value = "orchestrator")]
        seat: String,
        /// Request a graceful takeover from a live holder (same as `takeover --force`).
        #[clap(long)]
        force: bool,
        /// Hard takeover. Not available yet: always refused.
        #[clap(long, requires = "force")]
        kill: bool,
        /// Seconds to wait for the holder to acknowledge (1-3600).
        #[clap(long, value_name = "SECS", requires = "force")]
        grace_secs: Option<i64>,
        /// Send the request and return without waiting.
        #[clap(long, requires = "force")]
        no_wait: bool,
    },
    /// Show the holder, its liveness, any pending takeover and in-flight work.
    Status {
        #[clap(long, default_value = "orchestrator")]
        seat: String,
        #[clap(long)]
        json: bool,
    },
    /// Give the seat up. Only the current holder can, at its current
    /// generation; with a pending takeover, pass its request ID to hand over.
    Release {
        #[clap(long, default_value = "orchestrator")]
        seat: String,
        #[clap(long)]
        generation: u64,
        #[clap(long, value_name = "UUID")]
        request: Option<String>,
    },
    /// Ask the live holder to hand the seat over, and wait for its
    /// acknowledgment. Nothing is stopped if it does not answer in time.
    Takeover {
        #[clap(long, default_value = "orchestrator")]
        seat: String,
        /// Required: a takeover asks another session to stand down.
        #[clap(long)]
        force: bool,
        /// Hard takeover. Not available yet: always refused.
        #[clap(long, requires = "force")]
        kill: bool,
        /// Seconds to wait for the holder to acknowledge (1-3600).
        #[clap(long, value_name = "SECS")]
        grace_secs: Option<i64>,
        /// Resume your own interrupted request (keeps its original deadline).
        #[clap(long, value_name = "UUID")]
        request: Option<String>,
        /// Send the request and return without waiting.
        #[clap(long)]
        no_wait: bool,
    },
    /// As the holder: confirm you wrote your handoff and stopped dispatching,
    /// and hand the seat to the pending requester.
    Ack {
        #[clap(long, default_value = "orchestrator")]
        seat: String,
        #[clap(long, value_name = "UUID")]
        request: String,
        #[clap(long)]
        generation: u64,
        /// Confirms the handoff is written and no new dispatch will start.
        #[clap(long)]
        safe_to_stop: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum SessionCommand {
    /// List recent Claude Code conversations for this project (cwd)
    /// with role + spec context extracted from each conversation's
    /// .jsonl. By default shows conversations with activity in the last
    /// 24 hours, up to 20 entries — `--all` bypasses the recency cutoff.
    ///
    /// This is the HISTORICAL view (past Claude Code conversations).
    /// For the live view of which scoped work leases are held right
    /// now, use `aida session leases`.
    // trace:BUG-98 | ai:claude
    // trace:BUG-522 | ai:claude — renamed from `session list` (the old
    // name collided with the leases view); `list` is kept as a
    // deprecated clap alias.
    ///
    /// The INITIAL TOPIC column is the conversation title Claude Code
    /// set at start — it is fixed and does NOT track current work, so
    /// for a long-running conversation it can read stale. Identify a
    /// conversation by its SPEC + AGE, not the topic.
    // trace:TASK-236
    #[clap(alias = "list")]
    Conversations {
        /// Show at most N conversations (default 20).
        #[clap(long, short = 'n', default_value = "20")]
        limit: usize,

        /// Plain output (no color), suitable for piping.
        #[clap(long)]
        no_color: bool,

        /// Bypass the default 24h recency cutoff and show every
        /// conversation the limit allows.
        // trace:STORY-59 | ai:claude
        #[clap(long)]
        all: bool,
    },

    /// Pick a session interactively and resume it via `claude --resume`.
    /// When `id` is given, resume that session directly without the
    /// picker. The picker shows the same enriched columns as `list`.
    Resume {
        /// Specific session id (or 8-char prefix) to resume. When
        /// omitted, opens an interactive picker.
        id: Option<String>,

        /// Show at most N sessions in the picker (default 20).
        #[clap(long, short = 'n', default_value = "20")]
        limit: usize,
    },

    /// Focus the terminal pane/window for a live registered AIDA session.
    // trace:STORY-995 | ai:codex
    Focus {
        /// Session id/prefix, native session id/prefix, agent name, or SPEC-ID.
        target: String,
    },

    /// Send text to a live registered AIDA session through its terminal adapter.
    // trace:STORY-995 | ai:codex
    Send {
        /// Session id/prefix, native session id/prefix, agent name, or SPEC-ID.
        target: String,

        /// Text to inject through a supported terminal adapter.
        text: String,

        /// Send Enter after the text.
        #[clap(long)]
        enter: bool,

        /// Send a mailbox notice, then nudge the session with one Enter when possible.
        #[clap(long)]
        mail: bool,
    },

    /// Launch a new Claude Code session, recording the active role + a
    /// user-chosen title so `aida session list` can show them
    /// reliably (instead of greping the auto-generated subject). Execs
    /// `claude` once the launch metadata is recorded. By default this is
    /// a faithful launch — Claude's own native permission posture (it
    /// prompts). Turn on bypass for the whole fleet with `[agents] bypass
    /// = true` in `~/.aida/agents.toml` (project `.aida/agents.toml`
    /// overrides), or per-launch with `--permission-mode`.
    // trace:FR-1-044 trace:STORY-495 | ai:claude
    New {
        /// Title for the session (shown in `aida session list`). When
        /// omitted, you'll be prompted interactively.
        // trace:BUG-1294 | ai:claude
        #[clap(long, short = 't', allow_hyphen_values = true)]
        title: Option<String>,

        /// Claude Code permission mode. When omitted, no `--permission-mode`
        /// is injected — Claude uses its native posture (the faithful
        /// default). Most common explicit values: `bypassPermissions`
        /// (no prompts), `acceptEdits` (auto-accept edits, prompt other
        /// tools), `auto` (research preview: auto-approves tool calls with
        /// background safety checks), `default` (prompt for everything),
        /// `plan`. The string is passed straight through to
        /// `claude --permission-mode`, so any value the installed Claude
        /// Code understands works.
        // trace:TASK-83 trace:STORY-495 | ai:claude
        #[clap(long)]
        permission_mode: Option<String>,

        /// Launch Claude in contained mode: strict Bash sandboxing, no
        /// unsandboxed fallback, destructive-command deny rules, and
        /// project-relative edit auto-allow only.
        #[clap(long)]
        sandbox: bool,

        /// Override the role recorded for this session (defaults to
        /// $AIDA_SESSION_ROLE).
        #[clap(long)]
        role: Option<String>,

        /// Do not set the terminal/tab title for this launch.
        #[clap(long)]
        no_title: bool,
    },

    /// Start a scoped session: create a sibling git worktree on a fresh
    /// branch, symlink AIDA state, and record a lease declaring this
    /// session owns a logical or physical scope (an EPIC, SPEC-ID, or
    /// path glob). v1 leases are advisory — `aida edit` / `aida-pickup`
    /// don't enforce them yet, but multiple sessions running concurrently
    /// can use the lease list to coordinate.
    // trace:EPIC-20 | ai:claude
    Start {
        /// Scope this session owns (alias: --spec <SCOPE>). Examples:
        ///   - `EPIC-N` / `epic-N` (resolved against the store)
        ///   - `FR-N` (any spec id)
        ///   - `src/scaffolding/**` (path glob — stored, not validated)
        ///   - `feature:auth` (free-form tag)
        #[clap(long, alias = "spec", value_name = "SCOPE", value_parser = crate::spec_arg::spec_id)]
        owns: String,

        /// Branch name for the new worktree (default: derived from --owns).
        #[clap(long)]
        branch: Option<String>,

        /// Base branch to fork from (default: current branch).
        #[clap(long)]
        base: Option<String>,

        /// Check out the existing `--branch` instead of forking a new
        /// one — the fixup-on-an-existing-PR-branch flow (e.g.
        /// `aida session start --owns PR-N-fixup --branch <impl-branch>
        /// --reuse-branch`). Errors if the branch exists nowhere.
        /// Without this flag, an explicitly-named `--branch` that
        /// already exists is reused automatically (with a hint), and an
        /// auto-derived name always forks fresh. `--base` is ignored
        /// when reusing.
        // trace:TASK-245 | ai:claude
        #[clap(long)]
        reuse_branch: bool,

        /// Worktree directory (default: sibling of project root,
        /// `<repo-parent>/<repo-name>-<scope-slug>/`).
        #[clap(long, value_name = "PATH")]
        path: Option<String>,

        /// Forge override for `PR-N` / `MR-N` scopes when the origin
        /// URL doesn't auto-detect (self-hosted GHE/GitLab, multi-remote
        /// setups, etc.). Accepts `github` or `gitlab`.
        // trace:STORY-61 | ai:claude
        #[clap(long, value_name = "FORGE")]
        forge: Option<String>,

        /// Branch naming style when --branch isn't given.
        ///   `auto` (default): try slug, then slug-2..-10, then
        ///                     slug-YYYY-MM-DD, then slug-YYYY-MM-DD-2..-10.
        ///   `date`:           always slug-YYYY-MM-DD (with -N suffix on
        ///                     collision). Useful when every session
        ///                     should be traceable to its date.
        // trace:STORY-65 | ai:claude
        #[clap(long, value_name = "STYLE", default_value = "auto")]
        branch_style: String,

        /// After creating the worktree + lease, launch Claude Code inside
        /// it: chdirs into the worktree, records launch metadata in the
        /// same log as `aida session new`, then execs
        /// `claude --permission-mode <mode>`. Collapses the usual
        /// "start → cd → session new" three-step into one command.
        // trace:STORY-54 | ai:claude
        #[clap(long, short = 'l')]
        launch: bool,

        /// Title for the launched session (only used with --launch).
        /// Shown in `aida session list`. Prompted interactively when
        /// omitted; pass an empty string to skip the prompt.
        // trace:STORY-54 | ai:claude
        // trace:BUG-1294 | ai:claude
        #[clap(long, short = 't', allow_hyphen_values = true)]
        title: Option<String>,

        /// Do not set the terminal/tab title for this launch.
        #[clap(long)]
        no_title: bool,

        /// Display name passed to Claude Code via `--name`. Shown in
        /// claude's prompt box, /resume picker, and terminal title.
        /// Without this, --launch derives a name from scope+branch+role:
        /// `review@PR-N` for reviewer sessions, `EPIC-N:batchM` for
        /// implementer epic-batch shapes, `<role>@<scope>:<suffix>`
        /// fallback. Truncated to 64 chars. Only honored with --launch.
        // trace:TASK-31 | ai:claude
        #[clap(long, short = 'n')]
        name: Option<String>,

        /// Claude Code permission mode for the launch. When omitted, no
        /// `--permission-mode` is injected — Claude uses its native posture
        /// (the faithful default; turn on bypass fleet-wide with
        /// `[agents] bypass = true`). Common explicit values:
        /// `bypassPermissions`, `acceptEdits`, `auto` (research-preview
        /// middle ground — see `aida queue work --help` for the tradeoff),
        /// `default`, `plan`. Pass-through to `claude --permission-mode`, so
        /// any installed-claude value works. Ignored without --launch.
        // trace:STORY-54, TASK-83 trace:STORY-495 | ai:claude
        #[clap(long)]
        permission_mode: Option<String>,

        /// With --launch, launch Claude in contained mode: strict Bash
        /// sandboxing, no unsandboxed fallback, destructive-command deny
        /// rules, and project-relative edit auto-allow only.
        #[clap(long)]
        sandbox: bool,

        /// Override the role recorded in this session's lease (and, when
        /// `--launch` is set, the persona the launched Claude inherits).
        /// Without `--role`, the role is derived from the scope:
        /// `--owns PR-N` / `--owns MR-N` → reviewer; everything else →
        /// implementer. `$AIDA_SESSION_ROLE` is a last-resort fallback
        /// only — when it disagrees with the scope-derived default the
        /// scope wins and a warning is printed.
        // trace:TASK-67 | ai:claude
        #[clap(long)]
        role: Option<String>,

        /// Claim a spec whose status is ambiguous: already In Progress
        /// with no local lease (another worktree or machine holds it),
        /// or NeedsAttention (punted by an autonomous agent and awaiting
        /// advisor triage). Without this flag those states refuse and
        /// ask you to triage first. Done / Completed / Rejected / Draft
        /// always refuse — `--force-claim` does NOT override them.
        // trace:BUG-379 | ai:claude
        #[clap(long)]
        force_claim: bool,

        /// Acquire the session's worktree from the warm-pool (reset a recycled
        /// tree instead of creating a fresh one — keeps the build cache warm).
        /// Overrides the `[worktree_pool] enabled` config for this run. Only
        /// affects the default new-branch flow (not --reuse-branch or PR
        /// review checkouts).
        // trace:STORY-714 | ai:claude
        #[clap(long)]
        pool: bool,

        /// Force a fresh `git worktree add` even when `[worktree_pool] enabled`
        /// is on — the opt-out counterpart to `--pool`.
        // trace:STORY-714 | ai:claude
        #[clap(long, conflicts_with = "pool")]
        no_pool: bool,
    },

    /// Register an existing Claude harness worktree lease from a SubagentStart hook.
    #[clap(hide = true)]
    HarnessWorktreeRegister {
        #[clap(long)]
        agent_id: String,
        #[clap(long)]
        cwd: String,
        #[clap(long)]
        agent_type: Option<String>,
        #[clap(long)]
        branch: Option<String>,
        #[clap(long, value_parser = crate::spec_arg::spec_id)]
        scope: Option<String>,
    },

    /// Release a Claude harness worktree lease from a SubagentStop hook.
    #[clap(hide = true)]
    HarnessWorktreeRelease {
        #[clap(long)]
        agent_id: String,
    },

    /// Record that a live seat is blocked on an unanswered permission
    /// prompt — written by the Notification(permission_prompt) hook.
    /// Internal; never called by hand.
    // trace:TASK-1454 | ai:claude
    #[clap(hide = true)]
    PendingApprovalSet {
        /// The Claude Code session id the Notification hook fired for.
        #[clap(long)]
        session: String,
        /// The tool name Claude Code is asking permission for, when known.
        #[clap(long)]
        tool: Option<String>,
        /// The raw notification message, for debugging.
        #[clap(long)]
        message: Option<String>,
    },

    /// Clear a session's pending-approval marker (if any) — written by the
    /// UserPromptSubmit/PostToolUse hooks once the block has resolved.
    /// Internal; never called by hand.
    // trace:TASK-1454 | ai:claude
    #[clap(hide = true)]
    PendingApprovalClear {
        /// The Claude Code session id to clear.
        #[clap(long)]
        session: String,
    },

    /// End a scoped session: remove the worktree, delete the lease,
    /// leave the branch alone (merge/discard is up to the user). When
    /// `id` is omitted, ends the session whose lease names this cwd's
    /// worktree.
    ///
    /// As a side effect, when the just-ended session's branch has an
    /// open PR (detected via `gh`), files a Story-typed review item
    /// routed to the `reviewer` role with `implements` relations to
    /// every spec referenced in the PR's commit messages — so a
    /// forgotten `gh pr create` doesn't leave the reviewer unaware.
    // trace:STORY-66 | ai:claude
    End {
        /// Session id (8-char prefix accepted) to end. Omit to end the
        /// session matching the current cwd. Also accepts a SPEC-ID
        /// (e.g. `TASK-<n>` — treated as `--spec`) or a branch name
        /// (e.g. `task-<n>` — treated as `--branch`) when the lease
        /// covering cwd isn't the one you want to end.
        // trace:TASK-489 | ai:claude
        // A session id or branch name is not alias-shaped, so the spec-ID
        // parser keeps it as typed.
        // trace:BUG-B6B1278890774EF993D2EBC327862470-015 | ai:claude
        #[clap(value_parser = crate::spec_arg::spec_id)]
        id: Option<String>,

        /// Resolve the lease by spec ID. Errors when zero or multiple
        /// leases own the spec; in the multi-match case lists them so
        /// you can disambiguate with the lease id.
        // trace:TASK-489 | ai:claude
        #[clap(long, conflicts_with = "branch", value_parser = crate::spec_arg::spec_id)]
        spec: Option<String>,

        /// Resolve the lease by branch name. Errors when zero or
        /// multiple leases ride that branch.
        // trace:TASK-489 | ai:claude
        #[clap(long, conflicts_with = "spec")]
        branch: Option<String>,

        /// Skip the y/N confirmation (also implied by `--force`). Required to
        /// end a session non-interactively — a non-terminal stdin errors
        /// without it rather than silently aborting.
        #[clap(long, short = 'y')]
        yes: bool,

        /// Discard uncommitted work and skip the confirmation. Discards
        /// tracked and untracked-but-not-ignored changes in the worktree;
        /// without this, `session end` refuses and prints the dirty file
        /// list. Gitignored differences (`target/`, `.aida/cache.db`, etc.)
        /// never require `--force` and are always discarded. `--force` does
        /// NOT override the live-process check; that is `--include-in-use`.
        // trace:BUG-67 | ai:claude
        // trace:TASK-487 | ai:claude
        // trace:BUG-1986 | ai:claude
        #[clap(long)]
        force: bool,

        /// Remove the worktree, or reset it back into the warm pool, even
        /// while live processes use it. Without this, `session end` refuses
        /// and names each pid and command: a live `claude` inside the
        /// worktree, or any other process with its working directory, an
        /// open or memory-mapped file, or its program inside it (a cargo
        /// build, for example). With it, live `claude` processes inside the
        /// worktree are terminated (SIGTERM, 5s grace, then SIGKILL); other
        /// processes are not killed. Your own shell, its parents and
        /// background jobs, and the helpers of the agent you are ending
        /// from (such as its MCP server) are never counted.
        // trace:BUG-61 | ai:claude
        // trace:BUG-1947 | ai:claude
        // trace:BUG-1986 | ai:claude
        #[clap(long)]
        include_in_use: bool,

        /// After removing the worktree, also delete the corresponding
        /// `~/.claude/projects/<encoded-cwd>/` directory so `claude
        /// --resume` doesn't try to revive sessions whose cwd no longer
        /// exists. Without this flag, the dir is left in place and a
        /// hint is printed pointing at `aida session prune --orphans`.
        // trace:TASK-70 | ai:claude
        #[clap(long = "purge-cc")]
        purge_cc: bool,

        /// Block until the PR's CI run reaches a terminal state (green
        /// or red) before releasing the lease — the SILENT variant:
        /// polls every 30s and prints only the poll ticks + final
        /// result. Use it to tail a long/overnight build without prompt
        /// noise. For live per-check progress instead, use `--watch-ci`.
        /// No-op when the session has no associated PR or `gh` isn't on
        /// PATH.
        // trace:TASK-111 | ai:claude
        #[clap(long, conflicts_with = "skip_ci")]
        wait_ci: bool,

        /// Block until CI reaches a terminal state — the LIVE variant:
        /// streams `gh run watch` so each check resolves on screen as it
        /// happens. Use it when you want interactive feedback; use the
        /// quieter `--wait-ci` to tail a build without the live display.
        /// Same end-session decision tree as `--wait-ci` once CI is
        /// terminal (green proceeds, red prompts).
        // trace:TASK-233 | ai:claude
        #[clap(long, conflicts_with = "skip_ci")]
        watch_ci: bool,

        /// Skip CI awareness entirely (release the lease without probing
        /// the PR's CI run). Use when CI is broken, when you're not the
        /// one who'll merge, or when the probe would be slow + you don't
        /// care.
        // trace:TASK-111
        // trace:TASK-487 | ai:claude
        #[clap(long)]
        skip_ci: bool,

        /// Return this session's worktree to the warm-pool instead of
        /// removing it (now the DEFAULT for pooled worktrees): reset it to a
        /// clean detached base and mark it idle so the next acquire reuses it
        /// warm. Kept as an explicit opt-in for back-compat; a pooled tree is
        /// returned by default unless you pass `--remove`. No-op when the
        /// worktree isn't a registered pool tree.
        // trace:STORY-714 trace:TASK-985 | ai:claude
        #[clap(long = "return")]
        return_to_pool: bool,

        /// Delete the worktree instead of returning it to the warm-pool — the
        /// opt-out now that returning a pooled tree is the default. Forces the
        /// old destroy-and-recreate teardown (`git worktree remove`).
        // trace:TASK-985 | ai:claude
        #[clap(long, conflicts_with = "return_to_pool")]
        remove: bool,
    },

    /// List active session leases — the canonical "who holds what
    /// scoped work right now" view.
    ///
    /// For the historical record of past Claude Code conversations in
    /// this project, see `aida session conversations`.
    // trace:BUG-98 | ai:claude
    // trace:BUG-522 | ai:claude
    Leases {
        /// Probe live `claude` processes and warn about ones whose cwd is
        /// `(deleted)` — worktree was removed without ending claude. `-v`
        /// adds two columns: PID of the live claude (when present) and
        /// the Claude Code session id from
        /// `~/.claude/projects/<encoded>/<id>.jsonl`.
        // trace:STORY-69, TASK-56 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long, short = 'v')]
        verbose: bool,
        /// Include stale harness leases in the listing. Every view has a
        /// state column (live ● / dormant ◐ / stale ⚠). The default view
        /// keeps every spec-scoped lease, even a stale one, because it
        /// still blocks dispatch; it hides only stale harness leases.
        // trace:TASK-55 | ai:claude
        // trace:TASK-1743 | ai:claude
        #[clap(long)]
        all: bool,
        /// Emit the lease list as JSON for machine consumers. Each lease
        /// carries a `drain` cross-reference — the orchestrator drain/run
        /// driving it (run uuid, phase, mode, orchestrator pid), or null
        /// when the lease is not the active drain member. Honors `--all`
        /// (stale leases included) but omits the cross-clone section.
        // trace:TASK-345 | ai:claude
        #[clap(long)]
        json: bool,
        /// Release every CROSS-CLONE lease claim on the shared store that
        /// the staleness predicate reports dead — past its own `ttl_secs`,
        /// or (for a process-backed claim on this host) holding a pid whose
        /// identity no longer matches. Prints scope + reason per release and
        /// never touches a claim it reports live, so there is no `--force`.
        /// This is the only supported way to clear one: `aida session end
        /// --spec` cannot reach a foreign claim (it has no local lease) and
        /// `aida session reap` requires the spec Done + the branch merged +
        /// the process exited, so the alternative is deleting
        /// `.aida-store/coordination/leases/*.toml` by hand. Opt-in by
        /// design — releasing another clone's claim is a cross-clone
        /// mutation, so it is never automatic.
        // trace:BUG-1764 | ai:claude
        #[clap(long)]
        prune_stale: bool,
        /// Skip the y/N confirmation for `--prune-stale`. Required to prune
        /// non-interactively — a non-terminal stdin errors without it rather
        /// than silently releasing another clone's claims, the same posture
        /// `aida session end` takes.
        // trace:BUG-1764 | ai:claude
        #[clap(long, short = 'y')]
        yes: bool,
    },

    /// Show details for one session lease (defaults to the lease covering
    /// cwd, or matched by ancestor PID). Accepts an 8-char id prefix.
    /// Output: scope, branch, worktree, inherited role, owner, hostname,
    /// lease file path, recent activity entries from the session log, and
    /// liveness (live claude in the worktree).
    // trace:STORY-68 | ai:claude
    Show {
        /// Session id (8-char prefix accepted). Omit to show the lease
        /// covering cwd, or — if cwd doesn't help — the lease whose
        /// creator_pid is in this shell's ancestor chain.
        id: Option<String>,

        /// Render the session's planned-cluster manifest as a status table
        /// (✓ Done / ◐ In progress / ○ Pending per item). Empty when
        /// /aida-pickup hasn't written a manifest for this session.
        // trace:STORY-98 | ai:claude
        #[clap(long)]
        plan: bool,
    },

    /// Probe whether it is safer to start a fresh agent session or compact
    /// the current conversation. This is substrate-only: it can see live drains
    /// and this session's live leases, but not conversation-only residue.
    // trace:STORY-1006 | ai:codex
    Handoff {
        /// Print the fresh-session vs compact recommendation.
        #[clap(long)]
        check: bool,

        /// Write a seat handoff note from FILE (`-` reads stdin) under
        /// `.aida/handoff/<seat>/` and update its `latest.md`. The note is
        /// capped at ~4k tokens because it seeds the next session.
        // trace:STORY-1464 | ai:claude
        #[clap(long, value_name = "FILE", conflicts_with_all = ["check", "show"])]
        write: Option<String>,

        /// Print the seat's standing orders (if any), then its latest handoff
        /// note (the seed for a fresh session).
        // trace:STORY-1464 | ai:claude
        #[clap(long, conflicts_with = "check")]
        show: bool,

        /// The seat the handoff belongs to (advisor, reviewer, product, ...).
        /// Defaults to `$AIDA_SESSION_ROLE`.
        // trace:STORY-1464 | ai:claude
        #[clap(long, value_name = "SEAT")]
        seat: Option<String>,

        /// With `--write`: this is the last handoff of the run, so an
        /// `aida agent new --rotate` supervisor exits instead of relaunching.
        // trace:STORY-B6B1278890774EF993D2EBC327862470-101 | ai:claude
        #[clap(long = "final", requires = "write")]
        final_: bool,

        /// Manage the seat's repo-local standing orders
        /// (`.aida/standing-orders/<seat>.md`): `add "<text>"` appends a dated
        /// entry, `list` prints them. `--show` prints standing orders before
        /// the handoff.
        // trace:STORY-1693 | ai:claude
        #[clap(
            long,
            value_names = ["ACTION", "TEXT"],
            num_args = 1..=2,
            allow_hyphen_values = true,
            conflicts_with_all = ["check", "write", "show"]
        )]
        standing_orders: Option<Vec<String>>,
    },

    /// Single-occupancy seats: see who drives the orchestrator loop, claim
    /// or release it, and hand it over gracefully.
    // trace:TASK-1607 | ai:claude
    Seat {
        #[command(subcommand)]
        cmd: SeatCommand,
    },

    /// Manage the planned-cluster manifest for the active session
    /// (`.aida/sessions/<id>.manifest.toml`). Used by /aida-pickup to
    /// record which items the session intends to work, and by other
    /// commands to surface "planned by another session" cues.
    // trace:STORY-98 | ai:claude
    Manifest {
        #[command(subcommand)]
        cmd: SessionManifestCommand,
    },

    /// Delete Claude Code session metadata (`.jsonl` files under
    /// `~/.claude/projects/<encoded>/`) older than N days. Walks the
    /// current project + the parent project (when run inside a session
    /// worktree). Skips any project dir corresponding to an active
    /// `aida session` lease — a defensive guard so the user can run
    /// prune from anywhere without clobbering work in progress.
    ///
    /// Default behavior shows the candidates and asks for confirmation.
    /// Use `--dry-run` to preview without prompting, `--yes` to skip
    /// the prompt and delete. Each deletion is appended to
    /// `<project>/.aida/session-prune.log` so the action is auditable.
    // trace:STORY-60 | ai:claude
    Prune {
        /// Delete `.jsonl` files older than N days. Default 30.
        #[clap(long, default_value = "30")]
        days: u32,

        /// Show what would be deleted without touching anything. Use
        /// to preview before committing to a real prune.
        #[clap(long)]
        dry_run: bool,

        /// Skip the y/N confirmation. Without `--dry-run`, deletes
        /// immediately after printing the candidate list.
        #[clap(long, short = 'y')]
        yes: bool,

        /// Sweep `~/.claude/projects/*` for project dirs whose recorded
        /// cwd no longer exists on disk (orphans from removed worktrees).
        /// When set, each whole orphaned project dir is removed instead
        /// of file-by-file pruning. Composes with `--dry-run` and `--yes`.
        /// `--days` is ignored in this mode — orphan-ness is the trigger.
        // trace:TASK-70 | ai:claude
        #[clap(long)]
        orphans: bool,

        /// Sweep lingering session worktrees from `--escalate-blocks`
        /// punts whose spec has since been triaged out of Needs Attention.
        /// The autonomous-drain escalation path deliberately leaves the
        /// worktree alive in case the advisor's resume tier needs it; this
        /// flag finds the ones that never were resumed and removes the
        /// worktree + lease + manifest. Leases whose spec is still in
        /// Needs Attention are skipped (still awaiting triage). Composes
        /// with `--dry-run` and `--yes`. `--days` is ignored.
        // trace:TASK-358 | ai:claude
        #[clap(long, conflicts_with = "orphans")]
        escalations: bool,
    },

    /// Garbage-collect stale agent isolation worktrees under
    /// `.claude/worktrees/agent-*`. CONSERVATIVE by design: a worktree is
    /// removed ONLY when ALL of these hold — its branch is merged into the
    /// default branch (or it is detached/branchless), the worktree is CLEAN,
    /// it is NOT locked, and no live process / active session-lease references
    /// it. Anything else is PRESERVED and reported, never removed. Removal uses
    /// `git worktree remove` (NOT --force), so git's own safety checks add a
    /// second guard; a worktree git refuses to remove is skipped, never forced.
    ///
    /// Always runs the lossless `git worktree prune` first (clears bookkeeping
    /// for already-deleted worktree dirs). Never runs on the hot `aida status`
    /// read path — only on this explicit operator command and on
    /// `aida session end`.
    // trace:BUG-614 | ai:claude
    Gc {
        /// Show what would be removed and what is preserved without touching
        /// anything. Use to preview before committing to a real GC.
        #[clap(long)]
        dry_run: bool,

        /// Skip the y/N confirmation and remove the eligible worktrees
        /// immediately after printing the candidate list.
        #[clap(long, short = 'y')]
        yes: bool,
    },

    /// Reap sessions that have genuinely FINISHED — remove the worktree,
    /// release the lease, and delete the branch, in one pass, so a headless
    /// agent that exited when its work landed doesn't leave a stranded
    /// worktree behind for someone to clean up by hand.
    ///
    /// A session is reaped only when ALL of these hold: its spec is Done or
    /// Completed, its branch is merged (an ancestor of the default branch, or
    /// a merged PR for the squash case, with zero unique unmerged commits),
    /// and its process has EXITED. Everything else is reported and left
    /// exactly as it is.
    ///
    /// Reads substrate state only — spec status, branch-merged-ness, process
    /// liveness. It never inspects terminal output to guess whether an agent
    /// is finished, and it NEVER closes a running agent: a session whose
    /// process is still alive owns its worktree as its cwd, so it is skipped
    /// and reaped on a later pass once it exits on its own. Dirty worktrees,
    /// locked worktrees, and branches carrying unique unmerged commits are
    /// never removed — the same safety checks `aida worktree gc` applies.
    ///
    /// A lease held on the main checkout (or another protected checkout) is
    /// never reaped, but it is RELEASED — the lease file only, never a file of
    /// the checkout — when the agent process it recorded (pid plus start
    /// identity) is provably dead: gone, or its pid reused by a different
    /// process. A lease that records only the shell that created it, or no pid
    /// at all, has an unknown owner and is released only with
    /// --release-unknown-owner. A recorded process that is still running
    /// always keeps the lease.
    // trace:TASK-1177 | ai:claude
    // trace:BUG-2197 | ai:claude
    Reap {
        /// Show what would be reaped and what is left in place without
        /// touching anything.
        #[clap(long)]
        dry_run: bool,

        /// Skip the y/N confirmation and reap immediately after printing the
        /// candidate list.
        #[clap(long, short = 'y')]
        yes: bool,

        /// Emit machine-readable JSON.
        #[clap(long)]
        json: bool,

        /// Also release main-checkout leases with an unknown owner (no
        /// recorded agent process). Without it they are listed and kept.
        #[clap(long)]
        release_unknown_owner: bool,
    },
}

/// Planned-cluster manifest subcommands. The manifest is a per-session
/// file at `.aida/sessions/<id>.manifest.toml` listing the SPEC-IDs the
/// session intends to work, written by /aida-pickup on cluster confirm.
// trace:STORY-98 | ai:claude
#[derive(Subcommand, Debug)]
pub enum SessionManifestCommand {
    /// Write (replace) the planned-cluster manifest for the active
    /// session. Pass items as a comma-separated SPEC-ID list in the order
    /// /aida-pickup intends to work them.
    ///
    /// Example:
    ///   aida session manifest write --items STORY-N,STORY-M,BUG-K
    ///
    // trace:STORY-98 | ai:claude
    // trace:TASK-487 | ai:claude
    Write {
        /// Comma-separated SPEC-IDs in planned order (position derives
        /// from order in the list, starting at 1).
        #[clap(long, value_name = "SPEC-IDS", value_parser = crate::spec_arg::spec_id_csv)]
        items: String,

        /// Plan source — typically "user prompt" (user-confirmed cluster)
        /// or "auto" (skill picked the head item). Free-form.
        #[clap(long, default_value = "user prompt")]
        source: String,

        /// Target session id (8-char prefix accepted). Defaults to the
        /// lease covering cwd / this shell's ancestor chain.
        #[clap(long)]
        session: Option<String>,
    },

    /// Mark `spec_id` as started in the active session's manifest
    /// (records started_at = now). Invoked by `aida edit --status
    /// in-progress` so the chip status flips automatically.
    // trace:STORY-98 | ai:claude
    MarkStarted {
        /// SPEC-ID to mark.
        #[clap(value_parser = crate::spec_arg::spec_id)]
        spec_id: String,

        /// Target session id (8-char prefix accepted). Defaults to the
        /// lease covering cwd / this shell's ancestor chain.
        #[clap(long)]
        session: Option<String>,
    },

    /// Mark `spec_id` as completed in the active session's manifest
    /// (records completed_at = now). Invoked by `aida queue done` and
    /// `aida edit --status completed`.
    // trace:STORY-98 | ai:claude
    MarkCompleted {
        /// SPEC-ID to mark.
        #[clap(value_parser = crate::spec_arg::spec_id)]
        spec_id: String,

        /// Target session id (8-char prefix accepted). Defaults to the
        /// lease covering cwd / this shell's ancestor chain.
        #[clap(long)]
        session: Option<String>,
    },
}

/// Launch and track external AI agent processes under AIDA supervision.
// trace:STORY-432 | ai:codex
#[derive(Subcommand, Debug)]
pub enum AgentCommand {
    /// Launch a new agent process.
    ///
    /// This lane spawns an INTERACTIVE agent in the foreground and BLOCKS until that
    /// agent's session exits. A vendor TUI does not exit when its turn ends — it returns
    /// to its prompt — so a seat that has finished its work still holds this command.
    /// Budget for that: run it where you can leave it, or stop the seat with
    /// `aida agent stop <name>` once it reports. To tell a finished seat from a working
    /// one without attaching, read the CPU column in `aida agent list`: seconds of CPU
    /// across hours of age means the seat is idle at its prompt. Do NOT rely on the
    /// `status` column for that — it reports `busy` for a seat whose worktree any live
    /// lease covers, which for the main checkout is effectively always.
    ///
    /// It is also NOT the orchestrated pipeline — it does not run CI, the reviewer
    /// phase, or the merge for you. For a supervised end-to-end drain
    /// (implementer → CI → reviewer → merge → pull), use
    /// `aida queue work <SPEC> --auto-complete` instead.
    // trace:TASK-626 | ai:claude — plain `//` keeps the marker out of `--help`.
    // trace:TASK-837 | ai:claude — the subcommand is optional so a bare
    // `aida agent new` opens an arrow-key agent-type picker instead of
    // erroring to clap help (mirrors `aida human`/`aida solo`).
    New {
        #[clap(subcommand)]
        command: Option<AgentNewCommand>,
    },

    /// Register an already-running raw-launched agent process.
    // trace:TASK-543 | ai:codex
    Register {
        /// PID of the already-running process to register.
        pid: u32,

        /// Agent type: codex, claude, antigravity, or web.
        #[clap(long = "type", value_name = "TYPE")]
        agent_type: String,

        /// Agent role: implementer, advisor, reviewer, or integrator.
        #[clap(long, value_name = "ROLE")]
        role: String,

        /// SPEC-ID currently owned by the agent.
        #[clap(long, value_name = "SPEC-ID")]
        #[clap(value_parser = crate::spec_arg::spec_id)]
        spec: Option<String>,

        /// Optional human-readable instance name.
        #[clap(long)]
        name: Option<String>,

        /// Optional human-readable description of this agent session.
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true)]
        description: Option<String>,
    },

    /// List active agent processes.
    // trace:TASK-542 | ai:antigravity
    // trace:TASK-1184 | ai:codex
    // trace:STORY-790 | ai:codex
    // trace:TASK-B6B1278890774EF993D2EBC327862470-117 | ai:codex
    #[clap(visible_alias = "ls")]
    List {
        /// Include stale registry entries instead of pruning them first.
        #[clap(long)]
        all: bool,

        /// Show only stale registry entries without pruning them first.
        #[clap(long)]
        stale: bool,

        /// Show only ended launcher sessions that carry a native resume id.
        #[clap(long)]
        ended: bool,
    },

    /// Show active agent processes (alias of `list`).
    // trace:STORY-528 | ai:claude
    // trace:STORY-790 | ai:codex
    Status {
        /// Include stale registry entries instead of pruning them first.
        #[clap(long)]
        all: bool,

        /// Show only stale registry entries without pruning them first.
        #[clap(long)]
        stale: bool,

        /// Show only ended launcher sessions that carry a native resume id.
        #[clap(long)]
        ended: bool,
    },

    /// Prune dead-PID agent registry entries.
    // trace:TASK-1184 | ai:codex
    Gc {
        /// Show what would be removed without touching registry files.
        #[clap(long)]
        dry_run: bool,

        /// Also prune resumable dead entries older than this many days.
        #[clap(long, value_name = "DAYS")]
        older_than: Option<u64>,
    },

    /// Report dispatch health and recovery hints for active agent/spec work.
    // trace:STORY-759 | ai:codex
    #[clap(name = "dispatch-health")]
    DispatchHealth {
        /// Include paused/rate-limited vendors when choosing fallback guidance.
        #[clap(long)]
        force: bool,
    },

    /// Set a dispatch marker (NOT a process pause): marks the agent busy
    /// (e.g. budget exhausted / rate-limited) so status and brief-time dispatch
    /// logic skip it. The process keeps running — use `aida agent stop` to
    /// terminate it.
    // trace:STORY-528 | ai:claude
    // trace:BUG-521 | ai:claude
    Pause {
        /// Agent name, `<type>#<pid>`, or `<type>-<pid>` id.
        agent: String,

        /// Why it is paused: budget, rate-limit, manual, or unknown.
        #[clap(long, value_name = "REASON", default_value = "manual")]
        reason: String,

        /// When the quota is expected to reset — an RFC3339 timestamp
        /// (2026-06-09T18:00:00Z) or a relative duration (2h, 90m, 45s).
        #[clap(long, value_name = "WHEN")]
        resets: Option<String>,
    },

    /// Clear an agent's paused state back to available.
    // trace:STORY-528 | ai:claude
    Resume {
        /// Agent name, `<type>#<pid>`, or `<type>-<pid>` id.
        agent: String,
    },

    /// Edit an active agent's human-readable description.
    // trace:STORY-791 | ai:codex
    Describe {
        /// Agent name, `<type>#<pid>`, or `<type>-<pid>` id.
        agent: String,

        /// New one-line description.
        description: String,
    },

    /// Stop an active agent process by name.
    // trace:TASK-542 | ai:antigravity
    Stop {
        /// Name of the agent to stop.
        name: String,
    },

    /// Arm or retire a session's mail wake (internal; `agent new` arms it).
    // trace:STORY-HOME-096 | ai:claude
    #[clap(hide = true)]
    Wake {
        #[clap(subcommand)]
        command: AgentWakeCommand,
    },

    /// List the supported agent roles in the taxonomy.
    // trace:TASK-587 | ai:antigravity
    #[clap(name = "list-roles")]
    ListRoles {
        /// Format output as JSON.
        #[clap(long)]
        json: bool,
    },
}

/// `aida agent wake` verbs.
// trace:STORY-HOME-096 | ai:claude
#[derive(Subcommand, Debug)]
pub enum AgentWakeCommand {
    /// Wake this live Codex session's thread when its own inbox has mail.
    Arm {
        /// Agent name, `<type>#<pid>`, or `<type>-<pid>` id.
        agent: String,
    },
    /// Stop waking this session; its ledger is kept.
    Retire {
        /// Agent name, `<type>#<pid>`, or `<type>-<pid>` id.
        agent: String,
    },
}

/// Agent-specific launchers. Each variant maps AIDA's supervision layer onto
/// the corresponding CLI's launch flags.
// trace:STORY-432 | ai:codex
#[derive(Subcommand, Debug)]
pub enum AgentNewCommand {
    /// Spawn Claude Code with project-correct cwd/env and registry tracking.
    Claude {
        /// Role propagated to the spawned Claude process as AIDA_SESSION_ROLE.
        #[clap(long)]
        role: Option<String>,

        /// SPEC-ID to work. Creates a scoped worktree + lease before launch.
        #[clap(long)]
        #[clap(value_parser = crate::spec_arg::spec_id)]
        spec: Option<String>,

        /// Override the focus-scope drift guard: launch on a spec
        /// outside the active `aida focus` subtree even when `[focus]
        /// out_of_scope = "block"` would refuse, and silence the `warn` nudge.
        // trace:STORY-717 | ai:claude
        #[clap(long)]
        force: bool,

        /// AIDA project root or descendant used as the launch base.
        #[clap(long, value_name = "PATH")]
        cwd: Option<PathBuf>,

        /// Claude Code permission mode passed through to `claude`. When
        /// omitted, no `--permission-mode` is injected — Claude uses its
        /// native posture (the faithful default; turn on bypass fleet-wide
        /// with `[agents] bypass = true` in agents.toml).
        // trace:STORY-495 | ai:claude
        #[clap(long)]
        permission_mode: Option<String>,

        /// Opaque Claude model name passed through as `claude --model`.
        /// Overrides `[agents.claude] model`; AIDA does not validate it.
        // trace:STORY-1003 | ai:codex
        #[clap(long, value_name = "MODEL")]
        model: Option<String>,

        /// Launch Claude in contained mode: strict Bash sandboxing, no
        /// unsandboxed fallback, destructive-command deny rules, and
        /// project-relative edit auto-allow only.
        #[clap(long)]
        sandbox: bool,

        /// Do not write/inject the AIDA launch-context snapshot.
        #[clap(long)]
        no_context: bool,

        /// Do not set the terminal/tab title for this launch.
        #[clap(long)]
        no_title: bool,

        /// Print the generated launch-context snapshot before spawning.
        #[clap(long)]
        show_context: bool,

        /// Print the complete resolved launch contract — argv, active role,
        /// prompt source, launch-context snapshot path, AIDA environment
        /// inputs, and repository guidance files (AGENTS.md/CLAUDE.md) the
        /// child would consume — then exit without spawning a process,
        /// creating a worktree/lease, flipping spec status, or any network
        /// sync. See also `--show-prompt` (just the initial prompt) and
        /// `--show-context` (the full launch-context markdown body).
        // trace:TASK-1232 | ai:codex
        // trace:TASK-1467 | ai:claude
        #[clap(
            long = "no-exec",
            visible_alias = "noexec",
            visible_alias = "print-command"
        )]
        noexec: bool,

        /// Print the exact initial prompt (explicit `--prompt` or the
        /// auto-generated one) that would be sent, then exit without
        /// spawning or any other side effect.
        // trace:TASK-1467 | ai:claude
        #[clap(long)]
        show_prompt: bool,

        /// For a launch that actually executes: print launch-time diagnostics
        /// (resolved vendor/client, role, spec/session/name, working
        /// directory, prompt source and whether context was injected,
        /// guidance files discovered, generated child argv with sensitive
        /// values redacted, process registration, and child exit/result
        /// information) before/around spawn. Distinct from `--no-exec`,
        /// which previews and exits without launching; `--verbose` launches.
        /// Never prints secrets, tokens, full prompt contents, or sensitive
        /// environment values. Normal (non-verbose) output is unchanged.
        // trace:TASK-1498 | ai:claude
        #[clap(long)]
        verbose: bool,

        /// Initial message to pass to the spawned Claude session.
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true, conflicts_with = "prompt_file")]
        prompt: Option<String>,

        /// Read the initial message from a file instead of the command line. Prefer this from
        /// orchestrators: a brief passed as `--prompt "$(cat file)"` through a nested shell can
        /// lose its quoting and silently launch an agent that sits idle at an empty prompt.
        // trace:BUG-1696 | ai:claude
        #[clap(long, value_name = "PATH", conflicts_with_all = ["prompt", "no_prompt"])]
        prompt_file: Option<PathBuf>,

        /// Do not send the automatic role-aware initial message.
        #[clap(long)]
        no_prompt: bool,

        /// Skip the ended-session resume offer.
        #[clap(long)]
        no_resume: bool,

        /// Resume the latest ended matching session, or a named ended session, without prompting.
        #[clap(long, value_name = "latest|NAME")]
        resume: Option<String>,

        /// Skip the live-duplicate confirmation prompt while still checking for duplicates.
        #[clap(long)]
        allow_duplicate: bool,

        /// Skip the live same-vendor/same-role duplicate check entirely.
        #[clap(long)]
        no_duplicate_check: bool,

        /// Do not read per-agent default flags from ~/.aida/agents.toml or .aida/agents.toml.
        #[clap(long)]
        no_default_flags: bool,

        /// Append one additional raw flag to the spawned Claude argv.
        #[clap(long = "extra-flag", value_name = "FLAG", allow_hyphen_values = true)]
        extra_flags: Vec<String>,

        /// Optional human-readable instance name.
        #[clap(long)]
        name: Option<String>,

        /// Optional human-readable description of this agent session.
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true)]
        description: Option<String>,

        /// Dispatch to Claude Code's background supervisor via `claude
        /// --bg`. The session detaches from this terminal and shows up
        /// in `claude agents` (and `aida status`). When `--spec` is
        /// also passed, AIDA records the captured sessionId on the
        /// lease so the cross-substrate view links them automatically.
        /// Without `--bg`, the foreground launch path is unchanged.
        /// For non-interactive dispatch, first set `[agents] confirm_bypass = false`
        /// in `~/.aida/config.toml`; trusted project config can still require consent.
        /// Then use: `aida agent new claude --bg --role implementer
        /// --spec <ID> --cwd <WORKTREE> --prompt-file <FILE> --no-resume
        /// --permission-mode bypassPermissions`. An explicit owning cwd reuses
        /// its inactive same-role lease; it never takes over a live agent.
        // trace:BUG-2002 | ai:codex
        // trace:SPIKE-34 | ai:claude
        #[clap(long = "bg")]
        bg: bool,

        /// Supervise the session across context rotations: when it exits
        /// after writing a fresh seat handoff, relaunch it in the same
        /// worktree under the same handle and lease with the handoff loaded
        /// (a 5s countdown; Ctrl-C stops it). `aida session handoff --write
        /// --final`, or an exit without a fresh handoff, ends it for real.
        /// Default: `[agents.claude] rotate` in agents.toml, else off.
        // trace:STORY-B6B1278890774EF993D2EBC327862470-101 | ai:claude
        #[clap(long, conflicts_with_all = ["bg", "noexec", "show_context", "show_prompt"])]
        rotate: bool,
    },

    /// Spawn Codex CLI with project-correct cwd/env and registry tracking.
    // trace:STORY-433 | ai:codex
    Codex {
        /// Role propagated to the spawned Codex process as AIDA_SESSION_ROLE.
        #[clap(long)]
        role: Option<String>,

        /// SPEC-ID to work. Creates a scoped worktree + lease before launch.
        #[clap(long)]
        #[clap(value_parser = crate::spec_arg::spec_id)]
        spec: Option<String>,

        /// Override the focus-scope drift guard: launch on a spec
        /// outside the active `aida focus` subtree even when `[focus]
        /// out_of_scope = "block"` would refuse, and silence the `warn` nudge.
        // trace:STORY-717 | ai:claude
        #[clap(long)]
        force: bool,

        /// AIDA project root or descendant used as the launch base.
        #[clap(long, value_name = "PATH")]
        cwd: Option<PathBuf>,

        /// Pass Codex's unsafe autonomous-mode flag.
        ///
        /// This is opt-in by design. Interactive launches keep Codex's normal
        /// approval/sandbox behavior unless the operator explicitly asks for
        /// the empirical autonomous-drain posture used in prior dogfood runs.
        #[clap(long)]
        bypass_sandbox: bool,

        /// Opaque Codex model name passed through as `codex --model`.
        /// Overrides `[agents.codex] model`; AIDA does not validate it.
        // trace:STORY-1003 | ai:codex
        #[clap(long, value_name = "MODEL")]
        model: Option<String>,

        /// Do not write/inject the AIDA launch-context snapshot.
        #[clap(long)]
        no_context: bool,

        /// Do not set the terminal/tab title for this launch.
        #[clap(long)]
        no_title: bool,

        /// Print the generated launch-context snapshot before spawning.
        #[clap(long)]
        show_context: bool,

        /// Print the complete resolved launch contract — argv, active role,
        /// prompt source, launch-context snapshot path, AIDA environment
        /// inputs, and repository guidance files (AGENTS.md/CLAUDE.md) the
        /// child would consume — then exit without spawning a process,
        /// creating a worktree/lease, flipping spec status, or any network
        /// sync. See also `--show-prompt` (just the initial prompt) and
        /// `--show-context` (the full launch-context markdown body).
        // trace:TASK-1232 | ai:codex
        // trace:TASK-1467 | ai:claude
        #[clap(
            long = "no-exec",
            visible_alias = "noexec",
            visible_alias = "print-command"
        )]
        noexec: bool,

        /// Print the exact initial prompt (explicit `--prompt` or the
        /// auto-generated one) that would be sent, then exit without
        /// spawning or any other side effect.
        // trace:TASK-1467 | ai:claude
        #[clap(long)]
        show_prompt: bool,

        /// For a launch that actually executes: print launch-time diagnostics
        /// (resolved vendor/client, role, spec/session/name, working
        /// directory, prompt source and whether context was injected,
        /// guidance files discovered, generated child argv with sensitive
        /// values redacted, process registration, and child exit/result
        /// information) before/around spawn. Distinct from `--no-exec`,
        /// which previews and exits without launching; `--verbose` launches.
        /// Never prints secrets, tokens, full prompt contents, or sensitive
        /// environment values. Normal (non-verbose) output is unchanged.
        // trace:TASK-1498 | ai:claude
        #[clap(long)]
        verbose: bool,

        /// Initial message to pass to the spawned Codex session.
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true, conflicts_with = "prompt_file")]
        prompt: Option<String>,

        /// Read the initial message from a file instead of the command line. Prefer this from
        /// orchestrators: a brief passed as `--prompt "$(cat file)"` through a nested shell can
        /// lose its quoting and silently launch an agent that sits idle at an empty prompt.
        // trace:BUG-1696 | ai:claude
        #[clap(long, value_name = "PATH", conflicts_with_all = ["prompt", "no_prompt"])]
        prompt_file: Option<PathBuf>,

        /// Do not send the automatic role-aware initial message.
        #[clap(long)]
        no_prompt: bool,

        /// Skip the ended-session resume offer.
        #[clap(long)]
        no_resume: bool,

        /// Resume the latest ended matching session, or a named ended session, without prompting.
        #[clap(long, value_name = "latest|NAME")]
        resume: Option<String>,

        /// Skip the live-duplicate confirmation prompt while still checking for duplicates.
        #[clap(long)]
        allow_duplicate: bool,

        /// Skip the live same-vendor/same-role duplicate check entirely.
        #[clap(long)]
        no_duplicate_check: bool,

        /// Do not read per-agent default flags from ~/.aida/agents.toml or .aida/agents.toml.
        #[clap(long)]
        no_default_flags: bool,

        /// Append one additional raw flag to the spawned Codex argv.
        #[clap(long = "extra-flag", value_name = "FLAG", allow_hyphen_values = true)]
        extra_flags: Vec<String>,

        /// Optional human-readable instance name.
        #[clap(long)]
        name: Option<String>,

        /// Optional human-readable description of this agent session.
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true)]
        description: Option<String>,

        /// Do not wake this session's thread when its inbox has mail (default:
        /// `[agents.codex] mail_wake` in agents.toml, else on).
        // trace:STORY-HOME-096 | ai:claude
        #[clap(long)]
        no_mail_wake: bool,
    },

    /// Spawn Antigravity CLI with project-correct cwd/env and registry tracking.
    // trace:STORY-434 | ai:codex
    Antigravity {
        /// Role propagated to the spawned Antigravity process as AIDA_SESSION_ROLE.
        #[clap(long)]
        role: Option<String>,

        /// SPEC-ID to work. Creates a scoped worktree + lease before launch.
        #[clap(long)]
        #[clap(value_parser = crate::spec_arg::spec_id)]
        spec: Option<String>,

        /// Override the focus-scope drift guard: launch on a spec
        /// outside the active `aida focus` subtree even when `[focus]
        /// out_of_scope = "block"` would refuse, and silence the `warn` nudge.
        // trace:STORY-717 | ai:claude
        #[clap(long)]
        force: bool,

        /// AIDA project root or descendant used as the launch base.
        #[clap(long, value_name = "PATH")]
        cwd: Option<PathBuf>,

        /// Pass Antigravity CLI's unsafe permission-skipping flag.
        ///
        /// This is opt-in by design. Interactive launches keep Antigravity's
        /// normal approval/sandbox behavior unless the operator explicitly
        /// asks for autonomous-drain posture.
        #[clap(long)]
        bypass_sandbox: bool,

        /// Do not write/inject the AIDA launch-context snapshot.
        #[clap(long)]
        no_context: bool,

        /// Do not set the terminal/tab title for this launch.
        #[clap(long)]
        no_title: bool,

        /// Print the generated launch-context snapshot before spawning.
        #[clap(long)]
        show_context: bool,

        /// Print the complete resolved launch contract — argv, active role,
        /// prompt source, launch-context snapshot path, AIDA environment
        /// inputs, and repository guidance files (AGENTS.md/CLAUDE.md) the
        /// child would consume — then exit without spawning a process,
        /// creating a worktree/lease, flipping spec status, or any network
        /// sync. See also `--show-prompt` (just the initial prompt) and
        /// `--show-context` (the full launch-context markdown body).
        // trace:TASK-1232 | ai:codex
        // trace:TASK-1467 | ai:claude
        #[clap(
            long = "no-exec",
            visible_alias = "noexec",
            visible_alias = "print-command"
        )]
        noexec: bool,

        /// Print the exact initial prompt (explicit `--prompt` or the
        /// auto-generated one) that would be sent, then exit without
        /// spawning or any other side effect.
        // trace:TASK-1467 | ai:claude
        #[clap(long)]
        show_prompt: bool,

        /// For a launch that actually executes: print launch-time diagnostics
        /// (resolved vendor/client, role, spec/session/name, working
        /// directory, prompt source and whether context was injected,
        /// guidance files discovered, generated child argv with sensitive
        /// values redacted, process registration, and child exit/result
        /// information) before/around spawn. Distinct from `--no-exec`,
        /// which previews and exits without launching; `--verbose` launches.
        /// Never prints secrets, tokens, full prompt contents, or sensitive
        /// environment values. Normal (non-verbose) output is unchanged.
        // trace:TASK-1498 | ai:claude
        #[clap(long)]
        verbose: bool,

        /// Initial message to pass to the spawned Antigravity session.
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true, conflicts_with = "prompt_file")]
        prompt: Option<String>,

        /// Read the initial message from a file instead of the command line. Prefer this from
        /// orchestrators: a brief passed as `--prompt "$(cat file)"` through a nested shell can
        /// lose its quoting and silently launch an agent that sits idle at an empty prompt.
        // trace:BUG-1696 | ai:claude
        #[clap(long, value_name = "PATH", conflicts_with_all = ["prompt", "no_prompt"])]
        prompt_file: Option<PathBuf>,

        /// Do not send the automatic role-aware initial message.
        #[clap(long)]
        no_prompt: bool,

        /// Skip the ended-session resume offer.
        #[clap(long)]
        no_resume: bool,

        /// Resume the latest ended matching session, or a named ended session, without prompting.
        #[clap(long, value_name = "latest|NAME")]
        resume: Option<String>,

        /// Skip the live-duplicate confirmation prompt while still checking for duplicates.
        #[clap(long)]
        allow_duplicate: bool,

        /// Skip the live same-vendor/same-role duplicate check entirely.
        #[clap(long)]
        no_duplicate_check: bool,

        /// Do not read per-agent default flags from ~/.aida/agents.toml or .aida/agents.toml.
        #[clap(long)]
        no_default_flags: bool,

        /// Append one additional raw flag to the spawned Antigravity argv.
        #[clap(long = "extra-flag", value_name = "FLAG", allow_hyphen_values = true)]
        extra_flags: Vec<String>,

        /// Optional human-readable instance name.
        #[clap(long)]
        name: Option<String>,

        /// Optional human-readable description of this agent session.
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true)]
        description: Option<String>,
    },
}
