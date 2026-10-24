//! `aida worktree_gc` clap types, owned by `worktree_gc.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Scoped git-worktree management, mirroring `git worktree`. Create-or-enter
/// a per-epic workspace auto-scoped via `aida focus`, or a per-spec
/// workspace that ALSO takes the implementer lease so a single spec can be
/// worked by hand in one command, no agent launched.
///
/// A planned warm-pool surface (`aida worktree pool status`) and tiered
/// removal (`aida worktree remove`) are expected to land as sibling variants
/// under this enum later — the surface is shaped to leave room for them.
// trace:EPIC-55 | ai:claude
// trace:STORY-716 | ai:claude
// trace:STORY-742 | ai:claude
// trace:STORY-714 | ai:claude
#[derive(Subcommand, Debug)]
pub enum WorktreeCommand {
    /// Create a git worktree off origin/main and auto-scope it via `aida focus`.
    /// Default path `~/ai/aida-<slug>` (e.g. `~/ai/aida-epic54`), default branch
    /// `<id>-work` (e.g. `epic-54-work`). For an EPIC arg this is a scoping-only
    /// worktree; for a single non-epic SPEC it ALSO takes the implementer lease
    /// (spec → In Progress) so you can start working it by hand — no agent
    /// launched. Plain subcommand — prints the path, does NOT cd (mirrors
    /// `git worktree add`). Idempotent: an existing worktree/lease is reported +
    /// its focus re-affirmed, not re-created.
    Add {
        /// The epic or spec to scope the worktree to. Also the basis for
        /// the default path slug and branch.
        #[clap(value_name = "EPIC_OR_SPEC")]
        target: String,
        /// Override the worktree path (default `~/ai/aida-<slug>`).
        #[clap(long, value_name = "PATH")]
        path: Option<String>,
        /// Override the branch name (default `<id>-work`).
        #[clap(long, value_name = "BRANCH")]
        branch: Option<String>,
        /// The role recorded on the worktree's lease (default `implementer`).
        /// A session started inside the worktree takes its identity from the
        /// lease, not from an inherited environment.
        // trace:STORY-1662 | ai:claude
        #[clap(long, value_name = "ROLE")]
        role: Option<String>,
    },

    /// Create-if-missing, then cd into the worktree. This emits shell
    /// (`cd '<path>'`) on stdout for the `aida()` wrapper to auto-eval, so a
    /// subprocess can move the parent shell. For an EPIC arg it enters the
    /// epic's scoping worktree; for a single non-epic SPEC it also takes the
    /// implementer lease first (no agent launched) — one command to get into a
    /// ready worktree for that spec. Run it BARE (e.g.
    /// `aida worktree enter <id>`) — NOT wrapped in `eval "$(...)"`: the wrapper
    /// already auto-evals it, and double-eval would lose the cd (the
    /// no-double-eval convention). Without the wrapper installed, pipe the bare
    /// form through eval yourself: `eval "$(aida worktree enter <id>)"`.
    Enter {
        /// The epic or spec whose worktree to enter (created first if missing).
        #[clap(value_name = "EPIC_OR_SPEC")]
        target: String,
        /// Override the worktree path (default `~/ai/aida-<slug>`).
        #[clap(long, value_name = "PATH")]
        path: Option<String>,
        /// Override the branch name (default `<id>-work`).
        #[clap(long, value_name = "BRANCH")]
        branch: Option<String>,
    },

    /// Step back OUT of the current worktree — the symmetric inverse of
    /// `enter`. Emits shell on stdout for the `aida()` wrapper to auto-eval:
    /// cd to the main checkout, unset the session env exports `enter` applied
    /// (AIDA_SESSION_ID, CARGO_TARGET_DIR, ...), and strip the `(wt:...)`
    /// prompt segment. The session lease and the spec's status are UNTOUCHED —
    /// the worktree stays live for an idempotent re-enter; `aida session end`
    /// is the verb that finishes the work. Run it BARE (`aida worktree exit`)
    /// — the wrapper auto-evals it; double-eval would lose the cd. Standing
    /// outside a worktree while still CARRYING its session (you cd'd out, or
    /// the session was ended from another shell) still clears the env and the
    /// prompt segment — minus the cd. Only a shell carrying nothing is a no-op.
    // trace:TASK-1160 | ai:claude
    // trace:BUG-780 | ai:claude
    Exit,

