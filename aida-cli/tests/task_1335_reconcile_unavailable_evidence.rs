#![cfg(unix)]

//! Black-box coverage for `aida db reconcile-status` when completion evidence
//! is unavailable. Every test drives the real binary against a disposable
//! project (`aida init` code repo plus its `.aida-store` store) under a fake
//! HOME/XDG tree with a cleared environment. `gh` is a fixture script pinned
//! through the existing forge-binary override, so no test can reach a real
//! forge; `glab` and vendor CLIs are tripwires that must never run.
//!
//! The scan-to-apply barrier is real: the open-PR lookup happens after the
//! scan snapshot and before the store write, so a fixture `gh` that mutates
//! the store through the real CLI during that lookup is exactly the
//! concurrent edit the write seam must honour.
// trace:TASK-1335 | ai:claude

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const HISTORICAL_SUBJECT: &str =
    "[AI:codex] feat(ci): validate Windows weekly   on main (TASK-1588) (#2436)";

struct Fx {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
    bin: PathBuf,
    state: PathBuf,
    gh: PathBuf,
}

fn write_exec(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

const FAKE_GH: &str = r#"#!/bin/sh
state="${FIXTURE_STATE:?}"
printf '%s\n' "$*" >> "$state/gh.log"
if [ "$1" = "--version" ]; then echo "gh version fixture"; exit 0; fi
if [ "$1 $2" = "repo view" ]; then echo main; exit 0; fi
if [ "$1 $2" != "pr list" ]; then
  printf 'gh %s\n' "$*" >> "$state/unexpected.log"
  echo "unexpected fake gh call: $*" >&2
  exit 2
fi
search=""
json=""
while [ $# -gt 0 ]; do
  case "$1" in
    --search) search="$2"; shift ;;
    --json) json="$2"; shift ;;
  esac
  shift
done
if [ "$json" = "title,body" ]; then
  mode=$(cat "$state/diag" 2>/dev/null || echo clear)
else
  if [ -f "$state/barrier-$search" ]; then
    sh "$state/barrier-$search" >> "$state/barrier.log" 2>&1 || { cat "$state/barrier.log" >&2; exit 3; }
    rm -f "$state/barrier-$search"
  fi
  mode=$(cat "$state/search-$search" 2>/dev/null || cat "$state/search-default" 2>/dev/null || echo clear)
fi
case "$mode" in
  clear) echo '[]' ;;
  open:*) printf '[{"number":%s}]\n' "${mode#open:}" ;;
  exit) echo 'HTTP 401: Bad credentials (https://api.github.com/graphql)' >&2; exit 1 ;;
  garbage) echo 'this is not json' ;;
  object) echo '{"number":5}' ;;
  badrow) echo '[{"title":"no number"}]' ;;
  diag-missing) echo '[{}]' ;;
  diag-wrongtype) echo '[{"title":42,"body":false}]' ;;
  diag-mixed) echo '[{"title":"wip (TASK-11)","body":""},{"title":"x"}]' ;;
  diag-truncated) echo '[{"title":"wip (TASK-11)","body":""}' ;;
  diag-match) echo '[{"title":"wip (TASK-11)","body":""}]' ;;
  diag-nomatch) echo '[{"title":"other (TASK-110)","body":"TASK-1"}]' ;;
  *) echo "unknown fixture mode $mode" >&2; exit 9 ;;
esac
"#;

const TRIPWIRE: &str = r#"#!/bin/sh
printf '%s %s\n' "$(basename "$0")" "$*" >> "${FIXTURE_STATE:?}/unexpected.log"
exit 97
"#;

