//! STORY-737: three newcomer-facing delight fixes, verified end-to-end through
//! the real `aida` binary on the HUMAN TTY path (forced via `AIDA_AGENT_OUTPUT=0`):
//!
//!   #2 — after `aida add`, the HUMAN gets the same next-step nudge agents get
//!        (a `Next:` footer + a `// trace:<id>` breadcrumb), and agent mode is
//!        NOT double-rendered (it keeps the TOON `next` block only).
//!   #4 — `aida history` / `aida list` hide the 6 seeded META prompt-template
//!        rows by default; `--include-meta` and `--type meta` reveal them.
//!   #5 — `aida queue work` on an empty queue renders a SOFT, forward-pointing
//!        signpost (no red `Error:`), even though it still exits non-zero.
// trace:STORY-737

use std::path::Path;
use std::process::Command;

fn aida(repo: &Path, home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(repo);
    cmd.env("HOME", home);
    cmd.env("AIDA_TELEMETRY", "0");
    cmd.env_remove("AIDA_SESSION_ROLE");
    cmd.env_remove("AIDA_PERMISSION_MODE");
    // Force the HUMAN path: in a test pipe stdout is not a TTY, which would
    // otherwise select agent mode. Individual calls override when they want the
    // agent surface.
    cmd.env("AIDA_AGENT_OUTPUT", "0");
    cmd
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

/// A SPEC-ID is `UPPER-<digits>` (e.g. `TASK-1`).
fn is_spec_id(t: &str) -> bool {
    let mut parts = t.splitn(2, '-');
    match (parts.next(), parts.next()) {
        (Some(prefix), Some(num)) => {
            !prefix.is_empty()
                && prefix.chars().all(|c| c.is_ascii_uppercase())
                && !num.is_empty()
                && num.chars().all(|c| c.is_ascii_digit())
        }
        _ => false,
    }
}

#[test]
fn newcomer_delight_human_nudge_meta_hidden_soft_empty_queue() {
    let base = tempfile::tempdir().expect("tempdir");
    // BUG-671: on macOS the tempdir resolves under /var/folders/… which is a
    // symlink to /private/var/folders/…; canonicalize once so every path the
    // test passes matches the path `aida` records internally, on every OS.
    let base_dir = base.path().canonicalize().expect("canonicalize tempdir");
    let repo = base_dir.join("repo");
    let home = base_dir.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();

    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@t.t"]);
    git(&repo, &["config", "user.name", "t"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);

    let init = aida(&repo, &home)
        .args([
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ])
        .output()
        .expect("run aida init");
    assert!(
        init.status.success(),
        "aida init failed (exit {:?}):\n--- stdout ---\n{}\n--- stderr ---\n{}",
        init.status.code(),
        String::from_utf8_lossy(&init.stdout),
        String::from_utf8_lossy(&init.stderr)
    );

    // ---- FINDING #5 (fresh project): empty `queue work` is a soft signpost. ----
    let fresh = aida(&repo, &home)
        .args(["queue", "work", "--no-pull"])
        .output()
        .expect("run aida queue work");
    assert!(
        !fresh.status.success(),
        "an empty queue still exits non-zero so scripts gate"
    );
    let fresh_err = String::from_utf8_lossy(&fresh.stderr);
    assert!(
        !fresh_err.contains("Error:"),
        "day-one empty queue must NOT render a red `Error:`:\n{fresh_err}"
    );
    assert!(
        fresh_err.contains("fresh project") || fresh_err.contains("File a spec"),
        "expected a forward-pointing fresh-project signpost:\n{fresh_err}"
    );

    // ---- FINDING #2 (human add path): Next: footer + trace breadcrumb. ----
    let add = aida(&repo, &home)
        .env("AIDA_SESSION_ROLE", "advisor")
        .args([
            "add",
            "--type",
            "task",
            "--status",
            "approved",
            "--title",
            "Build login page",
        ])
        .output()
        .expect("run aida add");
    assert!(
        add.status.success(),
        "aida add failed: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    let add_out = String::from_utf8_lossy(&add.stdout);
    let spec = add_out
        .split(|c: char| c.is_whitespace() || c == ',')
        .find(|t| is_spec_id(t))
        .unwrap_or_else(|| panic!("could not parse spec id from add output:\n{add_out}"))
        .to_string();
    assert!(
        add_out.contains("Next:"),
        "human add path must render the Next: footer:\n{add_out}"
    );
    assert!(
        add_out.contains(&format!("// trace:{spec}")),
        "human add path must render the trace breadcrumb:\n{add_out}"
    );
    assert!(
        add_out.contains("Link your code to it"),
        "human add path must teach the trace step:\n{add_out}"
    );

    // ---- FINDING #2 (agent mode): TOON next block only, no double-render. ----
    let add_agent = aida(&repo, &home)
        .env("AIDA_SESSION_ROLE", "advisor")
        .env("AIDA_AGENT_OUTPUT", "1")
        .args([
            "add",
            "--type",
            "task",
            "--status",
            "approved",
            "--title",
            "Agent-mode spec",
        ])
        .output()
        .expect("run aida add (agent)");
    assert!(add_agent.status.success());
    let agent_out = String::from_utf8_lossy(&add_agent.stdout);
    assert!(
        agent_out.contains("next") && agent_out.contains("cmd"),
        "agent mode should emit the TOON `next` block:\n{agent_out}"
    );
    assert!(
        !agent_out.contains("Link your code to it"),
        "agent mode must NOT also render the human breadcrumb (no double-render):\n{agent_out}"
    );

    // ---- FINDING #4: history/list hide META by default; flags reveal it. ----
    let hist = aida(&repo, &home)
        .args(["history"])
        .output()
        .expect("run aida history");
    let hist_out = String::from_utf8_lossy(&hist.stdout);
    assert!(
        !hist_out.contains("META-"),
        "default history must hide seeded META rows:\n{hist_out}"
    );
    assert!(
        hist_out.contains(&spec),
        "default history must still show the real spec:\n{hist_out}"
    );

    let hist_meta = aida(&repo, &home)
        .args(["history", "--include-meta"])
        .output()
        .expect("run aida history --include-meta");
    let hist_meta_out = String::from_utf8_lossy(&hist_meta.stdout);
    assert!(
        hist_meta_out.contains("META-"),
        "--include-meta must reveal META rows:\n{hist_meta_out}"
    );

    let hist_type = aida(&repo, &home)
        .args(["history", "--type", "meta"])
        .output()
        .expect("run aida history --type meta");
    let hist_type_out = String::from_utf8_lossy(&hist_type.stdout);
    assert!(
        hist_type_out.contains("META-"),
        "--type meta must reveal META rows:\n{hist_type_out}"
    );

    // `aida list` parity: META hidden by default, shown via `--type meta`.
    let list = aida(&repo, &home)
        .args(["list"])
        .output()
        .expect("run aida list");
    let list_out = String::from_utf8_lossy(&list.stdout);
    assert!(
        !list_out.contains("META-"),
        "default list must hide META rows:\n{list_out}"
    );
    let list_meta = aida(&repo, &home)
        .args(["list", "--type", "meta"])
        .output()
        .expect("run aida list --type meta");
    let list_meta_out = String::from_utf8_lossy(&list_meta.stdout);
    assert!(
        list_meta_out.contains("META-"),
        "--type meta must reveal META rows on list:\n{list_meta_out}"
    );

    // ---- FINDING #5 (real specs exist, none queued): soft signpost again. ----
    // The approved spec above was never queued, so a no-arg head pickup finds an
    // empty queue. This is the non-fresh branch ("Nothing queued yet").
    let empty = aida(&repo, &home)
        .args(["queue", "work", "--no-pull"])
        .output()
        .expect("run aida queue work (non-fresh)");
    assert!(!empty.status.success(), "empty queue still exits non-zero");
    let empty_err = String::from_utf8_lossy(&empty.stderr);
    assert!(
        !empty_err.contains("Error:"),
        "an expected empty queue must NOT render a red `Error:`:\n{empty_err}"
    );
    assert!(
        empty_err.contains("Nothing queued yet"),
        "expected the forward-pointing empty-queue signpost:\n{empty_err}"
    );
}
