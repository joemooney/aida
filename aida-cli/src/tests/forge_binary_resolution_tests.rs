use super::{resolve_forge_cli, resolve_glab_binary};
use crate::forge::ForgeKind;
use std::path::PathBuf;

#[test]
fn glab_binary_honors_test_override() {
    let _g = crate::test_env::EnvVarGuard::set("AIDA_TEST_GLAB_BINARY", "/tmp/fake-glab");
    assert_eq!(resolve_glab_binary(), Some(PathBuf::from("/tmp/fake-glab")));
}

#[test]
fn forge_cli_dispatches_by_kind() {
    // EnvVarGuard holds a process-global lock for its whole lifetime, so
    // each guard must drop before the next is constructed — two live at
    // once would deadlock the second set() on the same thread. Scope them.
    {
        let _g = crate::test_env::EnvVarGuard::set("AIDA_TEST_GH_BINARY", "/tmp/fake-gh");
        assert_eq!(
            resolve_forge_cli(ForgeKind::GitHub),
            Some(PathBuf::from("/tmp/fake-gh"))
        );
    }
    {
        let _g = crate::test_env::EnvVarGuard::set("AIDA_TEST_GLAB_BINARY", "/tmp/fake-glab");
        assert_eq!(
            resolve_forge_cli(ForgeKind::GitLab),
            Some(PathBuf::from("/tmp/fake-glab"))
        );
    }
    assert_eq!(resolve_forge_cli(ForgeKind::None), None);
}
