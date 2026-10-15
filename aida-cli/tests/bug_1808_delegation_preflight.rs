// Binary-driving e2e suite; Linux-only like its siblings (BUG-688).
#![cfg(target_os = "linux")]
//! BUG-1808: `aida queue work` validates that the launcher seat may delegate
//! the routed child seat BEFORE any pickup mutation, on every launch arm that
//! mints a child grant — including `--no-human` resume, which used to skip the
//! preflight and refuse only after the status bump, queue row, worktree,
//! lease and manifest had landed. A refusal must leave state byte-identical.
// trace:BUG-1808 | ai:claude

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

mod support;

const RESUME_ID: &str = "0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b";

fn aida(repo: &Path, home: &Path, bin_dir: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(repo);
    cmd.env("HOME", home);
    cmd.env("AIDA_HOME", home);
    cmd.env("AIDA_TELEMETRY", "0");
    let path = std::env::join_paths(
        std::iter::once(bin_dir.to_path_buf())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    cmd.env("PATH", path);
    for key in [
        "AIDA_SESSION_ROLE",
        "AIDA_SESSION_GRANT",
        "AIDA_PERMISSION_MODE",
        "AIDA_AGENT_OUTPUT",
        "AIDA_HEADLESS_VENDOR",
        "AIDA_AGENT_MODEL",
        "AIDA_AGENT_CMD",
        "AIDA_REVIEW_VERDICT_FILE",
        "AIDA_AGENT_NAME",
        "AIDA_AGENT_TYPE",
        "AIDA_SESSION_ID",
    ] {
        cmd.env_remove(key);
    }
    cmd
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    repo: PathBuf,
    home: PathBuf,
    bin: PathBuf,
    spec: String,
}

/// A Claude-enabled project with one approved spec queued for `for_role`.
/// The fake `claude` on PATH never runs: every refusal here precedes launch.
fn fixture(for_role: &str) -> Fixture {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let repo = base.join("repo");
    let home = base.join("home");
    let bin = base.join("bin");
    for dir in [&repo, &home, &bin] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let claude = bin.join("claude");
    std::fs::write(&claude, "#!/bin/sh\nexit 97\n").unwrap();
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Hermetic forge: PR metadata lookups fail fast instead of reaching GitHub.
    let gh = bin.join("gh");
    std::fs::write(&gh, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();

    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@t.t"]);
    git(&repo, &["config", "user.name", "t"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let init = aida(&repo, &home, &bin)
        .args([
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ])
        .output()
        .unwrap();
    assert!(init.status.success(), "{init:?}");
    std::fs::write(
        repo.join(".aida/config.toml"),
        "[agents]\nenabled = [\"claude\"]\n",
    )
    .unwrap();

    let advisor = support::ensure_seat(&home, &repo, "advisor", &["implementer", "reviewer"])
        .expect("initialized");
    let add = aida(&repo, &home, &bin)
        .env("AIDA_SESSION_ROLE", "advisor")
        .env("AIDA_SESSION_GRANT", &advisor)
        .args([
            "add",
            "--type",
            "bug",
            "--status",
            "approved",
            "--title",
            "delegation preflight",
        ])
        .output()
        .unwrap();
    assert!(add.status.success(), "{add:?}");
    let stdout = String::from_utf8_lossy(&add.stdout);
    let spec = stdout
        .split(|c: char| c.is_whitespace() || c == ',')
        .find(|t| {
            t.split_once('-').is_some_and(|(p, n)| {
                !p.is_empty()
                    && p.chars().all(|c| c.is_ascii_uppercase())
                    && !n.is_empty()
                    && n.chars().all(|c| c.is_ascii_digit())
            })
        })
        .unwrap_or_else(|| panic!("no spec id in {stdout}"))
        .to_string();
    let queue = aida(&repo, &home, &bin)
        .env("AIDA_SESSION_ROLE", "advisor")
        .env("AIDA_SESSION_GRANT", &advisor)
        .args(["queue", "add", &spec, "--for", for_role])
        .output()
        .unwrap();
    assert!(queue.status.success(), "{queue:?}");
    // Settle the per-command bookkeeping every invocation writes (the derived
    // cache and the once-per-version notice marker) so the snapshots compare
    // pickup state only, byte for byte.
    let settle = aida(&repo, &home, &bin)
        .args(["queue", "list", "--all"])
        .output()
        .unwrap();
    assert!(settle.status.success(), "{settle:?}");
    Fixture {
        _tmp: tmp,
        base,
        repo,
        home,
        bin,
        spec,
    }
}

/// Every file under the fixture (outside git's object database, whose
/// externally visible effects — refs and worktrees — are captured separately),
/// plus refs and the worktree list. Byte-identical before and after a refusal
/// means no status, queue, branch, worktree, lease, manifest or grant write.
fn snapshot(fx: &Fixture) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let ft = entry.file_type().unwrap();
            if ft.is_dir() {
                if entry.file_name() == ".git" {
                    continue;
                }
                walk(root, &path, out);
            } else if ft.is_file() {
                let rel = path.strip_prefix(root).unwrap().display().to_string();
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(&fx.base, &fx.base, &mut out);
    out.insert(
        "<git refs>".into(),
        git(&fx.repo, &["for-each-ref"]).into_bytes(),
    );
    out.insert(
        "<git worktrees>".into(),
        git(&fx.repo, &["worktree", "list", "--porcelain"]).into_bytes(),
    );
    out
}

fn assert_unchanged(before: &BTreeMap<String, Vec<u8>>, after: &BTreeMap<String, Vec<u8>>) {
    let changed: Vec<&String> = before
        .keys()
        .chain(after.keys())
        .filter(|k| before.get(*k) != after.get(*k))
        .collect();
    assert!(changed.is_empty(), "refusal mutated state: {changed:?}");
}

fn run(fx: &Fixture, grant: &str, args: &[&str]) -> (bool, String) {
    let out = aida(&fx.repo, &fx.home, &fx.bin)
        .env("AIDA_SESSION_GRANT", grant)
        .args(args)
        .output()
        .unwrap();
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

#[test]
fn implementer_launching_implementer_routed_headless_resume_is_refused_without_mutation() {
    let fx = fixture("implementer");
    let grant = support::grant_seat(&fx.home, &fx.repo, "implementer", &[]);
    let before = snapshot(&fx);
    let (ok, text) = run(
        &fx,
        &grant,
        &[
            "queue",
            "work",
            &fx.spec,
            "--no-human",
            "--resume",
            RESUME_ID,
            "--no-pull",
        ],
    );
    assert!(!ok, "{text}");
    assert!(text.contains("child-seat delegation refused"), "{text}");
    assert!(text.contains("does not delegate `implementer`"), "{text}");
    assert!(
        text.contains("queue selection succeeded (cluster-derived chose role `implementer`)"),
        "{text}"
    );
    assert_unchanged(&before, &snapshot(&fx));
}

#[test]
fn implementer_launching_implementer_routed_interactive_work_is_refused_without_mutation() {
    let fx = fixture("implementer");
    let grant = support::grant_seat(&fx.home, &fx.repo, "implementer", &[]);
    let before = snapshot(&fx);
    let (ok, text) = run(
        &fx,
        &grant,
        &["queue", "work", &fx.spec, "--fresh", "--no-pull"],
    );
    assert!(!ok, "{text}");
    assert!(text.contains("does not delegate `implementer`"), "{text}");
    assert!(
        !text.contains("explicit role selector"),
        "implementer gets no selector: {text}"
    );
    assert_unchanged(&before, &snapshot(&fx));
}

#[test]
fn advisor_without_the_routed_seat_is_told_to_use_an_explicit_role() {
    let fx = fixture("implementer");
    let grant = support::grant_seat(&fx.home, &fx.repo, "advisor", &["reviewer"]);
    let before = snapshot(&fx);
    let (ok, text) = run(
        &fx,
        &grant,
        &[
            "queue",
            "work",
            &fx.spec,
            "--no-human",
            "--resume",
            RESUME_ID,
            "--no-pull",
        ],
    );
    assert!(!ok, "{text}");
    assert!(text.contains("does not delegate `implementer`"), "{text}");
    assert!(text.contains("`--role reviewer`"), "{text}");
    assert_unchanged(&before, &snapshot(&fx));
}

// trace:BUG-1808 | ai:claude
// POSITIVE control: a grant that delegates the routed seat passes the new
// headless-resume preflight and proceeds to setup and launch, so the check
// does not refuse a legitimate resume. The fake `claude` exits 97, which
// proves the launch was reached.
#[test]
fn delegating_grant_passes_headless_resume_preflight_and_launches() {
    let fx = fixture("implementer");
    let grant = support::grant_seat(&fx.home, &fx.repo, "advisor", &["implementer"]);
    let out = aida(&fx.repo, &fx.home, &fx.bin)
        .env("AIDA_SESSION_GRANT", &grant)
        .args([
            "queue",
            "work",
            &fx.spec,
            "--no-human",
            "--resume",
            RESUME_ID,
            "--no-pull",
        ])
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!text.contains("child-seat delegation refused"), "{text}");
    assert!(!text.contains("does not delegate"), "{text}");
    assert_eq!(
        out.status.code(),
        Some(97),
        "launch must be reached: {text}"
    );
    assert!(
        std::fs::read_dir(fx.repo.join(".aida/sessions"))
            .map(|mut d| d.next().is_some())
            .unwrap_or(false),
        "setup must have proceeded (lease minted): {text}"
    );
}

// trace:BUG-1808 | ai:claude
// A standalone `queue work PR-N --role reviewer --no-human` mints a `reviewer`
// child grant in `run_standalone_reviewer`; without a grant that delegates
// reviewer it must refuse before any store, queue, ref, worktree or manifest
// write.
#[test]
fn headless_standalone_reviewer_without_reviewer_delegation_is_refused_without_mutation() {
    let fx = fixture("implementer");
    // A review story ties PR-7 to the spec without any forge access.
    let advisor = support::ensure_seat(&fx.home, &fx.repo, "advisor", &["implementer", "reviewer"])
        .expect("initialized");
    let review = aida(&fx.repo, &fx.home, &fx.bin)
        .env("AIDA_SESSION_ROLE", "advisor")
        .env("AIDA_SESSION_GRANT", &advisor)
        .args([
            "add",
            "--type",
            "story",
            "--status",
            "approved",
            "--title",
            &format!("Review PR-7: delegation preflight ({})", fx.spec),
        ])
        .output()
        .unwrap();
    assert!(review.status.success(), "{review:?}");
    let review_out = String::from_utf8_lossy(&review.stdout);
    let review_id = review_out
        .split(|c: char| c.is_whitespace() || c == ',')
        .find(|t| t.starts_with("STORY-"))
        .unwrap_or_else(|| panic!("no story id in {review_out}"))
        .to_string();
    // Queue the review story so `PR-7` keeps its PR scope (standalone reviewer).
    let queued = aida(&fx.repo, &fx.home, &fx.bin)
        .env("AIDA_SESSION_ROLE", "advisor")
        .env("AIDA_SESSION_GRANT", &advisor)
        .args(["queue", "add", &review_id, "--for", "reviewer"])
        .output()
        .unwrap();
    assert!(queued.status.success(), "{queued:?}");
    let settle = aida(&fx.repo, &fx.home, &fx.bin)
        .args(["queue", "list", "--all"])
        .output()
        .unwrap();
    assert!(settle.status.success(), "{settle:?}");
    let grant = support::grant_seat(&fx.home, &fx.repo, "advisor", &["implementer"]);
    let before = snapshot(&fx);
    let (ok, text) = run(
        &fx,
        &grant,
        &[
            "queue",
            "work",
            "PR-7",
            "--role",
            "reviewer",
            "--no-human",
            "--fresh",
            "--no-pull",
        ],
    );
    assert!(!ok, "{text}");
    assert!(text.contains("child-seat delegation refused"), "{text}");
    assert!(text.contains("does not delegate `reviewer`"), "{text}");
    assert!(
        text.contains("queue selection succeeded (--role flag chose role `reviewer`)"),
        "{text}"
    );
    assert_unchanged(&before, &snapshot(&fx));
}
