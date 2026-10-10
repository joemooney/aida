#![cfg(target_os = "linux")]
//! Linux process fixtures: actual kernel PID/start identity, no simulated age waits.
// trace:BUG-1683 | ai:codex
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

// STORY-1473 / ADR-66: env roles confer no authority; the fixture rides a
// validated advisor seat grant (delegating the launch seats) instead.
mod support;

struct Holder(Child);
impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn script(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}
struct Fixture {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    sibling: PathBuf,
    home: PathBuf,
    bin: PathBuf,
    sentinel: PathBuf,
    spec: String,
}
impl Fixture {
    fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        let mut cmd = Command::new("timeout");
        cmd.args(["--kill-after=5", "30", env!("CARGO_BIN_EXE_aida")])
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("HOME", &self.home)
            .env("TMPDIR", self._tmp.path())
            .env("AIDA_USER", "fixture")
            .env("AIDA_SESSION_ROLE", "advisor");
        // ADR-66: the role env is only a hint; the advisor-gated steps ride a
        // validated seat grant delegating the launchable seats (set after
        // env_clear so it survives). trace:STORY-1473 | ai:claude
        if let Some(grant) = support::ensure_seat_for(
            &self.home,
            &self.repo,
            "fixture",
            "advisor",
            &["implementer", "reviewer"],
        ) {
            cmd.env("AIDA_SESSION_GRANT", grant);
        }
        cmd.env("AIDA_HEADLESS", "1")
            .env("AIDA_NO_HUMAN_ACKNOWLEDGED", "1")
            .env("AIDA_TELEMETRY", "0")
            .env("AIDA_DRAIN_LOCK_STALE_SECS", "0")
            .env("AIDA_OUTPUT_FORMAT", "human")
            .env("SENTINEL", &self.sentinel)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(&self.repo, args);
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.status.success(), "{args:?}: {text}");
        text
    }
    fn new() -> Self {
        let tmp = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let mut f = Self {
            _tmp: tmp,
            repo: root.join("repo"),
            sibling: root.join("sibling"),
            home: root.join("home"),
            bin: root.join("bin"),
            sentinel: root.join("launched"),
            spec: String::new(),
        };
        for p in [&f.repo, &f.home, &f.bin] {
            std::fs::create_dir_all(p).unwrap();
        }
        git(&f.repo, &["init", "-q", "-b", "main"]);
        git(&f.repo, &["config", "user.email", "fixture@example.com"]);
        git(&f.repo, &["config", "user.name", "Lock fixture"]);
        git(&f.repo, &["commit", "-q", "--allow-empty", "-m", "fixture"]);
        f.ok(&[
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ]);
        let added = f.ok(&[
            "add",
            "--type",
            "task",
            "--status",
            "Approved",
            "--title",
            "bounded fixture",
            "--tags",
            "batch:fixture,batch:second",
            "--description",
            "Implement a bounded fixture.\n\n## Acceptance\n- Targeted test passes.",
        ]);
        f.spec = added
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
            .find(|s| s.starts_with("TASK-") && s[5..].chars().all(|c| c.is_ascii_digit()))
            .expect("spec id")
            .into();
        f.ok(&["queue", "add", &f.spec, "--for", "implementer"]);
        git(
            &f.repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "sibling",
                f.sibling.to_str().unwrap(),
            ],
        );
        // Burndown's queue union scans <cwd-root>/.aida-store directly. Attach
        // the shared spec store so its sibling preflight sees the queued task.
        // Runtime .aida directories/locks remain separate, exposing the old
        // queue-integrate root bypass rather than masking it with a lock link.
        std::os::unix::fs::symlink(f.repo.join(".aida-store"), f.sibling.join(".aida-store"))
            .unwrap();
        // Arm launch sentinels after init's capability/MCP setup probes. All
        // commands under test run with these stubs; no real agents are on PATH.
        for worker in ["claude", "codex", "agy", "gh", "glab"] {
            script(
                &f.bin.join(worker),
                "#!/bin/sh\nif [ \"$#\" -eq 1 ] && [ \"$1\" = --version ]; then echo fixture-1.0.0; exit 0; fi\necho worker:$0:$* >> \"$SENTINEL\"\nexit 93\n",
            );
        }
        script(&f.bin.join("git"), "#!/bin/sh\ncase \" $* \" in *' merge '*|*' rebase '*|*' push '*) echo merge:$* >> \"$SENTINEL\"; exit 94;; esac\nexec /usr/bin/git \"$@\"\n");
        // No origin: shared acquisition must report unavailable, never mask the local gate.
        assert!(!Command::new("git")
            .current_dir(f.repo.join(".aida-store"))
            .args(["remote", "get-url", "origin"])
            .output()
            .unwrap()
            .status
            .success());
        f
    }
}

