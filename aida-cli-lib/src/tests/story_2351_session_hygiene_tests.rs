//! STORY-2351 — the hourly hygiene pass: its reclaim rails and its routing
//! decision. Pure over a gathered outcome; nothing here reaps, gcs or deletes.
// trace:STORY-2351 | ai:claude

use super::*;

const HOUR: u64 = 3600;

fn opts(dry_run: bool) -> HygieneOptions {
    HygieneOptions {
        dry_run,
        idle_hours: DEFAULT_IDLE_HOURS,
    }
}

fn salvage(idle_secs: u64) -> SalvageCandidate {
    SalvageCandidate {
        session: "abcd1234".into(),
        worktree: PathBuf::from("/wt/aida-take-9"),
        idle_secs,
        hint: Some("dead process, uncommitted work in /wt/aida-take-9 — salvage it".into()),
    }
}

/// AC1/AC2: reclaim only caches idle 24h+, and never waive a rail.
#[test]
fn reclaim_options_keep_every_rail_and_the_idle_window() {
    let r = reclaim_options(&opts(false));
    assert!(r.apply);
    assert_eq!(r.min_age_mins, 24 * 60);
    assert!(!r.include_live, "a live lease's cache is never reclaimed");
    assert!(!r.include_open_prs, "an open PR's cache is never reclaimed");
    assert!(
        r.no_scratch,
        "job tmp and session scratch are not regenerable"
    );
    assert_eq!(r.target_pct, None);
    assert!(
        !reclaim_options(&opts(true)).apply,
        "a dry run deletes nothing"
    );
    let custom = HygieneOptions {
        dry_run: false,
        idle_hours: 48,
    };
    assert_eq!(reclaim_options(&custom).min_age_mins, 48 * 60);
}

/// AC3: a clean pass, ordinary skips and fresh salvageable work route nothing.
#[test]
fn a_clean_pass_routes_nothing() {
    let outcome = HygieneOutcome {
        reap: vec![
            ("s1".into(), "reaped — merged, exited".into()),
            ("s2".into(), "skipped — worktree in use by pid 42".into()),
        ],
        agent_gc: Some(Ok(3)),
        reclaimed: vec![(PathBuf::from("/t/worktrees/x"), 10)],
        salvageable: vec![salvage(2 * HOUR)],
        ..Default::default()
    };
    assert!(keep_items(&outcome, DEFAULT_IDLE_HOURS).is_empty());
}

/// AC3: salvageable work untouched 24h+ is kept and routed with its act.
#[test]
fn old_salvageable_work_routes_with_the_salvage_act() {
    let outcome = HygieneOutcome {
        salvageable: vec![salvage(30 * HOUR)],
        ..Default::default()
    };
    let items = keep_items(&outcome, DEFAULT_IDLE_HOURS);
    assert_eq!(items.len(), 1);
    assert!(items[0].what.contains("/wt/aida-take-9"), "{:?}", items[0]);
    assert!(items[0].what.contains("30h"), "{:?}", items[0]);
    assert!(items[0].act.contains("salvage it"), "{:?}", items[0]);

    let no_hint = HygieneOutcome {
        salvageable: vec![SalvageCandidate {
            hint: None,
            ..salvage(25 * HOUR)
        }],
        ..Default::default()
    };
    let items = keep_items(&no_hint, DEFAULT_IDLE_HOURS);
    assert!(
        items[0].act.contains("git -C /wt/aida-take-9 status"),
        "{:?}",
        items[0]
    );
}

/// AC3: a reap refused because retention is not activated names the
/// activation command.
#[test]
fn retention_not_activated_routes_with_the_activation_command() {
    let refusal = format!(
        "skipped — mail retention refused; check whether every mail writer on this machine \
         takes a write permit: `{}`",
        aida_core::mailbox_lifecycle::ACTIVATION_COMMAND
    );
    let outcome = HygieneOutcome {
        reap: vec![("deadbeef".into(), refusal)],
        ..Default::default()
    };
    let items = keep_items(&outcome, DEFAULT_IDLE_HOURS);
    assert_eq!(items.len(), 1);
    assert!(items[0].what.contains("retention is not activated"));
    assert!(items[0]
        .act
        .contains(aida_core::mailbox_lifecycle::ACTIVATION_COMMAND));
}

/// Failures are kept and routed; the route is one stderr line naming each.
#[test]
fn failures_route_as_one_line() {
    let outcome = HygieneOutcome {
        reap: vec![(
            "s9".into(),
            "failed — could not remove worktree /wt/x".into(),
        )],
        agent_gc: Some(Err("registry unreadable".into())),
        reclaim_failed: vec![(PathBuf::from("/t/worktrees/y"), "EACCES".into())],
        ..Default::default()
    };
    let items = keep_items(&outcome, DEFAULT_IDLE_HOURS);
    assert_eq!(items.len(), 3, "{items:?}");
    let line = route_line(&items);
    assert_eq!(line.lines().count(), 1, "{line}");
    assert!(line.starts_with("session hygiene kept 3 item(s)"), "{line}");
    assert!(line.contains("aida session reap"), "{line}");
    assert!(line.contains("aida agent gc"), "{line}");
    assert!(line.contains("/t/worktrees/y"), "{line}");
}

/// The summary reports a deferred reclaim instead of failing the run.
#[test]
fn deferred_reclaim_is_reported_not_routed() {
    let outcome = HygieneOutcome {
        agent_gc: Some(Ok(0)),
        reclaim_skipped: Some("the 180s budget was spent on reap".into()),
        ..Default::default()
    };
    let text = render_summary(&outcome, false);
    assert!(
        text.contains("reclaim   : skipped — the 180s budget"),
        "{text}"
    );
    assert!(keep_items(&outcome, DEFAULT_IDLE_HOURS).is_empty());
}
