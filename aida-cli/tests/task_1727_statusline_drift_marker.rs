#![cfg(target_os = "linux")]
//! Black-box coverage of the assembled status line: `aida statusline` run in a
//! drifted primary checkout must print the drift marker (exactly `⌂!` plain,
//! exactly bold red when colour is forced), and a clean checkout must not.
//! Pins the wiring in the statusline handler, not just the segment helper.
// trace:TASK-1727 | ai:claude

use std::path::Path;
use std::process::{Command, Output};

const PLAIN: &str = "\u{2302}!";
const BOLD_RED: &str = "\u{1b}[1;31m\u{2302}!\u{1b}[0m";

fn git(root: &Path, home: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["-c", "user.email=t@example.com", "-c", "user.name=t"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn statusline(dir: &Path, home: &Path, color: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(dir)
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("AIDA_TELEMETRY", "0")
        .env_remove("AIDA_SESSION_ROLE")
        .env_remove("AIDA_STORE")
        .env_remove("REQ_DB_NAME")
        .env_remove("AIDA_REGISTRY_PATH")
        .env_remove("REQ_REGISTRY_PATH")
        .env_remove("NO_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .args(["statusline", "--color", color])
        .output()
        .expect("run aida statusline")
}

fn stdout_of(out: &Output) -> String {
    assert!(
        out.status.success(),
        "aida statusline failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn statusline_shows_drift_marker_only_for_a_drifted_checkout() {
    assert_eq!(PLAIN, "⌂!");
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let repo = tmp.path().join("primary");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &home, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("tracked.txt"), "one\n").unwrap();
    git(&repo, &home, &["add", "tracked.txt"]);
    git(&repo, &home, &["commit", "-q", "-m", "init"]);

    // Clean checkout on main: no marker in either colour mode.
    let clean = stdout_of(&statusline(&repo, &home, "never"));
    assert!(
        !clean.contains(PLAIN),
        "clean checkout must not show the drift marker: {clean:?}"
    );
    let clean_color = stdout_of(&statusline(&repo, &home, "always"));
    assert!(
        !clean_color.contains(PLAIN),
        "clean checkout must not show the drift marker: {clean_color:?}"
    );

    // Drifted (untracked file): plain marker exactly, with no ANSI around it.
    std::fs::write(repo.join("scratch.py"), "print(1)\n").unwrap();
    let plain = stdout_of(&statusline(&repo, &home, "never"));
    assert!(
        plain.contains(PLAIN),
        "drifted checkout must show '⌂!' in the status line: {plain:?}"
    );
    assert!(
        !plain.contains('\u{1b}'),
        "--color never must emit no ANSI: {plain:?}"
    );

    // Drifted with colour forced: exactly the bold-red form.
    let colored = stdout_of(&statusline(&repo, &home, "always"));
    assert!(
        colored.contains(BOLD_RED),
        "drifted checkout must show bold red '⌂!' with colour forced: {colored:?}"
    );
}
