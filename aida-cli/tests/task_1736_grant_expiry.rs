// Gated to Linux like every other binary-driving e2e suite here.
// trace:BUG-688 | ai:claude
#![cfg(target_os = "linux")]
//! TASK-1736: a seat grant near expiry is announced before it bites. With a
//! fixture grant record under an isolated HOME, `aida role expiry` warns
//! inside 2h, refuses a relay launch (exit 3) inside `--refuse-within`, and
//! the SessionStart hook prints the warning with the renew command.
// trace:TASK-1736 | ai:claude

mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn aida_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_aida"))
}

/// Strip every ambient input that could point the child at the developer's
/// own grant or store, and pin HOME to the fixture.
fn isolate(cmd: &mut Command, home: &Path) {
    cmd.env("HOME", home);
    cmd.env("AIDA_TELEMETRY", "0");
    for k in [
        "AIDA_HOME",
        "AIDA_SESSION_ROLE",
        "AIDA_SESSION_GRANT",
        "AIDA_SESSION_ID",
        "AIDA_STORE_PATH",
        "AIDA_PROJECT_ROOT",
        "AIDA_SESSION_PROJECT",
        "CLAUDE_PROJECT_DIR",
        "CODEX_PROJECT_DIR",
    ] {
        cmd.env_remove(k);
    }
}

/// Write a grant record the way `seat_authority::write_grant` does, expiring
/// `minutes_left` from now (negative: already expired). Returns its handle.
fn write_grant(home: &Path, seat: &str, minutes_left: i64) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now();
    let record = serde_json::json!({
        "id": id,
        "principal": "tester",
        "subject": "tester",
        "session_id": uuid::Uuid::new_v4().to_string(),
        "seat": seat,
        "tty_issued_at": now,
        "delegable_seats": [],
        "parent_grant_id": null,
        "issued_at": now - chrono::Duration::hours(23),
        "expires_at": now + chrono::Duration::minutes(minutes_left),
        "revoked_at": null,
    });
    let dir = home.join(".aida").join("session-grants");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{id}.json")),
        serde_json::to_vec_pretty(&record).unwrap(),
    )
    .unwrap();
    id
}

fn expiry(home: &Path, grant: Option<&str>, args: &[&str]) -> Output {
    let mut cmd = Command::new(aida_bin());
    isolate(&mut cmd, home);
    if let Some(grant) = grant {
        cmd.env("AIDA_SESSION_GRANT", grant);
    }
    cmd.current_dir(home)
        .args(["role", "expiry"])
        .args(args)
        .output()
        .expect("run aida role expiry")
}

fn show(o: &Output) -> String {
    format!(
        "exit {:?}\n--- stderr ---\n{}\n--- stdout ---\n{}",
        o.status.code(),
        String::from_utf8_lossy(&o.stderr),
        String::from_utf8_lossy(&o.stdout)
    )
}

#[test]
fn near_expiry_grant_warns_and_refuses_a_launch() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let grant = write_grant(home, "advisor", 30);

    let warn = expiry(home, Some(&grant), &[]);
    assert!(warn.status.success(), "{}", show(&warn));
    let stdout = String::from_utf8_lossy(&warn.stdout);
    assert!(
        stdout.contains("The advisor seat grant expires at"),
        "{}",
        show(&warn)
    );
    assert!(
        stdout.contains("eval \"$(aida role enter advisor)\""),
        "{}",
        show(&warn)
    );

    let refuse = expiry(home, Some(&grant), &["--refuse-within", "1h"]);
    assert_eq!(refuse.status.code(), Some(3), "{}", show(&refuse));
    let stderr = String::from_utf8_lossy(&refuse.stderr);
    assert!(stderr.starts_with("Not launching"), "{}", show(&refuse));
    assert!(refuse.stdout.is_empty(), "{}", show(&refuse));

    let ok = expiry(home, Some(&grant), &["--refuse-within", "20m"]);
    assert_eq!(ok.status.code(), Some(0), "{}", show(&ok));
}

#[test]
fn expired_grant_says_expired() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let grant = write_grant(home, "reviewer", -10);

    let warn = expiry(home, Some(&grant), &[]);
    assert!(
        String::from_utf8_lossy(&warn.stdout).contains("The reviewer seat grant expired at"),
        "{}",
        show(&warn)
    );
    let refuse = expiry(home, Some(&grant), &["--refuse-within", "1h"]);
    assert_eq!(refuse.status.code(), Some(3), "{}", show(&refuse));
    assert!(
        String::from_utf8_lossy(&refuse.stderr).contains("expired at"),
        "{}",
        show(&refuse)
    );
}

#[test]
fn fresh_or_missing_grant_is_silent() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let fresh = write_grant(home, "advisor", 5 * 60);
    for grant in [Some(fresh.as_str()), None] {
        for args in [&[][..], &["--refuse-within", "1h"][..]] {
            let out = expiry(home, grant, args);
            assert_eq!(out.status.code(), Some(0), "{}", show(&out));
            assert!(
                out.stdout.is_empty() && out.stderr.is_empty(),
                "{}",
                show(&out)
            );
        }
    }
}

#[test]
fn bad_duration_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let out = expiry(tmp.path(), None, &["--refuse-within", "soon"]);
    assert!(!out.status.success(), "{}", show(&out));
    assert!(show(&out).contains("invalid duration"), "{}", show(&out));
}