#[test]
fn two_hour_live_lock_blocks_all_launch_entry_points() {
    let f = Fixture::new();
    let mut holder = Holder(
        Command::new("sleep")
            .arg("300")
            .stdin(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let pid = holder.0.id();
    let identity = aida_core::liveness::process_start_identity(pid)
        .expect("Linux kernel start identity required");
    assert!(aida_core::liveness::process_identity_is_alive(
        pid,
        Some(&identity)
    ));
    let date = Command::new("date")
        .args(["-u", "-d", "2 hours ago", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .unwrap();
    assert!(date.status.success());
    let bytes = serde_json::to_vec(&serde_json::json!({"pid":pid,"pid_start_time":identity,"started_at_utc":String::from_utf8(date.stdout).unwrap().trim(),"command":"BUG-1683 independent fixture holder","host":"fixture"})).unwrap();
    let lock = f.repo.join(".aida/drain.lock");
    std::fs::write(&lock, &bytes).unwrap();
    let routes: Vec<Vec<&str>> = vec![
        vec!["queue", "work", &f.spec, "--auto-complete"],
        vec!["queue", "work", "--batch", "fixture", "--auto-complete"],
        vec![
            "queue",
            "work",
            "--batches",
            "fixture,second",
            "--auto-complete",
        ],
        vec!["queue", "work", "next2", "--auto-complete"],
        vec!["queue", "work", "--drain"],
        vec!["queue", "work", "PR-123", "--from-pr", "--auto-complete"],
        vec!["queue", "work", "--resume-drain", "--auto-complete"],
        vec!["burndown", "run", "--concurrency", "1", "--max", "1"],
        vec!["drain", "start", &f.spec],
        vec!["queue", "integrate"],
        vec!["queue", "integrate", "--watch"],
        vec!["integrate", "--run"],
        vec!["integrate", "--watch"],
    ];
    for cwd in [&f.repo, &f.sibling] {
        for args in &routes {
            let out = f.run(cwd, args);
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(
                !out.status.success(),
                "{cwd:?} {args:?}: unexpected success: {text}"
            );
            assert!(
                text.contains(&format!("a drain is already running (pid {pid},"))
                    && text.contains("BUG-1683 independent fixture holder"),
                "{cwd:?} {args:?}: missed holder gate: {text}"
            );
            if args[0] != "drain" {
                assert!(
                    text.contains("cross-clone coordination unavailable"),
                    "must exercise local-only gate: {text}"
                );
            }
            assert_eq!(std::fs::read(&lock).unwrap(), bytes, "{args:?}");
            assert!(
                !f.sentinel.exists(),
                "worker/merge sentinel for {args:?}: {}",
                std::fs::read_to_string(&f.sentinel).unwrap_or_default()
            );
            assert!(!f.sibling.join(".aida/drain.lock").exists());
            assert!(holder.0.try_wait().unwrap().is_none());
        }
    }
    let out = f.run(&f.repo, &["autoprogress"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("a drain is already running — skipping"));
    assert_eq!(std::fs::read(lock).unwrap(), bytes);
    assert!(!f.sentinel.exists());
}

/// Binding alias evidence: inspect actual dispatch bodies, paired with the
/// process fixtures at their independent queue-work/integrate boundaries.
// trace:BUG-1683 | ai:codex
#[test]
fn launch_aliases_delegate_to_the_tested_lock_boundaries() {
    fn body<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
        source
            .split_once(start)
            .unwrap()
            .1
            .split_once(end)
            .unwrap()
            .0
    }
    fn ordered(source: &str, needles: &[&str]) {
        let mut remaining = source;
        for &needle in needles {
            remaining = remaining
                .split_once(needle)
                .unwrap_or_else(|| panic!("missing or out-of-order route: {needle}"))
                .1;
        }
    }
    let mut lib = include_str!("../../aida-cli-lib/src/lib.rs").to_string();
    lib.push_str(include_str!("../../aida-cli-lib/src/lib_part1.rs"));
    lib.push_str(include_str!("../../aida-cli-lib/src/lib_part2.rs"));
    lib.push_str(include_str!("../../aida-cli-lib/src/lib_part3.rs"));
    lib.push_str(include_str!("../../aida-cli-lib/src/lib_part4.rs"));
    lib.push_str(include_str!("../../aida-cli-lib/src/lib_part5.rs"));
    lib.push_str(include_str!("../../aida-cli-lib/src/lib_part6.rs"));
    let lib = &lib;
    let queue = include_str!("../../aida-cli-lib/src/queue_cmd.rs");
    let drain = include_str!("../../aida-cli-lib/src/drain_cmd.rs");
    let auto = include_str!("../../aida-cli-lib/src/autoprogress.rs");
    let shift = include_str!("../../aida-cli-lib/src/shift.rs");
    let zen = include_str!("../../aida-cli-lib/src/zen_drive.rs");
    ordered(
        body(
            lib,
            "fn handle_integrate(opts:",
            "use std::collections::HashSet;",
        ),
        &[
            "if opts.run || opts.watch || opts.dry_run",
            "queue_cmd::handle_queue_integrate(",
            "opts.dry_run",
            "opts.watch",
        ],
    );
    ordered(
        body(drain, "fn handle_shelved_resume(", "fn handle_drain_start("),
        &[
            "Command::new(exe)",
            "\"queue\"",
            "\"work\"",
            "\"--auto-complete\"",
            "\"--from-pr\"",
            // BUG-1735 routed this spawn through the ETXTBSY retry, and
            // rustfmt's chain_width then split the receiver onto its own line.
            // Both needles are kept so the guard still binds the terminal call
            // to *this* command rather than to any `.status*()` in the body.
            // trace:BUG-1735 | ai:claude
            "let status = cmd",
            ".status_retrying_etxtbsy()",
        ],
    );
    ordered(
        body(lib, "fn run_do_drive(", "fn run_zen_drive("),
        &[
            "match effective",
            "ExecutionMode::Drain =>",
            "self_invoke(&[\"zen\", &display]",
            "ExecutionMode::Drive =>",
            "\"--auto-complete=through-ci\"",
        ],
    );
    ordered(
        body(lib, "fn run_zen_drive(", "fn zen_file_thought_as_draft("),
        &[
            "zen_drive::drive_args(",
            "Command::new(&exe)",
            "cmd.args(&args)",
            // trace:BUG-1735 | ai:claude — see the note above.
            "let status = cmd",
            ".status_retrying_etxtbsy()",
        ],
    );
    ordered(
        body(zen, "pub(crate) fn drive_args(", "///"),
        &["\"queue\"", "\"work\"", "\"--auto-complete\""],
    );
    ordered(
        body(
            auto,
            "pub(crate) fn handle_autoprogress(",
            "pub(crate) fn resolve_project(",
        ),
        &[
            "drain_lock::probe_lock(&root)",
            "return Ok(())",
            "match run_aida(",
            "\"queue\"",
            "\"work\"",
            "\"--auto-complete\"",
        ],
    );
    ordered(
        body(
            shift,
            "pub(crate) fn build_wave_argv(",
            "pub(crate) fn wave_max_tokens(",
        ),
        &[
            "\"queue\"",
            "\"work\"",
            "\"--batch\"",
            "\"--auto-complete\"",
        ],
    );
    ordered(
        shift,
        &[
            "report.argv = build_wave_argv(",
            "if live && all_pass",
            "exec.spawn_wave(&report.argv",
        ],
    );
    assert!(shift.contains("wave_command(&exe, argv, &project_root, log)?"));
    ordered(
        body(
            queue,
            "let _drain_guard = if *resume_dry_run",
            "// TASK-1003 / SPIKE-70",
        ),
        &[
            "find_main_worktree_root()?",
            "drain_lock::acquire_drain_lock(",
            "if *resume_drain",
            "handle_drain_resume(",
        ],
    );
}
