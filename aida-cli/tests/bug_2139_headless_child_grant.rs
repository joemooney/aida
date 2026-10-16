// Binary-driving e2e suite; Linux-only like its siblings (BUG-688).
#![cfg(target_os = "linux")]
//! BUG-2139: every headless queue-work child runs under its OWN grant, scoped
//! to the routed seat with no delegable seats — never the launcher's grant.
//! Before the fix a fresh `--no-human` pickup (the orchestrator's implementer
//! and reviewer phases, and the contained launch seen in the STORY-1654 e2e)
//! exec'd the vendor with the launcher's `AIDA_SESSION_GRANT` inherited, so an
//! advisor running implementer work headless did it with advisor authority.
// trace:BUG-2139 | ai:claude

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

const RESUME_ID: &str = "0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b";

fn aida(fx: &Fixture) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(&fx.repo);
    cmd.env("HOME", &fx.home);
    cmd.env("AIDA_HOME", &fx.home);
    cmd.env("AIDA_TELEMETRY", "0");
    let path = std::env::join_paths(
        std::iter::once(fx.bin.clone())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    cmd.env("PATH", path);
    for key in [
        "AIDA_SESSION_ROLE",
        "AIDA_SESSION_GRANT",
        "AIDA_SESSION_PURPOSE",
        "AIDA_PERMISSION_MODE",
        "AIDA_AGENT_OUTPUT",
        "AIDA_HEADLESS_VENDOR",
        "AIDA_AGENT_MODEL",
        "AIDA_AGENT_CMD",
        "AIDA_REVIEW_VERDICT_FILE",
        "AIDA_AGENT_NAME",
        "AIDA_AGENT_TYPE",
        "AIDA_SESSION_ID",
        "AIDA_CONTAINED",
        "AIDA_AUTO_COMPLETE_PHASE",
    ] {
        cmd.env_remove(key);
    }
    // The fake vendor writes its environment here.
    cmd.env("BUG2139_ENV_DUMP", &fx.dump);
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
    dump: PathBuf,
    spec: String,
    advisor: String,
}

fn spec_id(stdout: &str, prefix: Option<&str>) -> String {
    stdout
        .split(|c: char| c.is_whitespace() || c == ',')
        .find(|t| {
            t.split_once('-').is_some_and(|(p, n)| {
                !p.is_empty()
                    && prefix.is_none_or(|want| p == want)
                    && p.chars().all(|c| c.is_ascii_uppercase())
                    && !n.is_empty()
                    && n.chars().all(|c| c.is_ascii_digit())
            })
        })
        .unwrap_or_else(|| panic!("no spec id in {stdout}"))
        .to_string()
}

