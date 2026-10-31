//! BUG-049: seat worktrees are pre-trusted in a fixture `~/.claude.json`
//! before launch, the entry goes with the seat, and every write prunes the
//! entries of trees that are gone.
// trace:BUG-B6B1278890774EF993D2EBC327862470-049 | ai:claude

use super::*;
use serde_json::{json, Value};

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    main: PathBuf,
    seat: PathBuf,
}

fn config_body(enabled: bool) -> String {
    format!("[worktree_pool]\nworktree_parent = \"../wt\"\ntrust_seat_worktrees = {enabled}\n")
}

/// A git main checkout whose COMMITTED `main` config (the trusted copy the
/// opt-in is read from) has `[worktree_pool] worktree_parent = "../wt"`, a
/// seat directory inside that root, and a HOME with a populated
/// `~/.claude.json`.
fn fixture(enabled: bool) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let home = base.join("home");
    let main = base.join("main");
    let seat = base.join("wt").join("aida-seat-main-bug-1");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(main.join(".aida")).unwrap();
    std::fs::create_dir_all(&seat).unwrap();
    std::fs::write(main.join(".aida").join("config.toml"), config_body(enabled)).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    git(&main, &["add", "-A"]);
    git(&main, &["commit", "-q", "-m", "init"]);
    write_seed(&home);
    Fixture {
        _tmp: tmp,
        home,
        main,
        seat,
    }
}

fn write_seed(home: &Path) {
    std::fs::write(
        home.join(".claude.json"),
        serde_json::to_string_pretty(&json!({
            "numStartups": 7,
            "oauthAccount": {"emailAddress": "op@example.com"},
            "projects": {
                "/elsewhere": {"hasTrustDialogAccepted": false, "allowedTools": ["Bash"]}
            }
        }))
        .unwrap(),
    )
    .unwrap();
}

fn read(home: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(home.join(".claude.json")).unwrap()).unwrap()
}

