//! `aida db audit-ids` clap types, owned by `db_audit_ids.rs`.
// trace:BUG-2017 | ai:claude

use clap::Args;

#[derive(Args, Debug)]
pub struct AuditIdsArgs {
    /// Also audit the claims of this already-local commit or ref (for example
    /// a remote-tracking ref of the store branch). Repeatable. Nothing is
    /// fetched: fetch first if the ref should be current.
    #[clap(long = "against", value_name = "REF")]
    pub against: Vec<String>,

    /// Emit the audit as JSON.
    #[clap(long)]
    pub json: bool,
}
