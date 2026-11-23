// trace:TASK-3228 | ai:codex
#[path = "support/test_git.rs"]
mod test_git;

use std::{fs, process::Command};

#[test]
fn journal_failure_preserves_independent_landing_metrics() {
    let repo = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let root = repo.path();
    for args in [
        vec!["init", "-qb", "main"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--allow-empty",
            "-m",
            "fixture (#1)",
        ],
    ] {
        assert!(test_git::git_command()
            .current_dir(root)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    fs::create_dir_all(root.join(".aida/land")).unwrap();
    fs::create_dir(root.join(".aida-store")).unwrap();
    let until = chrono::Utc::now().to_rfc3339();
    let report = |format: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_aida"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", home.path())
            .current_dir(root)
            .args([
                "land", "metrics", "--since", "1h", "--until", &until, "--format", format,
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let json = || serde_json::from_str::<serde_json::Value>(&report("json")).unwrap();
    let journal = root.join(".aida/land/main-watch.jsonl");
    let baseline = json();
    assert_eq!(baseline["red_main"]["episodes"], 0);
    fs::write(&journal, b"unfinished").unwrap();
    assert_eq!(json()["red_main"], baseline["red_main"]);

    fs::write(&journal, b"invalid\n").unwrap();
    let broken = json();
    for key in [
        "hours",
        "ready_waits",
        "review_rounds",
        "review_rounds_unavailable_prs",
    ] {
        assert!(baseline[key].is_array());
        assert_eq!(broken[key], baseline[key]);
    }
    assert_eq!(broken["red_main"]["available"], false);
    let diagnostic = broken["red_main"]["diagnostic"].as_str().unwrap();
    assert!((1..=256).contains(&diagnostic.len()));
    let human = report("human");
    for label in [
        "merges (main git history); READY:",
        "Review rounds (verdict history):",
        "Red main (watch journal): unavailable",
        diagnostic,
    ] {
        assert!(human.contains(label), "{human}");
    }
    fs::remove_file(&journal).unwrap();
    fs::create_dir(&journal).unwrap();
    assert_eq!(json()["red_main"], broken["red_main"]);
}
