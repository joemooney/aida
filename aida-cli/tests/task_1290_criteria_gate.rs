#![cfg(target_os = "linux")]

//! Black-box coverage for TASK-1290: `aida queue done` runs the existing
//! `aida criteria` tracer (STORY-1178) and reports untraced acceptance
//! criteria — WARN by default, REFUSE (with a `--force` ledgered override)
//! under `[protocol] enforce = "refuse"`, and silent when every criterion
//! is traced. Also covers the round-1 headless reviewer prompt citing the
//! same untraced list. These deliberately execute the `aida` binary so
//! Clap dispatch + the real command surface are covered, not just the
//! `criteria_gate` helper functions (unit-tested separately).
// trace:TASK-1290 | ai:claude

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git");
    assert!(
        status.success(),
        "git {args:?} failed in {}",
        repo.display()
    );
}

fn aida(repo: &Path, home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(repo)
        .env("HOME", home)
        .env("AIDA_TELEMETRY", "0")
        .env("NO_COLOR", "1")
        .env_remove("AIDA_HEADLESS")
        .env_remove("AIDA_SESSION_ID")
        .env_remove("AIDA_SESSION_ROLE");
    cmd
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn is_spec_id(token: &str) -> bool {
    let Some((prefix, rest)) = token.split_once('-') else {
        return false;
    };
    !prefix.is_empty()
        && prefix.chars().all(|c| c.is_ascii_alphabetic())
        && !rest.is_empty()
        && rest.chars().all(|c| c.is_ascii_digit())
}

struct Project {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
}

impl Project {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().canonicalize().expect("canonical tempdir");
        let repo = root.join("repo");
        let home = root.join("home");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Criteria Gate Test"]);
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
            "aida init failed: {}",
            String::from_utf8_lossy(&init.stderr)
        );
        // Some reviewer-path git plumbing (PR base/head resolution, remote
        // fetches) assumes an `origin` remote exists — give it a real (if
        // empty) one so those calls degrade gracefully instead of hard
        // git-fatal-ing on a missing remote. trace:TASK-1290 | ai:claude
        let origin = root.join("origin.git");
        git(&root, &["init", "-q", "--bare", origin.to_str().unwrap()]);
        git(
            &repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git(&repo, &["push", "-q", "origin", "main", "aida-store"]);
        Self {
            _temp: temp,
            repo,
            home,
        }
    }

    /// File a spec with an explicit description (so the acceptance criteria
    /// are ours to control) and return its spec id.
    fn add_with_description(&self, kind: &str, title: &str, description: &str) -> String {
        let out = aida(&self.repo, &self.home)
            .env("AIDA_SESSION_ROLE", "advisor")
            .args([
                "add",
                "--type",
                kind,
                "--status",
                "approved",
                "--title",
                title,
                "--description",
                description,
            ])
            .output()
            .expect("run aida add");
        assert!(
            out.status.success(),
            "aida add failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        stdout
            .split(|c: char| c.is_whitespace() || c == ',')
            .find(|t| is_spec_id(t))
            .unwrap_or_else(|| panic!("no spec id in {}", text(&out)))
            .to_string()
    }

    /// Append a `[protocol]` block onto the scaffolded `.aida/config.toml`
    /// without clobbering what `aida init` already wrote.
    fn set_protocol_enforce_refuse(&self) {
        let path = self.repo.join(".aida/config.toml");
        let mut body = std::fs::read_to_string(&path).unwrap_or_default();
        body.push_str("\n[protocol]\nenforce = \"refuse\"\n");
        std::fs::write(&path, body).unwrap();
    }
}

const UNTRACED_DESCRIPTION: &str =
    "## Acceptance\n- AC1: the widget renders\n- AC2: the widget saves\n";

#[test]
fn queue_done_warns_and_names_the_untraced_criterion_verbatim() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let p = Project::new();
    let spec = p.add_with_description("task", "gate warn fixture", UNTRACED_DESCRIPTION);

    let done = aida(&p.repo, &p.home)
        .args(["queue", "done", &spec, "--skip-pr-check"])
        .output()
        .expect("run aida queue done");
    let out = text(&done);
    assert!(
        done.status.success(),
        "queue done should warn, not fail, under the default enforce=warn:\n{out}"
    );
    assert!(
        out.contains("untraced acceptance criteria"),
        "missing the untraced-criteria warning:\n{out}"
    );
    assert!(
        out.contains("the widget renders") && out.contains("the widget saves"),
        "warning must quote each criterion's text verbatim:\n{out}"
    );
    assert!(
        out.contains(&format!("{spec}.AC1")) && out.contains(&format!("{spec}.AC2")),
        "warning must name each criterion id:\n{out}"
    );
}

#[test]
fn queue_done_refuses_under_enforce_refuse() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let p = Project::new();
    p.set_protocol_enforce_refuse();
    let spec = p.add_with_description("task", "gate refuse fixture", UNTRACED_DESCRIPTION);

    let done = aida(&p.repo, &p.home)
        .args(["queue", "done", &spec, "--skip-pr-check"])
        .output()
        .expect("run aida queue done");
    let out = text(&done);
    assert!(
        !done.status.success(),
        "queue done must block under enforce=refuse:\n{out}"
    );
    assert!(
        out.contains("queue done refused"),
        "refusal must say so plainly:\n{out}"
    );
    assert!(
        out.contains("the widget renders"),
        "refusal must quote the untraced criterion verbatim:\n{out}"
    );
}

