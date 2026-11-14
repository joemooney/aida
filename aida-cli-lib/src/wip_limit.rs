//! The WIP limit on new work: while the project already carries
//! `[land] wip_max` open changes, `aida queue work` starts no new spec.
//! Rework and review pickups reduce the count, so they are never limited.
// trace:STORY-2768 | ai:claude

use std::path::Path;

use anyhow::Result;

/// What the limit says about one fresh pickup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Under the limit.
    Admit,
    /// At or over the limit, and `--over-wip` was passed: the line to record.
    Override(String),
    /// At or over the limit: the refusal.
    Refuse(String),
}

/// The decision for `open` open changes against `max`.
pub(crate) fn verdict(spec: &str, open: usize, max: u64, over_wip: bool) -> Verdict {
    if (open as u64) < max {
        return Verdict::Admit;
    }
    if over_wip {
        return Verdict::Override(format!(
            "{open} changes are open and the limit is {max}; {spec} is started anyway (--over-wip)"
        ));
    }
    Verdict::Refuse(format!(
        "{open} changes are open and the limit is {max} (`[land] wip_max`), so {spec} is not \
         started. `aida now` shows what is open; rework and review pickups are not limited. \
         To start it anyway, re-run with `--over-wip` (the override is recorded)."
    ))
}

/// The open-change count from a snapshot written at `generated_at`, when that
/// is recent enough to trust at `now`.
pub(crate) fn fresh_snapshot_count(
    snapshot: &crate::now::Snapshot,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<usize> {
    let at = chrono::DateTime::parse_from_rfc3339(&snapshot.generated_at).ok()?;
    let age = now.signed_duration_since(at).num_seconds();
    (0..=crate::now::STALE_SECS)
        .contains(&age)
        .then_some(snapshot.open_prs)
}

/// How many changes are open: the reconciler's snapshot while it is fresh,
/// else one forge listing.
fn open_changes(root: &Path) -> Result<usize> {
    if let Some(open) = crate::now::read(root)
        .ok()
        .and_then(|(_, snapshot)| fresh_snapshot_count(&snapshot, chrono::Utc::now()))
    {
        return Ok(open);
    }
    crate::land::open_change_count(root)
}

/// Apply the limit to a fresh pickup of `spec`. A count that cannot be read
/// does not refuse (an unreadable forge is not evidence of too much work):
/// the pickup proceeds and says the limit was not applied. A `preview`
/// refuses nothing and records nothing.
pub(crate) fn admit(root: &Path, spec: &str, over_wip: bool, preview: bool) -> Result<()> {
    let open = match open_changes(root) {
        Ok(open) => open,
        Err(why) => {
            eprintln!(
                "  the open-change count could not be read ({}), so the WIP limit was not applied",
                crate::land::one_line(&format!("{why:#}"), 160)
            );
            return Ok(());
        }
    };
    match verdict(spec, open, crate::now::wip_max(root), over_wip) {
        Verdict::Admit => Ok(()),
        Verdict::Override(line) => {
            if !preview {
                crate::events::record_gate_held(
                    root,
                    crate::events::GATE_WIP_LIMIT_OVERRIDE,
                    Some(spec.to_string()),
                    None,
                    &line,
                );
            }
            eprintln!("  WIP limit overridden: {line}");
            Ok(())
        }
        Verdict::Refuse(why) if preview => {
            eprintln!("  a real pickup would be refused: {why}");
            Ok(())
        }
        Verdict::Refuse(why) => {
            crate::events::record_gate_held(
                root,
                crate::events::GATE_WIP_LIMIT,
                Some(spec.to_string()),
                None,
                &why,
            );
            anyhow::bail!(why)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_the_limit_admits() {
        assert_eq!(verdict("BUG-1", 9, 10, false), Verdict::Admit);
        assert_eq!(verdict("BUG-1", 0, 1, true), Verdict::Admit);
    }

    #[test]
    fn at_the_limit_refuses_and_names_the_count_and_the_view() {
        let Verdict::Refuse(why) = verdict("BUG-1", 10, 10, false) else {
            panic!("at the limit must refuse");
        };
        assert!(why.contains("10 changes are open"), "{why}");
        assert!(why.contains("`aida now`"), "{why}");
        assert!(why.contains("--over-wip"), "{why}");
        assert!(matches!(
            verdict("BUG-1", 19, 10, false),
            Verdict::Refuse(_)
        ));
    }

    #[test]
    fn over_wip_passes_the_limit_with_a_line_to_record() {
        let Verdict::Override(line) = verdict("BUG-1", 12, 10, true) else {
            panic!("--over-wip must pass");
        };
        assert!(line.contains("12 changes are open"), "{line}");
        assert!(line.contains("BUG-1"), "{line}");
    }

    fn snapshot(generated_at: &str, open_prs: usize) -> crate::now::Snapshot {
        serde_json::from_value(serde_json::json!({
            "version": 1,
            "generated_at": generated_at,
            "pass_seconds": 1,
            "open_prs": open_prs,
            "wip_max": 10,
            "main": { "merges_today": 0 },
            "items": [],
            "queues": {
                "approved_not_started": 0,
                "awaiting_review": 0,
                "awaiting_rework": 0
            },
            "attention": []
        }))
        .expect("a snapshot this build reads")
    }

    #[test]
    fn only_a_recent_snapshot_supplies_the_count() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-14T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(
            fresh_snapshot_count(&snapshot("2026-11-14T11:55:00Z", 7), now),
            Some(7)
        );
        // Older than the staleness bound, from the future, or unreadable.
        assert_eq!(
            fresh_snapshot_count(&snapshot("2026-11-14T11:40:00Z", 7), now),
            None
        );
        assert_eq!(
            fresh_snapshot_count(&snapshot("2026-11-14T12:05:00Z", 7), now),
            None
        );
        assert_eq!(fresh_snapshot_count(&snapshot("yesterday", 7), now), None);
    }

    #[test]
    fn the_limit_reads_its_knob_range() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(crate::now::wip_max(root), 10, "unset: the default");
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(root.join(".aida/config.toml"), "[land]\nwip_max = 4\n").unwrap();
        assert_eq!(crate::now::wip_max(root), 4);
        std::fs::write(root.join(".aida/config.toml"), "[land]\nwip_max = 0\n").unwrap();
        assert_eq!(crate::now::wip_max(root), 10, "out of range: the default");
    }
}
