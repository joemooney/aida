//! Relay/operator-facing replacement command, separate from read-only expiry.
// trace:TASK-3119 | ai:codex
use std::path::{Path, PathBuf};
#[derive(clap::Args, Debug)]
pub struct Args {
    /// Existing relay run directory that will receive the private relay.grant handoff.
    #[clap(long)]
    run_dir: PathBuf,
    /// Requested lifetime, clamped to the mandate and grant cap.
    #[clap(long, default_value = "24h")]
    ttl: String,
    /// Minimum time required for the next rotation.
    #[clap(long, default_value = "2h")]
    refuse_within: String,
    /// Print handoff fields on success or the typed refusal reason as JSON.
    #[clap(long)]
    json: bool,
}
pub(crate) fn expiry_hint() {
    if crate::gate::relay_parent::enabled().unwrap_or(false) {
        let grant_env = crate::gate::seat_authority::GRANT_ENV;
        eprintln!("Relay operator: try `aida role renew-parent --run-dir <dir>`; on success read relay.grant and re-export {grant_env} before relaunching.");
    }
}
pub(crate) fn handle(root: &Path, args: &Args) -> anyhow::Result<()> {
    let duration = |raw: &str| -> anyhow::Result<chrono::Duration> {
        let d = crate::drain_caps::parse_duration(raw)
            .ok_or_else(|| anyhow::anyhow!("invalid-duration"))?;
        Ok(chrono::Duration::from_std(d)?)
    };
    let result = (|| {
        crate::gate::relay_parent::renew(
            root,
            &args.run_dir,
            duration(&args.ttl)?,
            duration(&args.refuse_within)?,
        )
    })();
    match result {
        Ok(r) => {
            if args.json {
                println!("{}", serde_json::to_string(&r)?);
            } else {
                println!(
                    "version=1\ngrant={}\nexpires_at={}\nmandate={}\nsupersedes={}",
                    r.grant,
                    r.expires_at.to_rfc3339(),
                    r.mandate,
                    r.supersedes
                );
            }
            Ok(())
        }
        Err(e) => {
            if args.json {
                println!(
                    "{}",
                    serde_json::json!({"version":1,"outcome":"refused","reason":crate::gate::relay_parent::refusal_code(&e),"detail":e.to_string()})
                );
            } else {
                eprintln!(
                    "renew-parent refused [{}]: {e:#}",
                    serde_json::to_value(crate::gate::relay_parent::refusal_code(&e))?
                );
            }
            std::process::exit(3)
        }
    }
}
