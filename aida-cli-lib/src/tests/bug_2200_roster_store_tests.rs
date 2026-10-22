//! Tests for BUG-2200: the seat check names an unreachable store or missing
//! roster as such, reads a solo store's roster before `objects/` exists, and
//! resolves the same store `aida team` is handed from the same cwd.
// trace:BUG-2200 | ai:claude

use super::*;
use std::fs;
use tempfile::tempdir;

const USER: &str = "bug-2200-user";

/// A project whose `.aida/config.toml` names `.aida-store`, with the store
/// directory present but no `objects/` — a fresh solo store. Returns the
/// store path.
fn solo_store_without_objects(root: &Path) -> PathBuf {
    fs::create_dir_all(root.join(".aida")).unwrap();
    fs::write(
        root.join(".aida").join("config.toml"),
        "store_path = \".aida-store\"\n",
    )
    .unwrap();
    let store = root.join(".aida-store");
    fs::create_dir_all(&store).unwrap();
    store
}

fn write_roster(store: &Path, body: &str) {
    fs::create_dir_all(store.join("registry")).unwrap();
    fs::write(store.join("registry").join("team.toml"), body).unwrap();
}

#[test]
fn solo_store_without_objects_still_reads_its_roster() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let store = solo_store_without_objects(root);
    write_roster(
        &store,
        &format!("[members]\n\"{USER}\" = [\"advisor\", \"implementer\"]\n"),
    );
    assert!(!store.join("objects").exists());

    assert_eq!(roster_store(root).unwrap(), store);
    assert!(roster_allows(root, USER, "advisor"));
    assert!(issuance_roster_store(root, USER, "advisor").is_ok());
    // Fail-closed: a seat or person the roster does not list is refused.
    assert!(!roster_allows(root, USER, "reviewer"));
    assert!(!roster_allows(root, "someone-else", "advisor"));
    let err = issuance_roster_store(root, USER, "reviewer")
        .unwrap_err()
        .to_string();
    assert!(err.contains("not allowed by the team roster"), "{err}");
    // trace:TASK-1953 | ai:claude — the refusal names the roster file it read.
    let roster = store.join("registry").join("team.toml");
    assert!(err.contains(&roster.display().to_string()), "{err}");
}

#[test]
fn store_without_roster_file_is_named_not_blamed_on_roster_contents() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let store = solo_store_without_objects(root);

    assert_eq!(
        roster_store(root),
        Err(RosterStoreError::NoRoster {
            store: store.clone()
        })
    );
    assert!(!roster_allows(root, USER, "advisor"));
    let err = issuance_roster_store(root, USER, "advisor")
        .unwrap_err()
        .to_string();
    assert!(err.contains("has no team roster"), "{err}");
    assert!(err.contains(&store.display().to_string()), "{err}");
    assert!(!err.contains("not allowed by the team roster"), "{err}");
}

#[test]
fn undetected_store_reports_the_probed_path_not_the_roster() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    // A config whose store directory does not exist resolves to nothing.
    fs::create_dir_all(root.join(".aida")).unwrap();
    fs::write(
        root.join(".aida").join("config.toml"),
        "store_path = \".aida-store\"\n",
    )
    .unwrap();

    assert_eq!(
        roster_store(root),
        Err(RosterStoreError::NotDetected {
            probed: root.to_path_buf(),
            cause: UndetectedCause::StoreMissing {
                config: root.join(".aida").join("config.toml"),
                store: root.join(".aida-store"),
            },
        })
    );
    assert!(!roster_allows(root, USER, "advisor"));
    let err = issuance_roster_store(root, USER, "advisor")
        .unwrap_err()
        .to_string();
    assert!(err.contains("no AIDA store was found"), "{err}");
    assert!(err.contains(&root.display().to_string()), "{err}");
    assert!(!err.contains("allow-seat"), "{err}");
    assert!(!err.contains("not allowed by the team roster"), "{err}");
}

/// The seat check (project root from the cwd, then [`roster_store`]) and the
/// dispatcher's cwd walk-up that hands `aida team` its store agree, and both
/// read the same roster entry.
#[test]
fn seat_check_and_team_resolve_the_same_store_from_one_cwd() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let store = solo_store_without_objects(root);
    write_roster(&store, &format!("[members]\n\"{USER}\" = [\"advisor\"]\n"));
    let cwd = root.join("src").join("nested");
    fs::create_dir_all(&cwd).unwrap();

    let team_store = crate::detect_distributed_store_from(&cwd).unwrap();
    let project_root = crate::statusline_root::statusline_project_root_from_with_roots(&cwd, &[]);
    let seat_store = roster_store(&project_root).unwrap();
    assert_eq!(seat_store, team_store);

    let team_view = crate::team::roles_by_user(&team_store);
    assert_eq!(team_view.get(USER).map(String::as_str), Some("advisor"));
    assert!(roster_allows(&project_root, USER, "advisor"));
}

