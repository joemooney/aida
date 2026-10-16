//! `[seat_occupancy]` in `.aida/config.toml` (TASK-1607).
//!
//! ```toml
//! [seat_occupancy]
//! single_seats = ["orchestrator"]
//! takeover_grace_secs = 120
//! inspection = "warn" # or "refuse"
//! ```
//!
//! The orchestrator seat is always single-occupancy; config can add seats but
//! never remove it. Only seats with a mapped ownership boundary are accepted
//! (today: orchestrator). No value here grants kill authority. An invalid
//! section fails closed: protected ops refuse with the parse error rather than
//! silently running unguarded.
//!
//! trace:TASK-1607 | ai:claude

use anyhow::{bail, Context, Result};
use std::path::Path;

use super::ORCHESTRATOR;

pub(crate) const DEFAULT_GRACE_SECS: u64 = 120;
pub(crate) const MAX_GRACE_SECS: u64 = 3600;

/// Seats whose mutation boundaries are wired to the occupancy gate.
const MAPPED_SEATS: &[&str] = &[ORCHESTRATOR];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Inspection {
    Warn,
    Refuse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SeatConfig {
    pub single_seats: Vec<String>,
    pub takeover_grace_secs: u64,
    pub inspection: Inspection,
}

impl Default for SeatConfig {
    fn default() -> Self {
        SeatConfig {
            single_seats: vec![ORCHESTRATOR.to_string()],
            takeover_grace_secs: DEFAULT_GRACE_SECS,
            inspection: Inspection::Warn,
        }
    }
}

impl SeatConfig {
    pub fn load(project_root: &Path) -> Result<Self> {
        // trace:TASK-1607 | ai:codex
        // Config and the permanent seat record have one authority root.
        let root = super::store::main_worktree_root(project_root)?;
        let path = root.join(".aida").join("config.toml");
        match std::fs::read_to_string(&path) {
            Ok(content) => Self::parse(&content).with_context(|| format!("in {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    pub fn parse(content: &str) -> Result<Self> {
        let doc: toml::Value = toml::from_str(content).context("config.toml is not valid TOML")?;
        let mut cfg = Self::default();
        let Some(section) = doc.get("seat_occupancy") else {
            return Ok(cfg);
        };
        let table = section
            .as_table()
            .context("[seat_occupancy] must be a table")?;
        for (key, value) in table {
            match key.as_str() {
                "single_seats" => {
                    let seats = value
                        .as_array()
                        .context("seat_occupancy.single_seats must be an array of role names")?;
                    for seat in seats {
                        let seat = seat
                            .as_str()
                            .context("seat_occupancy.single_seats entries must be strings")?;
                        let seat = aida_core::team::canonical_role(seat);
                        if !MAPPED_SEATS.contains(&seat.as_str()) {
                            bail!(
                                "seat_occupancy.single_seats: `{seat}` has no ownership boundary wired yet; only {} can be single-occupancy",
                                MAPPED_SEATS.join(", ")
                            );
                        }
                        if !cfg.single_seats.contains(&seat) {
                            cfg.single_seats.push(seat);
                        }
                    }
                }
                "takeover_grace_secs" => {
                    let secs = value
                        .as_integer()
                        .context("seat_occupancy.takeover_grace_secs must be an integer")?;
                    cfg.takeover_grace_secs = validate_grace(secs)?;
                }
                "inspection" => {
                    cfg.inspection = match value.as_str() {
                        Some("warn") => Inspection::Warn,
                        Some("refuse") => Inspection::Refuse,
                        _ => bail!("seat_occupancy.inspection must be \"warn\" or \"refuse\""),
                    };
                }
                other => bail!("seat_occupancy: unknown key `{other}`"),
            }
        }
        Ok(cfg)
    }

    pub fn is_single(&self, seat: &str) -> bool {
        self.single_seats.iter().any(|s| s == seat)
    }
}

/// Grace bounds shared by config and the `--grace-secs` override.
pub(crate) fn validate_grace(secs: i64) -> Result<u64> {
    if secs < 1 || secs as u64 > MAX_GRACE_SECS {
        bail!("takeover grace must be between 1 and {MAX_GRACE_SECS} seconds (got {secs})");
    }
    Ok(secs as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_section_is_orchestrator_default() {
        let cfg = SeatConfig::parse("[other]\nx = 1\n").unwrap();
        assert_eq!(cfg, SeatConfig::default());
        assert!(cfg.is_single("orchestrator"));
    }

    #[test]
    fn orchestrator_cannot_be_disabled() {
        let cfg = SeatConfig::parse("[seat_occupancy]\nsingle_seats = []\n").unwrap();
        assert!(cfg.is_single("orchestrator"));
    }

    #[test]
    fn unmapped_seat_refuses_activation() {
        let err = SeatConfig::parse("[seat_occupancy]\nsingle_seats = [\"integrator\"]\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("integrator"), "{err}");
    }

    #[test]
    fn grace_and_inspection_are_validated() {
        let cfg = SeatConfig::parse(
            "[seat_occupancy]\ntakeover_grace_secs = 30\ninspection = \"refuse\"\n",
        )
        .unwrap();
        assert_eq!(cfg.takeover_grace_secs, 30);
        assert_eq!(cfg.inspection, Inspection::Refuse);
        assert!(SeatConfig::parse("[seat_occupancy]\ntakeover_grace_secs = 0\n").is_err());
        assert!(SeatConfig::parse("[seat_occupancy]\ntakeover_grace_secs = 3601\n").is_err());
        assert!(SeatConfig::parse("[seat_occupancy]\ninspection = \"off\"\n").is_err());
        assert!(SeatConfig::parse("[seat_occupancy]\nkill = true\n").is_err());
    }

    #[test]
    fn invalid_toml_fails_closed() {
        assert!(SeatConfig::parse("[seat_occupancy\n").is_err());
    }
}
