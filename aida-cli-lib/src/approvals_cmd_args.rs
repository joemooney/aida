//! `aida approvals_cmd` clap types, owned by `approvals_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyApprovalIndependent {
    /// The proxy review was independent of the implementation.
    Yes,
    /// The proxy review was not independent of the implementation.
    No,
}

#[derive(Subcommand, Debug)]
pub enum ApprovalCommand {
    /// Write a proxy-approval ledger entry as a marked spec comment.
    Add {
        /// Requirement ID (UUID or SPEC-ID)
        spec: String,

        /// Decision being approved, e.g. merge, clear-merge-hold, fork-choice.
        #[clap(long)]
        decision: String,

        /// Verdict or reasoning behind the proxy approval.
        #[clap(long, visible_alias = "reasoning")]
        verdict: String,

        /// Proxy identity. Defaults to AIDA_AUTHOR / USER.
        #[clap(long)]
        proxy: Option<String>,

        /// Proxy session id. Defaults to the current AIDA session when known.
        #[clap(long)]
        session: Option<String>,

        /// Human whose standing authorization allows this proxy approval.
        #[clap(long = "authorized-by")]
        authorized_by: String,

        /// Whether the proxy review was independent of implementation. Required.
        #[clap(long, value_enum)]
        independent: ProxyApprovalIndependent,
    },
}
