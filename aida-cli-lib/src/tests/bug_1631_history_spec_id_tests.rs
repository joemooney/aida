//! BUG-1631: the multi-spec `aida history --full` feed names the spec on
//! every event block, relationship events show the edge, and the human,
//! TOON and JSON renderings agree on which spec each event belongs to.
//!
//! Every test builds a throwaway git store in its own temp dir.
// trace:BUG-1631 | ai:claude

use crate::history::{
    collect_filtered_events_git, events_json, render_events_human, render_events_toon,
    resolve_edge_targets, Event, HistoryOpts, HistorySource,
};
use std::path::Path;
use std::process::Command;

const STORY_UUID: &str = "11111111-1111-4111-8111-111111111111";
const TASK_UUID: &str = "22222222-2222-4222-8222-222222222222";

fn git(store: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(store)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write_spec(store: &Path, dir: &str, spec_id: &str, uuid: &str, status: &str, rels: &str) {
    let path = store.join(format!("objects/{dir}/000/{spec_id}.yaml"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        format!(
            "id: {uuid}\nspec_id: {spec_id}\ntitle: {spec_id} title\nreq_type: {dir}\nstatus: {status}\npriority: Medium\n{rels}"
        ),
    )
    .unwrap();
}

// Three commits: STORY-1 and TASK-2 added; one commit flipping BOTH
// statuses (so one sha spans two specs); STORY-1 gains a Parent edge to
// TASK-2.
fn multi_spec_store(tmp: &Path) -> std::path::PathBuf {
    let store = tmp.join("store");
    std::fs::create_dir_all(&store).unwrap();
    git(&store, &["init", "-q", "-b", "aida-store"]);
    git(&store, &["config", "user.email", "t@example.com"]);
    git(&store, &["config", "user.name", "t"]);
    git(&store, &["config", "commit.gpgsign", "false"]);
    write_spec(&store, "STORY", "STORY-1", STORY_UUID, "Draft", "");
    write_spec(&store, "TASK", "TASK-2", TASK_UUID, "Draft", "");
    git(&store, &["add", "-A"]);
    git(&store, &["commit", "-q", "-m", "add"]);
    write_spec(&store, "STORY", "STORY-1", STORY_UUID, "Approved", "");
    write_spec(&store, "TASK", "TASK-2", TASK_UUID, "Approved", "");
    git(&store, &["add", "-A"]);
    git(&store, &["commit", "-q", "-m", "approve both"]);
    let rels = format!("relationships:\n- rel_type: Parent\n  target_id: {TASK_UUID}\n");
    write_spec(&store, "STORY", "STORY-1", STORY_UUID, "Approved", &rels);
    git(&store, &["add", "-A"]);
    git(&store, &["commit", "-q", "-m", "link"]);
    store
}

fn opts(id: Option<&str>) -> HistoryOpts {
    HistoryOpts {
        limit: 100,
        max_commits: 100,
        max_commits_explicit: false,
        events_mode: true,
        id_filter: id.map(str::to_string),
        type_filter: None,
        author_filter: None,
        since: None,
        until: None,
        status_changes_only: false,
        shipped_only: false,
        comments_only: false,
        oneline: false,
        archived_specs: Default::default(),
        archived_only_specs: None,
        deferred_specs: Default::default(),
        deferred_only_specs: None,
        exclude_meta: false,
    }
}

fn events(store: &Path, id: Option<&str>) -> Vec<Event> {
    let (mut events, _, _) = collect_filtered_events_git(store, &opts(id)).unwrap();
    resolve_edge_targets(store, &mut events);
    events
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The spec each event line belongs to, read off the block header above it.
fn human_event_specs(human: &str) -> Vec<String> {
    let mut current: Option<String> = None;
    let mut specs = Vec::new();
    for line in human.lines() {
        if let Some(rest) = line.strip_prefix("commit ") {
            let fields: Vec<&str> = rest.split("  ").collect();
            assert!(
                fields.len() == 3 && !fields[1].starts_with('('),
                "multi-spec header must name the spec: {line:?}"
            );
            current = Some(fields[1].to_string());
        } else if line.starts_with("  ") {
            specs.push(current.clone().expect("event line before any header"));
        }
    }
    specs
}

#[test]
fn bug_1631_multi_spec_feed_names_the_spec_on_every_event_in_all_formats() {
    let tmp = tempfile::tempdir().unwrap();
    let store = multi_spec_store(tmp.path());
    let events = events(&store, None);
    let truth: Vec<String> = events.iter().map(|e| e.spec_id.clone()).collect();
    assert!(truth.contains(&"STORY-1".to_string()) && truth.contains(&"TASK-2".to_string()));

    // Human: the shared "approve both" commit gets one block per spec.
    let human = strip_ansi(&render_events_human(&events, true));
    assert_eq!(human_event_specs(&human), truth, "human feed:\n{human}");
    let approve_sha = &events
        .iter()
        .find(|e| matches!(e.kind, crate::history::EventKind::StatusChange { .. }))
        .unwrap()
        .sha[..8];
    assert_eq!(
        human
            .lines()
            .filter(|l| l.starts_with(&format!("commit {approve_sha}")))
            .count(),
        2,
        "a commit touching two specs renders one block per spec:\n{human}"
    );

    // TOON: an `id` column on every row.
    let toon = render_events_toon(&events);
    let header = toon
        .lines()
        .find(|l| l.starts_with("events["))
        .expect("toon table header");
    assert!(
        header.contains("{sha,when,author,id,kind,summary}"),
        "{toon}"
    );
    let toon_specs: Vec<String> = toon
        .lines()
        .skip_while(|l| !l.starts_with("events["))
        .skip(1)
        .map(|row| row.trim().split(',').nth(3).unwrap().to_string())
        .collect();
    assert_eq!(toon_specs, truth, "toon feed:\n{toon}");

    // JSON: `spec_id` on every record.
    let json = events_json(&events, false, &HistorySource::GitWalk { fallback: false });
    let json_specs: Vec<String> = json["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["spec_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(json_specs, truth);
    assert_eq!(json["count"], truth.len());
}

#[test]
fn bug_1631_relationship_event_renders_the_edge() {
    let tmp = tempfile::tempdir().unwrap();
    let store = multi_spec_store(tmp.path());
    let events = events(&store, None);

    let human = strip_ansi(&render_events_human(&events, true));
    assert!(
        human.contains("relationships: +STORY-1 \u{2192} child TASK-2"),
        "edge missing from human feed:\n{human}"
    );
    let toon = render_events_toon(&events);
    assert!(
        toon.contains("relationships: +STORY-1 \u{2192} child TASK-2"),
        "{toon}"
    );
    let json = events_json(&events, false, &HistorySource::GitWalk { fallback: false });
    let rel = json["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["kind"] == "relationships_change")
        .expect("relationship record");
    assert_eq!(rel["spec_id"], "STORY-1");
    let edge = &rel["detail"]["edges"][0];
    assert_eq!(edge["op"], "added");
    assert_eq!(edge["from"], "STORY-1");
    assert_eq!(edge["rel_type"], "Parent");
    assert_eq!(edge["to"], "TASK-2");
    assert_eq!(edge["target_id"], TASK_UUID);
    assert_eq!(rel["detail"]["added"], 1);
}

#[test]
fn bug_1631_single_spec_view_keeps_the_id_implicit() {
    let tmp = tempfile::tempdir().unwrap();
    let store = multi_spec_store(tmp.path());
    let events = events(&store, Some("TASK-2"));
    assert!(!events.is_empty());
    let human = strip_ansi(&render_events_human(&events, false));
    for line in human.lines().filter(|l| l.starts_with("commit ")) {
        let e = &events[0];
        assert!(
            !line.contains("TASK-2"),
            "single-spec header stays uncluttered: {line:?}"
        );
        let fields: Vec<&str> = line["commit ".len()..].split("  ").collect();
        assert_eq!(
            fields.len(),
            2,
            "header is `commit <sha>  (<when>, by <who>)`: {line:?}"
        );
        assert!(fields[1].starts_with('(') && fields[1].contains(&e.author));
    }
    // Same body lines as the multi-spec rendering, just without the ID.
    let multi = strip_ansi(&render_events_human(&events, true));
    let bodies = |s: &str| -> Vec<String> {
        s.lines()
            .filter(|l| l.starts_with("  "))
            .map(str::to_string)
            .collect()
    };
    assert_eq!(bodies(&human), bodies(&multi));
}

#[test]
fn bug_1631_history_json_flag_parses_on_both_sides_of_events() {
    use crate::cli::{Cli, Command};
    use clap::Parser;
    for argv in [
        vec!["aida", "history", "--full", "--json"],
        vec!["aida", "history", "events", "--json"],
    ] {
        let cli = Cli::try_parse_from(&argv).unwrap();
        assert!(
            matches!(cli.command, Command::History { json: true, .. }),
            "{argv:?}"
        );
    }
}
