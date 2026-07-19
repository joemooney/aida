use super::*;
use aida_core::{Relationship, RelationshipType};

/// BUG-453: headless_log_len returns the byte length of the session's JSONL
/// log (matched by `-<session_id>.jsonl` suffix) and reflects growth — the
/// signal the watchdog uses so a reading-but-productive session isn't
/// false-killed. None when no matching log exists.
#[test]
fn headless_log_len_tracks_session_log_growth() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    let dir = root.join(".aida").join("headless-logs");
    std::fs::create_dir_all(&dir).unwrap();
    let session = "019e9a07-8e2d-7800-ad4e-73534034f704";

    // No log yet → None.
    assert_eq!(headless_log_len(root, session), None);

    // A non-matching log (different session) is ignored.
    std::fs::write(dir.join("task-x-deadbeef-0000.jsonl"), "noise\n").unwrap();
    assert_eq!(headless_log_len(root, session), None);

    // The matching log → its byte length; appending grows it.
    let logp = dir.join(format!("task-673-{session}.jsonl"));
    std::fs::write(&logp, "a").unwrap();
    assert_eq!(headless_log_len(root, session), Some(1));
    std::fs::write(&logp, "abcdef").unwrap();
    assert_eq!(headless_log_len(root, session), Some(6));
}

// TASK-298: FULLY-ISOLATED tests of the pure stream-json stall parser.
// Each takes one JSONL line (or a small log body) and asserts on the
// detection verdict — no threads, no files, no orchestrator. trace:TASK-298

#[test]
fn stall_parser_silent_on_blank_and_non_json() {
    assert_eq!(headless_line_permission_stall(""), None);
    assert_eq!(headless_line_permission_stall("   "), None);
    assert_eq!(headless_line_permission_stall("{not-json"), None);
}

#[test]
fn stall_parser_silent_on_clean_events() {
    let init = r#"{"type":"system","subtype":"init","model":"x","cwd":"/w"}"#;
    assert_eq!(headless_line_permission_stall(init), None);
    let ok_result = r#"{"type":"result","subtype":"success","is_error":false}"#;
    assert_eq!(headless_line_permission_stall(ok_result), None);
    // An empty permission_denials array is NOT a stall.
    let empty = r#"{"type":"assistant","permission_denials":[]}"#;
    assert_eq!(headless_line_permission_stall(empty), None);
}

#[test]
fn stall_parser_detects_permission_denial_with_tool_name() {
    let line = r#"{"type":"assistant","permission_denials":[{"tool_name":"Bash"}]}"#;
    let reason = headless_line_permission_stall(line).expect("denial detected");
    assert!(reason.contains("permission gate"), "got: {reason}");
    assert!(reason.contains("Bash"), "got: {reason}");
}

#[test]
fn stall_parser_counts_multiple_denials_and_falls_back_to_tool_alias() {
    // First denial uses the `tool` alias (not `tool_name`); the count tail
    // reports the extras.
    let line =
        r#"{"type":"assistant","permission_denials":[{"tool":"Write"},{"tool_name":"Bash"}]}"#;
    let reason = headless_line_permission_stall(line).expect("denial detected");
    assert!(reason.contains("Write"), "got: {reason}");
    assert!(reason.contains("+1 more"), "got: {reason}");
}

#[test]
fn stall_parser_detects_is_error_with_result_detail() {
    // The SPIKE-7 false-positive: exit code 0 but is_error true.
    let line = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"permission gate refused\nsecond line"}"#;
    let reason = headless_line_permission_stall(line).expect("is_error detected");
    assert!(reason.contains("is_error"), "got: {reason}");
    // Multi-line detail collapses to the first non-empty line.
    assert!(reason.contains("permission gate refused"), "got: {reason}");
    assert!(!reason.contains("second line"), "got: {reason}");
}

#[test]
fn stall_parser_is_error_falls_back_to_subtype_when_no_result_text() {
    let line = r#"{"type":"result","subtype":"error_max_turns","is_error":true}"#;
    let reason = headless_line_permission_stall(line).expect("is_error detected");
    assert!(reason.contains("error_max_turns"), "got: {reason}");
}

#[test]
fn stall_parser_denial_wins_over_is_error_on_same_line() {
    let line =
        r#"{"type":"assistant","is_error":true,"permission_denials":[{"tool_name":"Edit"}]}"#;
    let reason = headless_line_permission_stall(line).expect("detected");
    assert!(reason.contains("permission gate"), "got: {reason}");
    assert!(reason.contains("Edit"), "got: {reason}");
}

