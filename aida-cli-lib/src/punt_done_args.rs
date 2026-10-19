//! `aida punt_done` clap types, owned by `punt_done.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Browse and analyze the local design-fork punt ledger.
// trace:STORY-325 | ai:claude
#[derive(Subcommand, Debug, Clone)]
pub enum PuntsCommand {
    /// Browse recent punt records from the ledger
    List {
        /// Filter by spec (display ID or UUID)
        #[clap(long)]
        spec: Option<String>,

        /// Filter by category (e.g. design-fork, ambiguous-spec, missing-context)
        #[clap(long, short = 'c')]
        category: Option<String>,

        /// Filter by resolution path (e.g. punted, advisor-resolved, escalated-to-human)
        #[clap(long, short = 'r')]
        resolution: Option<String>,

        /// Include auto-filed session-end visibility warnings (hidden by default)
        #[clap(long, visible_alias = "include-session-end")]
        all: bool,
    },

    /// Print rolling metrics and stats for recent punts
    Analyze {
        /// Include auto-filed session-end visibility warnings (hidden by default)
        #[clap(long, visible_alias = "include-session-end")]
        all: bool,
    },

    /// Resolve an open punt with a decision — writes the orchestrator resume
    /// response AND closes the ledger record
    Resolve {
        /// The spec ID of the punted record
        id: String,

        /// The decision / answer to record
        #[clap(long, short = 'a')]
        answer: String,

        /// Why this decision (audit trail)
        #[clap(long)]
        reasoning: Option<String>,

        /// A/B/C calibration class, if known
        #[clap(long)]
        classification: Option<String>,
    },

    /// Dismiss an open punt as not needing a decision — closes the ledger
    /// record without resuming an implementer
    Dismiss {
        /// The spec ID of the punted record
        id: String,

        /// Optional note on why it was dismissed
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true)]
        reason: Option<String>,
    },

    /// Escalate an open punt to a human — writes the orchestrator park response
    /// AND closes the ledger record as escalated-to-human
    Escalate {
        /// The spec ID of the punted record
        id: String,

        /// Why a human is needed (audit trail)
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true)]
        reasoning: String,

        /// Categorized reason a human is needed (e.g. strategy, irreversible)
        #[clap(long)]
        escalation_reason: Option<String>,

        /// A/B/C calibration class, if known
        #[clap(long)]
        classification: Option<String>,
    },

    /// Print the full ledger record for a spec
    Read {
        /// The spec ID to read
        id: String,
    },

    /// Extract a pattern from a punt into a new memory pack entry
    Promote {
        /// The spec ID of the punt record to promote
        id: String,

        /// The name of the memory file (e.g., 'discipline/operator_curation.md')
        memory_name: String,
    },
}
