#![cfg(target_os = "linux")]
//! Black-box coverage: run from a directory outside any AIDA project while the
//! (fake) HOME's project registry names an unrelated legacy
//! `requirements.db` as its default project. Writes and reads must refuse with
//! setup guidance and leave that store byte-for-byte untouched; an explicit
//! `--file` / `-p` still reaches it.
// trace:BUG-1603 | ai:claude

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    outside: PathBuf,
    legacy_db: PathBuf,
}

fn aida_in(fixture: &Fixture, dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(dir)
        .env("HOME", &fixture.home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_OUTPUT_FORMAT", "human")
        .env_remove("AIDA_STORE")
        .env_remove("REQ_DB_NAME")
        .env_remove("AIDA_REGISTRY_PATH")
        .env_remove("REQ_REGISTRY_PATH")
        .args(args)
        .output()
        .expect("run aida")
}

fn text(out: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn fixture() -> Fixture {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let other = tmp.path().join("other-project");
    let outside = tmp.path().join("outside");
    for d in [&home, &other, &outside] {
        std::fs::create_dir_all(d).unwrap();
    }
    let legacy_db = other.join("requirements.db");
    let f = Fixture {
        _tmp: tmp,
        home,
        outside,
        legacy_db,
    };
    // Seed the unrelated legacy store through its explicit path.
    let seeded = aida_in(
        &f,
        &other,
        &[
            "--file",
            f.legacy_db.to_str().unwrap(),
            "add",
            "seed spec",
            "--type",
            "task",
        ],
    );
    assert!(seeded.status.success(), "{}", text(&seeded));
    // The legacy registry names that store as the default (and only) project.
    std::fs::write(
        f.home.join(".aida.config"),
        format!(
            "projects:\n  other:\n    path: {}\n    description: unrelated\n\
             default_project: other\n",
            f.legacy_db.display()
        ),
    )
    .unwrap();
    f
}

#[test]
fn add_outside_any_project_refuses_and_writes_nothing() {
    let f = fixture();
    let before = std::fs::read(&f.legacy_db).unwrap();

    let out = aida_in(&f, &f.outside, &["add", "stray spec", "--type", "task"]);
    assert!(!out.status.success(), "add must refuse: {}", text(&out));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("No AIDA project found here"), "{}", text(&out));
    assert!(err.contains("aida init"), "{}", text(&out));
    assert!(err.contains("-p other"), "{}", text(&out));
    assert!(!err.contains("BUG-"), "no internal ids: {}", text(&out));

    assert_eq!(
        std::fs::read(&f.legacy_db).unwrap(),
        before,
        "the unrelated legacy store must be untouched"
    );
    assert_eq!(std::fs::read_dir(&f.outside).unwrap().count(), 0);
}

#[test]
fn reads_outside_any_project_do_not_silently_read_an_unrelated_store() {
    let f = fixture();
    let out = aida_in(&f, &f.outside, &["list"]);
    assert!(!out.status.success(), "list must refuse: {}", text(&out));
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("seed spec"),
        "{}",
        text(&out)
    );
}

#[test]
fn explicit_file_and_project_still_reach_the_legacy_store() {
    let f = fixture();
    let db = f.legacy_db.to_str().unwrap();

    let added = aida_in(
        &f,
        &f.outside,
        &["--file", db, "add", "explicit spec", "--type", "task"],
    );
    assert!(added.status.success(), "{}", text(&added));

    let listed = aida_in(&f, &f.outside, &["-p", "other", "list"]);
    assert!(listed.status.success(), "{}", text(&listed));
    let stdout = String::from_utf8_lossy(&listed.stdout);
    assert!(stdout.contains("seed spec"), "{}", text(&listed));
    assert!(stdout.contains("explicit spec"), "{}", text(&listed));
}
