//! BUG-049: seat worktrees are pre-trusted in a fixture `~/.claude.json`
//! before launch and the entry goes with the seat.
// trace:BUG-B6B1278890774EF993D2EBC327862470-049 | ai:claude

use super::*;
use serde_json::{json, Value};

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    main: PathBuf,
    seat: PathBuf,
}

/// A main checkout with `[worktree_pool] worktree_parent = "../wt"`, a seat
/// directory inside that root, and a HOME with a populated `~/.claude.json`.
fn fixture(enabled: bool) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let home = base.join("home");
    let main = base.join("main");
    let seat = base.join("wt").join("aida-seat-main-bug-1");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(main.join(".aida")).unwrap();
    std::fs::create_dir_all(&seat).unwrap();
    std::fs::write(
        main.join(".aida").join("config.toml"),
        format!("[worktree_pool]\nworktree_parent = \"../wt\"\ntrust_seat_worktrees = {enabled}\n"),
    )
    .unwrap();
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
    Fixture {
        _tmp: tmp,
        home,
        main,
        seat,
    }
}

fn read(home: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(home.join(".claude.json")).unwrap()).unwrap()
}

#[test]
fn launch_writes_the_trust_entry_before_the_spawn_and_dismissal_removes_it() {
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
    // No lock directory or staging file left behind.
    let litter: Vec<_> = std::fs::read_dir(&f.home)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != ".claude.json")
        .collect();
    assert!(litter.is_empty(), "left behind: {litter:?}");

    // Idempotent: a second launch does not rewrite.
    assert_eq!(
        ensure_seat_trusted_in(&f.home, &f.main, &f.seat).unwrap(),
        TrustWrite::AlreadyTrusted(f.seat.clone())
    );

    // Dismissal (the directory is already gone by then) removes only it.
    std::fs::remove_dir_all(&f.seat).unwrap();
    assert!(remove_seat_trust_in(&f.home, &f.main, &f.seat).unwrap());
    let after = read(&f.home);
    assert!(after["projects"].get(&key).is_none());
    assert!(after["projects"].get("/elsewhere").is_some());
    assert_eq!(after["numStartups"], json!(7));
    assert!(!remove_seat_trust_in(&f.home, &f.main, &f.seat).unwrap());
}

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

#[test]
fn a_path_outside_the_worktrees_root_is_refused_and_nothing_is_written() {
    let f = fixture(true);
    let before = std::fs::read(f.home.join(".claude.json")).unwrap();
    let outside = f.main.parent().unwrap().join("unrelated");
    std::fs::create_dir_all(&outside).unwrap();
    let nested = f.seat.join("sub");
    std::fs::create_dir_all(&nested).unwrap();
    let root = f.seat.parent().unwrap().to_path_buf();

    for path in [&outside, &f.main, &nested, &root] {
        assert!(
            matches!(
                ensure_seat_trusted_in(&f.home, &f.main, path).unwrap(),
                TrustWrite::Refused(_)
            ),
            "{} must be refused",
            path.display()
        );
        assert!(!remove_seat_trust_in(&f.home, &f.main, path).unwrap());
    }
    assert_eq!(std::fs::read(f.home.join(".claude.json")).unwrap(), before);
}

#[test]
fn the_write_is_opt_in() {
    let f = fixture(false);
    let before = std::fs::read(f.home.join(".claude.json")).unwrap();
    if std::env::var_os(ENV_OVERRIDE).is_none() {
        assert_eq!(
            ensure_seat_trusted_in(&f.home, &f.main, &f.seat).unwrap(),
            TrustWrite::Disabled
        );
        assert_eq!(std::fs::read(f.home.join(".claude.json")).unwrap(), before);
    }
    assert_eq!(parse_override("1"), Some(true));
    assert_eq!(parse_override(" ON "), Some(true));
    assert_eq!(parse_override("off"), Some(false));
    assert_eq!(parse_override("0"), Some(false));
    assert_eq!(parse_override("maybe"), None);
}

#[test]
fn an_unparseable_claude_json_is_never_overwritten() {
    let f = fixture(true);
    std::fs::write(f.home.join(".claude.json"), "{ not json").unwrap();
    assert!(ensure_seat_trusted_in(&f.home, &f.main, &f.seat).is_err());
    assert_eq!(
        std::fs::read_to_string(f.home.join(".claude.json")).unwrap(),
        "{ not json"
    );
    assert!(!f.home.join(".claude.json.lock").exists());
}

#[test]
fn a_held_lock_is_waited_on_and_a_stale_one_is_broken() {
    let f = fixture(true);
    let lock = f.home.join(".claude.json.lock");
    std::fs::create_dir(&lock).unwrap();
    let old = std::time::SystemTime::now() - Duration::from_secs(60);
    std::fs::File::open(&lock)
        .unwrap()
        .set_modified(old)
        .unwrap();
    assert!(matches!(
        ensure_seat_trusted_in(&f.home, &f.main, &f.seat).unwrap(),
        TrustWrite::Written(_)
    ));
    assert!(!lock.exists());
}

#[cfg(unix)]
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

#[test]
fn doctor_lists_entries_for_seats_that_no_longer_exist() {
    let f = fixture(true);
    ensure_seat_trusted_in(&f.home, &f.main, &f.seat).unwrap();
    assert!(stale_seat_entries(&f.home, &f.main).is_empty());
    std::fs::remove_dir_all(&f.seat).unwrap();
    assert_eq!(stale_seat_entries(&f.home, &f.main), vec![f.seat.clone()]);
}