// trace:TASK-2072, TASK-2055 | ai:claude
#[test]
fn undetected_store_names_the_config_and_the_missing_store_it_named() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join(".aida")).unwrap();
    let config = root.join(".aida").join("config.toml");
    fs::write(&config, "store_path = \".aida-store\"\n").unwrap();

    let err = roster_store(root).unwrap_err();
    assert_eq!(
        err,
        RosterStoreError::NotDetected {
            probed: root.to_path_buf(),
            cause: UndetectedCause::StoreMissing {
                config: config.clone(),
                store: root.join(".aida-store"),
            },
        }
    );
    let msg = err.to_string();
    assert!(msg.contains(&config.display().to_string()), "{msg}");
    assert!(
        msg.contains(&root.join(".aida-store").display().to_string()),
        "{msg}"
    );
    assert!(msg.contains("does not exist"), "{msg}");
}

// trace:TASK-2072, TASK-2055 | ai:claude
#[test]
fn undetected_store_cause_separates_no_config_from_a_temp_root_stop() {
    let tmp = tempdir().unwrap();
    let start = tmp.path().join("work");
    fs::create_dir_all(&start).unwrap();

    // The temp dir itself as a guarded root: the walk stops there.
    let roots = vec![tmp.path().to_path_buf()];
    let cause = undetected_cause(&start, &roots);
    assert_eq!(
        cause,
        UndetectedCause::TempRoot {
            root: tmp.path().canonicalize().unwrap()
        }
    );
    let msg = RosterStoreError::NotDetected {
        probed: start.clone(),
        cause,
    }
    .to_string();
    assert!(msg.contains("temp directory"), "{msg}");

    let msg = RosterStoreError::NotDetected {
        probed: start.clone(),
        cause: UndetectedCause::NoConfig,
    }
    .to_string();
    assert!(
        msg.contains("no `.aida/config.toml` exists at or above it"),
        "{msg}"
    );

    // A config with no store_path at all is named as such.
    fs::create_dir_all(start.join(".aida")).unwrap();
    let config = start.join(".aida").join("config.toml");
    fs::write(&config, "[id_format]\npolicy = \"node-aware-only\"\n").unwrap();
    let cause = undetected_cause(&start, &roots);
    assert_eq!(
        cause,
        UndetectedCause::NoStorePath {
            config: config.clone()
        }
    );
}

/// An `AIDA_STORE` that resolves to another store is named in the roster
/// refusal, since `aida team` reads that store while the seat check does not.
// trace:TASK-2072, TASK-2054, TASK-1666 | ai:claude
#[test]
fn roster_refusal_names_a_diverging_aida_store() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("project");
    fs::create_dir_all(&root).unwrap();
    let store = solo_store_without_objects(&root);
    write_roster(&store, &format!("[members]\n\"{USER}\" = [\"advisor\"]\n"));
    // Usable as an override too, so the same-store case below is a real match.
    fs::create_dir_all(store.join("objects")).unwrap();
    let other = tmp.path().join("sandbox");
    fs::create_dir_all(other.join("objects")).unwrap();

    let refusal = roster_check_with_override(&root, USER, "reviewer", Some(&other)).unwrap_err();
    let msg = refusal.describe("seat `reviewer` is refused");
    assert!(
        msg.contains(&roster_path(&store).display().to_string()),
        "{msg}"
    );
    assert!(msg.contains("AIDA_STORE"), "{msg}");
    assert!(
        msg.contains(&other.canonicalize().unwrap().display().to_string()),
        "{msg}"
    );

    // The same store, or an unusable override, adds no note.
    let same = roster_check_with_override(&root, USER, "reviewer", Some(&store)).unwrap_err();
    assert!(!same.describe("x").contains("AIDA_STORE"));
    let bogus = tmp.path().join("nope");
    let unusable = roster_check_with_override(&root, USER, "reviewer", Some(&bogus)).unwrap_err();
    assert!(!unusable.describe("x").contains("AIDA_STORE"));

    // An allowed seat passes and returns the one store it resolved.
    assert_eq!(
        roster_check_with_override(&root, USER, "advisor", Some(&other)).unwrap(),
        store
    );
}
