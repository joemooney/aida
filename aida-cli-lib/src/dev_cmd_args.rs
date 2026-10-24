//! `aida dev_cmd` clap types, owned by `dev_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// What `aida dev build --clean[=MODE]` deletes before building.
// trace:STORY-1646 | ai:claude
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum DevCleanMode {
    /// Delete only `target/agent/incremental`; keep compiled dependencies.
    Incremental,
    /// Run `cargo clean --profile agent`.
    Full,
}

/// of an in-repo build, running dev servers, installing shell helpers).
// trace:EPIC-1-001 | ai:claude
#[derive(Subcommand, Debug)]
pub enum DevCommand {
    /// Emit shell code that prepends the in-repo build dir to PATH.
    /// Use as: `eval "$(aida dev activate)"`. By default the release build
    /// (`target/release/aida`) is activated — pass `debug` / `release`
    /// (or `--debug` / `--release`) to PIN a specific profile across
    /// subsequent re-activations, or `auto` (`--auto`) to pin
    /// freshest-wins selection (newest SHA-matched of debug vs release,
    /// re-picked on every activate).
    // trace:TASK-1158 | ai:claude
    Activate {
        /// Override the AIDA repo path (defaults to current directory if it
        /// looks like the aida repo, or $AIDA_DEV_REPO if set; when that
        /// names a removed worktree, $AIDA_DEV_HOME_REPO).
        #[clap(long)]
        repo: Option<String>,

        /// Profile shortcut: `debug`, `release`, or `auto` (positional).
        /// Equivalent to `--debug` / `--release` / `--auto`. Pins for
        /// subsequent re-activations.
        #[clap(value_parser = ["debug", "release", "auto"])]
        profile: Option<String>,

        /// Pin to the debug build (target/debug). Sticky — subsequent plain
        /// `aida dev activate` invocations honor the pin until you pass
        /// --auto or --release.
        // trace:FR-1-068 | ai:claude
        #[clap(long, conflicts_with_all = ["release", "auto"])]
        debug: bool,

        /// Pin to the release build (target/release). Sticky. Also the
        /// default when nothing is pinned.
        // trace:FR-1-068 trace:TASK-1158 | ai:claude
        #[clap(long, conflicts_with_all = ["debug", "auto"])]
        release: bool,

        /// Pin freshest-wins selection: the newest SHA-matched of
        /// debug/release is re-picked on every activate. Sticky until you
        /// pin debug or release.
        // trace:TASK-1158 | ai:claude
        #[clap(long, conflicts_with_all = ["debug", "release"])]
        auto: bool,
    },

