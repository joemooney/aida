#![cfg(target_os = "linux")]
//! Black-box coverage for the `aida history` default change: with no
//! SPEC-ID and no mode flag it lists status transitions (one line per
//! transition, newest first) served from the history index; the old
//! one-row-per-spec digest moves to `--digest`, byte-identical to what the
//! bare command printed before; and when the index cannot answer, the
//! command falls back to the digest with a one-line note on stderr.
//!
//! The fixture store is hand-built with pinned commit dates and pinned
//! `modified_at` values, and every run pins `TZ=UTC`, so the digest's
//! clock column is deterministic.
//!
//! The digest goldens under `tests/fixtures/story_1650/` were captured from
//! the binary built just before this change (see `capture_digest_goldens`).
// trace:STORY-1650 | ai:claude

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn store(&self) -> PathBuf {
        self.repo.join(".aida-store")
    }
}

fn git(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run git")
}

fn git_ok(dir: &Path, args: &[&str]) {
    let out = git(dir, args);
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn aida_with(bin: &Path, fixture: &Fixture, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(bin);
    cmd.current_dir(&fixture.repo)
        .env("HOME", &fixture.home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_ROLE", "advisor")
        .env("NO_COLOR", "1")
        .env("TZ", "UTC")
        .env_remove("AIDA_HISTORY_CACHE")
        .env_remove("AIDA_HISTORY_INDEX_BUDGET_MS")
        .args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    if let Some(grant) = support::ensure_seat(&fixture.home, &fixture.repo, "advisor", &[]) {
        cmd.env("AIDA_SESSION_GRANT", grant);
    }
    cmd.output().expect("run aida")
}

fn aida_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_aida"))
}

fn aida(fixture: &Fixture, args: &[&str], envs: &[(&str, &str)]) -> Output {
    aida_with(&aida_bin(), fixture, args, envs)
}

fn ok(label: &str, out: &Output) -> String {
    assert!(
        out.status.success(),
        "{label} failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

const STORY_UUID: &str = "11111111-1111-4111-8111-111111111111";
const TASK_UUID: &str = "22222222-2222-4222-8222-222222222222";
const BUG_UUID: &str = "33333333-3333-4333-8333-333333333333";
const TASK4_UUID: &str = "44444444-4444-4444-8444-444444444444";

#[allow(clippy::too_many_arguments)]
fn write_obj(
    store: &Path,
    dir: &str,
    spec_id: &str,
    uuid: &str,
    req_type: &str,
    title: &str,
    status: &str,
    created: &str,
    modified: &str,
) {
    let path = store.join(format!("objects/{dir}/000/{spec_id}.yaml"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        format!(
            "id: {uuid}\nspec_id: {spec_id}\nagreed_id: {spec_id}\ntitle: {title}\n\
             description: ''\nstatus: {status}\npriority: Medium\nowner: ''\n\
             feature: Uncategorized\ncreated_at: {created}\nmodified_at: {modified}\n\
             req_type: {req_type}\n"
        ),
    )
    .unwrap();
}

fn commit_at(store: &Path, msg: &str, date: &str) {
    git_ok(store, &["add", "-A"]);
    let out = Command::new("git")
        .current_dir(store)
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .args(["commit", "-q", "-m", msg])
        .output()
        .expect("git commit");
    assert!(
        out.status.success(),
        "commit {msg}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `aida init`, then five pinned-date store commits:
/// 1. STORY-1, TASK-2, BUG-3 filed at Draft.
/// 2. STORY-1 Draft -> Approved.
/// 3. STORY-1 Approved -> InProgress; TASK-2 retitled (not a transition).
/// 4. STORY-1 InProgress -> Completed; BUG-3 deleted.
/// 5. TASK-4 filed at Approved.
fn init_fixture() -> Fixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path().canonicalize().expect("canonical tempdir");
    let repo = base.join("repo");
    let home = base.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git_ok(&repo, &["init", "-q", "-b", "main"]);
    git_ok(&repo, &["config", "user.email", "test@example.com"]);
    git_ok(&repo, &["config", "user.name", "Flow Test"]);
    git_ok(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let fixture = Fixture {
        _tmp: tmp,
        repo,
        home,
    };
    let out = aida(
        &fixture,
        &[
            "init",
            "--force",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ],
        // The store's first commits must predate the backdated fixture
        // commits; a child older than its parent is clock skew, which the
        // index rightly refuses to vouch for.
        &[
            ("GIT_AUTHOR_DATE", "2025-02-28T09:00:00Z"),
            ("GIT_COMMITTER_DATE", "2025-02-28T09:00:00Z"),
        ],
    );
    ok("aida init", &out);

    let store = fixture.store();
    git_ok(&store, &["config", "user.email", "flow@example.com"]);
    git_ok(&store, &["config", "user.name", "Flow Test"]);
    git_ok(&store, &["config", "commit.gpgsign", "false"]);
    let t1 = "2025-03-01T10:00:00Z";
    write_obj(
        &store,
        "STORY",
        "STORY-1",
        STORY_UUID,
        "Story",
        "Flowing story",
        "Draft",
        t1,
        t1,
    );
    write_obj(
        &store,
        "TASK",
        "TASK-2",
        TASK_UUID,
        "Task",
        "Plain task",
        "Draft",
        t1,
        t1,
    );
    write_obj(
        &store,
        "BUG",
        "BUG-3",
        BUG_UUID,
        "Bug",
        "Short-lived bug",
        "Draft",
        t1,
        t1,
    );
    commit_at(&store, "add STORY-1 TASK-2 BUG-3", t1);

    let t2 = "2025-03-02T11:00:00Z";
    write_obj(
        &store,
        "STORY",
        "STORY-1",
        STORY_UUID,
        "Story",
        "Flowing story",
        "Approved",
        t1,
        t2,
    );
    commit_at(&store, "update STORY-1", t2);

    let t3 = "2025-03-03T12:00:00Z";
    write_obj(
        &store,
        "STORY",
        "STORY-1",
        STORY_UUID,
        "Story",
        "Flowing story",
        "InProgress",
        t1,
        t3,
    );
    write_obj(
        &store,
        "TASK",
        "TASK-2",
        TASK_UUID,
        "Task",
        "Renamed task",
        "Draft",
        t1,
        t3,
    );
    commit_at(&store, "update STORY-1 TASK-2", t3);

    let t4 = "2025-03-04T13:00:00Z";
    write_obj(
        &store,
        "STORY",
        "STORY-1",
        STORY_UUID,
        "Story",
        "Flowing story",
        "Completed",
        t1,
        t4,
    );
    std::fs::remove_file(store.join("objects/BUG/000/BUG-3.yaml")).unwrap();
    commit_at(&store, "update STORY-1, delete BUG-3", t4);

    let t5 = "2025-03-05T14:00:00Z";
    write_obj(
        &store,
        "TASK",
        "TASK-4",
        TASK4_UUID,
        "Task",
        "Filed approved",
        "Approved",
        t5,
        t5,
    );
    commit_at(&store, "add TASK-4", t5);
    fixture
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/story_1650")
}

fn golden(name: &str) -> String {
    let path = golden_dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Records the digest goldens from a binary built BEFORE this change, where
/// bare `aida history` was the digest. Run once, by hand:
///
/// ```text
/// STORY_1650_GOLDEN_BIN=/path/to/pre-change/aida \
///   cargo test -p aida-cli --test story_1650_history_status_flow \
///   capture_digest_goldens -- --ignored
/// ```
#[test]
#[ignore = "writes goldens; run by hand against the pre-change binary"]
fn capture_digest_goldens() {
    let bin =
        PathBuf::from(std::env::var("STORY_1650_GOLDEN_BIN").expect("set STORY_1650_GOLDEN_BIN"));
    let fx = init_fixture();
    std::fs::create_dir_all(golden_dir()).unwrap();
    for (format, file) in [("human", "digest_human.txt"), ("toon", "digest_toon.txt")] {
        let out = aida_with(&bin, &fx, &["history", "--format", format], &[]);
        let stdout = ok("pre-change aida history", &out);
        std::fs::write(golden_dir().join(file), stdout).unwrap();
    }
}

#[test]
fn digest_flag_is_byte_identical_to_the_old_default_human_and_toon() {
    let fx = init_fixture();
    for (format, file) in [("human", "digest_human.txt"), ("toon", "digest_toon.txt")] {
        let want = golden(file);
        // Index on (the default) and switched off: the digest never reads it.
        for envs in [&[][..], &[("AIDA_HISTORY_CACHE", "0")][..]] {
            let out = aida(&fx, &["history", "--digest", "--format", format], envs);
            let got = ok("aida history --digest", &out);
            assert_eq!(
                got, want,
                "--digest --format {format} drifted (env {envs:?})"
            );
            assert!(
                !stderr(&out).contains("note:"),
                "--digest is asked for, so no fallback note: {}",
                stderr(&out)
            );
        }
    }
}

#[test]
fn digest_flag_json_projects_the_digest_rows() {
    let fx = init_fixture();
    let out = aida(&fx, &["history", "--digest", "--format", "json"], &[]);
    let stdout = ok("aida history --digest --format json", &out);
    let doc: serde_json::Value = serde_json::from_str(&stdout).expect("pure JSON on stdout");
    assert_eq!(doc["view"], "history");
    let rows = doc["history"].as_array().expect("history rows");
    assert_eq!(doc["count"], rows.len());
    let ids: Vec<&str> = rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["TASK-4", "STORY-1", "BUG-3", "TASK-2"], "{doc}");
    let story = &rows[1];
    assert_eq!(story["status"], "Completed");
    assert_eq!(story["change"], "add");
    assert_eq!(story["title"], "Flowing story");
    assert_eq!(story["type"], "Story");
    assert_eq!(rows[2]["change"], "del");
}

fn flow_lines(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with("WHEN"))
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect()
}

#[test]
fn default_human_lists_every_transition_newest_first() {
    let fx = init_fixture();
    let out = aida(&fx, &["history", "--format", "human"], &[]);
    let stdout = ok("aida history", &out);
    assert!(
        !stderr(&out).contains("note:"),
        "a current index serves the default: {}",
        stderr(&out)
    );
    assert_eq!(
        flow_lines(&stdout),
        [
            "2025-03-05 14:00 TASK-4 (new) → Approved Filed approved",
            "2025-03-04 13:00 STORY-1 InProgress → Completed Flowing story",
            "2025-03-03 12:00 STORY-1 Approved → InProgress Flowing story",
            "2025-03-02 11:00 STORY-1 Draft → Approved Flowing story",
            "2025-03-01 10:00 BUG-3 (new) → Draft Short-lived bug",
            "2025-03-01 10:00 STORY-1 (new) → Draft Flowing story",
            "2025-03-01 10:00 TASK-2 (new) → Draft Renamed task",
        ],
        "{stdout}"
    );
    assert!(stdout.starts_with("WHEN"), "a header row: {stdout}");
}

#[test]
fn default_toon_has_one_row_per_transition() {
    let fx = init_fixture();
    let out = aida(&fx, &["history", "--format", "toon"], &[]);
    let stdout = ok("aida history --format toon", &out);
    assert!(
        stdout.starts_with("view: history-flow\ncount: 7\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("transitions[7]{when,id,from,to,title}:"),
        "{stdout}"
    );
    assert!(
        stdout.contains("\"2025-03-04 13:00\",STORY-1,InProgress,Completed,Flowing story"),
        "{stdout}"
    );
    assert!(
        stdout.contains("\"2025-03-05 14:00\",TASK-4,(new),Approved,Filed approved"),
        "{stdout}"
    );
    assert_eq!(stdout.matches(",STORY-1,").count(), 4, "{stdout}");
    assert!(!stderr(&out).contains("note:"), "{}", stderr(&out));
}

#[test]
fn default_json_has_one_record_per_transition() {
    let fx = init_fixture();
    let out = aida(&fx, &["history", "--format", "json"], &[]);
    let stdout = ok("aida history --format json", &out);
    let doc: serde_json::Value = serde_json::from_str(&stdout).expect("pure JSON on stdout");
    assert_eq!(doc["view"], "history-flow");
    assert_eq!(doc["source"], "history-cache");
    assert_eq!(doc["window_exhausted"], false);
    let rows = doc["transitions"].as_array().expect("transitions");
    assert_eq!(doc["count"], rows.len());
    assert_eq!(rows.len(), 7, "{doc}");
    let story: Vec<(&str, &str)> = rows
        .iter()
        .filter(|r| r["id"] == "STORY-1")
        .map(|r| {
            (
                r["from"].as_str().unwrap_or("(new)"),
                r["to"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        story,
        [
            ("InProgress", "Completed"),
            ("Approved", "InProgress"),
            ("Draft", "Approved"),
            ("(new)", "Draft"),
        ]
    );
    let created = rows.iter().find(|r| r["id"] == "TASK-4").unwrap();
    assert_eq!(created["kind"], "added");
    assert!(created["from"].is_null());
    assert_eq!(created["to"], "Approved");
    assert_eq!(created["title"], "Filed approved");
    // `--json` is the same document as `--format json`.
    let alias = ok(
        "aida history --json",
        &aida(&fx, &["history", "--json"], &[]),
    );
    assert_eq!(alias, stdout);
}

#[test]
fn window_flags_narrow_the_default_like_the_events_feed() {
    let fx = init_fixture();
    let run = |args: &[&str]| {
        let mut argv = vec!["history", "--format", "human"];
        argv.extend_from_slice(args);
        flow_lines(&ok("aida history", &aida(&fx, &argv, &[])))
    };
    // --limit counts transitions, as it counts events in the feed.
    assert_eq!(run(&["--limit", "2"]).len(), 2);
    let since = run(&[
        "--since",
        "2025-03-02T00:00:00Z",
        "--until",
        "2025-03-03T23:00:00Z",
    ]);
    // The resolved-window banner the events feed prints leads, then the
    // transitions inside the window.
    assert_eq!(
        since,
        [
            "Window: 2025-03-02 00:00 +0000 → 2025-03-03 23:00 +0000",
            "2025-03-03 12:00 STORY-1 Approved → InProgress Flowing story",
            "2025-03-02 11:00 STORY-1 Draft → Approved Flowing story",
        ]
    );
    assert_eq!(
        run(&["--to", "completed"]),
        ["2025-03-04 13:00 STORY-1 InProgress → Completed Flowing story"]
    );
    assert_eq!(
        run(&["--from", "draft"]),
        ["2025-03-02 11:00 STORY-1 Draft → Approved Flowing story"]
    );
    let opened = run(&["--opened"]);
    assert_eq!(opened.len(), 4, "{opened:?}");
    assert!(opened.iter().all(|l| l.contains("(new) →")), "{opened:?}");
    assert_eq!(run(&["--author", "nobody-matches"]).len(), 0);
}

#[test]
fn disabled_index_falls_back_to_the_digest_with_a_note() {
    let fx = init_fixture();
    for (format, file) in [("human", "digest_human.txt"), ("toon", "digest_toon.txt")] {
        let out = aida(
            &fx,
            &["history", "--format", format],
            &[("AIDA_HISTORY_CACHE", "0")],
        );
        assert_eq!(
            ok("aida history", &out),
            golden(file),
            "fallback is the digest"
        );
        let err = stderr(&out);
        let notes: Vec<&str> = err.lines().filter(|l| l.starts_with("note:")).collect();
        assert_eq!(notes.len(), 1, "exactly one note line: {err}");
        assert!(notes[0].contains("AIDA_HISTORY_CACHE=0"), "{err}");
        assert!(notes[0].contains("aida cache rebuild --history"), "{err}");
    }
    // JSON stays parseable on stdout; the note is on stderr.
    let out = aida(
        &fx,
        &["history", "--format", "json"],
        &[("AIDA_HISTORY_CACHE", "0")],
    );
    let doc: serde_json::Value =
        serde_json::from_str(&ok("aida history --format json", &out)).expect("pure JSON");
    assert_eq!(doc["view"], "history");
    assert!(stderr(&out).contains("aida cache rebuild --history"));
}

#[test]
fn stale_index_falls_back_to_the_digest_with_a_note() {
    let fx = init_fixture();
    // Build the index at the current tip.
    ok("warm", &aida(&fx, &["history", "--format", "human"], &[]));
    // Pause inline catch-up the way repeated failed attempts do (the
    // stalled state), then advance the store: the index stays behind HEAD.
    let db = fx.repo.join(".aida").join("history-v6-5.db");
    assert!(db.exists(), "index built at {}", db.display());
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO meta(key, value) VALUES('catch_up_failures', '3') \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [],
        )
        .unwrap();
    let store = fx.store();
    let t6 = "2025-03-06T15:00:00Z";
    write_obj(
        &store,
        "TASK",
        "TASK-4",
        TASK4_UUID,
        "Task",
        "Filed approved",
        "Completed",
        t6,
        t6,
    );
    commit_at(&store, "update TASK-4", t6);
    let out = aida(&fx, &["history", "--format", "human"], &[]);
    let stdout = ok("aida history (stale index)", &out);
    assert!(
        stdout.starts_with(" ") || stdout.contains("ID"),
        "digest header: {stdout}"
    );
    assert!(!stdout.contains("→"), "the digest, not the flow: {stdout}");
    let err = stderr(&out);
    let notes: Vec<&str> = err.lines().filter(|l| l.starts_with("note:")).collect();
    assert_eq!(notes.len(), 1, "exactly one note line: {err}");
    assert!(notes[0].contains("behind"), "{err}");
    assert!(notes[0].contains("aida cache rebuild --history"), "{err}");
    // The note's fix works: after a rebuild the default serves the flow.
    ok(
        "aida cache rebuild --history",
        &aida(&fx, &["cache", "rebuild", "--history"], &[]),
    );
    let out = aida(&fx, &["history", "--format", "human"], &[]);
    let lines = flow_lines(&ok("aida history (rebuilt)", &out));
    assert_eq!(
        lines[0],
        "2025-03-06 15:00 TASK-4 Approved → Completed Filed approved"
    );
}

#[test]
fn single_spec_and_events_views_are_unchanged_by_the_new_default() {
    let fx = init_fixture();
    let single = ok(
        "aida history STORY-1",
        &aida(&fx, &["history", "STORY-1", "--format", "toon"], &[]),
    );
    assert!(!single.contains("view: history-flow"), "{single}");
    let events = ok(
        "aida history events",
        &aida(&fx, &["history", "events", "--format", "toon"], &[]),
    );
    assert!(events.starts_with("view: history-events"), "{events}");
}