fn project_keys(home: &Path) -> Vec<String> {
    let mut keys: Vec<String> = read(home)["projects"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-049.acaf7e35 | ai:claude
#[test]
fn the_write_adds_one_entry_and_leaves_everything_else() {
    let f = fixture(true);
    let key = f.seat.to_string_lossy().into_owned();

    let outcome = ensure_seat_trusted_in(&f.home, &f.main, &f.seat).unwrap();
    assert_eq!(outcome, TrustWrite::Written(f.seat.clone()));
    let after = read(&f.home);
    assert_eq!(
        after["projects"][&key]["hasTrustDialogAccepted"],
        json!(true)
    );
    assert_eq!(
        after["projects"][&key]["enabledMcpjsonServers"],
        json!(["aida"])
    );
    // Every other entry and top-level key is untouched.
    assert_eq!(after["numStartups"], json!(7));
    assert_eq!(
        after["oauthAccount"]["emailAddress"],
        json!("op@example.com")
    );
    assert_eq!(
        after["projects"]["/elsewhere"],
        json!({"hasTrustDialogAccepted": false, "allowedTools": ["Bash"]})
    );
    // No staging file left behind; the flock sidecar is the only extra file.
    let litter: Vec<_> = std::fs::read_dir(&f.home)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != ".claude.json" && n != LOCK_FILE)
        .collect();
    assert!(litter.is_empty(), "left behind: {litter:?}");

    // Idempotent: a second launch does not rewrite.
    assert_eq!(
        ensure_seat_trusted_in(&f.home, &f.main, &f.seat).unwrap(),
        TrustWrite::AlreadyTrusted(f.seat.clone())
    );
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-049.ac15c23c | ai:claude
#[test]
fn removal_drops_only_the_seats_entry() {
    let f = fixture(true);
    let key = f.seat.to_string_lossy().into_owned();
    ensure_seat_trusted_in(&f.home, &f.main, &f.seat).unwrap();
    // Dismissal (the directory is already gone by then) removes only it.
    std::fs::remove_dir_all(&f.seat).unwrap();
    assert!(remove_seat_trust_in(&f.home, &f.main, &f.seat).unwrap());
    let after = read(&f.home);
    assert!(after["projects"].get(&key).is_none());
    assert!(after["projects"].get("/elsewhere").is_some());
    assert_eq!(after["numStartups"], json!(7));
    assert!(!remove_seat_trust_in(&f.home, &f.main, &f.seat).unwrap());
}

/// Growth is bounded by design: three trees trusted, two of them deleted by
/// a teardown path that never touched `~/.claude.json`, and the next write
/// leaves exactly the live one (entries outside the root are untouched).
// trace:BUG-B6B1278890774EF993D2EBC327862470-049.ac15c23c | ai:claude
#[test]
fn every_write_prunes_entries_whose_tree_is_gone() {
    let f = fixture(true);
    let root = f.seat.parent().unwrap().to_path_buf();
    let trees: Vec<PathBuf> = ["aida-seat-main-a", "aida-pr-7", "spec-worktree"]
        .iter()
        .map(|n| root.join(n))
        .collect();
    for tree in &trees {
        std::fs::create_dir_all(tree).unwrap();
        ensure_seat_trusted_in(&f.home, &f.main, tree).unwrap();
    }
    // An entry outside the root whose directory is also gone stays: it is not
    // ours to judge.
    let mut cfg = read(&f.home);
    cfg["projects"]["/gone/elsewhere"] = json!({"hasTrustDialogAccepted": true});
    std::fs::write(f.home.join(".claude.json"), cfg.to_string()).unwrap();

    std::fs::remove_dir_all(&trees[1]).unwrap();
    std::fs::remove_dir_all(&trees[2]).unwrap();
    assert_eq!(
        stale_seat_entries(&f.home, &f.main).len(),
        2,
        "doctor sees every stale tree under the root, not only aida-seat-*"
    );

    // One write (for a tree that is already trusted) still prunes.
    assert_eq!(
        ensure_seat_trusted_in(&f.home, &f.main, &trees[0]).unwrap(),
        TrustWrite::AlreadyTrusted(trees[0].clone())
    );
    let mut want = vec![
        "/elsewhere".to_string(),
        "/gone/elsewhere".to_string(),
        trees[0].to_string_lossy().into_owned(),
    ];
    want.sort();
    assert_eq!(project_keys(&f.home), want);
    assert!(stale_seat_entries(&f.home, &f.main).is_empty());
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-049.acaf7e35 | ai:claude
#[test]
fn an_existing_entry_is_completed_not_replaced() {
    let f = fixture(true);
    let key = f.seat.to_string_lossy().into_owned();
    let mut cfg = read(&f.home);
    cfg["projects"][&key] = json!({"allowedTools": ["Read"], "enabledMcpjsonServers": ["other"]});
    std::fs::write(f.home.join(".claude.json"), cfg.to_string()).unwrap();

    ensure_seat_trusted_in(&f.home, &f.main, &f.seat).unwrap();
    let entry = &read(&f.home)["projects"][&key];
    assert_eq!(entry["allowedTools"], json!(["Read"]));
    assert_eq!(entry["enabledMcpjsonServers"], json!(["other", "aida"]));
    assert_eq!(entry["hasTrustDialogAccepted"], json!(true));
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-049.Test | ai:claude
#[test]
fn a_path_outside_the_worktrees_root_is_refused_and_nothing_is_written() {
    let f = fixture(true);
    let before = std::fs::read(f.home.join(".claude.json")).unwrap();
    let outside = f.main.parent().unwrap().join("unrelated");
    std::fs::create_dir_all(&outside).unwrap();
    let nested = f.seat.join("sub");
    std::fs::create_dir_all(&nested).unwrap();
    let root = f.seat.parent().unwrap().to_path_buf();
    let missing = root.join("aida-seat-never-made");

    for path in [&outside, &f.main, &nested, &root, &missing] {
        assert!(
            matches!(
                ensure_seat_trusted_in(&f.home, &f.main, path).unwrap(),
                TrustWrite::Refused(_)
            ),
            "{} must be refused",
            path.display()
        );
    }
    for path in [&outside, &f.main, &nested, &root] {
        assert!(!remove_seat_trust_in(&f.home, &f.main, path).unwrap());
    }
    assert_eq!(std::fs::read(f.home.join(".claude.json")).unwrap(), before);
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-049.ac891c92 | ai:claude
#[test]
fn the_write_is_opt_in() {
    let f = fixture(false);
    let before = std::fs::read(f.home.join(".claude.json")).unwrap();
    {
        let _env = crate::test_env::EnvVarsGuard::apply(&[(ENV_OVERRIDE, None)]);
        assert!(!trust_enabled(&f.main));
        assert_eq!(
            ensure_seat_trusted_in(&f.home, &f.main, &f.seat).unwrap(),
            TrustWrite::Disabled
        );
        assert_eq!(std::fs::read(f.home.join(".claude.json")).unwrap(), before);
        // BUG-1723: a branch-local (uncommitted) copy cannot switch it on;
        // only the default branch's committed copy counts.
        std::fs::write(f.main.join(".aida").join("config.toml"), config_body(true)).unwrap();
        assert!(!trust_enabled(&f.main));
        git(&f.main, &["commit", "-q", "-am", "enable"]);
        assert!(trust_enabled(&f.main));
    }
    {
        // The host override wins over the project config.
        let _env = crate::test_env::EnvVarsGuard::apply(&[(ENV_OVERRIDE, Some("0"))]);
        assert!(!trust_enabled(&f.main));
    }
    {
        let _env = crate::test_env::EnvVarsGuard::apply(&[(ENV_OVERRIDE, Some("1"))]);
        assert!(trust_enabled(&f.main));
    }
    assert_eq!(parse_override("1"), Some(true));
    assert_eq!(parse_override(" ON "), Some(true));
    assert_eq!(parse_override("off"), Some(false));
    assert_eq!(parse_override("0"), Some(false));
    assert_eq!(parse_override("maybe"), None);
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-049.acaf7e35 | ai:claude
#[test]
fn an_unparseable_claude_json_is_never_overwritten() {
    let f = fixture(true);
    std::fs::write(f.home.join(".claude.json"), "{ not json").unwrap();
    assert!(ensure_seat_trusted_in(&f.home, &f.main, &f.seat).is_err());
    assert_eq!(
        std::fs::read_to_string(f.home.join(".claude.json")).unwrap(),
        "{ not json"
    );
}

/// A held flock is waited on (the writer does not edit while another holds
/// it) and times out instead of hanging.
// trace:BUG-B6B1278890774EF993D2EBC327862470-049.acaf7e35 | ai:claude
#[test]
fn a_held_lock_blocks_the_writer_until_released_and_times_out() {
    let f = fixture(true);
    let held = ClaudeJsonLock::acquire(&f.home, Duration::from_secs(1)).unwrap();
    // Timeout: a second acquire gives up rather than hanging.
    let err = ClaudeJsonLock::acquire(&f.home, Duration::from_millis(100))
        .err()
        .expect("held lock must time out");
    assert!(
        format!("{err:#}").contains("held by another writer"),
        "{err:#}"
    );

    let before = std::fs::read(f.home.join(".claude.json")).unwrap();
    let (home, main, seat) = (f.home.clone(), f.main.clone(), f.seat.clone());
    let writer = std::thread::spawn(move || ensure_seat_trusted_in(&home, &main, &seat));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        std::fs::read(f.home.join(".claude.json")).unwrap(),
        before,
        "the writer edited while the lock was held"
    );
    drop(held);
    assert!(matches!(
        writer.join().unwrap().unwrap(),
        TrustWrite::Written(_)
    ));
}

/// Concurrent writers never lose each other's update: every one of them
/// lands.
// trace:BUG-B6B1278890774EF993D2EBC327862470-049.acaf7e35 | ai:claude
#[test]
fn concurrent_writers_do_not_lose_updates() {
    let f = fixture(true);
    let root = f.seat.parent().unwrap().to_path_buf();
    let seats: Vec<PathBuf> = (0..8)
        .map(|i| {
            let s = root.join(format!("aida-seat-main-c{i}"));
            std::fs::create_dir_all(&s).unwrap();
            s
        })
        .collect();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(seats.len()));
    let handles: Vec<_> = seats
        .iter()
        .cloned()
        .map(|seat| {
            let (home, main, barrier) = (f.home.clone(), f.main.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                ensure_seat_trusted_in(&home, &main, &seat).unwrap()
            })
        })
        .collect();
    for h in handles {
        assert!(matches!(h.join().unwrap(), TrustWrite::Written(_)));
    }
    let after = read(&f.home);
    for seat in &seats {
        assert_eq!(
            after["projects"][seat.to_string_lossy().as_ref()]["hasTrustDialogAccepted"],
            json!(true),
            "{} lost",
            seat.display()
        );
    }
    assert_eq!(after["numStartups"], json!(7));
}

#[cfg(unix)]
// trace:BUG-B6B1278890774EF993D2EBC327862470-049.acaf7e35 | ai:claude
#[test]
fn the_file_keeps_its_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture(true);
    let path = f.home.join(".claude.json");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    ensure_seat_trusted_in(&f.home, &f.main, &f.seat).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-049.ac891c92 | ai:claude
#[test]
fn doctor_lists_entries_for_trees_that_no_longer_exist() {
    let f = fixture(true);
    ensure_seat_trusted_in(&f.home, &f.main, &f.seat).unwrap();
    assert!(stale_seat_entries(&f.home, &f.main).is_empty());
    std::fs::remove_dir_all(&f.seat).unwrap();
    assert_eq!(stale_seat_entries(&f.home, &f.main), vec![f.seat.clone()]);
}

// ---- wiring: the real launcher and the real dismiss command ----

fn git(dir: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@localhost",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A git main checkout (trust on) with two real `git worktree`s under its
/// worktrees root — a headless reviewer's `aida-pr-N` tree and a spec
/// worktree (an `aida-seat-*` name is reserved for seat clones) — and a
/// fixture HOME.
fn git_fixture() -> (Fixture, PathBuf) {
    let mut f = fixture(true);
    std::fs::remove_dir_all(&f.seat).unwrap();
    f.seat = f.seat.parent().unwrap().join("aida-pr-1");
    git(
        &f.main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "bug-1",
            f.seat.to_str().unwrap(),
        ],
    );
    let codex_seat = f.seat.parent().unwrap().join("bug-2");
    git(
        &f.main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "bug-2",
            codex_seat.to_str().unwrap(),
        ],
    );
    (f, codex_seat)
}

/// A fake `claude` that exits 0 only if `$HOME/.claude.json` already trusts
/// its cwd when it starts — the assertion runs inside the spawned process.
#[cfg(unix)]
fn fake_claude(dir: &Path) -> PathBuf {
    let path = dir.join("fake-claude");
    crate::test_exec::write_executable(
        &path,
        "#!/bin/sh\n\
         seat=$(pwd -P)\n\
         grep -qF \"\\\"$seat\\\"\" \"$HOME/.claude.json\" || { echo \"untrusted: $seat\" >&2; exit 7; }\n\
         grep -q '\"hasTrustDialogAccepted\": true' \"$HOME/.claude.json\" || exit 8\n\
         exit 0\n",
    );
    path
}

/// Launch a seat through the real headless launcher with a fixture HOME and
/// a fake claude: the entry exists before the spawn (the fake asserts it);
/// a codex launch writes nothing; `aida worktree dismiss` removes the entry.
// trace:BUG-B6B1278890774EF993D2EBC327862470-049.Test | ai:claude
// trace:BUG-B6B1278890774EF993D2EBC327862470-049.acaf7e35 | ai:claude
// trace:BUG-B6B1278890774EF993D2EBC327862470-049.ac15c23c | ai:claude
#[cfg(unix)]
#[test]
fn launcher_trusts_before_the_spawn_and_dismiss_removes_it() {
    let (f, codex_seat) = git_fixture();
    let fake = fake_claude(&f.home);
    let key = f.seat.to_string_lossy().into_owned();
    let mut ambient = crate::test_env::AmbientGuard::hermetic_and(
        &f.main,
        None,
        &[
            ("AIDA_AGENT_CMD", Some(fake.to_str().unwrap())),
            (ENV_OVERRIDE, None),
            ("AIDA_CONTAINED", None),
            // The fixture's trees are private to this test; let dismiss run
            // inside a sandboxed (pid-namespaced) test host too.
            (
                aida_core::worktree_holders::TRUST_PID_NAMESPACE_ENV,
                Some("1"),
            ),
        ],
    );
    // The fixture HOME is the lib test process's hermetic temp home (never
    // the operator's); the production code resolves it via `home_dir()`.
    let home = crate::home_dir().expect("hermetic test home");
    let read_home = || -> Value {
        std::fs::read_to_string(home.join(".claude.json"))
            .ok()
            .and_then(|b| serde_json::from_str(&b).ok())
            .unwrap_or_else(|| json!({}))
    };

    let spawn = |program: &str, args: &[String]| {
        std::process::Command::new(program)
            .args(args)
            .current_dir(&f.seat)
            .env("HOME", &home)
            .status()
            .unwrap()
    };
    // The fake really checks: before any launch it refuses to start.
    assert_ne!(spawn(fake.to_str().unwrap(), &[]).code(), Some(0));

    // Codex is skipped: composing its launch writes no entry.
    {
        let _cwd = crate::session::contained_cwd_seam::pin(&codex_seat);
        crate::session::compose_headless_command(
            crate::session::HeadlessVendor::Codex,
            "hello",
            "sess-codex",
            false,
            None,
            None,
        )
        .unwrap();
    }
    assert!(read_home()["projects"]
        .get(codex_seat.to_string_lossy().as_ref())
        .is_none());

    // Claude: the launcher writes the entry before the spawn.
    let (program, args) = {
        let _cwd = crate::session::contained_cwd_seam::pin(&f.seat);
        crate::session::compose_headless_command(
            crate::session::HeadlessVendor::Claude,
            "hello",
            "sess-claude",
            false,
            None,
            None,
        )
        .unwrap()
    };
    assert_eq!(program, fake.to_str().unwrap());
    let status = spawn(&program, &args);
    assert!(status.success(), "fake claude saw no trust entry: {status}");
    assert_eq!(
        read_home()["projects"][&key]["enabledMcpjsonServers"],
        json!(["aida"])
    );

    // Dismiss: the tree and its entry go together.
    ambient.set_key("USER", "op");
    crate::worktree_dismiss::handle_dismiss(Some(f.seat.to_str().unwrap()), None).unwrap();
    assert!(!f.seat.exists(), "dismiss did not remove the seat");
    let after = read_home();
    assert!(after["projects"].get(&key).is_none(), "{after:#}");
}
