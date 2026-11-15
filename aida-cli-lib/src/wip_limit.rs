//! The WIP limit on new work: while the project already carries
//! `[land] wip_max` open changes, `aida queue work` starts no new spec.
//! Rework and review pickups reduce the count, so they are never limited.
// trace:STORY-2768 | ai:claude

use std::path::Path;

use anyhow::Result;

/// What a pickup is to the limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pickup {
    /// It continues existing work (an open change's rework, a review, a
    /// resumed or adopted session, a spec already started) or is not an
    /// implementer pickup: never limited, nothing to record.
    Continues,
    /// An explicit or resolved canonical branch: existing work is exempt,
    /// and passing the limit is recorded.
    Branch,
    /// A new spec: limited.
    New,
}

/// The facts of a pickup the limit reads, as `aida queue work` resolved them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Facts {
    pub implementer: bool,
    /// The target is a change to review.
    pub review: bool,
    /// An open change already carries the spec.
    pub rework: bool,
    /// `--resume`, or an orphaned session being re-entered.
    pub resumed: bool,
    /// The spec is already In Progress or Done.
    pub started: bool,
    /// `--branch` was passed, or an existing canonical branch was resolved.
    pub branch: bool,
}

pub(crate) fn classify(facts: &Facts) -> Pickup {
    if !facts.implementer || facts.review || facts.rework || facts.resumed || facts.started {
        Pickup::Continues
    } else if facts.branch {
        Pickup::Branch
    } else {
        Pickup::New
    }
}

/// The line to record when an existing-branch pickup passes the limit; `None`
/// under it.
pub(crate) fn branch_note(spec: &str, open: usize, max: u64) -> Option<String> {
    ((open as u64) >= max).then(|| {
        format!(
            "{open} changes are open and the limit is {max}; {spec} continues on the branch \
             selected for existing work, which is not limited"
        )
    })
}

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

/// Admit orchestrator work while its original status is still visible, before
/// queue insertion, ownership registration, the plan prelude, or the phase bump.
// trace:TASK-3799 | ai:codex
pub(crate) fn admit_phase1(root: &Path, storage: &aida_core::Storage, spec: &str) -> Result<()> {
    let store = storage.load()?;
    let req = store
        .requirements
        .iter()
        .find(|r| crate::spec_matches(r, spec))
        .ok_or_else(|| anyhow::anyhow!("no requirement matches `{spec}`"))?;
    if crate::auto_complete_phase1_target_status(&req.status).is_none() {
        return Ok(());
    }
    let display = req.display_id();
    let forge = crate::forge::forge_for_open_change(root);
    if crate::session_start::pickup_pr_branch(&*forge, &display, None)?.is_some() {
        return Ok(());
    }
    let canonical = crate::queue_cmd::canonical_resume_branch(root, &display, true)?;
    admit(
        root,
        &display,
        if canonical.is_some() {
            Pickup::Branch
        } else {
            Pickup::New
        },
        false,
        false,
    )
}

/// Apply the limit to a pickup of `spec`. A count that cannot be read
/// does not refuse (an unreadable forge is not evidence of too much work):
/// the pickup proceeds and says the limit was not applied. A `preview`
/// refuses nothing and records nothing.
pub(crate) fn admit(
    root: &Path,
    spec: &str,
    pickup: Pickup,
    over_wip: bool,
    preview: bool,
) -> Result<()> {
    if pickup == Pickup::Continues {
        return Ok(());
    }
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
    let max = crate::now::wip_max(root);
    if pickup == Pickup::Branch {
        if let Some(line) = branch_note(spec, open, max) {
            if !preview {
                crate::events::record_gate_held(
                    root,
                    crate::events::GATE_WIP_LIMIT_OVERRIDE,
                    Some(spec.to_string()),
                    None,
                    &line,
                );
            }
            eprintln!("  WIP limit passed: {line}");
        }
        return Ok(());
    }
    match verdict(spec, open, max, over_wip) {
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

    #[test]
    fn only_a_new_implementer_spec_is_limited_and_a_branch_pickup_is_recorded() {
        let new = Facts {
            implementer: true,
            ..Facts::default()
        };
        assert_eq!(classify(&new), Pickup::New);
        assert_eq!(
            classify(&Facts {
                branch: true,
                ..new
            }),
            Pickup::Branch,
            "--branch"
        );
        for (what, continues) in [
            (
                "a reviewer",
                Facts {
                    implementer: false,
                    ..new
                },
            ),
            (
                "a review target",
                Facts {
                    review: true,
                    ..new
                },
            ),
            (
                "an open change's rework",
                Facts {
                    rework: true,
                    ..new
                },
            ),
            (
                "--resume",
                Facts {
                    resumed: true,
                    ..new
                },
            ),
            (
                "a started spec",
                Facts {
                    started: true,
                    ..new
                },
            ),
            // What continues work stays unrecorded even with --branch.
            (
                "rework on --branch",
                Facts {
                    rework: true,
                    branch: true,
                    ..new
                },
            ),
        ] {
            assert_eq!(classify(&continues), Pickup::Continues, "{what}");
        }
        assert_eq!(branch_note("BUG-1", 9, 10), None);
        let line = branch_note("BUG-1", 12, 10).expect("at the limit");
        assert!(line.contains("12 changes are open"), "{line}");
        assert!(line.contains("limit is 10"), "{line}");
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
