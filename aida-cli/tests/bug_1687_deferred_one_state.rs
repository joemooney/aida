// BUG-1687: `aida defer` set the deferred view-flag, recorded the revisit
// trigger and dequeued the spec, but left `status` untouched AND said nothing
// about the deferral on the `show` surfaces — so `aida show BUG-1679` reported
// `status: needs-attention` while `aida show BUG-1679 --json` reported
// `deferred = true`. Two agents read the same store in one session and honestly
// reported two different states. Separately, `aida list --status deferred` —
// the query an orchestrator needs to honour "deferral never counts toward
// draining" — hard-errored as an unknown status filter.
//
// Binary-driving e2e against a throwaway repo and a fake HOME, sharing the
// Linux-only gate the sibling real-CLI suites use (see
// bug_1520_list_fields_json.rs, the same family on different verbs). Nothing
// here touches the operator's store, `~/.aida`, or any timer.
// trace:BUG-1687 | ai:claude
#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::{Command, Output};

fn aida(repo: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(repo)
        .env("HOME", home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_ROLE", "advisor")
        .env_remove("AIDA_AGENT_OUTPUT")
        .env_remove("AIDA_OUTPUT_FORMAT")
        .args(args)
        .output()
        .expect("run aida")
}

/// Run `aida <args>` and return stdout, failing the test on a non-zero exit.
///
/// Captured stdout is not a TTY, so `aida` auto-selects the agent (TOON) format
/// unless the caller pins `--format`; every assertion below pins the surface it
/// means to read rather than relying on that default.
fn ok(repo: &Path, home: &Path, args: &[&str]) -> String {
    let out = aida(repo, home, args);
    assert!(
        out.status.success(),
        "`aida {}` failed (exit {:?})\n--- stdout ---\n{}\n--- stderr ---\n{}",
        args.join(" "),
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn git(repo: &Path, args: &[&str]) {
    assert!(Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git")
        .success());
}

/// A fixture repo with three specs: one parked at `in-progress`, one parked at
/// `needs-attention` (the pair of stored statuses the spec names as
/// "someone should act now"), and one left live as the control.
fn init_repo() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path().canonicalize().expect("canonical tempdir");
    let repo = base.join("repo");
    let home = base.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "BUG-1687 Test"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    ok(
        &repo,
        &home,
        &[
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ],
    );
    for title in TITLES {
        ok(
            &repo,
            &home,
            &[
                "add", "--title", title, "--type", "task", "--status", "approved",
            ],
        );
    }
    (tmp, repo, home)
}

/// The three titles the fixture seeds, in the order `park_two` uses them.
const TITLES: [&str; 3] = ["parked in flight", "parked punted", "still live"];

/// Resolve the spec id the store minted for `title`. Looked up by title rather
/// than by row order so anything else the store happens to hold cannot shift it.
fn spec_id_for(repo: &Path, home: &Path, title: &str) -> String {
    let json = ok(repo, home, &["list", "--all", "--format", "json"]);
    let rows: serde_json::Value = serde_json::from_str(&json).expect("list json");
    rows.as_array()
        .expect("array of rows")
        .iter()
        .find(|r| r["title"] == serde_json::Value::String(title.to_string()))
        .and_then(|r| r["spec_id"].as_str())
        .unwrap_or_else(|| panic!("no spec titled {title:?} in:\n{json}"))
        .to_string()
}

fn show_json(repo: &Path, home: &Path, id: &str) -> serde_json::Value {
    serde_json::from_str(&ok(repo, home, &["show", id, "--json"])).expect("show json")
}

/// The fixture as the incident had it: two specs parked with a trigger while
/// still carrying an act-now lifecycle status, and one untouched control.
/// Returns `(in_progress_id, needs_attention_id, live_id)`.
fn park_two(repo: &Path, home: &Path) -> (String, String, String) {
    let a = spec_id_for(repo, home, TITLES[0]);
    let b = spec_id_for(repo, home, TITLES[1]);
    let live = spec_id_for(repo, home, TITLES[2]);
    assert!(
        a != b && b != live,
        "distinct fixture specs: {a} {b} {live}"
    );

    ok(repo, home, &["edit", &a, "--status", "in-progress"]);
    ok(repo, home, &["edit", &b, "--status", "in-progress"]);
    ok(repo, home, &["edit", &b, "--status", "needs-attention"]);
    ok(
        repo,
        home,
        &["defer", &a, "--until", "the upstream lease clears"],
    );
    ok(
        repo,
        home,
        &["defer", &b, "--until", "Joe releases the hold"],
    );

    // Guard the premise: the stored statuses really are the act-now ones, so a
    // regression that simply rewrote status on defer would fail here rather
    // than quietly making the rest of the suite vacuous.
    assert_eq!(show_json(repo, home, &a)["stored_status"], "In Progress");
    assert_eq!(
        show_json(repo, home, &b)["stored_status"],
        "Needs Attention"
    );
    (a, b, live)
}

/// Criterion 1: after `aida defer <ID> --until <t>`, `aida show <ID>` must not
/// present a status implying someone should act now. Both the human surface and
/// the agent (TOON) surface are checked, because the incident's two readers were
/// on different surfaces.
// trace:BUG-1687 | ai:claude
#[test]
fn show_does_not_present_an_act_now_status_for_a_deferred_spec() {
    let (_tmp, repo, home) = init_repo();
    let (a, b, _live) = park_two(&repo, &home);

    for (id, stored, trigger) in [
        (&a, "In Progress", "the upstream lease clears"),
        (&b, "Needs Attention", "Joe releases the hold"),
    ] {
        // Human surface: the Status line reads Deferred, the stored status is
        // still legible beside it, and the revisit trigger is right there.
        let human = ok(&repo, &home, &["show", id, "--format", "human"]);
        let status_line = human
            .lines()
            .find(|l| l.starts_with("Status:"))
            .unwrap_or_else(|| panic!("no Status line in `aida show {id}`:\n{human}"));
        assert!(
            status_line.contains("Deferred"),
            "`aida show {id}` Status line does not say Deferred: {status_line}"
        );
        assert!(
            !status_line.contains(stored),
            "`aida show {id}` still presents the act-now status: {status_line}"
        );
        assert!(
            human.contains(&format!("Deferred until: {trigger}")),
            "`aida show {id}` omits the revisit trigger:\n{human}"
        );
        assert!(
            human.contains(&format!("Stored status: {stored}")),
            "`aida show {id}` omits the stored status:\n{human}"
        );

        // Agent surface: the same answer as one TOON scalar.
        let toon = ok(&repo, &home, &["show", id, "--format", "toon"]);
        assert!(
            toon.contains("\nstatus: deferred\n") || toon.starts_with("status: deferred\n"),
            "`aida show {id} --format toon` does not report `status: deferred`:\n{toon}"
        );
        assert!(
            toon.contains("stored_status:") && toon.contains("deferred_until:"),
            "`aida show {id} --format toon` drops the stored status / trigger:\n{toon}"
        );
    }
}

/// Criterion 2: `aida list --status deferred` returns EVERY spec with
/// deferred = true — and only those. Before the fix it exited with
/// "Unknown status filter 'deferred'" while `show --json` reported
/// `deferred = true` on the very same specs.
// trace:BUG-1687 | ai:claude
#[test]
fn list_status_deferred_returns_every_deferred_spec() {
    let (_tmp, repo, home) = init_repo();
    let (a, b, live) = park_two(&repo, &home);

    let json = ok(
        &repo,
        &home,
        &["list", "--status", "deferred", "--format", "json"],
    );
    let rows: serde_json::Value = serde_json::from_str(&json).expect("list json");
    let mut got: Vec<&str> = rows
        .as_array()
        .expect("array")
        .iter()
        .map(|r| r["spec_id"].as_str().expect("spec_id"))
        .collect();
    got.sort_unstable();
    let mut want = vec![a.as_str(), b.as_str()];
    want.sort_unstable();
    assert_eq!(got, want, "`aida list --status deferred` rows:\n{json}");
    assert!(
        !json.contains(live.as_str()),
        "the live control spec leaked into the deferred view:\n{json}"
    );

    // The positional spelling is the same axis, and so is the narrowed form:
    // `deferred,in-progress` keeps only the deferred spec whose stored status
    // is In Progress.
    let positional = ok(&repo, &home, &["list", "deferred", "--format", "json"]);
    assert!(
        positional.contains(a.as_str()) && positional.contains(b.as_str()),
        "`aida list deferred` disagrees with `--status deferred`:\n{positional}"
    );
    let narrowed = ok(
        &repo,
        &home,
        &[
            "list",
            "--status",
            "deferred,in-progress",
            "--format",
            "json",
        ],
    );
    assert!(
        narrowed.contains(a.as_str()) && !narrowed.contains(b.as_str()),
        "`--status deferred,in-progress` did not narrow within the deferred set:\n{narrowed}"
    );
}

/// Criterion 3: un-deferring RESTORES the prior status rather than stranding
/// the spec. This holds because `aida defer` never overwrote status in the first
/// place — the fix relabels at the presentation layer instead of moving the
/// lifecycle state, so there is nothing to remember and nothing to lose. This
/// test is the regression guard on that property: it fails the moment anyone
/// "fixes" defer by rewriting status.
// trace:BUG-1687 | ai:claude
#[test]
fn undefer_restores_the_prior_status() {
    let (_tmp, repo, home) = init_repo();
    let (a, b, _live) = park_two(&repo, &home);

    for (id, prior) in [(&a, "In Progress"), (&b, "Needs Attention")] {
        ok(&repo, &home, &["undefer", id]);
        let json = show_json(&repo, &home, id);
        assert_eq!(
            json["status"], prior,
            "undefer did not restore {id} to {prior}: {json}"
        );
        assert_eq!(json["stored_status"], prior);
        assert_eq!(json["deferred"], serde_json::Value::Bool(false));
        assert!(
            json["deferred_until"].is_null(),
            "undefer left a revisit trigger on {id}: {json}"
        );
        // Back in the default open lens, and out of the deferred view.
        let open = ok(&repo, &home, &["list", "--format", "json"]);
        assert!(
            open.contains(id.as_str()),
            "{id} is stranded — undeferred but absent from `aida list`:\n{open}"
        );
        let deferred = ok(
            &repo,
            &home,
            &["list", "--status", "deferred", "--format", "json"],
        );
        assert!(
            !deferred.contains(id.as_str()),
            "{id} still shows in the deferred view after undefer:\n{deferred}"
        );
    }
}

/// Criterion 4: the defer path and the show/list surfaces agree on ONE answer.
/// This is the criterion the incident actually violated: every surface below was
/// individually self-consistent, and they contradicted each other.
// trace:BUG-1687 | ai:claude
#[test]
fn defer_show_and_list_surfaces_agree_on_one_state() {
    let (_tmp, repo, home) = init_repo();
    let (a, _b, _live) = park_two(&repo, &home);

    // 1. The defer path's own success output.
    let defer_out = ok(
        &repo,
        &home,
        &["defer", &a, "--until", "the reviewer signs off"],
    );
    assert!(
        defer_out.contains("Revisit when: the reviewer signs off"),
        "defer output lost the trigger:\n{defer_out}"
    );

    // 2. `aida show --json` — the machine projection. `status` is the presented
    //    state; `stored_status` carries the untouched lifecycle status, so the
    //    relabel is lossless and nobody has to infer which is which.
    let json = show_json(&repo, &home, &a);
    assert_eq!(json["status"], "Deferred", "show --json status: {json}");
    assert_eq!(json["stored_status"], "In Progress");
    assert_eq!(json["deferred"], serde_json::Value::Bool(true));
    assert_eq!(json["deferred_until"], "the reviewer signs off");

    // 3. `aida show` human + TOON.
    assert!(ok(&repo, &home, &["show", &a, "--format", "human"]).contains("Deferred"));
    assert!(ok(&repo, &home, &["show", &a, "--format", "toon"]).contains("status: deferred"));

    // 4. `aida list` — human column, TOON cell and JSON label, all in the
    //    deferred view, must say the same word. `status` stays the stored cache
    //    token for the STORY-1352 machine contract; `status_label` carries the
    //    presented state, which is the channel NeedsAttention rows already use.
    let human = ok(
        &repo,
        &home,
        &["list", "--status", "deferred", "--format", "human"],
    );
    assert!(
        human.contains("Deferred"),
        "`aida list --status deferred` human rows do not say Deferred:\n{human}"
    );
    let toon = ok(
        &repo,
        &home,
        &["list", "--status", "deferred", "--format", "toon"],
    );
    assert!(
        toon.contains("deferred"),
        "`aida list --status deferred --format toon` rows do not say deferred:\n{toon}"
    );
    let rows: serde_json::Value = serde_json::from_str(&ok(
        &repo,
        &home,
        &["list", "--status", "deferred", "--format", "json"],
    ))
    .expect("list json");
    let row = rows
        .as_array()
        .expect("array")
        .iter()
        .find(|r| r["spec_id"] == serde_json::Value::String(a.clone()))
        .unwrap_or_else(|| panic!("{a} missing from the deferred list view: {rows}"));
    assert_eq!(row["status_label"], "Deferred", "list json row: {row}");
    assert_eq!(row["deferred_until"], "the reviewer signs off");
    // Not a status rewrite: the stored token is still the lifecycle status.
    assert_eq!(row["status"], "InProgress", "list json row: {row}");
}