fn git_in(dir: &Path, args: &[&str], date: Option<&str>) -> String {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.com")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.com");
    if let Some(date) = date {
        cmd.env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date);
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn text(out: &Output) -> String {
    format!(
        "exit={:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Forge {
    PureGit,
    GitHub,
}

impl Fx {
    fn new(forge: Forge) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let repo = root.join("repo");
        let bin = root.join("bin");
        let state = root.join("state");
        for dir in [&repo, &bin, &state, &root.join("home")] {
            std::fs::create_dir_all(dir).unwrap();
        }
        write_exec(&bin.join("gh"), FAKE_GH);
        for tripwire in ["glab", "claude", "codex", "gemini", "antigravity"] {
            write_exec(&bin.join(tripwire), TRIPWIRE);
        }
        git_in(&repo, &["init", "-q", "-b", "main"], None);
        git_in(
            &repo,
            &["commit", "-q", "--allow-empty", "-m", "chore: base"],
            Some("2026-09-01T00:00:00Z"),
        );
        let fx = Self {
            _tmp: tmp,
            root,
            gh: bin.join("gh"),
            repo,
            bin,
            state,
        };
        let init = fx.aida(&[
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-post-hooks",
            "--no-roles",
            "--agent",
            "claude",
        ]);
        assert!(init.status.success(), "aida init failed:\n{}", text(&init));
        if forge == Forge::GitHub {
            fx.set_provider("github");
            git_in(
                &fx.repo,
                &[
                    "remote",
                    "add",
                    "origin",
                    "https://github.com/acme/reconcile-fixture.git",
                ],
                None,
            );
        }
        fx
    }

    fn command(&self) -> Command {
        let home = self.root.join("home");
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
        cmd.current_dir(&self.repo)
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("HOME", &home)
            .env("USER", "fixture-human")
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .env("XDG_STATE_HOME", home.join(".local/state"))
            .env("XDG_RUNTIME_DIR", home.join("run"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.com")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.com")
            .env("AIDA_TELEMETRY", "0")
            .env("AIDA_OUTPUT_FORMAT", "human")
            .env("NO_COLOR", "1")
            .env("AIDA_TEST_GH_BINARY", &self.gh)
            .env("AIDA_TEST_GLAB_BINARY", self.bin.join("glab"))
            .env("FIXTURE_STATE", &self.state)
            .env("FIXTURE_AIDA", env!("CARGO_BIN_EXE_aida"))
            .env("FIXTURE_STORE", self.store())
            .stdin(std::process::Stdio::null());
        cmd
    }

    fn aida(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    fn reconcile(&self, extra: &[&str]) -> Output {
        let mut args = vec!["db", "reconcile-status"];
        args.extend_from_slice(extra);
        self.aida(&args)
    }

    fn config_path(&self) -> PathBuf {
        self.repo.join(".aida/config.toml")
    }

    fn set_provider(&self, provider: &str) {
        let config = std::fs::read_to_string(self.config_path()).unwrap();
        let mut out = String::new();
        let mut replaced = false;
        for line in config.lines() {
            if line.trim_start().starts_with("provider =") {
                out.push_str(&format!("provider = \"{provider}\"\n"));
                replaced = true;
            } else {
                out.push_str(line);
                out.push('\n');
            }
        }
        assert!(replaced, "scaffolded config has no [forge] provider line");
        std::fs::write(self.config_path(), out).unwrap();
    }

    fn store(&self) -> PathBuf {
        self.repo.join(".aida-store")
    }

    fn object_path(&self, spec_id: &str) -> PathBuf {
        let (prefix, rest) = spec_id.split_once('-').unwrap();
        let seq: u32 = rest.rsplit('-').next().unwrap().parse().unwrap();
        let shard = if seq == 0 { 0 } else { (seq - 1) / 1000 };
        self.store()
            .join("objects")
            .join(prefix)
            .join(format!("{shard:03}"))
            .join(format!("{spec_id}.yaml"))
    }

    /// Seed one requirement object and commit it to the store branch.
    fn seed(&self, spec_id: &str, agreed: &str, status: &str, extra: &str) -> String {
        let uuid = uuid::Uuid::new_v4().to_string();
        let path = self.object_path(spec_id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!(
                "id: {uuid}\nspec_id: {spec_id}\nagreed_id: {agreed}\ntitle: fixture {agreed}\n\
                 description: fixture\nstatus: {status}\npriority: Medium\nowner: ''\n\
                 feature: Uncategorized\ncreated_at: 2026-09-01T00:00:00Z\n\
                 modified_at: 2026-09-01T00:00:00Z\nreq_type: Task\n{extra}"
            ),
        )
        .unwrap();
        git_in(&self.store(), &["add", "-A"], None);
        git_in(
            &self.store(),
            &["commit", "-q", "-m", &format!("seed {spec_id}")],
            None,
        );
        uuid
    }

    fn object(&self, spec_id: &str) -> String {
        std::fs::read_to_string(self.object_path(spec_id)).unwrap()
    }

    fn status(&self, spec_id: &str) -> String {
        self.object(spec_id)
            .lines()
            .find_map(|l| l.strip_prefix("status: "))
            .unwrap()
            .trim()
            .to_string()
    }

    fn commit(&self, subject: &str, date: &str) -> String {
        git_in(
            &self.repo,
            &["commit", "-q", "--allow-empty", "-m", subject],
            Some(date),
        );
        git_in(&self.repo, &["rev-parse", "HEAD"], None)
    }

    fn fillers(&self, count: usize) {
        for i in 0..count {
            git_in(
                &self.repo,
                &[
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    &format!("chore: filler {i}"),
                ],
                Some("2026-09-20T00:00:00Z"),
            );
        }
    }

    fn mode(&self, name: &str, mode: &str) {
        std::fs::write(self.state.join(name), mode).unwrap();
    }

    /// Run `aida <args>` from inside the open-PR lookup for `spec_id`.
    fn barrier(&self, spec_id: &str, args: &str) {
        std::fs::write(
            self.state.join(format!("barrier-{spec_id}")),
            format!("exec \"$FIXTURE_AIDA\" {args}\n"),
        )
        .unwrap();
    }

    /// Run a raw shell script from inside the open-PR lookup for `spec_id`
    /// (for concurrent store writes the CLI does not offer, e.g. a peer
    /// replacing an object). `$FIXTURE_STORE` is the store worktree.
    fn barrier_script(&self, spec_id: &str, script: &str) {
        std::fs::write(self.state.join(format!("barrier-{spec_id}")), script).unwrap();
    }

    /// Replace the scaffolded `[forge]` table with `root_line`, written as a
    /// root-level TOML entry (inline table, dotted key, ...).
    fn rewrite_forge(&self, root_line: &str) {
        let config = std::fs::read_to_string(self.config_path()).unwrap();
        let kept: Vec<&str> = config
            .lines()
            .filter(|l| l.trim() != "[forge]" && !l.trim_start().starts_with("provider ="))
            .collect();
        assert!(kept.len() + 2 == config.lines().count(), "no [forge] table");
        std::fs::write(
            self.config_path(),
            format!("{root_line}\n{}\n", kept.join("\n")),
        )
        .unwrap();
    }

    fn set_title(&self, spec_id: &str, title: &str) {
        let path = self.object_path(spec_id);
        let body = std::fs::read_to_string(&path).unwrap().replace(
            &format!("title: fixture {spec_id}"),
            &format!("title: '{title}'"),
        );
        std::fs::write(&path, body).unwrap();
        git_in(&self.store(), &["commit", "-qam", "retitle"], None);
    }

    /// `SpecCompleted` events this project recorded for `spec_id` from
    /// reconcile-status.
    fn completion_events(&self, spec_id: &str) -> usize {
        std::fs::read_to_string(self.repo.join(".aida/events.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains(&format!("\"{spec_id}\"")) && l.contains("reconcile-status"))
            .count()
    }

    fn gh_log(&self) -> String {
        std::fs::read_to_string(self.state.join("gh.log")).unwrap_or_default()
    }

    fn assert_no_tripwire(&self) {
        let log = std::fs::read_to_string(self.state.join("unexpected.log")).unwrap_or_default();
        assert!(log.is_empty(), "unexpected forge/vendor calls:\n{log}");
    }

    /// Config bytes, canonical objects (incl. history), store HEAD/worktree
    /// state and code refs — what a no-write run must leave untouched.
    fn snapshot(&self) -> String {
        let mut snap = String::new();
        snap.push_str(&std::fs::read_to_string(self.config_path()).unwrap());
        snap.push_str(&git_in(&self.store(), &["rev-parse", "HEAD"], None));
        // Canonical content only: any CLI start recreates the per-node
        // `.aida/dispenser.toml` runtime file in the store worktree.
        snap.push_str(&git_in(
            &self.store(),
            &["status", "--porcelain", "--", "objects", "registry"],
            None,
        ));
        snap.push_str(&git_in(&self.store(), &["log", "--format=%H %s"], None));
        let mut files = Vec::new();
        let mut stack = vec![self.store().join("objects")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    files.push(path);
                }
            }
        }
        files.sort();
        for file in files {
            snap.push_str(&file.display().to_string());
            snap.push_str(&std::fs::read_to_string(&file).unwrap_or_default());
        }
        snap.push_str(&git_in(
            &self.repo,
            &["for-each-ref", "--format=%(refname) %(objectname)"],
            None,
        ));
        snap.push_str(&git_in(&self.repo, &["rev-parse", "HEAD"], None));
        snap
    }
}