/// A Claude-enabled project with one approved spec queued for the
/// implementer. The fake `claude` dumps its environment, then exits 97.
fn fixture() -> Fixture {
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
    std::fs::write(
        &claude,
        "#!/bin/sh\nenv -0 > \"$BUG2139_ENV_DUMP\"\nexit 97\n",
    )
    .unwrap();
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    let gh = bin.join("gh");
    std::fs::write(&gh, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();

    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@t.t"]);
    git(&repo, &["config", "user.name", "t"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let mut fx = Fixture {
        _tmp: tmp,
        dump: base.join("child-env"),
        base,
        repo,
        home,
        bin,
        spec: String::new(),
        advisor: String::new(),
    };
    let init = aida(&fx)
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
        fx.repo.join(".aida/config.toml"),
        "[agents]\nenabled = [\"claude\"]\n",
    )
    .unwrap();
    fx.advisor = support::ensure_seat(&fx.home, &fx.repo, "advisor", &["implementer", "reviewer"])
        .expect("initialized");
    let add = as_advisor(&fx)
        .args([
            "add",
            "--type",
            "bug",
            "--status",
            "approved",
            "--title",
            "headless child grant",
        ])
        .output()
        .unwrap();
    assert!(add.status.success(), "{add:?}");
    fx.spec = spec_id(&String::from_utf8_lossy(&add.stdout), None);
    let queue = as_advisor(&fx)
        .args(["queue", "add", &fx.spec, "--for", "implementer"])
        .output()
        .unwrap();
    assert!(queue.status.success(), "{queue:?}");
    settle(&fx);
    fx
}

fn as_advisor(fx: &Fixture) -> Command {
    let mut cmd = aida(fx);
    cmd.env("AIDA_SESSION_ROLE", "advisor")
        .env("AIDA_SESSION_GRANT", &fx.advisor);
    cmd
}

fn settle(fx: &Fixture) {
    let settle = aida(fx).args(["queue", "list", "--all"]).output().unwrap();
    assert!(settle.status.success(), "{settle:?}");
}

/// Queue a `Review PR-7` story so `queue work PR-7` keeps its PR scope, and
/// give the repo a local `origin` carrying `refs/pull/7/head` so the review
/// checkout resolves without a forge.
fn queue_review_story(fx: &Fixture) {
    let origin = fx.base.join("origin.git");
    git(
        &fx.base,
        &["init", "-q", "--bare", origin.to_str().unwrap()],
    );
    git(
        &fx.repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(
        &fx.repo,
        &["push", "-q", "origin", "main", "HEAD:refs/pull/7/head"],
    );
    let review = as_advisor(fx)
        .args([
            "add",
            "--type",
            "story",
            "--status",
            "approved",
            "--title",
            &format!("Review PR-7: headless child grant ({})", fx.spec),
        ])
        .output()
        .unwrap();
    assert!(review.status.success(), "{review:?}");
    let id = spec_id(&String::from_utf8_lossy(&review.stdout), Some("STORY"));
    let queued = as_advisor(fx)
        .args(["queue", "add", &id, "--for", "reviewer"])
        .output()
        .unwrap();
    assert!(queued.status.success(), "{queued:?}");
    settle(fx);
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The fake vendor's environment. `None` for a key it did not have.
fn child_env(fx: &Fixture) -> BTreeMap<String, String> {
    let raw = std::fs::read(&fx.dump).expect("the fake vendor ran and dumped its env");
    raw.split(|b| *b == 0)
        .filter_map(|kv| {
            let kv = String::from_utf8_lossy(kv);
            kv.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect()
}

fn grant_record(fx: &Fixture, id: &str) -> serde_json::Value {
    let path = fx
        .home
        .join(".aida")
        .join("session-grants")
        .join(format!("{id}.json"));
    serde_json::from_slice(
        &std::fs::read(&path)
            .unwrap_or_else(|e| panic!("grant record {} must exist: {e}", path.display())),
    )
    .unwrap()
}

/// Assert the child ran under a grant minted for it: distinct from the
/// launcher's, routed seat, no delegable seats, parented on the launcher,
/// revoked once the child exited, and stamped on its lease. Returns its id.
fn assert_own_child_grant(fx: &Fixture, launcher: &str, seat: &str, log: &str) -> String {
    let env = child_env(fx);
    let id = env
        .get("AIDA_SESSION_GRANT")
        .cloned()
        .unwrap_or_else(|| panic!("the child's grant is set explicitly: {log}"));
    assert!(!id.is_empty(), "a delegated child gets a real grant: {log}");
    assert_ne!(
        id, launcher,
        "the child never inherits the launcher grant: {log}"
    );
    assert_eq!(env.get("AIDA_SESSION_ROLE").map(String::as_str), Some(seat));
    assert!(
        !env.contains_key("AIDA_SESSION_PURPOSE"),
        "the launcher's purpose is scrubbed: {log}"
    );
    let grant = grant_record(fx, &id);
    assert_eq!(grant["seat"], seat, "{grant}");
    assert_eq!(grant["delegable_seats"], serde_json::json!([]), "{grant}");
    assert_eq!(grant["parent_grant_id"], launcher, "{grant}");
    assert!(
        !grant["revoked_at"].is_null(),
        "the child's grant is revoked once the child exits: {grant}"
    );
    assert!(
        grant_record(fx, launcher)["revoked_at"].is_null(),
        "the launcher's own grant is untouched"
    );
    id
}

fn lease_files(fx: &Fixture) -> Vec<String> {
    std::fs::read_dir(fx.repo.join(".aida/sessions"))
        .map(|d| {
            d.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
                .map(|e| std::fs::read_to_string(e.path()).unwrap())
                .collect()
        })
        .unwrap_or_default()
}

/// Forget any env dump a setup command wrote (`aida add` may evaluate the
/// spec through the same fake vendor), so only the launch under test counts.
fn clear_dump(fx: &Fixture) {
    let _ = std::fs::remove_file(&fx.dump);
}

fn fresh_headless(fx: &Fixture, grant: Option<&str>, contained: bool) -> Output {
    clear_dump(fx);
    let mut cmd = aida(fx);
    if let Some(grant) = grant {
        cmd.env("AIDA_SESSION_GRANT", grant);
    }
    if contained {
        cmd.env("AIDA_CONTAINED", "1");
    }
    // The launcher's own session purpose must never reach the child.
    cmd.env("AIDA_SESSION_PURPOSE", "advisor relay")
        .args([
            "queue",
            "work",
            &fx.spec,
            "--no-human",
            "--fresh",
            "--no-pull",
        ])
        .output()
        .unwrap()
}

/// The STORY-1654 e2e case: a CONTAINED fresh headless child of an advisor
/// launcher used to run under the advisor grant. It now runs under its own
/// implementer grant with no delegable seats, stamped on its lease.
#[test]
fn contained_fresh_headless_child_gets_its_own_implementer_grant() {
    let fx = fixture();
    let out = fresh_headless(&fx, Some(&fx.advisor), true);
    let log = text(&out);
    assert_eq!(out.status.code(), Some(97), "launch reached: {log}");
    let env = child_env(&fx);
    assert_eq!(
        env.get("AIDA_CONTAINED").map(String::as_str),
        Some("1"),
        "{log}"
    );
    let id = assert_own_child_grant(&fx, &fx.advisor, "implementer", &log);
    assert!(
        lease_files(&fx)
            .iter()
            .any(|l| l.contains(&format!("child_grant_id = \"{id}\""))),
        "the child grant is stamped on its lease: {:?}",
        lease_files(&fx)
    );
}

/// The uncontained fresh headless path behaves the same way.
#[test]
fn uncontained_fresh_headless_child_gets_its_own_implementer_grant() {
    let fx = fixture();
    let out = fresh_headless(&fx, Some(&fx.advisor), false);
    let log = text(&out);
    assert_eq!(out.status.code(), Some(97), "launch reached: {log}");
    assert!(!child_env(&fx).contains_key("AIDA_CONTAINED"), "{log}");
    assert_own_child_grant(&fx, &fx.advisor, "implementer", &log);
}

/// A launcher that cannot delegate the BASELINE implementer seat — no grant
/// at all, or an implementer grant delegating nothing — still launches, but
/// the child runs with `AIDA_SESSION_GRANT=""` (baseline authority only),
/// never the launcher's grant, and the launch says so in one line.
#[test]
fn launcher_that_cannot_delegate_implementer_gets_a_baseline_child() {
    for implementer_grant in [false, true] {
        let fx = fixture();
        let implementer = support::grant_seat(&fx.home, &fx.repo, "implementer", &[]);
        let grant = implementer_grant.then_some(implementer.as_str());
        clear_dump(&fx);
        let out = {
            let mut cmd = aida(&fx);
            if let Some(grant) = grant {
                cmd.env("AIDA_SESSION_GRANT", grant);
            }
            cmd.args([
                "queue",
                "work",
                &fx.spec,
                "--no-human",
                "--fresh",
                "--no-pull",
            ])
            .output()
            .unwrap()
        };
        let log = text(&out);
        assert_eq!(out.status.code(), Some(97), "grant {grant:?}: {log}");
        assert!(
            log.contains("runs ungranted at baseline authority"),
            "grant {grant:?}: {log}"
        );
        let env = child_env(&fx);
        assert_eq!(
            env.get("AIDA_SESSION_GRANT").map(String::as_str),
            Some(""),
            "grant {grant:?}: the baseline child's grant is set EMPTY, never inherited: {log}"
        );
        assert_eq!(
            env.get("AIDA_SESSION_ROLE").map(String::as_str),
            Some("implementer")
        );
    }
}

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
                if path.extension().is_some_and(|e| e == "db-shm") {
                    continue;
                }
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

fn orchestrated_reviewer(fx: &Fixture, grant: &str) -> Output {
    let verdict = fx.base.join("verdict.json");
    clear_dump(fx);
    aida(fx)
        .env("AIDA_SESSION_GRANT", grant)
        .env("AIDA_SESSION_ROLE", "reviewer")
        .env("AIDA_REVIEW_VERDICT_FILE", &verdict)
        .args([
            "queue",
            "work",
            "PR-7",
            "--role",
            "reviewer",
            "--no-human",
            "--fresh",
            "--no-pull",
        ])
        .output()
        .unwrap()
}

/// The orchestrator's reviewer phase (`AIDA_REVIEW_VERDICT_FILE` set) goes
/// through the fresh headless arm. It used to run under the orchestrator's
/// grant; it now gets its own reviewer grant.
#[test]
fn orchestrated_headless_reviewer_gets_its_own_reviewer_grant() {
    let fx = fixture();
    queue_review_story(&fx);
    let out = orchestrated_reviewer(&fx, &fx.advisor);
    let log = text(&out);
    assert_eq!(out.status.code(), Some(97), "launch reached: {log}");
    assert_own_child_grant(&fx, &fx.advisor, "reviewer", &log);
}

/// A non-baseline seat is never run ungranted: without reviewer delegation
/// the orchestrated reviewer is refused before any pickup state changes.
#[test]
fn orchestrated_headless_reviewer_without_delegation_is_refused_without_mutation() {
    let fx = fixture();
    queue_review_story(&fx);
    let narrow = support::grant_seat(&fx.home, &fx.repo, "advisor", &["implementer"]);
    clear_dump(&fx);
    let before = snapshot(&fx);
    let out = orchestrated_reviewer(&fx, &narrow);
    let log = text(&out);
    assert!(!out.status.success(), "{log}");
    assert!(log.contains("child-seat delegation refused"), "{log}");
    assert!(log.contains("does not delegate `reviewer`"), "{log}");
    assert!(!fx.dump.exists(), "the vendor never ran: {log}");
    let after = snapshot(&fx);
    let changed: Vec<&String> = before
        .keys()
        .chain(after.keys())
        .filter(|k| before.get(*k) != after.get(*k))
        .collect();
    assert!(changed.is_empty(), "refusal mutated state: {changed:?}");
}

/// Resume still works: a headless resume child gets its own grant (minted
/// by the resume spawn path) and the launcher's purpose is scrubbed.
#[test]
fn headless_resume_child_gets_its_own_grant() {
    let fx = fixture();
    clear_dump(&fx);
    let out = aida(&fx)
        .env("AIDA_SESSION_GRANT", &fx.advisor)
        .env("AIDA_SESSION_PURPOSE", "advisor relay")
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
    let log = text(&out);
    assert_eq!(out.status.code(), Some(97), "launch reached: {log}");
    let env = child_env(&fx);
    let id = env.get("AIDA_SESSION_GRANT").cloned().unwrap_or_default();
    assert!(!id.is_empty() && id != fx.advisor, "{log}");
    assert!(!env.contains_key("AIDA_SESSION_PURPOSE"), "{log}");
    let grant = grant_record(&fx, &id);
    assert_eq!(grant["seat"], "implementer", "{grant}");
    assert_eq!(grant["delegable_seats"], serde_json::json!([]), "{grant}");
    assert_eq!(grant["parent_grant_id"], fx.advisor.as_str(), "{grant}");
}
