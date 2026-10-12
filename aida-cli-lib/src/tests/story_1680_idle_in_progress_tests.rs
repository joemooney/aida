//! STORY-1680: idle in-progress specs in `aida awaiting`. Fixed clocks, fake
//! driver pids (nothing here probes the process table: the driver context is
//! built directly), and temp project / home directories passed explicitly.
// trace:STORY-1680 | ai:claude

use super::*;
use crate::awaiting_you::AwaitingReport;
use crate::driver_view::{DrainCover, DriverContext};
use aida_core::driver_registry::{DriverKind, DriverRecord};
use std::collections::HashSet;

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-12T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn hours_ago(h: i64) -> DateTime<Utc> {
    now() - chrono::Duration::hours(h)
}

fn record(kind: DriverKind, id: &str, pid: u32, selector: &str) -> DriverRecord {
    DriverRecord {
        kind,
        id: id.into(),
        pid,
        pid_start_time: Some("2026-10-10T00:00:00+00:00".into()),
        selector: Some(selector.into()),
        started_at: now(),
        heartbeat_at: now(),
        ttl_secs: None,
        seat: None,
        handoff: None,
    }
}

// A context with a fake-pid relay (selector `batch:relay`), a fake-pid
// registered agent (selector `batch:agent`) and a fake-pid drain covering
// `TASK-3`.
fn ctx() -> DriverContext {
    DriverContext {
        now: now(),
        live: vec![
            record(DriverKind::Relay, "lib-split", 990_001, "batch:relay"),
            record(DriverKind::Agent, "seat-7", 990_002, "batch:agent"),
        ],
        drain: Some(DrainCover {
            pid: 990_003,
            specs: HashSet::from(["TASK-3".to_string()]),
            batch_tag: None,
        }),
    }
}

// A candidate whose driver comes from the real resolver over [`ctx`], with
// no lease (nothing of its own is alive).
fn candidate(spec: &str, tags: &[&str], status_hours_ago: i64) -> IdleCandidate {
    let tags: Vec<String> = tags.iter().map(|t| t.to_string()).collect();
    let status_changed_at = Some(hours_ago(status_hours_ago));
    IdleCandidate {
        spec_id: spec.into(),
        title: format!("{spec} title"),
        driver: Some(ctx().resolve(&[spec], &tags, None, false, status_changed_at)),
        branch_commit: None,
        status_changed_at,
        modified_at: None,
    }
}

#[test]
fn idle_spec_past_the_threshold_is_listed_with_its_age() {
    let items = select_idle(vec![candidate("TASK-1", &[], 30)], now(), 24);
    assert_eq!(items.len(), 1);
    let i = &items[0];
    assert_eq!(i.spec_id, "TASK-1");
    assert_eq!(i.idle_secs, 30 * 3600);
    assert_eq!(i.age_source, IdleAgeSource::StatusChange);
    assert_eq!(i.threshold_hours, 24);
    assert_eq!(
        i.age_label(),
        "no commits or status change for 30h (dated from its last status change)"
    );
}

#[test]
fn idle_spec_under_the_threshold_is_not_listed() {
    assert!(select_idle(vec![candidate("TASK-1", &[], 23)], now(), 24).is_empty());
    // Exactly at the threshold is not "exceeds".
    assert!(select_idle(vec![candidate("TASK-1", &[], 24)], now(), 24).is_empty());
}

#[test]
fn driven_and_queued_specs_are_never_listed_whatever_their_age() {
    let cands = vec![
        candidate("TASK-2", &["batch:relay"], 10_000),
        candidate("TASK-3", &[], 10_000),
        candidate("TASK-4", &["batch:agent"], 10_000),
        candidate("TASK-5", &["batch:nobody"], 10_000),
    ];
    let kinds: Vec<&str> = cands
        .iter()
        .map(|c| c.driver.as_ref().unwrap().kind())
        .collect();
    assert_eq!(kinds, ["relay", "drain", "agent", "queued"]);
    assert_eq!(cands[0].driver.as_ref().unwrap().label(), "relay:lib-split");
    assert_eq!(cands[1].driver.as_ref().unwrap().label(), "drain:990003");
    assert!(select_idle(cands, now(), 24).is_empty());
}

