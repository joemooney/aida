#![cfg(target_os = "linux")]
//! Black-box coverage for markdown rendering in `aida show`'s human output,
//! against the real binary:
//! - at an interactive terminal (a PTY) the description and comment bodies
//!   are rendered: headings and emphasis without their markers, and a table
//!   drawn as an aligned box that fits the terminal width;
//! - `--raw` and `AIDA_MARKDOWN=raw` restore the source exactly;
//! - piped human output, `--format toon` and `--format json` carry the source
//!   byte-for-byte, with or without `--raw`.
// trace:STORY-1652 | ai:claude

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

const DESCRIPTION: &str = "**Both run the same engine.**\n\n## Where they differ\n| | `aida queue work --auto-complete` | `aida drain start` |\n|---|---|---|\n| No selector | The head of your role's queue: one spec | The whole queue (`--drain`): every spec that can run unattended, skipping blocked, human-only or in-flight ones |\n| Sessions | Interactive by default: you can talk to each phase's session | Headless by default (`--no-human=both`) |\n\n- one *item*\n  - nested `item`\n";

const COMMENT: &str = "Comment with **emphasis** and `code`.";

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
    grant: Option<String>,
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?} failed: {out:?}");
}

const CLEARED_ENV: &[&str] = &[
    "AIDA_OUTPUT_FORMAT",
    "AIDA_MARKDOWN",
    "AIDA_STORE",
    "AIDA_PROJECT_ROOT",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "COLUMNS",
];

fn aida_with(fx: &Fixture, envs: &[(&str, &str)], args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(&fx.repo)
        .env("HOME", &fx.home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_ROLE", "advisor")
        .env("NO_COLOR", "1")
        .env("AIDA_AGENT_OUTPUT", "0")
        .args(args);
    for key in CLEARED_ENV {
        cmd.env_remove(key);
    }
    if let Some(grant) = &fx.grant {
        cmd.env("AIDA_SESSION_GRANT", grant);
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().expect("run aida")
}

fn ok_stdout(label: &str, out: &Output) -> String {
    assert!(
        out.status.success(),
        "{label} failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn spec_id_from(stdout: &str) -> String {
    stdout
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|t| {
            t.split_once('-').is_some_and(|(prefix, n)| {
                prefix.len() >= 2
                    && prefix.chars().all(|c| c.is_ascii_uppercase())
                    && !n.is_empty()
                    && n.chars().all(|c| c.is_ascii_digit())
            })
        })
        .unwrap_or_else(|| panic!("missing spec id in {stdout}"))
        .to_string()
}

/// A project holding one spec whose description and comment are markdown.
fn fixture_with_spec() -> (Fixture, String) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path().canonicalize().expect("canonical tempdir");
    let repo = base.join("repo");
    let home = base.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Markdown Test"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let mut fx = Fixture {
        _tmp: tmp,
        repo,
        home,
        grant: None,
    };
    ok_stdout(
        "aida init",
        &aida_with(
            &fx,
            &[],
            &[
                "init",
                "--force",
                "--no-skills",
                "--no-hooks",
                "--no-agent-config",
                "--no-roles",
            ],
        ),
    );
    fx.grant = support::ensure_seat(&fx.home, &fx.repo, "advisor", &[]);
    let added = ok_stdout(
        "aida add",
        &aida_with(
            &fx,
            &[],
            &[
                "add",
                "--title",
                "markdown fixture",
                "--type",
                "task",
                "--status",
                "approved",
                "--description",
                DESCRIPTION,
            ],
        ),
    );
    let id = spec_id_from(&added);
    ok_stdout(
        "aida comment add",
        &aida_with(&fx, &[], &["comment", "add", &id, COMMENT]),
    );
    (fx, id)
}

/// Run `aida` inside a PTY `cols` wide; returns the combined output with
/// `\r\n` normalised to `\n`.
fn aida_at_tty(fx: &Fixture, cols: u16, envs: &[(&str, &str)], args: &[&str]) -> String {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 200,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");
    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_aida"));
    cmd.cwd(&fx.repo);
    cmd.env("HOME", &fx.home);
    cmd.env("AIDA_TELEMETRY", "0");
    cmd.env("AIDA_SESSION_ROLE", "advisor");
    cmd.env("AIDA_AGENT_OUTPUT", "0");
    cmd.env("TERM", "xterm-256color");
    for key in CLEARED_ENV {
        cmd.env_remove(key);
    }
    cmd.env_remove("NO_COLOR");
    if let Some(grant) = &fx.grant {
        cmd.env("AIDA_SESSION_GRANT", grant);
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    for a in args {
        cmd.arg(a);
    }
    let mut child = pair.slave.spawn_command(cmd).expect("spawn in pty");
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().expect("pty reader");
    // Read on a thread so a child that never exits cannot hang the suite. The
    // reader streams chunks back; EIO at child exit ends the read on Linux.
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let deadline = std::time::Instant::now() + PTY_TIMEOUT;
    let mut raw = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match rx.recv_timeout(left) {
            Ok(chunk) => raw.extend_from_slice(&chunk),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                panic!(
                    "aida {args:?} at a tty did not finish within {PTY_TIMEOUT:?}; partial output:\n{}",
                    String::from_utf8_lossy(&raw)
                );
            }
        }
    }
    let status = child.wait().expect("wait");
    let text = String::from_utf8_lossy(&raw).replace("\r\n", "\n");
    assert!(status.success(), "aida {args:?} at a tty failed:\n{text}");
    text
}