#[test]
fn queue_done_force_overrides_and_ledgers_the_override() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let p = Project::new();
    p.set_protocol_enforce_refuse();
    let spec = p.add_with_description("task", "gate force fixture", UNTRACED_DESCRIPTION);

    let done = aida(&p.repo, &p.home)
        .args(["queue", "done", &spec, "--skip-pr-check", "--force"])
        .output()
        .expect("run aida queue done --force");
    let out = text(&done);
    assert!(
        done.status.success(),
        "--force must override the refusal:\n{out}"
    );
    assert!(
        out.contains("--force"),
        "override must be announced loudly:\n{out}"
    );

    let comments = aida(&p.repo, &p.home)
        .args(["comment", "list", &spec])
        .output()
        .expect("run aida comment list");
    let comments_out = text(&comments);
    assert!(
        comments_out.contains("--force override"),
        "the override must be ledgered as a comment on the spec:\n{comments_out}"
    );
    assert!(
        comments_out.contains("the widget renders"),
        "the ledger comment must name the untraced criterion:\n{comments_out}"
    );
}

#[test]
fn queue_done_is_quiet_when_all_criteria_are_traced() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let p = Project::new();
    let spec = p.add_with_description(
        "task",
        "gate quiet fixture",
        "## Acceptance\n- AC1: the widget renders\n",
    );
    std::fs::write(
        p.repo.join("gate_quiet_test.rs"),
        format!(
            "// trace:{spec}.AC1 | ai:claude\n#[test]\nfn widget_renders() {{\n    assert!(true);\n}}\n"
        ),
    )
    .unwrap();
    git(&p.repo, &["add", "gate_quiet_test.rs"]);
    git(&p.repo, &["commit", "-q", "-m", "test: add traced fixture"]);

    let done = aida(&p.repo, &p.home)
        .args(["queue", "done", &spec, "--skip-pr-check"])
        .output()
        .expect("run aida queue done");
    let out = text(&done);
    assert!(done.status.success(), "queue done failed:\n{out}");
    assert!(
        !out.to_ascii_lowercase().contains("untraced"),
        "a spec whose criteria are all traced must produce no criteria-gate output:\n{out}"
    );
}

#[test]
fn reviewer_prompt_round_one_cites_the_untraced_criteria_list() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let p = Project::new();
    let spec = p.add_with_description("bug", "reviewer prompt fixture", UNTRACED_DESCRIPTION);

    // Local branch matching the fallback `pr_base_head` head-naming
    // (`pr-<N>` — the GitHub fallback used when `gh` can't resolve the
    // real PR, which is always true for this throwaway repo). A commit
    // carrying the `(SPEC-ID)` trailer is what `generate_review_prompt`'s
    // resolution (and this wiring) reads to find the PR's covered specs.
    git(&p.repo, &["checkout", "-q", "-b", "pr-77"]);
    std::fs::write(p.repo.join("widget.txt"), "widget\n").unwrap();
    git(&p.repo, &["add", "widget.txt"]);
    git(
        &p.repo,
        &["commit", "-q", "-m", &format!("feat: widget ({spec})")],
    );
    git(&p.repo, &["checkout", "-q", "main"]);

    // A synthetic "Review PR-77: ..." story is how `aida queue work`
    // resolves a PR-scoped review pickup without a real forge round trip
    // (mirrors the existing `drain_dry_run_skips_reviewer_routed_head`
    // fixture in queue_work_dry_run.rs).
    let reviewer_spec = p.add_with_description(
        "story",
        "Review PR-77: routed review",
        "## Acceptance\n- fixture acceptance\n",
    );
    let queue_add = aida(&p.repo, &p.home)
        .env("AIDA_SESSION_ROLE", "advisor")
        .args(["queue", "add", &reviewer_spec, "--for", "reviewer"])
        .output()
        .expect("run aida queue add reviewer");
    assert!(
        queue_add.status.success(),
        "aida queue add reviewer failed: {}",
        String::from_utf8_lossy(&queue_add.stderr)
    );

    let work = aida(&p.repo, &p.home)
        .args([
            "queue",
            "work",
            &reviewer_spec,
            "--no-launch",
            "--no-human",
            "--no-pull",
        ])
        .output()
        .expect("run aida queue work (reviewer)");
    let out = text(&work);
    // Not asserting exit success here: this throwaway repo has no real PR
    // for `gh` to resolve and no `pull/77/head` ref for git to fetch, so the
    // reviewer pickup's (unrelated) worktree/branch setup fails later in the
    // same run. The prompt — including this wiring's block — is derived and
    // printed before that happens, which is what these assertions check:
    // the real `aida queue work` dispatch path produced it, not a shortcut
    // straight to the `criteria_gate`/`append_untraced_criteria_prompt_block`
    // helpers. trace:TASK-1290 | ai:claude
    assert!(
        out.contains("Untraced acceptance criteria"),
        "round-1 reviewer prompt must cite the untraced-criteria block:\n{out}"
    );
    assert!(
        out.contains(&format!("{spec}.AC1")) && out.contains("the widget renders"),
        "round-1 reviewer prompt must name the untraced criterion verbatim:\n{out}"
    );
}
