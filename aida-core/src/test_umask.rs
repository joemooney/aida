//! Normalize fixture creation permissions before libtest starts its threads.
//! Included only in the core and CLI library unit-test binaries; production
//! permission checks and explicit chmod-based refusal fixtures are unchanged.
// trace:BUG-2573 | ai:codex

#[cfg(target_os = "linux")]
#[used]
#[unsafe(link_section = ".init_array")]
static NORMALIZE_UMASK_BEFORE_MAIN: extern "C" fn() = normalize_umask;

#[cfg(target_os = "linux")]
extern "C" fn normalize_umask() {
    // SAFETY: the ELF initializer runs before main and libtest's worker
    // threads. Never change this process-global setting inside a test.
    unsafe { libc::umask(0o022) };
}

#[cfg(target_os = "linux")]
#[test]
fn fixture_permissions_are_private_under_ambient_group_writable_umask() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("node.toml");
    std::fs::write(&file, "fixture").unwrap();
    assert_eq!(
        std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
        0o644
    );

    let output = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .arg(root.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::metadata(root.path().join(".git"))
            .unwrap()
            .permissions()
            .mode()
            & 0o022,
        0,
        "git children must inherit the normalized umask"
    );
}
