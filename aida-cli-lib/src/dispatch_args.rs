//! `aida dispatch` clap types, owned by `dispatch.rs` and re-exported from
//! `cli.rs`.
// trace:STORY-1917 | ai:agy

use clap::{Subcommand, ValueEnum};
use std::path::PathBuf;

/// Vendor harness for a dispatched reviewer or implementer lane.
// trace:STORY-1917 | ai:agy
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DispatchVendor {
    Claude,
    Codex,
    Agy,
}

impl DispatchVendor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Agy => "agy",
        }
    }
}

/// Filter for `aida dispatch list --status`.
// trace:STORY-1917 | ai:agy
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
#[clap(rename_all = "lowercase")]
pub enum DispatchStatusFilter {
    Pending,
    Running,
    Done,
    Refused,
    Failed,
    Cancelled,
}

/// Closed verbs accepted by `aida dispatch request <verb>`.
// trace:STORY-1917 | ai:agy
#[derive(Subcommand, Debug, Clone)]
pub enum DispatchRequestVerb {
    /// Drive a PR from its current state through update-branch, CI wait,
    /// headless review, and ship/train.
    #[clap(alias = "pr-flow")]
    Flow {
        /// Pull request number (1..=999999).
        pr: u64,
        /// Stop after review without shipping.
        #[clap(long)]
        no_ship: bool,
        /// Vendor harness for the reviewer (`claude`, `codex`, `agy`).
        #[clap(long, value_enum)]
        vendor: Option<DispatchVendor>,
    },
    /// Run a standalone headless reviewer on `PR-<N>`.
    Review {
        /// Pull request number (1..=999999).
        pr: u64,
        /// Vendor harness for the reviewer (`claude`, `codex`, `agy`).
        #[clap(long, value_enum)]
        vendor: Option<DispatchVendor>,
    },
    /// Create/reuse a takeover worktree on a PR's branch and launch a
    /// background implementer with a frozen brief.
    #[clap(alias = "takeover")]
    Take {
        /// Pull request number (1..=999999).
        pr: u64,
        /// Head branch name on origin.
        #[clap(long)]
        branch: String,
        /// Target spec ID (e.g. `<SPEC-ID>`).
        #[clap(long)]
        spec: String,
        /// Path to the markdown brief file to freeze (`<= 64 KiB`).
        #[clap(long)]
        brief: PathBuf,
        /// Vendor harness for the implementer (`claude`, `codex`, `agy`).
        #[clap(long, value_enum)]
        vendor: Option<DispatchVendor>,
    },
    /// Create/reuse a fresh worktree off `origin/main` and launch a background
    /// implementer lane with a frozen brief.
    #[clap(alias = "lane")]
    Impl {
        /// Short worktree/session name (`^[A-Za-z0-9._-]{1,60}$`).
        #[clap(long)]
        name: String,
        /// Branch name to create or reuse.
        #[clap(long)]
        branch: String,
        /// Target spec ID (e.g. `<SPEC-ID>`).
        #[clap(long)]
        spec: String,
        /// Path to the markdown brief file to freeze (`<= 64 KiB`).
        #[clap(long)]
        brief: PathBuf,
        /// Vendor harness for the implementer (`claude`, `codex`, `agy`).
        #[clap(long, value_enum)]
        vendor: Option<DispatchVendor>,
    },
    /// Ship a serialized queue of PRs (`aida pr ship` -> `update-branch` -> `train`).
    Train {
        /// Pull request numbers (1..=16 items).
        #[clap(required = true)]
        prs: Vec<u64>,
    },
}

/// `aida dispatch` subcommands.
// trace:STORY-1917 | ai:agy
#[derive(Subcommand, Debug)]
pub enum DispatchAction {
    /// Queue a validated, mandate-authorized dispatch request.
    Request {
        #[clap(subcommand)]
        verb: DispatchRequestVerb,
        /// Print the queued (or deduplicated) request as JSON.
        #[clap(long, global = true)]
        json: bool,
    },
    /// List queued, running, and completed dispatch requests.
    List {
        /// Filter by status (`pending`, `running`, `done`, `refused`, `failed`, `cancelled`).
        #[clap(long, value_enum)]
        status: Option<DispatchStatusFilter>,
        /// Print JSON.
        #[clap(long)]
        json: bool,
    },
    /// Show one dispatch request and its current result record.
    Show {
        /// Request ID (UUID or unique prefix).
        id: String,
        /// Print JSON.
        #[clap(long)]
        json: bool,
    },
    /// Cancel a pending dispatch request.
    Cancel {
        /// Request ID (UUID or unique prefix).
        id: String,
    },
}
