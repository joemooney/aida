//! trace:BUG-2515 | ai:codex
use super::*;

#[test]
fn bug_2515_gitlab_ci_detection() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(!repo_has_ci_workflows(tmp.path()));
    std::fs::create_dir(tmp.path().join(".gitlab-ci.yml")).unwrap();
    assert!(!repo_has_ci_workflows(tmp.path()));
    std::fs::remove_dir(tmp.path().join(".gitlab-ci.yml")).unwrap();
    std::fs::write(tmp.path().join(".gitlab-ci.yml"), "include: ci/build.yml\n").unwrap();
    assert!(repo_has_ci_workflows(tmp.path()));
}

#[test]
fn bug_2515_configured_gitlab_metadata_and_ci_use_glab() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[forge]\nprovider = \"gitlab\"\n",
    )
    .unwrap();
    let bin = root.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let calls = root.join("calls");
    let script = format!(
        r#"#!/bin/sh
[ "$1" = --version ] && exit 0
printf '%s\n' "$*" >> '{}'
case "$*" in
  *merge_requests/2*) echo '{{"title":"Ship title","description":"aida: no-complete","state":"opened","detailed_merge_status":"conflict","source_branch":"topic","target_branch":"main","sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}' ;;
  *pipelines/1/jobs*) echo '[{{"id":5,"name":"build","status":"failed"}}]' ;;
  *pipelines*) echo '[{{"id":1,"status":"failed","ref":"topic"}}]' ;;
  *) exit 91 ;;
esac
"#,
        calls.display()
    );
    crate::test_exec::write_executable(&bin.join("glab"), script);
    crate::test_exec::write_executable(
        &bin.join("gh"),
        "#!/bin/sh\n[ \"$1\" = --version ] && exit 0\nexit 92\n",
    );
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let _path = crate::test_env::EnvVarGuard::set("PATH", std::env::join_paths(paths).unwrap());
    let meta = fetch_pr_ship_metadata_via_forge(root, 2).unwrap();
    assert_eq!(meta.title, "Ship title");
    assert_eq!(meta.body, "aida: no-complete");
    assert_eq!(meta.number, Some(2));
    let kind = crate::forge::resolve_forge_kind(root);
    assert_eq!(kind, crate::forge::ForgeKind::GitLab);
    let info = fetch_change_info_via_resolved_forge(root, 2, kind).unwrap();
    assert_eq!(info.head_ref, "topic");
    let forge = crate::forge::forge_for_kind(root, kind);
    let change = crate::forge::ChangeRef {
        id: 2,
        url: String::new(),
        branch: "topic".into(),
        base: "main".into(),
        title: None,
    };
    // A terminal red pipeline exercises ship's real wait without sleeping or merging.
    let result = crate::ci_gate::wait_for_ship_ci(
        root,
        forge.as_ref(),
        &change,
        false,
        false,
        Some(std::time::Duration::ZERO),
        std::time::Duration::ZERO,
    );
    let error = result.unwrap_err();
    assert!(format!("{error:#}").contains("CI is red"), "{error:#}");
    let error = forge
        .merge_change(
            &change,
            &crate::forge::MergeOptions::squash(),
            &mut crate::network_retry::NoopSink,
        )
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("detailed_merge_status=`conflict`"),
        "{error:#}"
    );
    let log = std::fs::read_to_string(calls).unwrap();
    assert!(log.contains("merge_requests/2"), "{log}");
    assert!(log.contains("pipelines"), "{log}");
}

#[test]
fn bug_2515_github_title_and_body_preserved() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[forge]\nprovider = \"github\"\n",
    )
    .unwrap();
    let gh = root.join("gh");
    crate::test_exec::write_executable(
        &gh,
        r#"#!/bin/sh
[ "$1" = --version ] && exit 0
[ "$*" = 'pr view 7 --json title,body' ] || exit 93
echo '{"title":"GitHub title","body":"GitHub body"}'
"#,
    );
    let mut paths = vec![root.to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let _path = crate::test_env::EnvVarGuard::set("PATH", std::env::join_paths(paths).unwrap());
    let meta = fetch_pr_ship_metadata_via_forge(root, 7).unwrap();
    assert_eq!(meta.title, "GitHub title");
    assert_eq!(meta.body, "GitHub body");
    assert_eq!(meta.number, Some(7));
}