#[test]
fn a_candidate_with_no_resolved_driver_or_no_measurable_age_is_skipped() {
    let mut unresolved = candidate("TASK-6", &[], 100);
    unresolved.driver = None;
    let mut ageless = candidate("TASK-7", &[], 100);
    ageless.status_changed_at = None;
    assert!(select_idle(vec![unresolved, ageless], now(), 24).is_empty());
}

#[test]
fn a_newer_branch_commit_keeps_a_spec_out_of_the_idle_list() {
    // Fresh branch commit, ancient status change: the branch is the last
    // activity, so the spec is NOT idle past the threshold.
    let mut fresh_branch = candidate("TASK-8", &[], 500);
    fresh_branch.branch_commit = Some(("task-8-work".into(), hours_ago(2)));
    assert!(select_idle(vec![fresh_branch], now(), 24).is_empty());
}

#[test]
fn a_newer_status_change_wins_over_an_old_branch_commit() {
    // An old branch (e.g. a squash-merged earlier slice) must not make a spec
    // sent back to In Progress an hour ago read as long idle.
    let mut reopened = candidate("TASK-9", &[], 1);
    reopened.branch_commit = Some(("task-9-work".into(), hours_ago(48)));
    assert!(select_idle(vec![reopened], now(), 24).is_empty());
    // Both old: the age runs from the later one (status change, 30h).
    let mut both_old = candidate("TASK-9", &[], 30);
    both_old.branch_commit = Some(("task-9-work".into(), hours_ago(48)));
    let items = select_idle(vec![both_old], now(), 24);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].age_source, IdleAgeSource::StatusChange);
    assert_eq!(items[0].idle_secs, 30 * 3600);
    assert_eq!(items[0].branch, None);
}

#[test]
fn an_older_status_change_dates_the_age_by_the_newer_branch_commit() {
    let mut c = candidate("TASK-13", &[], 100);
    c.branch_commit = Some(("task-13-work".into(), hours_ago(40)));
    let items = select_idle(vec![c], now(), 24);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].age_source, IdleAgeSource::BranchCommit);
    assert_eq!(items[0].idle_secs, 40 * 3600);
    assert_eq!(items[0].branch.as_deref(), Some("task-13-work"));
    assert_eq!(
        items[0].age_label(),
        "no commits or status change for 40h (dated from its last branch commit, task-13-work)"
    );
}

#[test]
fn spec_branches_merge_named_branches_with_the_lease_branch() {
    let named = std::collections::HashMap::from([(
        "TASK-14".to_string(),
        vec!["task-14-work".to_string()],
    )]);
    // Lease branch not named for the spec is added.
    assert_eq!(
        spec_branches(&named, "task-14", Some("relay/abc")),
        ["task-14-work", "relay/abc"]
    );
    // Already-named lease branch is not duplicated; blank is ignored.
    assert_eq!(
        spec_branches(&named, "TASK-14", Some("task-14-work")),
        ["task-14-work"]
    );
    assert_eq!(
        spec_branches(&named, "TASK-14", Some("  ")),
        ["task-14-work"]
    );
    // No named branch: the lease branch alone.
    assert_eq!(spec_branches(&named, "TASK-15", Some("wt-15")), ["wt-15"]);
    assert!(spec_branches(&named, "TASK-15", None).is_empty());
}

fn orphan(spec: &str) -> PsOrphan {
    PsOrphan {
        spec: spec.into(),
        title: format!("{spec} title"),
        stale_lease: false,
        likely_fanout: false,
        possibly_subagent: false,
        driver: Some(DerivedDriver::Idle { since_secs: None }),
    }
}

#[test]
fn a_spec_reported_idle_is_dropped_from_the_orphan_list() {
    let mut idle = sample_item();
    idle.spec_id = "TASK-16".into();
    let kept = orphans_not_idle(vec![orphan("task-16"), orphan("TASK-17")], &[idle]);
    let ids: Vec<&str> = kept.iter().map(|o| o.spec.as_str()).collect();
    assert_eq!(ids, ["TASK-17"]);
}

