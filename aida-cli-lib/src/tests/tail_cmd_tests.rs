// Resolver tests for `aida tail` — pure functions over an in-memory
// `TailIndex`, no filesystem, no cwd, no git.
// trace:TASK-1167 | ai:claude

use super::*;
use std::time::{Duration as StdDuration, UNIX_EPOCH};

fn t(secs: u64) -> SystemTime {
    UNIX_EPOCH + StdDuration::from_secs(secs)
}

fn drain(id: &str, secs: u64) -> DrainLog {
    DrainLog {
        id: id.to_string(),
        path: PathBuf::from(format!("/repo/.aida/burndown/{id}.jsonl")),
        mtime: t(secs),
    }
}

fn headless(filename: &str, secs: u64) -> LogEntry {
    let (kind, spec, lease) = headless_tail::parse_filename(filename);
    LogEntry {
        path: PathBuf::from(format!("/repo/.aida/headless-logs/{filename}")),
        filename: filename.to_string(),
        mtime: t(secs),
        size: 128,
        kind,
        spec,
        lease,
    }
}

fn session(id: &str, scope: &str, branch: &str) -> SessionRef {
    SessionRef {
        id: id.to_string(),
        scope: scope.to_string(),
        branch: branch.to_string(),
        role: Some("implementer".to_string()),
    }
}

fn index() -> TailIndex {
    TailIndex {
        drains: vec![drain("20260721T064452Z-019f836b", 500)],
        headless: vec![
            headless("task-1167-019e4405-5073-7672-9395-16d4ca8be1a4.jsonl", 400),
            headless("advise-TASK-403-019e50db.jsonl", 300),
        ],
        sessions: vec![
            session("aabbccdd1122", "TASK-1167", "task-1167"),
            session("eeff00113344", "TASK-999", "task-999"),
        ],
    }
}

fn found_path(r: &Resolution) -> &Path {
    match r {
        Resolution::Found { path, .. } => path.as_path(),
        other => panic!("expected Found, got {other:?}"),
    }
}

#[test]
fn drain_keyword_selects_the_newest_drain_log() {
    let idx = index();
    let r = resolve(&idx, Some("drain"));
    assert_eq!(
        found_path(&r),
        Path::new("/repo/.aida/burndown/20260721T064452Z-019f836b.jsonl")
    );
    // `burndown` is accepted as the same thing.
    assert_eq!(found_path(&resolve(&idx, Some("BURNDOWN"))), found_path(&r));
}

#[test]
fn drain_keyword_with_no_drain_log_reports_cleanly() {
    let idx = TailIndex::default();
    match resolve(&idx, Some("drain")) {
        Resolution::NoLog { .. } => {}
        other => panic!("expected NoLog, got {other:?}"),
    }
}

#[test]
fn session_id_resolves_to_its_branch_log() {
    let idx = index();
    let r = resolve(&idx, Some("aabbccdd1122"));
    assert_eq!(
        found_path(&r),
        Path::new("/repo/.aida/headless-logs/task-1167-019e4405-5073-7672-9395-16d4ca8be1a4.jsonl")
    );
}

#[test]
fn session_id_prefix_resolves_when_unambiguous() {
    let idx = index();
    let r = resolve(&idx, Some("aabb"));
    assert_eq!(
        found_path(&r),
        Path::new("/repo/.aida/headless-logs/task-1167-019e4405-5073-7672-9395-16d4ca8be1a4.jsonl")
    );
}

#[test]
fn ambiguous_session_prefix_is_reported_not_guessed() {
    let mut idx = index();
    idx.sessions
        .push(session("aabbeeff9999", "TASK-2", "task-2"));
    match resolve(&idx, Some("aabb")) {
        Resolution::NotFound { message } => {
            assert!(message.contains("matches 2 sessions"), "{message}")
        }
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn session_with_no_log_reports_cleanly_rather_than_erroring() {
    let idx = index();
    // TASK-999 is leased but has written no headless log — the interactive case.
    match resolve(&idx, Some("eeff00113344")) {
        Resolution::NoLog { what, .. } => {
            assert!(what.contains("eeff00113344"), "{what}");
            assert!(what.contains("TASK-999"), "{what}");
        }
        other => panic!("expected NoLog, got {other:?}"),
    }
}

#[test]
fn spec_id_finds_its_log() {
    let idx = index();
    assert_eq!(
        found_path(&resolve(&idx, Some("TASK-1167"))),
        Path::new("/repo/.aida/headless-logs/task-1167-019e4405-5073-7672-9395-16d4ca8be1a4.jsonl")
    );
    // The advisor-tier log form resolves off its spec too.
    assert_eq!(
        found_path(&resolve(&idx, Some("TASK-403"))),
        Path::new("/repo/.aida/headless-logs/advise-TASK-403-019e50db.jsonl")
    );
}

#[test]
fn spec_id_picks_the_newest_of_several_logs() {
    let mut idx = index();
    idx.headless
        .insert(0, headless("resume-TASK-1167-019f9999.jsonl", 900));
    assert_eq!(
        found_path(&resolve(&idx, Some("TASK-1167"))),
        Path::new("/repo/.aida/headless-logs/resume-TASK-1167-019f9999.jsonl")
    );
}

#[test]
fn drain_id_resolves_by_stem_or_substring() {
    let idx = index();
    assert_eq!(
        found_path(&resolve(&idx, Some("20260721T064452Z-019f836b"))),
        Path::new("/repo/.aida/burndown/20260721T064452Z-019f836b.jsonl")
    );
    assert_eq!(
        found_path(&resolve(&idx, Some("019f836b"))),
        Path::new("/repo/.aida/burndown/20260721T064452Z-019f836b.jsonl")
    );
}

#[test]
fn unknown_selector_is_an_error_with_a_next_step() {
    let idx = index();
    match resolve(&idx, Some("NOPE-42")) {
        Resolution::NotFound { message } => {
            assert!(message.contains("aida ps"), "{message}");
            assert!(message.contains("--list"), "{message}");
        }
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn no_selector_picks_the_most_recently_written_log_of_any_kind() {
    let idx = index();
    // The drain log is newest here.
    assert_eq!(
        found_path(&resolve(&idx, None)),
        Path::new("/repo/.aida/burndown/20260721T064452Z-019f836b.jsonl")
    );

    let mut idx2 = index();
    idx2.headless[0].mtime = t(9_000);
    assert_eq!(
        found_path(&resolve(&idx2, None)),
        Path::new("/repo/.aida/headless-logs/task-1167-019e4405-5073-7672-9395-16d4ca8be1a4.jsonl")
    );
}

#[test]
fn empty_project_reports_cleanly_with_no_selector() {
    match resolve(&TailIndex::default(), None) {
        Resolution::NoLog { .. } => {}
        other => panic!("expected NoLog, got {other:?}"),
    }
}

#[test]
fn blank_selector_is_treated_as_no_selector() {
    let idx = index();
    assert_eq!(
        found_path(&resolve(&idx, Some("   "))),
        found_path(&resolve(&idx, None))
    );
}

#[test]
fn discover_drain_logs_is_empty_for_a_missing_directory() {
    assert!(discover_drain_logs(Path::new("/definitely/not/a/real/dir")).is_empty());
}

#[test]
fn one_character_selector_never_prefix_matches_a_session() {
    let idx = index();
    // Too short to be a session prefix — falls through to the not-found path
    // rather than silently picking `aabbccdd1122`.
    match resolve(&idx, Some("a")) {
        Resolution::NotFound { .. } => {}
        other => panic!("expected NotFound, got {other:?}"),
    }
}