#[test]
fn stall_scan_folds_log_and_returns_first_signal() {
    // A realistic log: clean lines, then a denial, then more lines. The
    // scan returns the first stall signal and ignores the clean prefix.
    let log = [
        r#"{"type":"system","subtype":"init","model":"x","cwd":"/w"}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"working"}]}}"#,
        r#"{"type":"assistant","permission_denials":[{"tool_name":"Bash"}]}"#,
        r#"{"type":"result","subtype":"success","is_error":false}"#,
    ]
    .join("\n");
    let reason = headless_log_permission_stall(&log).expect("stall found in log");
    assert!(reason.contains("Bash"), "got: {reason}");

    // A clean log yields no stall.
    let clean = [
        r#"{"type":"system","subtype":"init","model":"x","cwd":"/w"}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":10}"#,
    ]
    .join("\n");
    assert_eq!(headless_log_permission_stall(&clean), None);
}

fn req(spec_id: &str, title: &str, description: &str) -> Requirement {
    let mut req = Requirement::new(title.to_string(), description.to_string());
    req.spec_id = Some(spec_id.to_string());
    req.status = RequirementStatus::Approved;
    req.req_type = RequirementType::Task;
    req.tags.insert("codex".to_string());
    req
}

fn store_with_related() -> RequirementsStore {
    let mut parent = req("STORY-425", "Communication model", "Parent story");
    parent.req_type = RequirementType::Story;
    let mut task = req(
        "TASK-492",
        "aida brief command",
        "Full description\n\n## Acceptance\n- embed this faithfully",
    );
    let child = req("TASK-493", "Follow-up", "Child task");
    task.relationships.push(Relationship {
        rel_type: RelationshipType::Parent,
        target_id: parent.id,
        created_at: None,
        created_by: None,
    });
    parent.relationships.push(Relationship {
        rel_type: RelationshipType::Child,
        target_id: task.id,
        created_at: None,
        created_by: None,
    });
    task.relationships.push(Relationship {
        rel_type: RelationshipType::Child,
        target_id: child.id,
        created_at: None,
        created_by: None,
    });
    let mut store = RequirementsStore::new();
    store.requirements = vec![parent, task, child];
    store
}

/// TASK-502: the `.pending` sentinel lifecycle — add is idempotent,
/// clear drops the matching entry, and an emptied sentinel is removed.
#[test]
fn pending_brief_sentinel_add_clear_lifecycle() {
    use super::{add_pending_brief, clear_pending_brief, pending_briefs_path, read_pending_briefs};
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let agent_dir = root.join(".aida").join("agent-briefs").join("codex");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let brief_a = agent_dir.join("TASK-1-stamp.md");
    let brief_b = agent_dir.join("TASK-2-stamp.md");
    std::fs::write(&brief_a, "a").unwrap();
    std::fs::write(&brief_b, "b").unwrap();

    let pending = pending_briefs_path(root, "codex");

    // add A twice → one entry (idempotent); add B → two entries.
    add_pending_brief(root, "codex", &brief_a).unwrap();
    add_pending_brief(root, "codex", &brief_a).unwrap();
    add_pending_brief(root, "codex", &brief_b).unwrap();
    let entries = read_pending_briefs(&pending);
    assert_eq!(entries.len(), 2, "idempotent add: {entries:?}");
    // stored project-relative
    assert!(entries.iter().all(|e| e.starts_with(".aida/agent-briefs/")));

    // clear A → only B remains, file still present.
    clear_pending_brief(&brief_a);
    let entries = read_pending_briefs(&pending);
    assert_eq!(entries.len(), 1, "after clearing A: {entries:?}");
    assert!(entries[0].ends_with("TASK-2-stamp.md"));
    assert!(pending.exists());

    // clear B → empty → sentinel removed.
    clear_pending_brief(&brief_b);
    assert!(read_pending_briefs(&pending).is_empty());
    assert!(!pending.exists(), "emptied .pending should be deleted");
}

/// TASK-102: `aida show --rels` (and the `--relations` alias) parse to the
/// `rels` flag.
#[test]
fn show_rels_flag_parses() {
    for arg in ["--rels", "--relations"] {
        let cli = Cli::try_parse_from(["aida", "show", "TASK-1", arg]).unwrap();
        match cli.command {
            Command::Show { rels, .. } => assert!(rels, "{arg} should set rels"),
            other => panic!("unexpected command: {other:?}"),
        }
    }
    // default: off
    let cli = Cli::try_parse_from(["aida", "show", "TASK-1"]).unwrap();
    match cli.command {
        Command::Show { rels, .. } => assert!(!rels),
        other => panic!("unexpected command: {other:?}"),
    }
}

/// STORY-457: the untracked safe-to-remove classifier flags editor scratch,
/// build droppings, and OS cruft — never source/docs.
#[test]
fn untracked_safe_to_remove_classifier() {
    use super::untracked_is_safe_to_remove;
    for p in [
        "foo.bak",
        "a/b/c.tmp",
        ".x.swp",
        "main.rs.orig",
        "mod.pyc",
        ".DS_Store",
        "pkg/__pycache__/m.pyc",
        "editor~",
    ] {
        assert!(untracked_is_safe_to_remove(p), "should flag: {p}");
    }
    for p in [
        "src/main.rs",
        "docs/plans/2026-06-05-x.md",
        "scripts/deploy.sh",
        "README.md",
        "Cargo.toml",
    ] {
        assert!(!untracked_is_safe_to_remove(p), "should NOT flag: {p}");
    }
}

