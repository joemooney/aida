//! Clap types for `aida condition`, owned by `condition_cmd.rs`.
// trace:STORY-2402 | ai:claude

/// `aida condition` subcommands.
#[derive(clap::Subcommand, Debug, Clone)]
pub enum ConditionCommand {
    /// Evaluate a typed condition now: met, not met, unknown (a fact could
    /// not be read right now) or manual (a person decides). Read-only.
    Check {
        /// The condition: spec:<ID>=<status>, pr:<N>=merged,
        /// release:>=<version>, date:<YYYY-MM-DD>, query:<key>=<value>[,...]
        /// count<op><n> (keys: tag, status, type, feature; counts
        /// non-archived specs) or manual:<who>. Quote it when it contains a
        /// space or `>`.
        #[clap(value_name = "CONDITION", allow_hyphen_values = true)]
        condition: String,

        /// Emit JSON.
        #[clap(long)]
        json: bool,
    },
}
