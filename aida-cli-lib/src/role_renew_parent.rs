//! Relay/operator-facing replacement command, separate from read-only expiry.
// trace:TASK-3119 | ai:codex
// trace:TASK-3139 | ai:codex
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
pub(crate) fn expiry_hint() -> String {
    if crate::gate::relay_parent::enabled().unwrap_or(false) {
        let grant_env = crate::gate::seat_authority::GRANT_ENV;
        format!("Relay operator: try `aida role renew-parent --run-dir <dir>`; on success read relay.grant and re-export {grant_env} before relaunching.")
    } else {
        "Relay operator: re-enter the parent role at your terminal and relaunch the agent through its launcher.".into()
    }
}
pub(crate) fn handle(root: &Path, args: &Args) -> anyhow::Result<()> {
    use crate::gate::relay_parent::RefusalCode::InvalidDuration;
    let duration = |raw: &str| {
        let d = crate::drain_caps::parse_duration(raw).ok_or_else(|| InvalidDuration.refuse())?;
        chrono::Duration::from_std(d).map_err(|e| InvalidDuration.caused_by(e))
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
                    serde_json::json!({"version":1,"outcome":"refused","reason":e.code,"detail":e.to_string()})
                );
            } else {
                eprintln!(
                    "renew-parent refused [{}]: {e:#}",
                    serde_json::to_value(e.code)?
                );
            }
            std::process::exit(3)
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn task_3139_expiry_hint_follows_operator_policy() {
        let home = tempfile::tempdir().unwrap();
        let mut env = crate::test_env::EnvVarsGuard::snapshot(&["AIDA_HOME"]);
        env.set_key("AIDA_HOME", home.path());
        let aida_dir = home.path().join(".aida");
        std::fs::create_dir(&aida_dir).unwrap();
        let config = aida_dir.join("config.toml");
        std::fs::write(&config, "[seat]\nmandate_relay_parent_renewal=false\n").unwrap();
        let hint = super::expiry_hint();
        assert!(hint.contains("re-enter the parent role at your terminal"));
        assert!(hint.contains("relaunch the agent through its launcher"));
        assert!(!hint.contains("renew-parent"));
        std::fs::write(&config, "[seat]\nmandate_relay_parent_renewal=true\n").unwrap();
        let hint = super::expiry_hint();
        assert!(hint.contains("aida role renew-parent --run-dir <dir>"));
        assert!(hint.contains("relay.grant"));
        assert!(hint.contains(crate::gate::seat_authority::GRANT_ENV));
    }
}