/// TASK-666 (STORY-457): the untracked-history reconcile core upserts new
/// paths with first-observed=now and prunes paths no longer untracked,
/// preserving the original timestamp of paths that persist.
#[test]
fn untracked_history_upsert_and_prune() {
    use super::reconcile_untracked_map;
    use std::collections::HashMap;
    let now = chrono::Utc::now();
    let older = (now - chrono::Duration::days(2)).to_rfc3339();
    let mut stored: HashMap<String, String> = HashMap::new();
    stored.insert("kept.md".to_string(), older.clone());
    stored.insert("gone.md".to_string(), older.clone());

    let current = vec!["kept.md".to_string(), "fresh.md".to_string()];
    let merged = reconcile_untracked_map(stored, &current, now);

    // Pruned: no longer untracked.
    assert!(!merged.contains_key("gone.md"));
    // Kept: original (older) timestamp preserved, not reset to now.
    assert_eq!(
        merged.get("kept.md").map(String::as_str),
        Some(older.as_str())
    );
    // New: first-observed = now.
    assert_eq!(
        merged.get("fresh.md").map(String::as_str),
        Some(now.to_rfc3339().as_str())
    );
    assert_eq!(merged.len(), 2);
}

/// TASK-666 (STORY-457): age classification buckets first-observed times into
/// recent (<1h) / mid / stale (≥1d); a missing first-seen reads as recent.
#[test]
fn untracked_age_classification() {
    use super::{classify_untracked_age, UntrackedAge};
    let now = chrono::Utc::now();
    assert_eq!(classify_untracked_age(None, now), UntrackedAge::Recent);
    assert_eq!(
        classify_untracked_age(Some(now - chrono::Duration::minutes(30)), now),
        UntrackedAge::Recent
    );
    assert_eq!(
        classify_untracked_age(Some(now - chrono::Duration::hours(5)), now),
        UntrackedAge::Mid
    );
    assert_eq!(
        classify_untracked_age(Some(now - chrono::Duration::days(3)), now),
        UntrackedAge::Stale
    );
    // Exactly 1h is no longer "recent"; exactly 1d is "stale".
    assert_eq!(
        classify_untracked_age(Some(now - chrono::Duration::hours(1)), now),
        UntrackedAge::Mid
    );
    assert_eq!(
        classify_untracked_age(Some(now - chrono::Duration::days(1)), now),
        UntrackedAge::Stale
    );
}

/// TASK-666 (STORY-457): last-status timestamp round-trips through
/// `.aida/last-status.toml`, and is absent (None) before the first write.
#[test]
fn last_status_at_round_trip() {
    use super::{read_last_status_at, write_last_status_at};
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida")).unwrap();

    // No file yet → None.
    assert!(read_last_status_at(root).is_none());

    let now = chrono::Utc::now();
    write_last_status_at(root, now);
    let read = read_last_status_at(root).expect("recorded timestamp");
    // RFC3339 round-trip is second-or-finer exact; compare at second precision.
    assert_eq!(read.timestamp(), now.timestamp());
}

/// TASK-666 (STORY-457): `write_last_status_at` is a no-op when `.aida/` is
/// absent (uninitialized project) — never creates state or panics.
#[test]
fn last_status_at_noop_without_aida_dir() {
    use super::{read_last_status_at, write_last_status_at};
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    write_last_status_at(root, chrono::Utc::now());
    assert!(!root.join(".aida").exists());
    assert!(read_last_status_at(root).is_none());
}

/// TASK-662: the first run (no prior snapshot) yields no delta — there's
/// no baseline to diff against, so consumers see `null`, not an all-new
/// burst.
#[test]
fn findings_delta_first_run_is_none() {
    use super::compute_findings_delta;
    let current = vec!["TASK-1".to_string(), "TASK-2".to_string()];
    assert!(compute_findings_delta(None, &current).is_none());
}

/// TASK-662: a prior run with zero findings is distinct from "no prior
/// run" — `Some(empty)` produces a real delta where everything current is
/// new.
#[test]
fn findings_delta_empty_baseline_counts_all_as_new() {
    use super::compute_findings_delta;
    let prev: Vec<String> = vec![];
    let current = vec!["TASK-3".to_string(), "TASK-1".to_string()];
    let d = compute_findings_delta(Some(&prev), &current).expect("delta");
    assert_eq!(d.previous_total, 0);
    assert_eq!(d.current_total, 2);
    assert_eq!(d.new_count, 2);
    // new_ids is sorted regardless of input order.
    assert_eq!(d.new_ids, vec!["TASK-1".to_string(), "TASK-3".to_string()]);
    assert_eq!(d.resolved_count, 0);
}