/// The SessionStart hook surfaces the warning, so a session opens knowing
/// its grant is about to lapse.
#[test]
#[cfg(target_os = "linux")]
fn session_start_hook_prints_the_expiry_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    let grant = write_grant(&home, "advisor", 45);

    let hook = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../aida-core/templates/hooks/aida-session-context.sh");
    let bin_dir = aida_bin().parent().unwrap().to_path_buf();
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut cmd = Command::new("bash");
    isolate(&mut cmd, &home);
    let out = cmd
        .arg(&hook)
        .env("PATH", path)
        .env("CLAUDE_PROJECT_DIR", &project)
        .env("AIDA_SESSION_GRANT", &grant)
        .current_dir(&project)
        .output()
        .expect("run session-context hook");
    assert!(out.status.success(), "{}", show(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("The advisor seat grant expires at"),
        "{}",
        show(&out)
    );
    assert!(stdout.contains("aida role enter advisor"), "{}", show(&out));
}

/// A grant handle that names no usable record (missing, revoked, malformed) refuses a
/// launch rather than passing it as "no grant": the session it would start
/// meets refusals mid-run.
// trace:TASK-1736 | ai:claude
#[test]
fn unresolvable_grant_refuses_a_launch() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();

    let missing = uuid::Uuid::new_v4().to_string();
    let out = expiry(home, Some(&missing), &["--refuse-within", "1h"]);
    assert_eq!(out.status.code(), Some(3), "{}", show(&out));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("Not launching"), "{}", show(&out));
    assert!(stderr.contains("missing"), "{}", show(&out));
    assert!(stderr.contains("aida role enter"), "{}", show(&out));
    assert!(!stderr.contains("TASK-"), "{}", show(&out));
    assert!(out.stdout.is_empty(), "{}", show(&out));

    let revoked = write_grant(home, "advisor", 5 * 60);
    let path = home
        .join(".aida")
        .join("session-grants")
        .join(format!("{revoked}.json"));
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    record["revoked_at"] = serde_json::json!(chrono::Utc::now());
    std::fs::write(&path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    let out = expiry(home, Some(&revoked), &["--refuse-within", "1h"]);
    assert_eq!(out.status.code(), Some(3), "{}", show(&out));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("Not launching"), "{}", show(&out));
    assert!(stderr.contains("revoked"), "{}", show(&out));
    assert!(!stderr.contains("TASK-"), "{}", show(&out));

    let malformed = uuid::Uuid::new_v4().to_string();
    std::fs::write(
        home.join(".aida")
            .join("session-grants")
            .join(format!("{malformed}.json")),
        b"{ not json",
    )
    .unwrap();
    let out = expiry(home, Some(&malformed), &["--refuse-within", "1h"]);
    assert_eq!(out.status.code(), Some(3), "{}", show(&out));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("Not launching"), "{}", show(&out));
    assert!(stderr.contains("malformed"), "{}", show(&out));
    assert!(!stderr.contains("TASK-"), "{}", show(&out));

    // Warn mode stays quiet: there is no expiry to announce.
    for grant in [missing.as_str(), revoked.as_str(), malformed.as_str()] {
        let out = expiry(home, Some(grant), &[]);
        assert_eq!(out.status.code(), Some(0), "{}", show(&out));
        assert!(out.stdout.is_empty(), "{}", show(&out));
    }
}

/// Run `aida statusline` from `project` holding a valid, roster-allowed
/// advisor grant that expires `minutes_left` from now.
// trace:TASK-1736 | ai:claude
fn statusline_with_grant(minutes_left: i64) -> Output {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    let subject = "tester";
    let grant = support::grant_seat_for(&home, &project, subject, "advisor", &[]);
    let path = home
        .join(".aida")
        .join("session-grants")
        .join(format!("{grant}.json"));
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    record["expires_at"] =
        serde_json::json!(chrono::Utc::now() + chrono::Duration::minutes(minutes_left));
    std::fs::write(&path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();

    let mut cmd = Command::new(aida_bin());
    isolate(&mut cmd, &home);
    let out = cmd
        .env("AIDA_USER", subject)
        .env("AIDA_SESSION_GRANT", &grant)
        .env("AIDA_SESSION_ROLE", "advisor")
        .current_dir(&project)
        .args(["statusline", "--color", "never"])
        .output()
        .expect("run aida statusline");
    drop(tmp);
    out
}

/// The statusline tags a held seat with the time left inside the warning
/// window, and says nothing about expiry while there is time to spare.
// trace:TASK-1736 | ai:claude
#[test]
fn statusline_tags_a_held_seat_near_expiry() {
    let near = statusline_with_grant(30);
    assert!(near.status.success(), "{}", show(&near));
    let stdout = String::from_utf8_lossy(&near.stdout);
    assert!(stdout.contains("advisor"), "{}", show(&near));
    assert!(!stdout.contains("no grant"), "{}", show(&near));
    assert!(
        stdout.contains("(expires in 29m)") || stdout.contains("(expires in 30m)"),
        "{}",
        show(&near)
    );

    let fresh = statusline_with_grant(5 * 60);
    assert!(fresh.status.success(), "{}", show(&fresh));
    let stdout = String::from_utf8_lossy(&fresh.stdout);
    assert!(stdout.contains("advisor"), "{}", show(&fresh));
    assert!(!stdout.contains("expires in"), "{}", show(&fresh));
    assert!(!stdout.contains("no grant"), "{}", show(&fresh));
}