const PTY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// True when every line of `expected` appears in `actual`, in order. A PTY
/// merges stdout and stderr, so a stray stderr hint may be interleaved;
/// this still proves the stdout content is unchanged and in place.
fn lines_in_order(expected: &str, actual: &str) -> bool {
    let mut actual_lines = actual.lines();
    expected
        .lines()
        .all(|want| actual_lines.any(|got| got == want))
}

/// The rendered table box, if any: the lines from `┌` to `└`.
fn table_lines(out: &str) -> Vec<&str> {
    out.lines()
        .skip_while(|l| !l.starts_with('┌'))
        .take_while(|l| !l.starts_with('└'))
        .collect()
}

fn display_width(s: &str) -> usize {
    s.chars().count()
}

#[test]
fn piped_toon_and_json_output_carry_the_markdown_source_unchanged() {
    let (fx, id) = fixture_with_spec();

    let human = ok_stdout(
        "show human",
        &aida_with(&fx, &[], &["show", &id, "--format", "human"]),
    );
    assert!(human.contains(DESCRIPTION.trim_end()), "{human}");
    assert!(human.contains(COMMENT), "{human}");
    for variant in [
        aida_with(&fx, &[], &["show", &id, "--format", "human", "--raw"]),
        aida_with(
            &fx,
            &[("AIDA_MARKDOWN", "raw")],
            &["show", &id, "--format", "human"],
        ),
    ] {
        assert_eq!(ok_stdout("show human variant", &variant), human);
    }

    let toon = ok_stdout(
        "show toon",
        &aida_with(&fx, &[], &["show", &id, "--format", "toon"]),
    );
    assert!(!toon.contains('┌'), "{toon}");
    let toon_raw = ok_stdout(
        "show toon --raw",
        &aida_with(&fx, &[], &["show", &id, "--format", "toon", "--raw"]),
    );
    assert_eq!(toon, toon_raw);

    let json = ok_stdout(
        "show json",
        &aida_with(&fx, &[], &["show", &id, "--format", "json"]),
    );
    let doc: serde_json::Value = serde_json::from_str(&json).expect("json");
    assert_eq!(doc["description"].as_str(), Some(DESCRIPTION));
    let json_raw = ok_stdout(
        "show json --raw",
        &aida_with(&fx, &[], &["show", &id, "--format", "json", "--raw"]),
    );
    assert_eq!(json, json_raw);
}

#[test]
fn tty_human_output_renders_markdown_to_the_terminal_width() {
    let (fx, id) = fixture_with_spec();
    for cols in [80u16, 120] {
        let out = aida_at_tty(&fx, cols, &[("NO_COLOR", "1")], &["show", &id]);
        assert!(!out.contains("**"), "emphasis markers left in:\n{out}");
        assert!(!out.contains("## Where"), "heading marker left in:\n{out}");
        assert!(out.contains("Where they differ\n────"), "{out}");
        assert!(out.contains("• one item"), "{out}");
        // Under NO_COLOR inline code keeps its backticks; emphasis markers go.
        assert!(out.contains("Comment with emphasis and `code`."), "{out}");
        assert!(!out.contains('\x1b'), "NO_COLOR must drop styling:\n{out}");
        let table = table_lines(&out);
        assert!(table.len() > 3, "no table drawn at {cols} cols:\n{out}");
        let widths: Vec<usize> = table.iter().map(|l| display_width(l)).collect();
        assert!(
            widths
                .iter()
                .all(|w| *w == widths[0] && *w <= cols as usize),
            "table not aligned within {cols} cols: {widths:?}\n{out}"
        );
    }

    let styled = aida_at_tty(&fx, 100, &[], &["show", &id]);
    assert!(
        styled.contains("\x1b[1m"),
        "bold styling expected:\n{styled}"
    );
}

#[test]
fn raw_at_a_tty_restores_the_unrendered_human_output() {
    let (fx, id) = fixture_with_spec();
    let piped = ok_stdout(
        "show human",
        &aida_with(&fx, &[], &["show", &id, "--format", "human"]),
    );
    let raw_flag = aida_at_tty(&fx, 80, &[("NO_COLOR", "1")], &["show", &id, "--raw"]);
    let raw_env = aida_at_tty(
        &fx,
        80,
        &[("NO_COLOR", "1"), ("AIDA_MARKDOWN", "raw")],
        &["show", &id],
    );
    for (label, out) in [("--raw", &raw_flag), ("AIDA_MARKDOWN=raw", &raw_env)] {
        assert!(
            out.contains(DESCRIPTION.trim_end()),
            "{label}: description source not verbatim:\n{out}"
        );
        assert!(
            out.contains(COMMENT),
            "{label}: comment source not verbatim:\n{out}"
        );
        assert!(!out.contains('┌'), "{label}: rendered table leaked:\n{out}");
        assert!(
            lines_in_order(&piped, out),
            "{label}: output differs from piped human output\n--- piped ---\n{piped}\n--- tty ---\n{out}"
        );
    }
}