/// TASK-662: a steady state (same set both runs) reports no new and no
/// resolved findings — the common quiet case.
#[test]
fn findings_delta_unchanged_is_quiet() {
    use super::compute_findings_delta;
    let prev = vec!["TASK-1".to_string(), "TASK-2".to_string()];
    let current = vec!["TASK-2".to_string(), "TASK-1".to_string()];
    let d = compute_findings_delta(Some(&prev), &current).expect("delta");
    assert_eq!(d.new_count, 0);
    assert!(d.new_ids.is_empty());
    assert_eq!(d.resolved_count, 0);
    assert_eq!(d.previous_total, 2);
    assert_eq!(d.current_total, 2);
}

/// TASK-662: a mixed run — one finding filed, one triaged away — reports
/// both the new and the resolved counts independently.
#[test]
fn findings_delta_mixed_new_and_resolved() {
    use super::compute_findings_delta;
    let prev = vec!["TASK-1".to_string(), "TASK-2".to_string()];
    let current = vec!["TASK-2".to_string(), "TASK-9".to_string()];
    let d = compute_findings_delta(Some(&prev), &current).expect("delta");
    assert_eq!(d.new_count, 1);
    assert_eq!(d.new_ids, vec!["TASK-9".to_string()]);
    assert_eq!(d.resolved_count, 1); // TASK-1 gone
    assert_eq!(d.previous_total, 2);
    assert_eq!(d.current_total, 2);
}

/// TASK-662: duplicate IDs in either snapshot can't fabricate a delta —
/// the diff is set-based, so a doubled entry collapses.
#[test]
fn findings_delta_is_dedup_safe() {
    use super::compute_findings_delta;
    let prev = vec!["TASK-1".to_string(), "TASK-1".to_string()];
    let current = vec![
        "TASK-1".to_string(),
        "TASK-1".to_string(),
        "TASK-2".to_string(),
    ];
    let d = compute_findings_delta(Some(&prev), &current).expect("delta");
    assert_eq!(d.previous_total, 1);
    assert_eq!(d.current_total, 2);
    assert_eq!(d.new_count, 1);
    assert_eq!(d.new_ids, vec!["TASK-2".to_string()]);
    assert_eq!(d.resolved_count, 0);
}

/// TASK-662: the findings snapshot round-trips through
/// `.aida/last-findings.toml`, sorts/dedups on write, and is a no-op
/// (None) when `.aida/` is absent.
#[test]
fn last_findings_round_trip_and_noop() {
    use super::{read_last_findings, write_last_findings};
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();

    // No file yet → None (no prior run).
    assert!(read_last_findings(root).is_none());

    // No `.aida/` dir → write is a no-op, still None.
    write_last_findings(root, &["TASK-2".to_string()]);
    assert!(read_last_findings(root).is_none());

    std::fs::create_dir_all(root.join(".aida")).unwrap();
    write_last_findings(
        root,
        &[
            "TASK-2".to_string(),
            "TASK-1".to_string(),
            "TASK-2".to_string(),
        ],
    );
    let read = read_last_findings(root).expect("recorded snapshot");
    // Sorted + deduped on write.
    assert_eq!(read, vec!["TASK-1".to_string(), "TASK-2".to_string()]);
}