/// Snapshot equality that names the first differing lines.
fn assert_same(after: &str, before: &str, context: &str) {
    if after == before {
        return;
    }
    let diff: Vec<String> = before
        .lines()
        .zip(after.lines())
        .filter(|(b, a)| b != a)
        .take(6)
        .map(|(b, a)| format!("- {b}\n+ {a}"))
        .collect();
    panic!(
        "state changed {context} (lines before={} after={}):\n{}",
        before.lines().count(),
        after.lines().count(),
        diff.join("\n")
    );
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Group 1: the historical subject, 150 commits below HEAD, inside the
/// default 200-commit window. Every unavailable variant names the matched
/// commit and the failed required check, exits nonzero in dry-run AND normal
/// mode, and writes nothing; an open PR is an ordinary refusal; only an
/// authoritative `[]` completes, with correct provenance.
#[test]
fn historical_trailer_unavailable_then_clear() {
    let fx = Fx::new(Forge::GitHub);
    fx.seed("TASK-1588", "TASK-1588", "Done", "");
    let evidence = fx.commit(HISTORICAL_SUBJECT, "2026-09-10T00:00:00Z");
    fx.fillers(150);
    let short = &evidence[..7];

    let variants: [(&str, Option<&str>, &str); 5] = [
        ("missing", None, "`gh` executable not found"),
        (
            "exit",
            Some("exit"),
            "`gh pr list` exited with status 1: HTTP 401: Bad credentials",
        ),
        ("garbage", Some("garbage"), "output was not JSON"),
        ("object", Some("object"), "JSON that is not an array"),
        ("badrow", Some("badrow"), "row without a positive PR number"),
    ];
    for (name, mode, reason) in variants {
        let before = fx.snapshot();
        for extra in [
            &["--spec", "TASK-1588", "--dry-run"][..],
            &["--spec", "TASK-1588"][..],
            &["--dry-run"][..],
            &[][..],
        ] {
            let mut cmd = fx.command();
            match mode {
                Some(mode) => fx.mode("search-TASK-1588", mode),
                None => {
                    cmd.env("AIDA_TEST_GH_BINARY", fx.bin.join("gh-not-installed"));
                }
            }
            let out = cmd
                .args(["db", "reconcile-status"])
                .args(extra)
                .output()
                .unwrap();
            let all = text(&out);
            assert!(!out.status.success(), "{name} {extra:?} must fail:\n{all}");
            let err = stderr(&out);
            assert!(
                err.contains(&format!(
                    "TASK-1588: commit {short} (subject trailer) matched"
                )),
                "{name} {extra:?}:\n{all}"
            );
            assert!(err.contains("required open-PR check unavailable"), "{all}");
            assert!(err.contains(reason), "{name} wants `{reason}`:\n{all}");
            assert!(err.contains("Status unchanged (Done)"), "{all}");
            assert!(err.contains("retry when forge access is restored"), "{all}");
            assert!(
                err.contains("could not verify required completion evidence for 1 candidate"),
                "{all}"
            );
            assert!(!all.contains("no commit in the scan"), "{all}");
            assert!(!all.contains("No eligible flips"), "{all}");
            assert_same(
                &fx.snapshot(),
                &before,
                &format!("{name} {extra:?} wrote state"),
            );
        }
        assert_eq!(fx.status("TASK-1588"), "Done");
    }

    // An open PR is the existing refusal: success, stays Done.
    fx.mode("search-TASK-1588", "open:2436");
    let before = fx.snapshot();
    let out = fx.reconcile(&["--spec", "TASK-1588"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        stderr(&out).contains("TASK-1588 stays Done — open PR #2436 still references it"),
        "{}",
        text(&out)
    );
    assert_same(&fx.snapshot(), &before, "");

    // Authoritative `[]`: dry-run predicts without writing, normal completes.
    fx.mode("search-TASK-1588", "clear");
    let out = fx.reconcile(&["--spec", "TASK-1588", "--dry-run"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        stdout(&out).contains("would flip 1 spec → Completed"),
        "{}",
        text(&out)
    );
    assert!(
        stdout(&out).contains(&format!("commit {short}")),
        "{}",
        text(&out)
    );
    assert_same(&fx.snapshot(), &before, "dry-run wrote state");
    let out = fx.reconcile(&["--spec", "TASK-1588"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(fx.status("TASK-1588"), "Completed");
    let object = fx.object("TASK-1588");
    assert!(
        object.contains(&format!("completion_sha: {evidence}")),
        "{object}"
    );
    assert!(object.contains("author: aida-reconcile"), "{object}");
    assert!(
        fx.gh_log().contains("--search TASK-1588"),
        "{}",
        fx.gh_log()
    );
    fx.assert_no_tripwire();
}

/// Group 1b: a controlled distance outside the default window is a genuine
/// no-match (success, explicit pinned range); `--since` brings it back.
#[test]
fn subject_outside_default_window_is_an_honest_no_match() {
    let fx = Fx::new(Forge::GitHub);
    fx.seed("TASK-1588", "TASK-1588", "Done", "");
    let evidence = fx.commit(HISTORICAL_SUBJECT, "2026-09-10T00:00:00Z");
    fx.fillers(200);
    fx.mode("search-default", "exit");
    let head = git_in(&fx.repo, &["rev-parse", "HEAD"], None);
    let before = fx.snapshot();
    let out = fx.reconcile(&["--spec", "TASK-1588", "--dry-run"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        stdout(&out).contains("No eligible flips for TASK-1588"),
        "{}",
        text(&out)
    );
    assert!(
        stdout(&out).contains(&format!("Scanned main@{} (last 200 commits)", &head[..7])),
        "{}",
        text(&out)
    );
    assert!(!fx.gh_log().contains("--search"), "no candidate, no lookup");
    assert_same(&fx.snapshot(), &before, "");

    let since = format!("{evidence}~1");
    let out = fx.reconcile(&["--spec", "TASK-1588", "--since", &since, "--dry-run"]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(
        stderr(&out).contains("required open-PR check unavailable"),
        "{}",
        text(&out)
    );
    fx.assert_no_tripwire();
}

/// Group 2: genuine no-match vs a diagnostic-only outage vs a required
/// outage, in targeted and sweep form.
#[test]
fn genuine_no_match_vs_diagnostic_and_required_outage() {
    let fx = Fx::new(Forge::GitHub);
    fx.seed("TASK-10", "TASK-10", "Done", "");
    fx.seed("TASK-11", "TASK-11", "Completed", "");
    fx.commit("fix: shipped earlier (TASK-11)", "2026-09-10T00:00:00Z");
    fx.mode("diag", "exit");

    // Only the already-Completed diagnostic fails: warning, success.
    let before = fx.snapshot();
    for extra in [&["--dry-run"][..], &[][..]] {
        let out = fx.reconcile(extra);
        assert!(out.status.success(), "{extra:?}:\n{}", text(&out));
        assert!(
            stdout(&out).contains("unknown (open-PR diagnostic unavailable)"),
            "{}",
            text(&out)
        );
        assert!(
            stderr(&out).contains("This diagnostic does not gate completion"),
            "{}",
            text(&out)
        );
        assert!(stdout(&out).contains("already Completed"), "{}", text(&out));
        assert!(stdout(&out).contains("Scanned main@"), "{}", text(&out));
        assert_same(&fx.snapshot(), &before, "");
    }

    // F1335-4: a diagnostic response whose rows lack the requested string
    // fields is unavailable (warning, still exit 0) — never a measured zero.
    for (mode, reason) in [
        ("diag-missing", "without string `title` and `body` fields"),
        ("diag-wrongtype", "without string `title` and `body` fields"),
        ("diag-mixed", "without string `title` and `body` fields"),
        ("diag-truncated", "output was not JSON"),
    ] {
        fx.mode("diag", mode);
        for extra in [&["--dry-run"][..], &[][..], &["--spec", "TASK-11"][..]] {
            let out = fx.reconcile(extra);
            let all = text(&out);
            assert!(out.status.success(), "{mode} {extra:?}:\n{all}");
            assert!(
                stdout(&out).contains("unknown (open-PR diagnostic unavailable)"),
                "{mode}:\n{all}"
            );
            assert!(
                stderr(&out).contains(reason),
                "{mode} wants `{reason}`:\n{all}"
            );
            assert!(
                !stdout(&out).contains(" 0 Completed spec"),
                "{mode}:\n{all}"
            );
            assert_same(&fx.snapshot(), &before, mode);
        }
    }
    for (mode, count) in [
        ("clear", "0 Completed specs"),
        ("diag-match", "1 Completed spec still"),
        ("diag-nomatch", "0 Completed specs"),
    ] {
        fx.mode("diag", mode);
        let out = fx.reconcile(&["--dry-run"]);
        assert!(out.status.success(), "{}", text(&out));
        assert!(stdout(&out).contains(count), "{mode}:\n{}", text(&out));
        assert!(
            !stderr(&out).contains("diagnostic does not gate"),
            "{}",
            text(&out)
        );
    }
    fx.mode("diag", "exit");
    let out = fx.reconcile(&["--spec", "TASK-10"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        stdout(&out).contains("No eligible flips for TASK-10"),
        "{}",
        text(&out)
    );

    // A real candidate whose required check fails, next to a Completed spec
    // whose diagnostic now succeeds and a spec with no commit at all.
    fx.mode("diag", "clear");
    fx.seed("TASK-12", "TASK-12", "Done", "");
    fx.commit("fix: needs a check (TASK-12)", "2026-09-11T00:00:00Z");
    fx.mode("search-TASK-12", "exit");
    let before = fx.snapshot();
    let out = fx.reconcile(&[]);
    assert!(!out.status.success(), "{}", text(&out));
    let all = text(&out);
    assert!(
        stdout(&out).contains("0 Completed specs still referenced"),
        "{all}"
    );
    assert!(stderr(&out).contains("TASK-12: commit"), "{all}");
    assert!(
        !all.contains("TASK-10"),
        "no-commit spec is not a candidate:\n{all}"
    );
    assert_same(&fx.snapshot(), &before, "");
    // Targeted runs stay independent of the other candidate's outage.
    let out = fx.reconcile(&["--spec", "TASK-10"]);
    assert!(out.status.success(), "{}", text(&out));
    let out = fx.reconcile(&["--spec", "TASK-11"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(stdout(&out).contains("already Completed"), "{}", text(&out));
    fx.assert_no_tripwire();
}

/// Group 3: only the existing accepted trailer forms earn completion.
#[test]
fn attribution_is_not_free_text() {
    let fx = Fx::new(Forge::PureGit);
    for id in [
        "TASK-20", "TASK-21", "TASK-22", "TASK-23", "TASK-30", "TASK-31", "TASK-32", "TASK-33",
        "TASK-34",
    ] {
        fx.seed(id, id, "Done", "");
    }
    fx.seed("TASK-1-40", "TASK-1540", "Done", "");
    // Two requirements answering to one display id: ambiguous, refused.
    fx.seed("TASK-50", "TASK-50", "Done", "");
    fx.seed("TASK-1-50", "TASK-50", "Done", "");

    let date = "2026-09-10T00:00:00Z";
    fx.commit("fix: mentions TASK-20 only in prose", date);
    fx.commit("docs(plans): plan the next slice (TASK-21)", date);
    fx.commit("fix: no squash marker\n\nDelivers more (TASK-22)", date);
    fx.commit("fix: unterminated group (TASK-23", date);
    fx.commit("TASK-30: leading form", date);
    fx.commit("[AI:claude] fix(x): terminal form (TASK-31) (#77)", date);
    fx.commit("fix: multi (TASK-32, TASK-33)", date);
    fx.commit(
        "fix: squash (TASK-99) (#78)\n\n- child work (TASK-34)",
        date,
    );
    fx.commit("fix: remapped display id (TASK-1540)", date);
    fx.commit("fix: ambiguous (TASK-50)", date);

    let before = fx.snapshot();
    let out = fx.reconcile(&["--dry-run"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_same(&fx.snapshot(), &before, "");
    let out = fx.reconcile(&[]);
    assert!(out.status.success(), "{}", text(&out));
    for id in [
        "TASK-30",
        "TASK-31",
        "TASK-32",
        "TASK-33",
        "TASK-34",
        "TASK-1-40",
    ] {
        assert_eq!(fx.status(id), "Completed", "{id}:\n{}", text(&out));
    }
    for id in [
        "TASK-20",
        "TASK-21",
        "TASK-22",
        "TASK-23",
        "TASK-50",
        "TASK-1-50",
    ] {
        assert_eq!(fx.status(id), "Done", "{id} gained credit:\n{}", text(&out));
    }
    let out = fx.reconcile(&["--spec", "TASK-50"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        stderr(&out).contains("shared by more than one requirement"),
        "{}",
        text(&out)
    );
    assert!(
        !fx.gh_log().contains("pr list"),
        "pure git never queries open changes:\n{}",
        fx.gh_log()
    );
    fx.assert_no_tripwire();
}

/// Group 4a: SHA and legacy human-reopen fences refuse old evidence,
/// including after an automated overwrite; a later commit is accepted.
#[test]
fn reopen_fences_hold_and_later_evidence_proceeds() {
    let fx = Fx::new(Forge::PureGit);
    let old = fx.commit(
        "fix: first attempt (TASK-60) (TASK-61)",
        "2026-09-10T00:00:00Z",
    );
    let reopen = fx.commit("chore: reopen point", "2026-09-11T00:00:00Z");
    fx.seed(
        "TASK-60",
        "TASK-60",
        "Done",
        &format!("implementation_info:\n  implemented: false\n  reopened_at_sha: {reopen}\n"),
    );
    // Legacy human reopen (no SHA marker), then an automated overwrite.
    fx.seed(
        "TASK-61",
        "TASK-61",
        "Done",
        "history:\n\
         - id: 01a11c00-0000-7000-8000-000000000001\n  author: fixture-human\n  \
         timestamp: 2026-09-12T00:00:00Z\n  changes:\n  - field_name: status\n    \
         old_value: Done\n    new_value: Approved\n\
         - id: 01a11c00-0000-7000-8000-000000000002\n  author: aida-auto-bump\n  \
         timestamp: 2026-09-13T00:00:00Z\n  changes:\n  - field_name: status\n    \
         old_value: Approved\n    new_value: Done\n",
    );
    let before = fx.snapshot();
    for id in ["TASK-60", "TASK-61"] {
        let out = fx.reconcile(&["--spec", id]);
        assert!(out.status.success(), "{}", text(&out));
        assert!(
            stderr(&out).contains("reopened after that evidence"),
            "{id}:\n{}",
            text(&out)
        );
        assert!(stderr(&out).contains(&old[..7]), "{}", text(&out));
        assert_eq!(fx.status(id), "Done");
    }
    assert_same(&fx.snapshot(), &before, "");

    let later = fx.commit(
        "fix: second attempt (TASK-60) (TASK-61)",
        "2026-09-14T00:00:00Z",
    );
    let out = fx.reconcile(&[]);
    assert!(out.status.success(), "{}", text(&out));
    for id in ["TASK-60", "TASK-61"] {
        assert_eq!(fx.status(id), "Completed", "{id}:\n{}", text(&out));
        assert!(fx.object(id).contains(&format!("completion_sha: {later}")));
    }
    fx.assert_no_tripwire();
}

/// Group 4b: changes made between the scan and the write — a human reopen,
/// a new closure blocker, a covered spec reopened — defeat the scan-time
/// evidence at the real write seam, while an untouched spec in the same
/// batch completes. The report is truthful and a rerun is idempotent.
#[test]
fn write_seam_rechecks_reopen_closure_and_covers() {
    let fx = Fx::new(Forge::GitHub);
    fx.seed("TASK-70", "TASK-70", "Done", ""); // reopened mid-run
    fx.seed("TASK-71", "TASK-71", "Done", ""); // blocked mid-run
    fx.seed("TASK-72", "TASK-72", "InProgress", ""); // the new blocker
    fx.seed("TASK-73", "TASK-73", "Done", ""); // untouched control
    let covered = fx.seed("TASK-74", "TASK-74", "Completed", "");
    let review = format!("relationships:\n- rel_type: implements\n  target_id: {covered}\n");
    fx.seed("STORY-80", "STORY-80", "Done", &review);
    // Make STORY-80 a review story: the covers chain keys on the title shape.
    let path = fx.object_path("STORY-80");
    let body = std::fs::read_to_string(&path)
        .unwrap()
        .replace("title: fixture STORY-80", "title: 'Review PR-901: fixture'");
    std::fs::write(&path, body).unwrap();
    git_in(&fx.store(), &["commit", "-qam", "review story"], None);

    fx.seed("TASK-75", "TASK-75", "Done", ""); // object replaced mid-run
    fx.seed("TASK-76", "TASK-76", "Done", ""); // id made ambiguous mid-run
    fx.commit(
        "fix: lands (TASK-70) (TASK-71) (TASK-73) (TASK-75) (TASK-76)",
        "2026-09-10T00:00:00Z",
    );
    fx.barrier_script(
        "TASK-75",
        "sed -i 's/^id: .*/id: 01a11c00-0000-7000-8000-0000000000aa/' \
         \"$FIXTURE_STORE/objects/TASK/000/TASK-75.yaml\" && \
         git -C \"$FIXTURE_STORE\" commit -qam 'peer replaces TASK-75'\n",
    );
    fx.barrier_script(
        "TASK-76",
        "printf 'id: 01a11c00-0000-7000-8000-0000000000bb\\nspec_id: TASK-1-76\\n\
         agreed_id: TASK-76\\ntitle: peer twin\\ndescription: d\\nstatus: Done\\n\
         priority: Medium\\nowner: x\\nfeature: Uncategorized\\n\
         created_at: 2026-09-01T00:00:00Z\\nmodified_at: 2026-09-01T00:00:00Z\\n\
         req_type: Task\\n' > \"$FIXTURE_STORE/objects/TASK/000/TASK-1-76.yaml\" && \
         git -C \"$FIXTURE_STORE\" add -A && \
         git -C \"$FIXTURE_STORE\" commit -qm 'peer adds TASK-76 twin'\n",
    );
    fx.barrier("TASK-70", "edit TASK-70 --status approved");
    fx.barrier("TASK-71", "rel add TASK-71 TASK-72 --type blocked-by");
    // The covered spec itself is reopened: a real Completed → Approved
    // decision, which the CLI takes only from a validated advisor seat (the
    // shared ADR-66 grant fixture) with --force.
    let grant = support::grant_seat_for(
        &fx.root.join("home"),
        &fx.repo,
        "fixture-human",
        "advisor",
        &[],
    );
    git_in(&fx.store(), &["add", "-A"], None);
    git_in(&fx.store(), &["commit", "-qm", "fixture roster"], None);
    fx.barrier_script(
        "STORY-80",
        &format!(
            "AIDA_SESSION_ROLE=advisor AIDA_SESSION_GRANT={grant} \
             \"$FIXTURE_AIDA\" edit TASK-74 --status approved --force\n"
        ),
    );

    let out = fx.reconcile(&[]);
    let all = text(&out);
    assert!(out.status.success(), "{all}");
    let barrier_log = std::fs::read_to_string(fx.state.join("barrier.log")).unwrap_or_default();
    assert_eq!(
        std::fs::read_dir(&fx.state)
            .unwrap()
            .filter(|e| e
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("barrier-"))
            .count(),
        0,
        "every barrier ran:\n{barrier_log}\n{all}"
    );
    assert_eq!(fx.status("TASK-73"), "Completed", "{all}");
    assert_eq!(fx.status("TASK-70"), "Approved", "{all}");
    assert_eq!(fx.status("TASK-71"), "Done", "{all}");
    assert_eq!(fx.status("STORY-80"), "Done", "{all}");
    assert_eq!(fx.status("TASK-74"), "Approved", "{all}");
    assert_eq!(fx.status("TASK-75"), "Done", "{all}");
    assert_eq!(fx.status("TASK-76"), "Done", "{all}");
    assert_eq!(fx.status("TASK-1-76"), "Done", "{all}");
    let err = stderr(&out);
    assert!(
        err.contains("TASK-75: commit")
            && err.contains("(its ID now resolves to a different requirement)"),
        "{all}"
    );
    assert!(
        err.contains("TASK-76: commit")
            && err.contains("(its ID now names more than one requirement)"),
        "{all}"
    );
    for id in ["TASK-70", "TASK-71", "TASK-75", "TASK-76", "STORY-80"] {
        assert_eq!(fx.completion_events(id), 0, "{id} emitted a completion");
    }
    assert_eq!(fx.completion_events("TASK-73"), 1, "{all}");
    assert!(
        err.contains("TASK-70: commit")
            && err.contains("changed after the scan (it was reopened after this evidence)"),
        "{all}"
    );
    assert!(
        err.contains("TASK-71: commit")
            && err.contains("changed after the scan (its completion is now held"),
        "{all}"
    );
    assert!(
        err.contains("STORY-80: covers chain evidence matched")
            && err.contains("changed after the scan (no covered spec is still completed)"),
        "{all}"
    );
    assert!(stdout(&out).contains("TASK-73"), "{all}");

    // Idempotent: a rerun completes nothing new and adds no history.
    let control = fx.object("TASK-73");
    let out = fx.reconcile(&[]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(fx.object("TASK-73"), control);
    assert_eq!(
        control.matches("author: aida-reconcile").count(),
        1,
        "{control}"
    );
    fx.assert_no_tripwire();
}

/// Group 5: one candidate's outage defers the whole normal batch, with each
/// candidate's own reason; a Draft landing (an independent path) is reported
/// as applied; all-clear and mixed open/clear complete exactly as before.
#[test]
fn multi_candidate_failure_is_conservative_and_truthful() {
    let fx = Fx::new(Forge::GitHub);
    fx.seed("TASK-90", "TASK-90", "Done", "");
    fx.seed("TASK-91", "TASK-91", "Done", "");
    fx.commit("fix: a (TASK-90)", "2026-09-10T00:00:00Z");
    fx.commit("fix: b (TASK-91)", "2026-09-10T00:00:01Z");
    fx.mode("search-TASK-91", "garbage");

    let before = fx.snapshot();
    for extra in [&["--dry-run"][..], &[][..]] {
        let out = fx.reconcile(extra);
        let all = text(&out);
        assert!(!out.status.success(), "{all}");
        let err = stderr(&out);
        assert!(
            err.contains("TASK-91: commit") && err.contains("not JSON"),
            "{all}"
        );
        assert!(
            err.contains("TASK-90: commit")
                && err.contains("its open-PR check was clear; completion deferred"),
            "{all}"
        );
        assert!(err.contains("for 1 candidate (TASK-91)"), "{all}");
        assert!(!all.contains("No eligible flips"), "{all}");
        assert_same(&fx.snapshot(), &before, &format!("{extra:?}"));
    }

    // A Draft landing is independent and does apply; the error says so.
    fx.seed("TASK-92", "TASK-92", "Draft", "");
    fx.commit("fix: c (TASK-92)", "2026-09-10T00:00:02Z");
    let out = fx.reconcile(&[]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(
        stderr(&out)
            .contains("other independent changes WERE applied: 1 Draft spec landed at Done"),
        "{}",
        text(&out)
    );
    assert_eq!(fx.status("TASK-92"), "Done");
    assert_eq!(fx.status("TASK-90"), "Done");
    assert_eq!(fx.status("TASK-91"), "Done");

    // Mixed open/clear: the clear one completes, the open one stays.
    fx.mode("search-TASK-91", "open:12");
    let out = fx.reconcile(&[]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(fx.status("TASK-90"), "Completed");
    assert_eq!(fx.status("TASK-91"), "Done");
    // All clear: the rest completes; a rerun changes nothing.
    fx.mode("search-TASK-91", "clear");
    let out = fx.reconcile(&[]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(fx.status("TASK-91"), "Completed");
    let settled = fx.snapshot();
    let out = fx.reconcile(&[]);
    assert!(out.status.success(), "{}", text(&out));
    assert_same(&fx.snapshot(), &settled, "");
    fx.assert_no_tripwire();
}

/// Group 6: unknown is never empty. Unsupported, unrecognized and
/// unclassifiable forges refuse without touching config; a stale pure-git
/// config with a GitHub origin is classified as GitHub WITHOUT being
/// rewritten; real pure git (no origin, or a non-forge origin) completes.
#[test]
fn unavailable_forge_classification_is_not_empty() {
    let fx = Fx::new(Forge::PureGit);
    fx.seed("TASK-95", "TASK-95", "Done", "");
    fx.commit("fix: shipped (TASK-95)", "2026-09-10T00:00:00Z");

    let refuse = |provider: &str, origin: Option<&str>, want: &str| {
        fx.set_provider(provider);
        let _ = Command::new("git")
            .current_dir(&fx.repo)
            .args(["remote", "remove", "origin"])
            .output();
        if let Some(url) = origin {
            git_in(&fx.repo, &["remote", "add", "origin", url], None);
        }
        let before = fx.snapshot();
        for extra in [&["--dry-run"][..], &[][..]] {
            let out = fx.reconcile(extra);
            let all = text(&out);
            assert!(!out.status.success(), "{provider}: {all}");
            assert!(
                stderr(&out).contains(want),
                "{provider} wants `{want}`:\n{all}"
            );
            assert_same(&fx.snapshot(), &before, &format!("{provider} {extra:?}"));
        }
    };
    refuse(
        "gitlab",
        Some("https://gitlab.com/acme/reconcile-fixture.git"),
        "the configured forge `gitlab` has no supported open-change query",
    );
    refuse(
        "bitbucket",
        None,
        "unrecognized forge provider \"bitbucket\"",
    );

    // Stale pure-git config + GitHub origin: GitHub, config bytes untouched.
    fx.set_provider("pure-git");
    let _ = Command::new("git")
        .current_dir(&fx.repo)
        .args(["remote", "remove", "origin"])
        .output();
    git_in(
        &fx.repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/reconcile-fixture.git",
        ],
        None,
    );
    fx.mode("search-TASK-95", "exit");
    let config = std::fs::read_to_string(fx.config_path()).unwrap();
    let out = fx.reconcile(&["--dry-run"]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(stderr(&out).contains("HTTP 401"), "{}", text(&out));
    assert_eq!(std::fs::read_to_string(fx.config_path()).unwrap(), config);
    assert!(!stderr(&out).contains("updated stale"), "{}", text(&out));

    // A non-forge origin (local bare remote) is affirmative pure git.
    let bare = fx.root.join("bare.git");
    git_in(
        &fx.root,
        &["init", "-q", "--bare", bare.to_str().unwrap()],
        None,
    );
    git_in(
        &fx.repo,
        &["remote", "set-url", "origin", bare.to_str().unwrap()],
        None,
    );
    let calls = fx.gh_log();
    let out = fx.reconcile(&[]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(fx.status("TASK-95"), "Completed");
    assert_eq!(fx.gh_log(), calls, "pure git queried a forge");
    fx.assert_no_tripwire();
}

/// F1335-3: every valid TOML spelling of a configured forge is honoured by
/// the read-only classifier, and a value it cannot interpret refuses — a
/// configured forge never silently becomes pure git. Required check (normal
/// and dry-run) and the diagnostic-only route, config bytes unchanged.
#[test]
fn configured_forge_is_read_from_any_toml_spelling() {
    let fx = Fx::new(Forge::PureGit);
    fx.seed("TASK-96", "TASK-96", "Done", "");
    fx.seed("TASK-98", "TASK-98", "Completed", "");
    fx.commit("fix: shipped (TASK-96) (TASK-98)", "2026-09-10T00:00:00Z");
    let scaffolded = std::fs::read_to_string(fx.config_path()).unwrap();
    let cases: [(&str, &str); 5] = [
        (
            "forge = { provider = \"gitlab\" }",
            "the configured forge `gitlab` has no supported open-change query",
        ),
        (
            "forge.provider = \"gitlab\"",
            "the configured forge `gitlab` has no supported open-change query",
        ),
        (
            "forge = { provider = 7 }",
            "`[forge] provider` is not a string",
        ),
        ("forge = \"gitlab\"", "`forge` is not a table"),
        (
            "forge = { provider = \"svn\" }",
            "unrecognized forge provider \"svn\"",
        ),
    ];
    for (line, want) in cases {
        std::fs::write(fx.config_path(), &scaffolded).unwrap();
        fx.rewrite_forge(line);
        let before = fx.snapshot();
        for extra in [&["--dry-run"][..], &[][..]] {
            let out = fx.reconcile(extra);
            let all = text(&out);
            assert!(!out.status.success(), "{line} {extra:?}:\n{all}");
            assert!(stderr(&out).contains(want), "{line} wants `{want}`:\n{all}");
            assert!(stderr(&out).contains("TASK-96: commit"), "{all}");
            assert_same(&fx.snapshot(), &before, line);
        }
        // Diagnostic-only route: the Completed spec alone is a warning.
        let out = fx.reconcile(&["--spec", "TASK-98"]);
        assert!(out.status.success(), "{line}:\n{}", text(&out));
        assert!(
            stdout(&out).contains("unknown (open-PR diagnostic unavailable)"),
            "{line}:\n{}",
            text(&out)
        );
        assert!(stderr(&out).contains(want), "{line}:\n{}", text(&out));
        assert_same(&fx.snapshot(), &before, line);
    }
    // Positives: a dotted pure-git provider, and a document with no provider
    // and no origin, are affirmative pure git.
    for line in ["forge.provider = \"pure-git\"", "# no forge configured"] {
        std::fs::write(fx.config_path(), &scaffolded).unwrap();
        fx.rewrite_forge(line);
        let out = fx.reconcile(&["--dry-run"]);
        assert!(out.status.success(), "{line}:\n{}", text(&out));
        assert!(
            stdout(&out).contains("would flip 1 spec → Completed"),
            "{line}:\n{}",
            text(&out)
        );
        assert!(stdout(&out).contains("0 Completed specs"), "{}", text(&out));
    }
    assert!(!fx.gh_log().contains("pr list"), "{}", fx.gh_log());
    fx.assert_no_tripwire();
}

/// F1335-1: covers support must be rooted in a live Completed requirement or
/// a surviving non-covers completion. A cycle whose real anchor disappears,
/// and a chain whose only anchor is reopened mid-run, stay Done; an anchored
/// story and an independently justified one still complete.
#[test]
fn covers_support_is_rooted_not_circular() {
    let fx = Fx::new(Forge::GitHub);
    let anchor = fx.seed("TASK-41", "TASK-41", "Completed", "");
    let kept = fx.seed("TASK-42", "TASK-42", "Completed", "");
    let pending = fx.seed("TASK-43", "TASK-43", "Done", ""); // reopened mid-run
    let fresh = fx.seed("TASK-44", "TASK-44", "Done", ""); // untouched normal flip
    let s81 = uuid::Uuid::new_v4().to_string();
    let s82 = uuid::Uuid::new_v4().to_string();
    let implements = |targets: &[&str]| {
        let mut out = String::from("relationships:\n");
        for t in targets {
            out.push_str(&format!("- rel_type: implements\n  target_id: {t}\n"));
        }
        out
    };
    // The cycle: each story implements the anchor and the other story.
    let seed_with_id = |id: &str, uuid: &str, extra: &str| {
        let path = fx.object_path(id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!(
                "id: {uuid}\nspec_id: {id}\nagreed_id: {id}\ntitle: fixture {id}\n\
                 description: fixture\nstatus: Done\npriority: Medium\nowner: ''\n\
                 feature: Uncategorized\ncreated_at: 2026-09-01T00:00:00Z\n\
                 modified_at: 2026-09-01T00:00:00Z\nreq_type: Story\n{extra}"
            ),
        )
        .unwrap();
        git_in(&fx.store(), &["add", "-A"], None);
        git_in(&fx.store(), &["commit", "-qm", &format!("seed {id}")], None);
    };
    seed_with_id("STORY-81", &s81, &implements(&[&anchor, &s82]));
    seed_with_id("STORY-82", &s82, &implements(&[&anchor, &s81]));
    fx.seed("STORY-83", "STORY-83", "Done", &implements(&[&pending]));
    fx.seed("STORY-84", "STORY-84", "Done", &implements(&[&kept, &s81]));
    fx.seed("STORY-85", "STORY-85", "Done", &implements(&[&fresh]));
    for (id, pr) in [
        ("STORY-81", 981),
        ("STORY-82", 982),
        ("STORY-83", 983),
        ("STORY-84", 984),
        ("STORY-85", 985),
    ] {
        fx.set_title(id, &format!("Review PR-{pr}: fixture"));
    }
    fx.commit("fix: lands (TASK-43) (TASK-44)", "2026-09-10T00:00:00Z");
    fx.barrier("STORY-81", "rel remove STORY-81 TASK-41 --type implements");
    fx.barrier("STORY-82", "rel remove STORY-82 TASK-41 --type implements");
    fx.barrier("TASK-43", "edit TASK-43 --status approved");

    let out = fx.reconcile(&[]);
    let all = text(&out);
    assert!(out.status.success(), "{all}");
    let err = stderr(&out);
    for id in ["STORY-81", "STORY-82", "STORY-83"] {
        assert_eq!(
            fx.status(id),
            "Done",
            "{id} completed without a rooted anchor:\n{all}"
        );
        let line = err
            .lines()
            .find(|l| l.contains(&format!("↷ {id}: ")))
            .unwrap_or_default();
        assert!(
            line.contains("covers chain") && line.contains("(no covered spec is still completed)"),
            "{id}:\n{all}"
        );
        assert!(!fx.object(id).contains("author: aida-reconcile"), "{id}");
        assert_eq!(fx.completion_events(id), 0, "{id}");
    }
    for id in ["STORY-84", "STORY-85", "TASK-44"] {
        assert_eq!(fx.status(id), "Completed", "{id}:\n{all}");
        assert_eq!(fx.completion_events(id), 1, "{id}:\n{all}");
        assert_eq!(fx.object(id).matches("author: aida-reconcile").count(), 1);
    }
    assert!(stdout(&out).contains("3 specs"), "applied count:\n{all}");
    fx.assert_no_tripwire();
}

/// F1335-2: the independent stale-review path is decided at the same live
/// seam — a blocker added mid-run holds it, a status change or object
/// replacement refuses it — while untouched stories still complete.
#[test]
fn stale_review_path_is_decided_at_the_live_seam() {
    let fx = Fx::new(Forge::GitHub);
    fx.seed("TASK-10", "TASK-10", "Done", "");
    fx.seed("TASK-12", "TASK-12", "InProgress", "");
    for (id, status, pr) in [
        ("STORY-20", "Approved", 901),   // new blocker mid-run
        ("STORY-21", "Approved", 902),   // untouched control
        ("STORY-22", "InProgress", 903), // rejected mid-run
        ("STORY-23", "Approved", 904),   // object replaced mid-run
    ] {
        fx.seed(id, id, status, "");
        fx.set_title(id, &format!("Review PR-{pr}: fixture"));
    }
    for pr in [901, 902, 903, 904] {
        fx.commit(
            &format!("fix: merged review {pr} (TASK-10) (#{pr})"),
            "2026-09-10T00:00:00Z",
        );
    }
    fx.barrier_script(
        "TASK-10",
        "\"$FIXTURE_AIDA\" rel add STORY-20 TASK-12 --type blocked-by && \
         \"$FIXTURE_AIDA\" edit STORY-22 --status rejected && \
         sed -i 's/^id: .*/id: 01a11c00-0000-7000-8000-0000000000cc/' \
         \"$FIXTURE_STORE/objects/STORY/000/STORY-23.yaml\" && \
         git -C \"$FIXTURE_STORE\" commit -qam 'peer replaces STORY-23'\n",
    );
    let out = fx.reconcile(&[]);
    let all = text(&out);
    assert!(out.status.success(), "{all}");
    assert!(!std::fs::read_dir(&fx.state)
        .unwrap()
        .any(|e| e.unwrap().file_name() == "barrier-TASK-10"));
    let err = stderr(&out);
    assert_eq!(fx.status("TASK-10"), "Completed", "{all}");
    assert_eq!(fx.status("STORY-21"), "Completed", "{all}");
    assert_eq!(fx.status("STORY-20"), "Approved", "{all}");
    assert_eq!(fx.status("STORY-22"), "Rejected", "{all}");
    assert_eq!(fx.status("STORY-23"), "Approved", "{all}");
    assert!(
        err.contains("STORY-20: commit") && err.contains("but its completion is now held"),
        "{all}"
    );
    assert!(
        err.contains("STORY-22: commit") && err.contains("(its status changed to Rejected)"),
        "{all}"
    );
    assert!(
        err.contains("STORY-23: commit")
            && err.contains("(its ID now resolves to a different requirement)"),
        "{all}"
    );
    assert!(stdout(&out).contains("1 review story"), "{all}");
    for id in ["STORY-20", "STORY-22", "STORY-23"] {
        assert_eq!(fx.completion_events(id), 0, "{id}");
        assert!(!fx.object(id).contains("author: aida-reconcile"), "{id}");
    }
    assert_eq!(fx.completion_events("STORY-21"), 1, "{all}");
    fx.assert_no_tripwire();
}

/// F1335-5: with a required outage, a review story a PEER completed during
/// the run is not counted, reported or emitted as this run's work; a stale
/// story this run genuinely completed is.
#[test]
fn independent_effects_report_only_this_runs_writes() {
    let fx = Fx::new(Forge::GitHub);
    fx.seed("TASK-30", "TASK-30", "Done", "");
    fx.seed("STORY-30", "STORY-30", "Approved", "");
    fx.set_title("STORY-30", "Review PR-910: fixture");
    fx.commit("fix: merged (TASK-30) (#910)", "2026-09-10T00:00:00Z");
    fx.barrier("TASK-30", "edit STORY-30 --status completed");
    fx.mode("search-TASK-30", "garbage");
    let out = fx.reconcile(&[]);
    let all = text(&out);
    assert!(!out.status.success(), "{all}");
    assert!(stderr(&out).contains("and nothing else changed"), "{all}");
    assert!(!all.contains("auto-completed"), "{all}");
    assert!(!all.contains("WERE applied"), "{all}");
    assert_eq!(fx.status("STORY-30"), "Completed");
    assert!(!fx.object("STORY-30").contains("author: aida-reconcile"));
    assert_eq!(fx.completion_events("STORY-30"), 0);
    assert_eq!(fx.status("TASK-30"), "Done");

    // Genuine: this run completes the stale story despite the outage.
    fx.seed("TASK-31", "TASK-31", "Done", "");
    fx.seed("STORY-31", "STORY-31", "Approved", "");
    fx.set_title("STORY-31", "Review PR-911: fixture");
    fx.commit("fix: merged (TASK-31) (#911)", "2026-09-11T00:00:00Z");
    fx.mode("search-TASK-31", "garbage");
    let out = fx.reconcile(&[]);
    let all = text(&out);
    assert!(!out.status.success(), "{all}");
    assert!(
        stderr(&out).contains("other independent changes WERE applied: 0 Draft specs landed at Done, 1 review story completed"),
        "{all}"
    );
    assert_eq!(fx.status("STORY-31"), "Completed");
    assert_eq!(
        fx.object("STORY-31")
            .matches("author: aida-reconcile")
            .count(),
        1
    );
    assert_eq!(fx.completion_events("STORY-31"), 1, "{all}");
    assert_eq!(fx.completion_events("STORY-30"), 0);
    fx.assert_no_tripwire();
}

/// F1335-6: two accepted ids of one requirement complete it ONCE — one
/// applied decision, one processing record, one event — whether they share
/// a subject or arrive in separate commits; reruns stay idempotent.
#[test]
fn aliases_of_one_requirement_complete_once() {
    let fx = Fx::new(Forge::PureGit);
    fx.seed("TASK-1-40", "TASK-1540", "Done", "");
    fx.seed("TASK-1-41", "TASK-1541", "Done", "");
    let first = fx.commit("fix: display id (TASK-1540)", "2026-09-10T00:00:00Z");
    let second = fx.commit("fix: origin id (TASK-1-40)", "2026-09-11T00:00:00Z");
    fx.commit(
        "fix: both forms (TASK-1-41) (TASK-1541)",
        "2026-09-12T00:00:00Z",
    );
    let out = fx.reconcile(&[]);
    let all = text(&out);
    assert!(out.status.success(), "{all}");
    assert!(stdout(&out).contains("2 specs"), "applied count:\n{all}");
    assert!(!stdout(&out).contains("4 specs"), "{all}");
    for id in ["TASK-1-40", "TASK-1-41"] {
        let object = fx.object(id);
        assert_eq!(fx.status(id), "Completed", "{all}");
        assert_eq!(object.matches("Completed via merge").count(), 1, "{object}");
        assert_eq!(
            object.matches("author: aida-reconcile").count(),
            1,
            "{object}"
        );
    }
    // The first accepted id in scan order supplies the completion evidence;
    // the other alias is reported, not re-applied.
    let object = fx.object("TASK-1-40");
    assert!(
        object.contains(&format!("completion_sha: {second}"))
            || object.contains(&format!("completion_sha: {first}")),
        "{object}"
    );
    let events: usize = ["TASK-1540", "TASK-1-40"]
        .iter()
        .map(|id| fx.completion_events(id))
        .sum();
    assert_eq!(events, 1, "{all}");
    let out = fx.reconcile(&["--spec", "TASK-1540", "--dry-run"]);
    assert!(out.status.success(), "{}", text(&out));
    let settled = fx.snapshot();
    let out = fx.reconcile(&[]);
    assert!(out.status.success(), "{}", text(&out));
    assert_same(&fx.snapshot(), &settled, "alias rerun");
    fx.assert_no_tripwire();
}

/// Closed-but-unmerged: a PR whose commit never reached the default branch
/// earns nothing — not for its spec, not for its review story — even when
/// the forge reports no open PR.
#[test]
fn closed_unmerged_work_earns_no_credit() {
    let fx = Fx::new(Forge::GitHub);
    fx.seed("TASK-97", "TASK-97", "Done", "");
    fx.seed("STORY-97", "STORY-97", "Approved", "");
    fx.set_title("STORY-97", "Review PR-950: fixture");
    git_in(&fx.repo, &["checkout", "-q", "-b", "closed-pr"], None);
    fx.commit("fix: never merged (TASK-97) (#950)", "2026-09-10T00:00:00Z");
    git_in(&fx.repo, &["checkout", "-q", "main"], None);
    for extra in [&["--dry-run"][..], &[][..], &["--spec", "TASK-97"][..]] {
        let out = fx.reconcile(extra);
        assert!(out.status.success(), "{extra:?}:\n{}", text(&out));
        assert!(!stdout(&out).contains("would flip"), "{}", text(&out));
    }
    assert_eq!(fx.status("TASK-97"), "Done");
    assert_eq!(fx.status("STORY-97"), "Approved");
    fx.assert_no_tripwire();
}
