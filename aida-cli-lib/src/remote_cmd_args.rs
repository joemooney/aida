//! `aida remote_cmd` clap types, owned by `remote_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Guided origin bootstrap for a project with no git `origin`.
// trace:STORY-537 | ai:claude
#[derive(Subcommand, Debug)]
pub enum RemoteCommand {
    /// Create (or attach) an `origin` for a project that has none. At a TTY,
    /// walks a menu: GitHub (via gh), a remembered GitLab host, another GitLab
    /// host (push-to-create over SSH), or attach-existing. Without a TTY (and
    /// no route flag) prints the manual recipe and exits cleanly. Pre-select a
    /// route with --github / --gitlab <host> / --attach <url> to stay
    /// scriptable.
    Create {
        /// Attach this existing repo URL as origin and push (skip the menu).
        #[clap(long)]
        attach: Option<String>,

        /// Create on GitHub via `gh repo create … --source . --remote origin
        /// --push` (skip the menu).
        #[clap(long, conflicts_with_all = ["attach", "gitlab"])]
        github: bool,

        /// Push-to-create on this GitLab host over SSH (skip the menu).
        #[clap(long, conflicts_with_all = ["attach", "github"], value_name = "HOST")]
        gitlab: Option<String>,

        /// Create the repo public instead of private (GitHub path).
        #[clap(long)]
        public: bool,
    },

    /// Wire an existing repo's URL as `origin` and push the current branch.
    /// The clean fallback when auto-create isn't possible (corporate GitLab):
    /// create the repo in the UI, then `aida remote attach <url>`.
    Attach {
        /// The existing repo's clone URL (SSH or HTTPS).
        url: String,
    },

    /// Report how far each configured remote has drifted from local — and from
    /// each other — for the shared branches (the trunk + the store branch).
    /// Read-only: refreshes refs best-effort but never writes or pushes. Exits
    /// non-zero when two remotes hold different tips for a branch, so it can
    /// gate CI or a pre-push hook.
    // trace:TASK-1095 | ai:claude
    Status {
        /// Emit machine-readable JSON instead of the table.
        #[clap(long)]
        json: bool,

        /// Skip the best-effort `git fetch`; compare using the existing
        /// remote-tracking refs only (offline-friendly, may be stale).
        #[clap(long)]
        no_fetch: bool,
    },

    /// Register an extra hub as a full mirror: add the git remote if missing
    /// (with --url), list it in `[store.sync] mirror_remotes` so store pushes
    /// fan out, and install a pre-push hook that mirrors every code ref pushed
    /// to `origin` out to each mirror hub. Idempotent — safe to re-run. The
    /// mirror legs are best-effort: an unreachable or diverged mirror warns
    /// and never blocks the origin push.
    // trace:TASK-1097 | ai:claude
    Mirror {
        /// The git remote name of the mirror hub (e.g. `gitlab`).
        name: String,

        /// Clone URL to add the remote with when it doesn't exist yet.
        #[clap(long)]
        url: Option<String>,
    },

    /// (plumbing) Fan the refs of an in-flight `git push` out to every mirror
    /// hub. The pre-push hook shim calls this with the pushed remote's name,
    /// piping through the ref lines git feeds the hook on stdin. Says why it
    /// skipped when the push does not target `origin` or carries only the
    /// store branch; exits non-zero when a mirror push fails (the hook shim
    /// tolerates that, so the origin push is never blocked).
    // trace:TASK-1097 trace:BUG-1676 | ai:claude
    #[clap(hide = true)]
    MirrorPush {
        /// The remote the triggering push targets (hook argument $1).
        pushed_remote: String,
        /// Preview mirror updates without changing any mirror remote.
        #[clap(long)]
        dry_run: bool,
    },

    /// (plumbing) Ask whether a push to the default branch is an attested
    /// `aida remote mirror-sync` of origin's reviewed tip. A project's
    /// pre-push main guard pipes its stdin ref lines here before refusing;
    /// exit 0 allows the push only when the remote is a registered mirror
    /// (never origin), every default-branch sha equals origin's tip, and
    /// mirror-sync attested exactly that push. Anything else exits non-zero
    /// with the reason, so the guard keeps refusing. Guard snippet:
    /// `printf '%s\n' "$refs" | aida remote mirror-guard-check "$1" && exit 0`
    /// `--install-guard` installs AIDA's opt-in main guard, which already
    /// consults this check, chained in front of the existing pre-push hook.
    // trace:BUG-2015 | ai:claude
    #[clap(hide = true)]
    MirrorGuardCheck {
        /// The remote the push targets (pre-push hook argument $1).
        #[clap(required_unless_present = "install_guard")]
        remote: Option<String>,
        /// Install the main guard and chain it in front of the pre-push hook.
        #[clap(long, conflicts_with = "remote")]
        install_guard: bool,
        /// With --install-guard: chain in front of a custom pre-push hook too
        /// (shell hooks only; a Python or Node hook is refused).
        #[clap(long, requires = "install_guard")]
        force: bool,
    },

    /// Push origin's tips of the default branch and the spec store to every
    /// mirror hub, by sha. The pre-push hook only mirrors what this machine
    /// pushes; a merge performed on the forge and a store push from another
    /// clone never fire it, so mirrors drift. `aida pull` runs this after a
    /// successful pull; run it by hand when `aida remote status` shows a hub
    /// behind. Never force-pushes: a diverged hub is reported and exits
    /// non-zero.
    // trace:BUG-1676 | ai:claude
    MirrorSync {
        /// Emit machine-readable JSON instead of the report.
        #[clap(long)]
        json: bool,
    },

    /// Reconcile a diverged spec store across every configured hub: fetch each
    /// hub's store branch, union-merge the diverged tips (spec objects, the
    /// operation log, and the id-block/node registries all merge
    /// structurally), then push the union so every hub fast-forwards to it.
    /// Dry-run by default — prints the plan and exits 2 when the hubs
    /// disagree; add --execute to perform the merge and the pushes. Never
    /// force-pushes; a conflict the structural union cannot resolve stops the
    /// reconcile with the manual path.
    // trace:BUG-714 | ai:claude
    Reconcile {
        /// Perform the union merge and push it to every hub (without this,
        /// print the dry-run plan and change nothing).
        #[clap(long)]
        execute: bool,

        /// Emit machine-readable JSON instead of the report.
        #[clap(long)]
        json: bool,

        /// Consent to publishing identity-bearing registry content (email
        /// addresses currently on one hub only) to every hub; without it the
        /// reconcile stops and lists what would be published.
        #[clap(long)]
        yes: bool,
    },
}