/// STORY-446: `--blocked-by` (repeatable) + the `--depends-on` alias parse
/// onto `Add`, and `--blocked-by` / `--remove-blocked-by` onto `Edit`.
#[test]
fn blocked_by_flags_parse_on_add_and_edit() {
    let add = Cli::try_parse_from([
        "aida",
        "add",
        "--title",
        "t",
        "--type",
        "task",
        "--blocked-by",
        "TASK-1",
        "--depends-on",
        "TASK-2",
    ])
    .unwrap();
    match add.command {
        Command::Add { blocked_by, .. } => {
            assert_eq!(blocked_by, vec!["TASK-1".to_string(), "TASK-2".to_string()]);
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let edit = Cli::try_parse_from([
        "aida",
        "edit",
        "TASK-9",
        "--blocked-by",
        "TASK-1",
        "--remove-blocked-by",
        "TASK-3",
    ])
    .unwrap();
    match edit.command {
        Command::Edit {
            blocked_by,
            remove_blocked_by,
            ..
        } => {
            assert_eq!(blocked_by, vec!["TASK-1".to_string()]);
            assert_eq!(remove_blocked_by, vec!["TASK-3".to_string()]);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn brief_cli_parses_canonical_create_shape() {
    let cli =
        Cli::try_parse_from(["aida", "brief", "codex", "TASK-492", "--note", "ship it"]).unwrap();
    match cli.command {
        Command::Brief {
            agent,
            spec,
            note,
            depends_on,
            as_deep_link,
            notify,
            authorized_by,
            cmd,
        } => {
            assert_eq!(agent.as_deref(), Some("codex"));
            assert_eq!(spec.as_deref(), Some("TASK-492"));
            assert_eq!(note.as_deref(), Some("ship it"));
            assert_eq!(depends_on, None);
            assert!(!as_deep_link);
            assert!(!notify);
            assert_eq!(authorized_by, None);
            assert!(cmd.is_none());
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let cli = Cli::try_parse_from([
        "aida",
        "brief",
        "codex",
        "TASK-493",
        "--depends-on",
        "TASK-492",
    ])
    .unwrap();
    match cli.command {
        Command::Brief { depends_on, .. } => {
            assert_eq!(depends_on.as_deref(), Some("TASK-492"));
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

/// STORY-711 slice 2: `--authorized-by` parses onto the `Brief` command.
// trace:TASK-1140 | ai:claude
#[test]
fn brief_cli_parses_authorized_by_flag() {
    let cli = Cli::try_parse_from([
        "aida",
        "brief",
        "codex",
        "TASK-492",
        "--authorized-by",
        "advisor-a",
    ])
    .unwrap();
    match cli.command {
        Command::Brief { authorized_by, .. } => {
            assert_eq!(authorized_by.as_deref(), Some("advisor-a"));
        }
        other => panic!("unexpected command: {other:?}"),
    }

    // Omitted → None (the overwhelmingly common case).
    let cli = Cli::try_parse_from(["aida", "brief", "codex", "TASK-492"]).unwrap();
    match cli.command {
        Command::Brief { authorized_by, .. } => {
            assert_eq!(authorized_by, None);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn brief_cli_parses_list_and_ack_subcommands() {
    let list = Cli::try_parse_from([
        "aida",
        "brief",
        "list",
        "--for-agent",
        "codex",
        "--include-acked",
    ])
    .unwrap();
    match list.command {
        Command::Brief {
            cmd:
                Some(BriefCommand::List {
                    for_agent,
                    include_acked,
                }),
            ..
        } => {
            assert_eq!(for_agent.as_deref(), Some("codex"));
            assert!(include_acked);
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let ack = Cli::try_parse_from(["aida", "brief", "ack", "brief.md"]).unwrap();
    match ack.command {
        Command::Brief {
            cmd: Some(BriefCommand::Ack { brief_file }),
            ..
        } => assert_eq!(brief_file, std::path::PathBuf::from("brief.md")),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn create_brief_writes_frontmatter_sections_and_relationships() {
    let temp = tempfile::tempdir().unwrap();
    let store = store_with_related();
    let path = create_agent_brief(
        temp.path(),
        &store,
        "codex",
        "TASK-492",
        Some("why this, why now"),
        None,
        None,
    )
    .unwrap();

    let relative = path.strip_prefix(temp.path()).unwrap();
    let components = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        &components[..3],
        &[
            ".aida".to_string(),
            "agent-briefs".to_string(),
            "codex".to_string()
        ]
    );
    let file_name = relative.file_name().unwrap().to_string_lossy();
    assert!(file_name.starts_with("TASK-492-"));
    assert!(file_name.ends_with("Z.md"));
    assert!(!file_name.contains(':'));

    let body = std::fs::read_to_string(path).unwrap();
    assert!(body.contains("spec_id: TASK-492"));
    assert!(body.contains("agent: codex"));
    assert!(body.contains("status: pending"));
    assert!(body.contains("## Routing"));
    assert!(body.contains("## Optional preamble"));
    assert!(body.contains("why this, why now"));
    assert!(body.contains("## Spec"));
    assert!(body.contains("## Acceptance\n- embed this faithfully"));
    assert!(body.contains("## Composes with"));
    assert!(body.contains("parent: STORY-425"));
    assert!(body.contains("child: TASK-493"));
    assert!(body.contains("## Discipline"));
    assert!(body.contains("docs/agents/codex-mcp-setup.md"));
    assert!(body.contains("## Setup"));
    assert!(body.contains("aida session start --owns TASK-492"));
    assert!(body.contains("## Trailer reminder"));
    assert!(body.contains("(TASK-492)"));
}

/// STORY-711 slice 2: `--authorized-by` round-trips end to end —
/// `create_agent_brief` writes the frontmatter field, `collect_agent_briefs_inner`
/// (which backs both `aida brief list` and `launch_authorized_by`) reads it
/// back, and a brief with NO `--authorized-by` carries no field at all
/// (the overwhelmingly common case must stay silent, not `authorized_by: ""`).
// trace:TASK-1140 | ai:claude
#[test]
fn brief_authorized_by_round_trips_through_frontmatter_and_listing() {
    let temp = tempfile::tempdir().unwrap();
    let store = store_with_related();
    let path = create_agent_brief(
        temp.path(),
        &store,
        "codex",
        "TASK-492",
        None,
        None,
        Some("advisor-a"),
    )
    .unwrap();
    let body = std::fs::read_to_string(&path).unwrap();
    assert!(body.contains("authorized_by: advisor-a"), "got: {body}");

    let entries = collect_agent_briefs(temp.path(), Some("codex"), false).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].authorized_by.as_deref(), Some("advisor-a"));

    // launch_authorized_by (the fn render_agent_launch_context calls) finds
    // it by agent + spec.
    assert_eq!(
        launch_authorized_by(temp.path(), "codex", "TASK-492").as_deref(),
        Some("advisor-a")
    );
    // A different spec/agent has no authorization.
    assert_eq!(launch_authorized_by(temp.path(), "codex", "TASK-999"), None);
    assert_eq!(
        launch_authorized_by(temp.path(), "antigravity", "TASK-492"),
        None
    );
}

/// A brief created with NO `--authorized-by` carries no `authorized_by:`
/// frontmatter line at all — the default path stays byte-identical to
/// pre-slice-2 briefs.
// trace:TASK-1140 | ai:claude
#[test]
fn brief_without_authorized_by_carries_no_frontmatter_field() {
    let temp = tempfile::tempdir().unwrap();
    let store = store_with_related();
    let path =
        create_agent_brief(temp.path(), &store, "codex", "TASK-492", None, None, None).unwrap();
    let body = std::fs::read_to_string(&path).unwrap();
    assert!(!body.contains("authorized_by"), "got: {body}");
    assert_eq!(launch_authorized_by(temp.path(), "codex", "TASK-492"), None);
}

// BUG-583: the Setup block must reference the TARGET PROJECT's location
// (the invocation's project root, here the throwaway temp dir), never the
// AIDA binary's compiled-in source-repo path. A cold vendor agent following
// the Setup steps literally must stay inside its own project. This test
// fails against the old code (which hardcoded `/home/joe/ai/aida`) and
// passes with the runtime-resolved path. trace:BUG-583
#[test]
fn setup_block_references_target_project_not_binary_repo() {
    let temp = tempfile::tempdir().unwrap();
    let store = store_with_related();
    let path =
        create_agent_brief(temp.path(), &store, "codex", "TASK-492", None, None, None).unwrap();
    let body = std::fs::read_to_string(&path).unwrap();

    // Isolate the Setup fenced block so later sections can't mask a leak.
    let setup_idx = body.find("## Setup").expect("brief has a Setup section");
    let setup_block = &body[setup_idx..];

    // The Setup block must point at the project root we briefed for.
    let project_root = temp.path().display().to_string();
    assert!(
        setup_block.contains(&format!("cd {project_root}")),
        "Setup block must `cd` into the target project root, got:\n{setup_block}"
    );
    assert!(
        setup_block.contains("--path "),
        "Setup block must pass a runtime-resolved worktree --path, got:\n{setup_block}"
    );

    // The Setup block must NEVER emit the aida source-repo path or any
    // CARGO_MANIFEST_DIR-style compiled-in path.
    assert!(
        !setup_block.contains("/home/joe/ai/aida"),
        "Setup block leaks the aida binary's source-repo path:\n{setup_block}"
    );
    assert!(
        !setup_block.contains(env!("CARGO_MANIFEST_DIR")),
        "Setup block leaks the compiled-in CARGO_MANIFEST_DIR path:\n{setup_block}"
    );
}

#[test]
fn list_excludes_acked_by_default_and_ack_renames_file() {
    let temp = tempfile::tempdir().unwrap();
    let store = store_with_related();
    let path =
        create_agent_brief(temp.path(), &store, "codex", "TASK-492", None, None, None).unwrap();

    let pending = collect_agent_briefs(temp.path(), Some("codex"), false).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].spec_id, "TASK-492");
    assert!(!pending[0].acked);

    ack_agent_brief(&path).unwrap();
    let pending = collect_agent_briefs(temp.path(), Some("codex"), false).unwrap();
    assert!(pending.is_empty());
    let all = collect_agent_briefs(temp.path(), Some("codex"), true).unwrap();
    assert_eq!(all.len(), 1);
    assert!(all[0].acked);
    let body = std::fs::read_to_string(&all[0].path).unwrap();
    assert!(body.contains("status: acked"));
}

// BUG-569: an advisor `aida edit --status` / `aida comment add` fires the
// pending-brief banner, which scans briefs by the running agent's bare TYPE
// ("antigravity"). When 2+ live antigravity agents are registered, the
// type-class matches both and `resolve_brief_directories` used to emit a
// spurious "agent target 'antigravity' is ambiguous" warning on the banner
// path. The internal (non-targeted) scan must stay silent — the banner must
// render without any ambiguity line. trace:BUG-569 | ai:claude
#[test]
fn banner_scan_does_not_warn_when_agent_type_is_ambiguous() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // Two live agents of the same TYPE (antigravity), distinct names/pids —
    // the exact condition that makes the bare type-class resolve
    // ambiguously. Registry entries are keyed by `<type>#<pid>`, so two
    // distinct LIVE pids are required (same pid would collapse to one
    // entry). Use this process's pid plus a spawned child held alive for
    // the duration of the test.
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn a second live process for the second agent pid");
    let pid_a = std::process::id();
    let pid_b = child.id();
    agent_registry::register_existing_agent(
        root,
        "antigravity",
        pid_a,
        "advisor".to_string(),
        None,
        root.to_path_buf(),
        Some("antigravity-advisor-1".to_string()),
    )
    .unwrap();
    agent_registry::register_existing_agent(
        root,
        "antigravity",
        pid_b,
        "implementer".to_string(),
        None,
        root.to_path_buf(),
        Some("antigravity-implementer-2".to_string()),
    )
    .unwrap();

    // The explicit-target path (warn_on_ambiguity = true) must STILL warn —
    // ambiguity is a real user error when someone typed `--for-agent`.
    let (_dirs, warn_explicit) =
        resolve_brief_dirs_with_optional_warning(root, "antigravity", true);
    assert!(
        warn_explicit
            .as_deref()
            .is_some_and(|w| w.contains("is ambiguous")),
        "explicit type-class targeting should still warn: {warn_explicit:?}"
    );

    // The internal/banner path (warn_on_ambiguity = false) must be SILENT —
    // this is the spurious-warning suppression that BUG-569 fixes.
    let (_dirs, warn_internal) =
        resolve_brief_dirs_with_optional_warning(root, "antigravity", false);
    assert!(
        warn_internal.is_none(),
        "internal type-class scan must not warn on ambiguity: {warn_internal:?}"
    );

    // A pending brief under the `antigravity` type dir so the banner has
    // something to render.
    let brief_dir = root.join(".aida").join("agent-briefs").join("antigravity");
    std::fs::create_dir_all(&brief_dir).unwrap();
    std::fs::write(
        brief_dir.join("TASK-999-20260101T000000Z.md"),
        "---\nspec_id: TASK-999\nagent: antigravity\nstatus: pending\n---\nbody\n",
    )
    .unwrap();

    // The banner path (the bug's surface) must render the brief WITHOUT the
    // spurious ambiguity line.
    let lines = pending_brief_banner_lines(root, "antigravity")
        .expect("banner should render for a pending brief");
    assert!(
        lines.iter().any(|l| l.contains("NEW BRIEF(S) PENDING")),
        "banner should announce the pending brief"
    );
    assert!(
        !lines.iter().any(|l| l.contains("is ambiguous")),
        "banner must not leak the ambiguous-target warning: {lines:?}"
    );

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn test_brief_read_roundtrip() {
    let temp = tempfile::tempdir().unwrap();
    let store = store_with_related();
    let path = create_agent_brief(
        temp.path(),
        &store,
        "codex",
        "TASK-492",
        Some("Pre-note"),
        None,
        None,
    )
    .unwrap();

    // 1. Read directly via path
    let res = read_agent_brief(temp.path(), &path.to_string_lossy(), false);
    assert!(res.is_ok());

    // 2. Read using shortcut
    let filename = path.file_name().unwrap().to_str().unwrap();
    let shortcut = format!("codex/{}", filename);
    let res2 = read_agent_brief(temp.path(), &shortcut, false);
    assert!(res2.is_ok());

    // 3. Read using --latest
    let res3 = read_agent_brief(temp.path(), "codex", true);
    assert!(res3.is_ok());

    // 4. Test error handling when not found
    let res4 = read_agent_brief(temp.path(), "nonexistent", false);
    assert!(res4.is_err());
    let err_msg = res4.unwrap_err().to_string();
    assert!(err_msg.contains("aida brief list"));
}

// trace:TASK-541 | ai:codex
#[test]
fn brief_depends_on_persists_orders_and_blocks_until_prereq_acked() {
    let temp = tempfile::tempdir().unwrap();
    let store = store_with_related();

    let dependent = create_agent_brief(
        temp.path(),
        &store,
        "codex",
        "TASK-493",
        None,
        Some("TASK-492"),
        None,
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let prereq =
        create_agent_brief(temp.path(), &store, "codex", "TASK-492", None, None, None).unwrap();

    let entries = collect_agent_briefs(temp.path(), Some("codex"), false).unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.spec_id.as_str())
            .collect::<Vec<_>>(),
        vec!["TASK-492", "TASK-493"],
        "dependency order should override chronological order"
    );
    assert_eq!(entries[1].depends_on.as_deref(), Some("TASK-492"));

    let body = std::fs::read_to_string(&dependent).unwrap();
    assert!(body.contains("depends_on: TASK-492"));
    let rendered = render_agent_brief_read(temp.path(), &dependent, &body).unwrap();
    assert!(rendered.starts_with("Blocked by: TASK-492\n\n"));

    ack_agent_brief(&prereq).unwrap();
    let rendered = render_agent_brief_read(temp.path(), &dependent, &body).unwrap();
    assert!(!rendered.starts_with("Blocked by: TASK-492"));
}

// trace:TASK-541 | ai:codex
#[test]
fn brief_depends_on_validates_target_and_rejects_cycles() {
    let temp = tempfile::tempdir().unwrap();
    let store = store_with_related();

    let err = create_agent_brief(
        temp.path(),
        &store,
        "codex",
        "TASK-493",
        None,
        Some("TASK-404"),
        None,
    )
    .expect_err("missing dependency target should fail")
    .to_string();
    assert!(err.contains("TASK-404"), "{err}");

    create_agent_brief(
        temp.path(),
        &store,
        "codex",
        "TASK-493",
        None,
        Some("TASK-492"),
        None,
    )
    .unwrap();
    let cycle = create_agent_brief(
        temp.path(),
        &store,
        "codex",
        "TASK-492",
        None,
        Some("TASK-493"),
        None,
    )
    .expect_err("reverse dependency should create a cycle")
    .to_string();
    assert!(cycle.contains("dependency cycle"), "{cycle}");
}

#[test]
fn invalid_agent_names_are_rejected() {
    assert!(validate_brief_agent("").is_err());
    assert!(validate_brief_agent("../codex").is_err());
    assert!(validate_brief_agent("codex").is_ok());
}

// BUG-378: substrate-as-bouncer banner that catches the scratchpad-drift
// failure mode. The pure-function core returns the banner lines (or
// None when the gate stays silent) so the test can assert on output
// shape without capturing stderr.
// trace:BUG-378 | ai:claude

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1B && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            i += 2;
            while i < bytes.len() && !bytes[i].is_ascii_alphabetic() {
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

#[test]
fn pending_brief_banner_silent_for_other_agent_type() {
    let temp = tempfile::tempdir().unwrap();
    let store = store_with_related();
    create_agent_brief(temp.path(), &store, "codex", "TASK-492", None, None, None).unwrap();
    // Running shell / unknown caller — banner must NOT fire even though
    // a pending brief exists. Avoids noising up every human queue-done.
    assert!(pending_brief_banner_lines(temp.path(), "other").is_none());
    assert!(pending_brief_banner_lines(temp.path(), "").is_none());
}

#[test]
fn pending_brief_banner_silent_when_no_briefs_for_running_type() {
    let temp = tempfile::tempdir().unwrap();
    let store = store_with_related();
    create_agent_brief(
        temp.path(),
        &store,
        "antigravity",
        "TASK-492",
        None,
        None,
        None,
    )
    .unwrap();
    // A Codex session must not see Antigravity briefs — cross-type
    // false positives teach agents to ignore the banner.
    assert!(pending_brief_banner_lines(temp.path(), "codex").is_none());
}

#[test]
fn pending_brief_banner_silent_when_no_briefs_at_all() {
    let temp = tempfile::tempdir().unwrap();
    // No .aida/agent-briefs/ directory at all — common happy-path case.
    assert!(pending_brief_banner_lines(temp.path(), "codex").is_none());
}

#[test]
fn pending_brief_banner_lists_all_pending_briefs_for_running_type() {
    let temp = tempfile::tempdir().unwrap();
    let store = store_with_related();
    let first =
        create_agent_brief(temp.path(), &store, "codex", "TASK-492", None, None, None).unwrap();
    // Second brief — banner must enumerate ALL of them, not just one.
    // (Per the BUG-378 master verdict: a missed brief implies the
    // multi-missed case is plausible too.)
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let second = create_agent_brief(
        temp.path(),
        &store,
        "codex",
        "TASK-492",
        Some("note"),
        None,
        None,
    )
    .unwrap();

    let lines = pending_brief_banner_lines(temp.path(), "codex")
        .expect("banner should fire for codex with pending briefs");
    let rendered: String = lines
        .iter()
        .map(|l| strip_ansi(l))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        rendered.contains("NEW BRIEF(S) PENDING for agent `codex`"),
        "banner header missing: {rendered}"
    );
    assert!(
        rendered.contains(&first.display().to_string()),
        "first brief path not listed: {rendered}"
    );
    assert!(
        rendered.contains(&second.display().to_string()),
        "second brief path not listed: {rendered}"
    );
    assert!(
        rendered.contains("aida brief list --for-agent codex"),
        "remediation command missing: {rendered}"
    );
    assert!(
        rendered.contains("scratchpad is NOT ground truth"),
        "discipline reminder missing: {rendered}"
    );
}

#[test]
fn pending_brief_banner_skips_acked_briefs() {
    let temp = tempfile::tempdir().unwrap();
    let store = store_with_related();
    let path =
        create_agent_brief(temp.path(), &store, "codex", "TASK-492", None, None, None).unwrap();
    ack_agent_brief(&path).unwrap();
    // An acked brief is no longer pending — banner must stay silent.
    assert!(pending_brief_banner_lines(temp.path(), "codex").is_none());
}
