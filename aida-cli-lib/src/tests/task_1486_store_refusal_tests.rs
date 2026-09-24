//! Agent-mode rendering of the no-project refusal, and the `init --refresh`
//! seed-target decision in a distributed project whose store isn't attached.
// trace:TASK-1486 | ai:claude

use super::*;

#[test]
fn agent_block_for_no_project_keeps_every_explicit_store_option() {
    let err: anyhow::Error = aida_core::NoProjectFound {
        default_project: Some("other".to_string()),
    }
    .into();
    let msg = format!("{err:?}");
    let block = agent_error_block(&err, &msg);
    assert!(
        block.starts_with("error: \"No AIDA project found here"),
        "{block}"
    );
    assert!(block.contains("\nhelp[5]:\n  - "), "{block}");
    for needle in [
        "aida init",
        "--file <path>",
        "-p <project>",
        "REQ_DB_NAME",
        "AIDA_STORE",
        "-p other",
    ] {
        assert!(block.contains(needle), "missing {needle:?}:\n{block}");
    }
}

#[test]
fn agent_block_finds_the_typed_refusal_under_added_context() {
    let err = anyhow::Error::from(aida_core::NoProjectFound {
        default_project: None,
    })
    .context("while resolving the store");
    let block = agent_error_block(&err, &format!("{err:?}"));
    assert!(block.contains("help[4]:"), "{block}");
    assert!(!block.contains("default project"), "{block}");
}

#[test]
fn agent_block_for_other_errors_keeps_the_single_help_shape() {
    let msg = "Requirement not found: NOSUCH-1\n  \
               Hint: check the spec ID (try `aida list` or `aida search <terms>`).";
    let err = anyhow::anyhow!("{msg}");
    assert_eq!(
        agent_error_block(&err, msg),
        "error: \"Requirement not found: NOSUCH-1\"\nhelp: aida list"
    );
}

#[test]
fn refresh_seeds_the_distributed_store_when_attached() {
    let store = std::path::PathBuf::from("/p/.aida-store");
    let got = refresh_seed_target(Some(store.clone()), false, || {
        panic!("legacy resolver must not run when the distributed store resolves")
    });
    assert_eq!(got, RefreshSeedTarget::Store(store));
}

#[test]
fn refresh_never_seeds_a_legacy_db_in_an_unattached_distributed_project() {
    let got = refresh_seed_target(None, true, || {
        panic!("legacy resolver must not run in a distributed project")
    });
    assert_eq!(got, RefreshSeedTarget::DistributedUnattached);
}

#[test]
fn refresh_uses_the_legacy_store_only_outside_distributed_mode() {
    let legacy = std::path::PathBuf::from("requirements.db");
    let got = refresh_seed_target(None, false, || Ok(legacy.clone()));
    assert_eq!(got, RefreshSeedTarget::Store(legacy));
    let none = refresh_seed_target(None, false, || anyhow::bail!("no store"));
    assert_eq!(none, RefreshSeedTarget::NoStore);
}

#[test]
fn a_declared_distributed_config_marks_the_project_distributed() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    std::fs::write(
        tmp.path().join(".aida/config.toml"),
        "mode = \"distributed\"\n",
    )
    .unwrap();
    let sub = tmp.path().join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    assert_eq!(
        unattached_distributed_root(&sub).as_deref(),
        Some(tmp.path())
    );
    let plain = tempfile::TempDir::new().unwrap();
    assert!(unattached_distributed_root(plain.path()).is_none());
}
