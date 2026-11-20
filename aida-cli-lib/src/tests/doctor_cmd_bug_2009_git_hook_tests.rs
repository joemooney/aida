// trace:TASK-3780 | ai:codex
use super::*;
use std::os::unix::fs::PermissionsExt;

/// `aida doctor` reports a non-executable AIDA hook (and a stale one),
/// with the hook-only refresh as the remedy; a non-executable custom
/// hook and a healthy repository are silent; the category filter and
/// its aliases select it.
// trace:BUG-2009 | ai:claude
#[test]
fn non_executable_aida_hook_is_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let status = crate::test_git::git_command()
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success());
    let hooks = root.join(".git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    assert!(scan_aida_git_hooks(root).is_empty(), "healthy: silent");

    let wrapped = |name: &str, key: &str| {
        aida_core::scaffolding::wrap_with_aida_header(
            std::path::Path::new(&format!(".git/hooks/{name}")),
            aida_core::templates::EMBEDDED_TEMPLATES[key],
        )
    };
    let put = |name: &str, body: &str, mode: u32| {
        let path = hooks.join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    put(
        "post-commit",
        &wrapped("post-commit", "hooks/aida-post-commit.sh"),
        0o644,
    );
    put(
        "pre-commit",
        &format!(
            "{}\necho old\n",
            wrapped("pre-commit", "hooks/aida-pre-commit.sh").trim_end()
        ),
        0o755,
    );
    put("post-merge", "#!/bin/sh\necho mine\n", 0o644);

    let findings = scan_aida_git_hooks(root);
    let ids: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "aida-hook-stale-pre-commit",
            "aida-hook-not-executable-post-commit"
        ],
        "{findings:?}"
    );
    assert!(findings.iter().all(|f| f.category == "git-hooks"));
    assert!(
        findings[1].summary.contains("silently skips"),
        "{findings:?}"
    );
    assert!(findings
        .iter()
        .all(|f| f.action.contains("aida scaffold upgrade --only git-hooks")));
    assert!(findings.iter().all(|f| !f.safe_heal));
    // Report-only: the doctor scan wrote nothing.
    let mode = std::fs::metadata(hooks.join("post-commit"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o644);

    for raw in [
        "git-hooks",
        "aida-hooks",
        "hook-mode",
        "non-executable-hooks",
    ] {
        assert!(
            doctor_category_selected(Some(raw), "git-hooks").unwrap(),
            "{raw}"
        );
    }
    assert!(!doctor_category_selected(Some("mirror-hook-drift"), "git-hooks").unwrap());
}

/// Outside a git repository the scan is silent.
// trace:BUG-2009 | ai:claude
#[test]
fn non_git_directory_is_silent() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(scan_aida_git_hooks(tmp.path()).is_empty());
}
