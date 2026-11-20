use super::distributed_init_footprint;
use crate::cli::InitFootprint;

// trace:TASK-b6b1278890774ef993d2ebc327862470-036 | ai:claude
#[test]
fn an_adopted_clone_never_reaches_the_setup_style_prompt() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
    let manifest = tmp.path().join(aida_core::adoption_projection::MANIFEST);
    std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
    std::fs::write(&manifest, "").unwrap();

    let got = distributed_init_footprint(tmp.path(), None, |_, _| {
        panic!("the Setup style prompt path was reached in an adopted clone")
    })
    .unwrap();
    assert_eq!(got, None);
}

#[test]
fn a_plain_repo_still_resolves_its_footprint() {
    let tmp = tempfile::TempDir::new().unwrap();
    aida_core::git_ops::init(tmp.path()).unwrap();

    let got = distributed_init_footprint(tmp.path(), None, |_, explicit| {
        assert_eq!(explicit, None);
        Ok(InitFootprint::Minimal)
    })
    .unwrap();
    assert_eq!(got, Some(InitFootprint::Minimal));
}