    /// List AIDA-managed worktrees, each annotated with its `.aida/focus`.
    List {
        /// Machine-readable JSON (`[{path, branch, focus}]`).
        #[clap(long)]
        json: bool,
    },

    /// Prune verified merged agent worktrees using the same safety checks as
    /// `aida doctor --heal --category merged-agent-worktrees`.
    // trace:TASK-1145 | ai:codex
    Gc {
        /// Skip confirmation prompts.
        #[clap(long, short = 'y')]
        yes: bool,

        /// Permit destructive cleanup: remove verified merged worktrees and
        /// delete their local branches. Still refused in unattended contexts.
        #[clap(long)]
        force: bool,

        /// Emit machine-readable JSON.
        #[clap(long)]
        json: bool,
    },

    /// Disposition a worktree whose content is undecidable (e.g. batched
    /// integration). Archives the branch tip under refs/archive/, removes the
    /// worktree and branch, and ledgers the operator decision.
    // trace:BUG-1800 | ai:antigravity
    Dismiss {
        /// Path to the specific worktree to dismiss.
        #[clap(value_name = "PATH")]
        path: Option<String>,

        /// Dismiss all worktrees in this category (e.g., `batched-undecidable`).
        #[clap(long)]
        category: Option<String>,
    },

    /// Reclaim disk by deleting stale `target/` build caches inside this
    /// repository's worktrees. A `target/` is a cache, never a source of truth
    /// — nothing but rebuild time is lost. DRY RUN BY DEFAULT: it reports
    /// reclaimable bytes and every worktree it held back, and deletes only with
    /// `--apply`. Where `gc` removes worktrees that are FINISHED, `reclaim`
    /// removes the regenerable cache inside worktrees that are KEPT, which is
    /// the space `aida session reap` cannot reach and `cargo clean` targets
    /// wrongly (inside a worktree it clears the SHARED CARGO_TARGET_DIR, i.e.
    /// the main checkout's live cache, not the stale one). The main checkout's
    /// `target/` is never a candidate; nor is `.aida-store`, a worktree held by
    /// a live session, a worktree whose branch has an open PR, or a cache
    /// touched in the last `--min-age-mins` minutes.
    // trace:TASK-1562 | ai:claude
    Reclaim {
        /// Actually delete. Without this the command only reports.
        #[clap(long)]
        apply: bool,

        /// Treat a `target/` touched within this many minutes as a possible
        /// in-flight build and skip it.
        #[clap(long, value_name = "MINS", default_value_t = 60)]
        min_age_mins: u64,

        /// Stop deleting once the filesystem is at or below this used
        /// percentage, leaving the remaining caches warm.
        #[clap(long, value_name = "PCT")]
        target_pct: Option<u8>,

        /// Also reclaim worktrees held by a live session lease.
        #[clap(long)]
        include_live: bool,

        /// Also reclaim worktrees whose branch has an open pull request. Also
        /// waives the fail-safe hold applied when open-PR state cannot be read.
        #[clap(long)]
        include_open_prs: bool,

        /// Emit machine-readable JSON.
        #[clap(long)]
        json: bool,

        /// Treat agent scratch (terminal jobs' tmp dirs, session scratch,
        /// idle build caches) younger than this many days as live and keep
        /// it.
        // trace:TASK-2325 | ai:claude
        #[clap(long, value_name = "DAYS", default_value_t = 3)]
        scratch_age_days: u64,

        /// Skip the agent-scratch scan (jobs tmp, TMPDIR scratch, build
        /// caches) and consider only this repository's worktree caches.
        // trace:TASK-2325 | ai:claude
        #[clap(long)]
        no_scratch: bool,
    },

    /// Manage the warm-pool of recycled worktrees — kept warm (reset-not-delete
    /// on hand-back) so build caches survive across fan-outs. Dissolves the
    /// cargo-cache poison and branch-stacking hazards of destroy-and-recreate.
    // trace:STORY-714 | ai:claude
    #[clap(subcommand)]
    Pool(WorktreePoolCommand),
}