    /// Build, smoke-test and promote the live dev binary. The one build
    /// command: an incremental `--profile agent` workspace build, then a
    /// smoke test of the new `aida` / `aida-server`, then an atomic promote
    /// into the live directory (default `~/.local/share/aida/live`, outside
    /// target/), keeping the previous pair in `live/prev/`. Promotes only from
    /// the primary checkout on a clean `main` that matches `origin/main` (the
    /// local remote-tracking ref; it does not fetch, so run `git fetch`
    /// first); in a worktree it builds and smoke-tests only.
    // trace:STORY-1646 | ai:claude
    Build {
        /// Clear the incremental cache first. Bare `--clean` deletes only
        /// `target/agent/incremental` (keeps compiled dependencies);
        /// `--clean=full` runs `cargo clean --profile agent`.
        #[clap(
            long,
            value_enum,
            num_args = 0..=1,
            require_equals = true,
            default_missing_value = "incremental"
        )]
        clean: Option<DevCleanMode>,

        /// Build and smoke-test only; never promote.
        #[clap(long)]
        no_promote: bool,

        /// Promote even when the checkout is dirty, off `main`, or not at
        /// `origin/main` (`aida dev status` then shows the dirty SHA).
        #[clap(long)]
        allow_dirty: bool,

        /// Point `~/.local/bin/aida` and `~/.local/bin/aida-server` at the
        /// live directory, showing anything it replaces first.
        #[clap(long)]
        install_links: bool,

        /// With --install-links: replace existing files/links without
        /// asking (required when stdin is not a terminal).
        #[clap(long, requires = "install_links")]
        yes: bool,

        /// Wait for a live drain wave pinned to the current binary to finish
        /// instead of refusing.
        #[clap(long)]
        after_wave: bool,

        /// For a schedule: promote only when origin/main has moved past the
        /// live binary and every CI check on that commit passed. Fetches,
        /// fast-forwards a clean primary `main`, and launches the build in
        /// the background; skips quietly otherwise. Acts only from the
        /// checkout the live binary was built from.
        // trace:STORY-1689 | ai:claude
        #[clap(
            long,
            conflicts_with_all = ["clean", "no_promote", "allow_dirty", "install_links", "after_wave"]
        )]
        if_main_green: bool,

        /// Internal: the background build an `--if-main-green` run launched.
        // trace:STORY-1689 | ai:claude
        #[clap(
            long,
            hide = true,
            value_name = "SHA",
            conflicts_with_all = ["clean", "no_promote", "allow_dirty", "install_links", "after_wave", "if_main_green"]
        )]
        autopromote_child: Option<String>,
    },

    /// Swap the previous live binaries (`live/prev/`) back in, in one step.
    /// Running it again rolls forward. Refuses while a live drain wave is
    /// pinned to the live binary unless --after-wave or --force.
    // trace:STORY-1646 | ai:claude
    Rollback {
        /// Wait for a live drain wave pinned to the live binary to finish
        /// instead of refusing.
        #[clap(long)]
        after_wave: bool,

        /// Swap even while a live drain wave is pinned to the live binary.
        #[clap(long, conflicts_with = "after_wave")]
        force: bool,
    },

    /// Internal pre-build safety gate used by `make build-fast`.
    #[command(hide = true)]
    BuildGuard {
        #[clap(value_parser = ["debug", "release", "agent"])]
        profile: String,
        #[clap(long)]
        after_wave: bool,
    },

    /// Emit shell code that undoes a previous `aida dev activate`.
    /// Use as: `eval "$(aida dev deactivate)"`.
    Deactivate,

    /// Run aida-server (REST/gRPC) and the React dev server (vite) in the
    /// foreground. Ctrl+C stops both. Defaults to serving the current
    /// project's store; React dev server only starts when run from the
    /// AIDA repo (since aida-web-react/ lives there).
    Serve {
        /// Override the REST/HTTP port for aida-server (default: 8080)
        #[clap(long)]
        rest_port: Option<u16>,

        /// Override the gRPC port for aida-server (default: 50051)
        #[clap(long)]
        grpc_port: Option<u16>,

        /// Override the vite dev server port (default: 5173)
        #[clap(long)]
        web_port: Option<u16>,

        /// Skip starting the React dev server even when aida-web-react/ exists
        #[clap(long)]
        no_web: bool,
    },

    /// Show current dev-activation state: is AIDA_DEV_ACTIVE set, what
    /// binary is being used, when was it built, what's the AIDA repo path.
    Status,

    /// Print the compact PS1 staleness marker for the active dev binary.
    Ps1,

    /// Print shell helper functions (aida-on, aida-off) suitable for
    /// pasting into ~/.bashrc or ~/.zshrc. Pass --install to append them
    /// directly.
    ShellInit {
        /// Detect the user's shell and append the helpers to its rc file
        #[clap(long)]
        install: bool,
    },

    /// One-command release: bump the version through a release PR, tag the
    /// merged commit and push the tag (via scripts/release.sh), wait for the
    /// GitHub Actions workflow to publish binary tarballs, then upgrade your
    /// sibling installs to the new version. Default bump: patch.
    // trace:TASK-1721 | ai:claude
    Release {
        /// Version bump kind: major, minor, patch, or an explicit version like "0.5.0"
        #[clap(default_value = "patch")]
        bump: String,
    },

    /// Alias for `aida dev release patch` — the most common case.
    Patch,
}
