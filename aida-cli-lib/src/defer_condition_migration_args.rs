//! Arguments owned by the deferral migration, nested in the registered `db` command.
// trace:STORY-2402 | ai:codex

#[derive(clap::Args, Debug)]
pub struct MigrateDeferConditionsArgs {
    /// Write the proposed migration. Without this flag, report only.
    #[clap(long)]
    pub apply: bool,

    /// Emit the migration report as JSON.
    #[clap(long)]
    pub json: bool,
}