// trace:STORY-1680 | ai:claude
#[test]
fn modified_at_dates_the_age_only_when_no_status_change_is_on_record() {
    let mut c = candidate("TASK-18", &[], 0);
    c.status_changed_at = None;
    c.modified_at = Some(hours_ago(30));
    let items = select_idle(vec![c.clone()], now(), 24);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].age_source, IdleAgeSource::Modified);
    assert_eq!(items[0].age_source.as_str(), "last-modified");
    assert_eq!(
        items[0].age_label(),
        "no commits or status change for 30h (dated from its last modification)"
    );
    // A recorded status change wins over the modification time.
    c.status_changed_at = Some(hours_ago(50));
    let items = select_idle(vec![c], now(), 24);
    assert_eq!(items[0].age_source, IdleAgeSource::StatusChange);
    assert_eq!(items[0].idle_secs, 50 * 3600);
}

// M1 (advisor review): a stale-leased spec with a live subagent in its
// worktree (BUG-1656) may be being worked; it is not listed as idle and
// stays on the orphan channel ("possibly worked by a subagent").
// trace:STORY-1680 | ai:claude
#[test]
fn a_possibly_subagent_orphan_is_not_idle_and_stays_orphaned() {
    let mut sub = orphan("TASK-19");
    sub.stale_lease = true;
    sub.possibly_subagent = true;
    let mut fanout = orphan("TASK-21");
    fanout.likely_fanout = true;
    let orphans = vec![sub, orphan("TASK-20"), fanout];
    let ids: Vec<&str> = idle_orphans(&orphans)
        .iter()
        .map(|o| o.spec.as_str())
        .collect();
    assert_eq!(ids, ["TASK-20"]);
    let mut idle = sample_item();
    idle.spec_id = "TASK-20".into();
    let kept = orphans_not_idle(orphans, &[idle]);
    let kept: Vec<(&str, bool)> = kept
        .iter()
        .map(|o| (o.spec.as_str(), o.possibly_subagent))
        .collect();
    assert_eq!(kept, [("TASK-19", true), ("TASK-21", false)]);
}

#[test]
fn rows_are_ordered_oldest_first() {
    let items = select_idle(
        vec![candidate("TASK-10", &[], 30), candidate("TASK-11", &[], 90)],
        now(),
        24,
    );
    let ids: Vec<&str> = items.iter().map(|i| i.spec_id.as_str()).collect();
    assert_eq!(ids, ["TASK-11", "TASK-10"]);
}

#[test]
fn threshold_defaults_to_24h_and_reads_the_config_key() {
    assert_eq!(DEFAULT_IDLE_IN_PROGRESS_HOURS, 24);
    assert_eq!(threshold_from_toml(""), None);
    assert_eq!(
        threshold_from_toml("[awaiting]\nidle_in_progress_hours = 48\n"),
        Some(48)
    );
    // Non-positive / non-integer values fall through to the next layer.
    assert_eq!(
        threshold_from_toml("[awaiting]\nidle_in_progress_hours = 0\n"),
        None
    );
    assert_eq!(
        threshold_from_toml("[awaiting]\nidle_in_progress_hours = \"2d\"\n"),
        None
    );
}

fn write_config(dir: &Path, body: &str) {
    std::fs::create_dir_all(dir.join(".aida")).unwrap();
    std::fs::write(dir.join(".aida").join("config.toml"), body).unwrap();
}

#[test]
fn threshold_layers_project_over_isolated_home_over_default() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    assert_eq!(
        idle_threshold_hours(project.path(), Some(home.path())),
        24,
        "no config anywhere → default"
    );
    write_config(home.path(), "[awaiting]\nidle_in_progress_hours = 12\n");
    assert_eq!(idle_threshold_hours(project.path(), Some(home.path())), 12);
    write_config(project.path(), "[awaiting]\nidle_in_progress_hours = 72\n");
    assert_eq!(idle_threshold_hours(project.path(), Some(home.path())), 72);
}

