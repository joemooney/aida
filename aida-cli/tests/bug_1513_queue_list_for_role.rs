// BUG-1513: `aida queue list --for <role>` merged the CALLER's own queue
// entries into the role-filtered view regardless of their `for_role`, while
// the declared `count:` header was computed from that same unfiltered set.
// A reviewer asking `--for reviewer` therefore saw their own unrelated
// implementer-routed row folded into both the printed rows and the count,
// so the header disagreed with the filter it claimed to apply — and the
// only caller positioned to notice (the row's owner) had every reason to
// read it as correct, since it was a spec they already recognized.
//
// This drives the real binary in TOON/agent mode (stdout piped, not a TTY,
// which is the `agent_output_mode()` default — see BUG-1513's own repro,
// which surfaced via `queue[N]{id,title,status,for_role}` TOON output) and
// asserts every row's `for_role` matches the requested role, and that the
// declared count equals the row count. Binary-driving e2e, same family as
// bug_1520_list_fields_json.rs. trace:BUG-1513 | ai:claude
#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::{Command, Output};

fn aida(repo: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(repo)
        .env("HOME", home)
        .env("AIDA_TELEMETRY", "0")
        .env_remove("AIDA_SESSION_ROLE")
        .env_remove("AIDA_AGENT_OUTPUT")
        .env_remove("AIDA_OUTPUT_FORMAT")
        .args(args)
        .output()
        .expect("run aida")
}

fn git(repo: &Path, args: &[&str]) {
    assert!(Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git")
        .success());
}

fn add_spec(repo: &Path, home: &Path, title: &str) -> String {
    let out = aida(
        repo,
        home,
        &[
            "add", "--title", title, "--type", "task", "--status", "approved",
        ],
    );
    assert!(
        out.status.success(),
        "add: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The success message names the new spec id, e.g. "Added TASK-1: ...".
    stdout
        .split_whitespace()
        .find(|w| w.contains('-') && w.chars().next().is_some_and(|c| c.is_ascii_uppercase()))
        .expect("spec id in add output")
        .trim_end_matches(':')
        .to_string()
}

/// Criterion 1/2/4: `--for reviewer` returns ONLY reviewer-routed rows, and
/// the declared `count:` equals the row count — even when the caller's own
/// queue also holds a same-user entry routed to a different role.
// trace:BUG-1513 | ai:claude
#[test]
fn queue_list_for_role_excludes_callers_own_mismatched_role_row() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path().canonicalize().expect("canonical tempdir");
    let repo = base.join("repo");
    let home = base.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "BUG-1513 Test"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let init = aida(
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
    assert!(
        init.status.success(),
        "init: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let reviewer_spec = add_spec(&repo, &home, "BUG-1513 reviewer-routed fixture");
    let implementer_spec = add_spec(&repo, &home, "BUG-1513 implementer-routed fixture");

    // Both entries land in the SAME user's queue file — the exact shape the
    // spec's repro measured (BUG-1420 sat in `joe`'s own queue while `joe`
    // asked `--for reviewer`).
    for (spec, role) in [
        (reviewer_spec.as_str(), "reviewer"),
        (implementer_spec.as_str(), "implementer"),
    ] {
        // Routing `--for implementer` is an execution-dispatch route and
        // needs dispatch authority — wear `advisor` for the add so both
        // fixture rows can be seeded regardless of role.
        let out = Command::new(env!("CARGO_BIN_EXE_aida"))
            .current_dir(&repo)
            .env("HOME", &home)
            .env("AIDA_TELEMETRY", "0")
            .env("AIDA_SESSION_ROLE", "advisor")
            .env_remove("AIDA_AGENT_OUTPUT")
            .env_remove("AIDA_OUTPUT_FORMAT")
            .args(["queue", "add", spec, "--for", role, "--user", "bug1513user"])
            .output()
            .expect("run aida");
        assert!(
            out.status.success(),
            "queue add {spec} --for {role}: stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // stdout is piped here (not a TTY), so this is the agent/TOON path that
    // printed the unfiltered `count:` before the fix.
    let out = aida(
        &repo,
        &home,
        &[
            "queue",
            "list",
            "--for",
            "reviewer",
            "--user",
            "bug1513user",
        ],
    );
    assert!(
        out.status.success(),
        "queue list --for reviewer: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);

    // Criterion 4: the implementer-routed row (the caller's own, mismatched
    // role) must not appear at all.
    assert!(
        !stdout.contains(&implementer_spec),
        "the caller's own implementer-routed row leaked into a `--for reviewer` view:\n{stdout}"
    );
    assert!(
        stdout.contains(&reviewer_spec),
        "the reviewer-routed row is missing from its own `--for reviewer` view:\n{stdout}"
    );

    // Criterion 2: the declared `count:` must equal the number of queue rows
    // actually printed (one TOON table row per queued spec here).
    let declared: usize = stdout
        .lines()
        .find_map(|l| l.strip_prefix("count: "))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no `count: N` header in output:\n{stdout}"));
    // Count actual TOON table rows (each is `  <id>,<title>,<status>,<role>`),
    // not every mention of the id — the id also appears in the trailing
    // `next[]{cmd,to}` hint block.
    let printed_rows = stdout
        .lines()
        .filter(|l| l.trim_start().starts_with(&format!("{reviewer_spec},")))
        .count();
    assert_eq!(
        declared, printed_rows,
        "declared count disagrees with the rows the filter admitted:\n{stdout}"
    );
    assert_eq!(
        declared, 1,
        "expected exactly the one reviewer row:\n{stdout}"
    );
}