/// Subcommands under `aida worktree pool`.
// trace:STORY-714 | ai:claude
#[derive(Subcommand, Debug)]
pub enum WorktreePoolCommand {
    /// Show every pooled worktree with its live state (available / in-use /
    /// leased / dirty / destroying) and current HEAD. Read-only.
    Status {
        /// Emit JSON instead of the human table.
        #[clap(long)]
        json: bool,
    },

    /// Acquire a worktree from the pool: reuse an idle warm tree (reset to a
    /// clean detached base) or create a new one up to the cap. Prints the path.
    Acquire {
        /// Stamp a durable reservation under this holder name (survives with no
        /// live process) — for a headless drain that parks work mid-flight.
        #[clap(long)]
        lease_holder: Option<String>,

        /// Emit JSON (`{"path": "..."}`) instead of the bare path.
        #[clap(long)]
        json: bool,
    },

    /// Return a worktree to the pool: reset it to a clean detached base and
    /// mark it idle. The directory PERSISTS (the warm cache is kept). Defaults
    /// to the pool tree containing the current directory.
    Return {
        /// Worktree path to return (default: the pool tree at the cwd).
        path: Option<String>,

        /// Reset the tree even while live processes use it (a working
        /// directory, open or memory-mapped file, or program inside it).
        /// Without this, the return refuses and names each pid and command,
        /// and also refuses when the process list cannot be fully checked.
        // trace:BUG-1986 | ai:claude
        #[clap(long)]
        include_in_use: bool,
    },

    /// Register EXISTING worktrees into the warm-pool — the migration path for
    /// sibling worktrees created before the pool existed. DRY-RUN BY DEFAULT:
    /// pass `--no-dry-run` to write the registry. Never touches a worktree
    /// (no reset, no checkout, no removal) and is safe to re-run. A tree that
    /// still carries uncommitted or unmerged work needs `--include-unlanded`
    /// and is registered as a RESERVATION, so it is never handed out until you
    /// release it with `pool return`.
    // trace:TASK-1009 | ai:claude
    Adopt {
        /// Worktree path(s) to adopt. Omit with `--all` to consider every
        /// worktree registered against this repository.
        paths: Vec<String>,

        /// Consider every worktree of this repo (the main checkout and any
        /// worktree nested inside it are never adopted).
        #[clap(long)]
        all: bool,

        /// Actually write the registry (the default is a preview).
        #[clap(long)]
        no_dry_run: bool,

        /// Also adopt trees with uncommitted changes or unmerged commits,
        /// registering each as a reservation rather than an idle tree.
        #[clap(long)]
        include_unlanded: bool,

        /// Emit JSON instead of the human report.
        #[clap(long)]
        json: bool,
    },

    /// Tear down pool worktrees. DRY-RUN BY DEFAULT — pass `--no-dry-run` to
    /// actually remove. Each tree is classified by risk; only `disposable`
    /// trees are removed unless you opt into a riskier class with one
    /// `--include-*` flag. This is the ONLY path that deletes a directory.
    Destroy {
        /// Specific worktree path(s) to destroy. Omit with `--all` for a bulk
        /// sweep (in which `--include-leased` is ignored as a safety stop).
        paths: Vec<String>,

        /// Target every pooled worktree.
        #[clap(long)]
        all: bool,

        /// Actually remove (the default is a preview).
        #[clap(long)]
        no_dry_run: bool,

        /// Allow removing dirty / unmerged / unverified trees (uncommitted or
        /// unlanded work is patch-salvaged first).
        #[clap(long)]
        include_unlanded: bool,

        /// Allow removing a tree with a live owner process.
        #[clap(long)]
        include_in_use: bool,

        /// Allow removing a durably-leased tree — honored only when the exact
        /// path is named, never in a `--all` sweep.
        #[clap(long)]
        include_leased: bool,

        /// Emit JSON instead of the human report.
        #[clap(long)]
        json: bool,
    },
}
