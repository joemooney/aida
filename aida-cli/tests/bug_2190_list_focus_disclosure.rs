#![cfg(target_os = "linux")]
//! Real-CLI coverage for open status filtering and the hidden persistent focus
//! that made every open status appear empty in agent output.
// trace:BUG-2190 | ai:codex

#[path = "support/test_git.rs"]
mod test_git;

use std::path::PathBuf;
use std::process::{Command, Output};

mod support;

const OPEN_STATUSES: [&str; 5] = [
    "draft",
    "approved",
    "planned",
    "in-progress",
    "needs-attention",
];

struct Fixture {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
    open: Vec<String>,
    closed: String,
}

impl Fixture {
    fn aida(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
        cmd.current_dir(&self.repo)
            .args(args)
            .env("HOME", &self.home)
            .env("AIDA_HOME", &self.home)
            .env("AIDA_TELEMETRY", "0")
            .env("AIDA_SESSION_ROLE", "advisor")
            .env("NO_COLOR", "1")
            .env_remove("AIDA_FOCUS")
            .env_remove("AIDA_AGENT_OUTPUT")
            .env_remove("AIDA_OUTPUT_FORMAT")
            .env_remove("AIDA_SESSION_PROJECT")
            .env_remove("AIDA_SESSION_GRANT");
        if let Some(grant) = support::ensure_seat(&self.home, &self.repo, "advisor", &[]) {
            cmd.env("AIDA_SESSION_GRANT", grant);
        }
        let out = cmd.output().expect("run fixture aida");
        assert!(
            out.status.success(),
            "{args:?}: stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn add(&self, status: &str) -> String {
        // NeedsAttention is a lifecycle transition, not an initial status.
        let initial = if status == "needs-attention" {
            "in-progress"
        } else {
            status
        };
        let out = self.aida(&[
            "add", "--type", "task", "--title", status, "--status", initial,
        ]);
        let id = String::from_utf8_lossy(&out.stdout)
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
            .find(|word| word.starts_with("TASK-") && word[5..].chars().all(|c| c.is_ascii_digit()))
            .expect("created task id")
            .to_string();
        if status == "needs-attention" {
            self.aida(&[
                "punt",
                &id,
                "--category",
                "other",
                "--reason",
                "list fixture",
            ]);
        }
        id
    }

    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let home = temp.path().join("home");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.name", "List fixture"],
            vec!["config", "user.email", "list@example.test"],
            vec!["commit", "-q", "--allow-empty", "-m", "init"],
        ] {
            assert!(Command::new("git")
                .current_dir(&repo)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        // trace:BUG-2017 | ai:codex — explicit positive HOME fixture.
        test_git::enroll_synthetic_home(&repo);
        let mut fixture = Self {
            _temp: temp,
            repo,
            home,
            open: Vec::new(),
            closed: String::new(),
        };
        fixture.aida(&[
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ]);
        fixture.open = OPEN_STATUSES.iter().map(|s| fixture.add(s)).collect();
        fixture.closed = fixture.add("completed");
        fixture
    }
}

fn ids(out: &Output) -> Vec<String> {
    let rows: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON array stdout");
    rows.as_array()
        .unwrap()
        .iter()
        .map(|row| row["spec_id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn open_statuses_remain_visible_and_persistent_focus_is_never_silent() {
    let fixture = Fixture::new();
    assert_eq!(
        ids(&fixture.aida(&["list", "--format", "json"])),
        fixture.open
    );
    let toon = fixture.aida(&["list", "--format", "toon"]);
    let stdout = String::from_utf8_lossy(&toon.stdout);
    for id in &fixture.open {
        assert!(stdout.contains(id), "bare list omitted {id}: {stdout}");
    }
    for (status, id) in OPEN_STATUSES.iter().zip(&fixture.open) {
        for spelling in [
            status.to_string(),
            status.to_uppercase(),
            status.replace('-', "_"),
        ] {
            assert_eq!(
                ids(&fixture.aida(&["list", "--status", &spelling, "--format", "json"])),
                vec![id.clone()],
                "status spelling {spelling}"
            );
        }
    }

    // Reproduce the primary-checkout failure: a persisted focus on a closed
    // leaf has no open matches, while open work exists elsewhere.
    fixture.aida(&["focus", &fixture.closed]);
    for status in std::iter::once(None).chain(OPEN_STATUSES.iter().map(|s| Some(*s))) {
        // Every renderer gets the empty-focus case; status filtering is
        // orthogonal and needs one structured assertion per status.
        let formats: &[&str] = if status.is_none() {
            &["toon", "json", "human", "short"]
        } else {
            &["json"]
        };
        for &format in formats {
            let mut args = vec!["list"];
            if let Some(status) = status {
                args.extend(["--status", status]);
            }
            if format == "short" {
                args.push("--short");
            } else {
                args.extend(["--format", format]);
            }
            let focused = fixture.aida(&args);
            let stdout = String::from_utf8_lossy(&focused.stdout);
            let stderr = String::from_utf8_lossy(&focused.stderr);
            let disclosure = if matches!(format, "json" | "short") {
                &stderr
            } else {
                &stdout
            };
            assert!(disclosure.contains(&format!("focused: {}", fixture.closed)));
            assert!(disclosure.contains("--no-focus"));
            let hidden = if status.is_none() {
                fixture.open.len()
            } else {
                1
            };
            // init seeds open META specs, excluded by the ordinary work lens.
            // They must not inflate the number focus hides from this listing.
            assert!(
                disclosure.contains(&format!("{hidden} matching specs hidden by focus")),
                "{disclosure}"
            );
            assert!(disclosure.contains("aida focus clear"));
            if format == "json" {
                assert!(ids(&focused).is_empty());
            } else if format == "short" {
                // The default TOON envelope can append cache metadata after
                // --short returns. It is existing output, not a focus note or
                // a spec ID; the focused ID set must still be empty.
                assert!(!stdout.contains("focused:"), "{stdout}");
                assert!(
                    stdout.lines().all(|line| !line.starts_with("TASK-")),
                    "focused short output unexpectedly contained IDs: {stdout}"
                );
            } else if format == "toon" {
                assert!(stdout.contains("count: 0 of 0"));
            }

            args.push("--no-focus");
            let widened = fixture.aida(&args);
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&widened.stdout),
                String::from_utf8_lossy(&widened.stderr)
            );
            assert!(!combined.contains("focused:"));
            let expected = status.map(|s| {
                &fixture.open[OPEN_STATUSES
                    .iter()
                    .position(|candidate| *candidate == s)
                    .unwrap()]
            });
            for id in expected.into_iter().chain(if status.is_none() {
                fixture.open.iter().collect::<Vec<_>>()
            } else {
                Vec::new()
            }) {
                assert!(
                    combined.contains(id),
                    "widened {args:?} omitted {id}: {combined}"
                );
            }
        }
    }
    let all = ids(&fixture.aida(&["list", "--all", "--format", "json"]));
    for id in fixture.open.iter().chain(std::iter::once(&fixture.closed)) {
        assert!(all.contains(id));
    }
}