#[test]
fn a_config_override_changes_which_specs_are_listed() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let thirty_hours = || vec![candidate("TASK-12", &[], 30)];
    let default = idle_threshold_hours(project.path(), Some(home.path()));
    assert_eq!(select_idle(thirty_hours(), now(), default).len(), 1);
    write_config(project.path(), "[awaiting]\nidle_in_progress_hours = 48\n");
    let raised = idle_threshold_hours(project.path(), Some(home.path()));
    assert!(select_idle(thirty_hours(), now(), raised).is_empty());
}

fn git(dir: &Path, args: &[&str], date: &str) {
    let st = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .unwrap();
    assert!(st.status.success(), "git {args:?}: {st:?}");
}

#[test]
fn branch_commit_is_the_newest_commit_ahead_of_main_on_a_branch_named_for_the_spec() {
    let repo = tempfile::tempdir().unwrap();
    let r = repo.path();
    git(r, &["init", "-q", "-b", "main"], "2026-10-01T00:00:00Z");
    git(
        r,
        &["commit", "-q", "--allow-empty", "-m", "base"],
        "2026-10-11T00:00:00Z",
    );
    // A branch with no commits of its own carries no spec activity, even
    // though its tip (main's) is recent.
    git(r, &["branch", "task-20-empty"], "2026-10-11T00:00:00Z");
    git(
        r,
        &["checkout", "-q", "-b", "task-21-work"],
        "2026-10-11T00:00:00Z",
    );
    git(
        r,
        &["commit", "-q", "--allow-empty", "-m", "work"],
        "2026-10-05T06:00:00Z",
    );
    git(r, &["checkout", "-q", "main"], "2026-10-11T00:00:00Z");

    let named = branches_by_spec(r);
    assert_eq!(
        named.get("TASK-21").map(Vec::as_slice),
        Some(&["task-21-work".to_string()][..])
    );
    assert_eq!(base_ref(r), "main");
    assert_eq!(newest_commit_ahead(r, "task-20-empty", "main"), None);
    let at = newest_commit_ahead(r, "task-21-work", "main").unwrap();
    assert_eq!(at.to_rfc3339(), "2026-10-05T06:00:00+00:00");
}

fn sample_item() -> IdleInProgressItem {
    IdleInProgressItem {
        spec_id: "TASK-30".into(),
        title: "abandoned work".into(),
        idle_secs: 50 * 3600,
        age_source: IdleAgeSource::BranchCommit,
        branch: Some("task-30-work".into()),
        threshold_hours: 24,
    }
}

#[test]
fn json_and_toon_carry_the_entry() {
    let report = AwaitingReport {
        idle_in_progress: vec![sample_item()],
        ..Default::default()
    };
    let json = report.to_json();
    let row = &json["idle_in_progress"][0];
    assert_eq!(row["spec_id"], "TASK-30");
    assert_eq!(row["driver_kind"], "idle");
    assert_eq!(row["idle_secs"], 50 * 3600);
    assert_eq!(row["idle_age"], "2d");
    assert_eq!(row["age_source"], "branch-commit");
    assert_eq!(row["branch"], "task-30-work");
    assert_eq!(row["threshold_hours"], 24);

    let toon = report.render_agent_view();
    assert!(toon.contains("idle_in_progress[1]"), "{toon}");
    assert!(toon.contains("TASK-30"), "{toon}");
    assert!(toon.contains("branch-commit"), "{toon}");

    assert_eq!(report.total(), 1);
    let line = report.compact_line().expect("an idle spec awaits");
    assert!(line.contains("1 idle in-progress"), "{line}");

    let mut human = Vec::new();
    report.render(true, &mut human).unwrap();
    let human = String::from_utf8(human).unwrap();
    assert!(
        human.contains("in-progress but nothing is working it"),
        "{human}"
    );
    assert!(
        human.contains("no commits or status change for 2d (dated from its last branch commit"),
        "{human}"
    );
}
