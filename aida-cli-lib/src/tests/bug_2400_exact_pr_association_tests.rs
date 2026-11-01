//! GitHub search is candidate retrieval, including shared UUID search hits.
// trace:BUG-2400 | ai:codex
use super::*;

#[test]
fn bug_2400_search_candidates_require_exact_store_association() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let init = crate::test_git::git_command()
        .args(["init", "-q"])
        .arg(root)
        .output()
        .unwrap();
    assert!(init.status.success(), "{init:?}");
    std::fs::create_dir(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[deployment]\nmode = 'distributed'\nstore_path = '.aida-store'\n",
    )
    .unwrap();
    let node = "b6b1278890774ef993d2ebc327862470";
    let canonical = format!("BUG-{node}-166");
    let old = format!("BUG-{node}-165");
    let unrelated = format!("BUG-{node}-057");
    let mut map = aida_core::node_alias::NodeAliasMap::default();
    map.set("HOME", node).unwrap();
    map.rename("HOME", "DESK").unwrap();
    map.retired_ids
        .insert(aida_core::id_identity::id_key(&old), canonical.clone());
    let _map = aida_core::node_alias::install_for_thread(map);
    let backend = crate::test_env::home_fixture_backend(&root.join(".aida-store"))
        .unwrap()
        .with_auto_commit(false);
    let mut wanted = aida_core::Requirement::new("wanted".into(), String::new());
    wanted.spec_id = Some(canonical.clone());
    wanted.agreed_id = Some("BUG-2392".into());
    let mut other = aida_core::Requirement::new("other".into(), String::new());
    other.spec_id = Some(unrelated.clone());
    let mut store = RequirementsStore::new();
    store.requirements = vec![wanted, other];
    backend.save(&store).unwrap();

    let gh = root.join("gh");
    crate::test_exec::write_executable(
        &gh,
        r#"#!/bin/sh
base=$(dirname "$0")
printf '%s\n' "$*" >> "$base/args"
case "$*" in
  api*)
    pr=${2#*/pulls/}; pr=${pr%%/*}
    if [ -f "$base/fail-$pr" ]; then cat "$base/fail-$pr" >&2; exit 1; fi
    if [ -f "$base/slow-$pr" ]; then sleep 2; fi
    case "$2" in
      *'/commits?'*)
        page=${2##*page=}
        if [ -f "$base/fail-page-$page" ]; then echo 'page failed' >&2; exit 1; fi
        if [ "$pr" = 3200 ]; then cat "$base/commits-$page"; else echo '[]'; fi ;;
      *) if [ "$pr" = 3200 ]; then cat "$base/detail";
         else echo '{"commits":0,"head":{"sha":"0000000000000000000000000000000000000000"}}'; fi ;;
    esac ;;
  *'--search'*) cat "$base/search" ;;
  *) cat "$base/all" ;;
esac
"#,
    );
    let _gh = crate::test_env::EnvVarGuard::set("AIDA_TEST_GH_BINARY", &gh);
    std::fs::write(root.join("commits-1"), "[]").unwrap();
    std::fs::write(
        root.join("detail"),
        r#"{"commits":0,"head":{"sha":"0000000000000000000000000000000000000000"}}"#,
    )
    .unwrap();
    std::fs::write(root.join("all"), "").unwrap();
    let row = |n, title: &str, branch: &str| {
        format!("{n}\t{title}\thttps://example.test/pr/{n}\t{branch}\n")
    };
    let noise = row(
        3177,
        &format!("fix ({unrelated})"),
        &format!("codex/{unrelated}"),
    );
    for requested in [&canonical, "BUG-2392", "BUG-HOME-166", "BUG-DESK-166", &old] {
        for (title, branch) in [
            (format!("fix ({canonical})"), "unscoped".to_string()),
            ("fix (BUG-2392)".into(), "unscoped".to_string()),
            ("fix".into(), "codex/bug-home-166-work".to_string()),
            ("fix".into(), "codex/bug-desk-166-work".to_string()),
            (format!("fix ({old})"), "unscoped".to_string()),
        ] {
            std::fs::write(
                root.join("search"),
                format!("{noise}{}", row(3200, &title, &branch)),
            )
            .unwrap();
            assert!(
                matches!(detect_open_pr_for_spec_github(root, requested), PrLookup::Found(pr) if pr.number == 3200),
                "{requested}: {title}, {branch}"
            );
            assert_eq!(
                all_open_prs_for_spec_via_forge(root, requested)
                    .ok()
                    .unwrap(),
                vec![(3200, branch)]
            );
        }
    }
    // Substrings, shared nodes and tokenized search hits do not claim work.
    let false_hits = format!(
        "{noise}{}{}{}{}",
        row(3205, "fix", "codex/bug-2392-100-work"),
        row(3201, &format!("fix ({canonical}0)"), "unscoped"),
        row(3202, "fix (BUG-23920)", "codex/bug-23920"),
        row(3203, "BUG 2392", "codex/unrelated")
    );
    std::fs::write(root.join("search"), &false_hits).unwrap();
    std::fs::write(root.join("all"), &false_hits).unwrap();
    assert!(matches!(
        detect_open_pr_for_spec_github(root, &canonical),
        PrLookup::NoOpenPr
    ));
    assert!(all_open_prs_for_spec_via_forge(root, &canonical)
        .ok()
        .unwrap()
        .is_empty());

    // Alias searches may miss; an exact branch or commit trailer still protects work.
    std::fs::write(root.join("all"), row(3200, "fix", "codex/bug-2392-work")).unwrap();
    assert!(
        matches!(detect_open_pr_for_spec_github(root, &canonical), PrLookup::Found(pr) if pr.number == 3200)
    );
    assert_eq!(
        all_open_prs_for_spec_via_forge(root, &canonical)
            .ok()
            .unwrap(),
        vec![(3200, "codex/bug-2392-work".into())]
    );
    std::fs::write(root.join("all"), row(3200, "fix", "unscoped")).unwrap();
    let commit = |n: u64, message: &str| serde_json::json!({"sha":format!("{n:040x}"),"commit":{"message":message}});
    std::fs::write(
        root.join("commits-1"),
        serde_json::to_vec(&vec![commit(1, "fix: actual work (BUG-2392)")]).unwrap(),
    )
    .unwrap();
    std::fs::write(
        root.join("detail"),
        serde_json::json!({"commits":1,"head":{"sha":format!("{:040x}",1)}}).to_string(),
    )
    .unwrap();
    assert!(
        matches!(detect_open_pr_for_spec_github(root, &canonical), PrLookup::Found(pr) if pr.number == 3200)
    );

    assert_eq!(
        all_open_prs_for_spec_via_forge(root, &canonical)
            .ok()
            .unwrap(),
        vec![(3200, "unscoped".into())]
    );

    // Typed failures never prove absence. An earlier unreadable candidate
    // must not prevent inspection of a later commit-associated match.
    for (message, network) in [
        ("authentication failed", false),
        ("could not resolve host: api.github.com", true),
    ] {
        std::fs::write(root.join("search"), &noise).unwrap();
        std::fs::write(root.join("fail-3200"), message).unwrap();
        let lookup = detect_open_pr_for_spec_github(root, &canonical);
        assert!(if network {
            matches!(lookup, PrLookup::GhUnreachable(_))
        } else {
            matches!(lookup, PrLookup::GhFailed(_))
        });
        assert!(all_open_prs_for_spec_via_forge(root, &canonical).is_err());
        std::fs::remove_file(root.join("fail-3200")).unwrap();
    }
    std::fs::write(root.join("slow-3200"), "").unwrap();
    assert!(matches!(
        with_forge_lookup_timeout(std::time::Duration::from_millis(100), || {
            detect_open_pr_for_spec_github(root, &canonical)
        }),
        PrLookup::GhUnreachable(_)
    ));
    assert!(
        with_forge_lookup_timeout(std::time::Duration::from_millis(100), || {
            all_open_prs_for_spec_via_forge(root, &canonical)
        })
        .is_err()
    );
    std::fs::remove_file(root.join("slow-3200")).unwrap();
    std::fs::write(root.join("fail-3177"), "authentication failed").unwrap();
    assert!(
        matches!(detect_open_pr_for_spec_github(root, &canonical), PrLookup::Found(pr) if pr.number == 3200)
    );
    assert!(all_open_prs_for_spec_via_forge(root, &canonical).is_err());
    std::fs::remove_file(root.join("fail-3177")).unwrap();

    // Commit 101 must protect the PR, including a title-visible duplicate.
    let first_page: Vec<_> = (1..=100).map(|n| commit(n, "ordinary work")).collect();
    std::fs::write(
        root.join("commits-1"),
        serde_json::to_vec(&first_page).unwrap(),
    )
    .unwrap();
    std::fs::write(
        root.join("commits-2"),
        serde_json::to_vec(&vec![commit(101, "late work (BUG-2392)")]).unwrap(),
    )
    .unwrap();
    std::fs::write(
        root.join("detail"),
        serde_json::json!({"commits":101,"head":{"sha":format!("{:040x}",101)}}).to_string(),
    )
    .unwrap();
    assert!(
        matches!(detect_open_pr_for_spec_github(root, &canonical), PrLookup::Found(pr) if pr.number == 3200)
    );
    assert_eq!(
        all_open_prs_for_spec_via_forge(root, &canonical)
            .ok()
            .unwrap(),
        vec![(3200, "unscoped".into())]
    );
    std::fs::write(root.join("search"), row(3204, "fix (BUG-2392)", "visible")).unwrap();
    assert_eq!(
        all_open_prs_for_spec_via_forge(root, &canonical)
            .ok()
            .unwrap(),
        vec![(3204, "visible".into()), (3200, "unscoped".into())]
    );
    // A later page failure cannot return a complete duplicate set. Existence
    // remains proven by the visible title; without it the result is unknown.
    std::fs::write(root.join("fail-page-2"), "").unwrap();
    assert!(
        matches!(detect_open_pr_for_spec_github(root, &canonical), PrLookup::Found(pr) if pr.number == 3204)
    );
    assert!(all_open_prs_for_spec_via_forge(root, &canonical).is_err());
    std::fs::write(root.join("search"), "").unwrap();
    assert!(matches!(
        detect_open_pr_for_spec_github(root, &canonical),
        PrLookup::GhFailed(_)
    ));
    assert!(all_open_prs_for_spec_via_forge(root, &canonical).is_err());
    std::fs::remove_file(root.join("fail-page-2")).unwrap();
    // Successful malformed JSON/schema, duplicate pages and endpoint ceilings
    // are unknown. None of these empty/short responses proves absence.
    for bad in [
        "",
        "null",
        "{}",
        "[{}]",
        "[]",
        r#"[{"sha":7,"commit":{"message":"work"}}]"#,
    ] {
        std::fs::write(root.join("commits-2"), bad).unwrap();
        assert!(
            matches!(
                detect_open_pr_for_spec_github(root, &canonical),
                PrLookup::GhFailed(_)
            ),
            "{bad}"
        );
        assert!(all_open_prs_for_spec_via_forge(root, &canonical).is_err());
    }
    std::fs::write(
        root.join("commits-2"),
        serde_json::to_vec(&first_page).unwrap(),
    )
    .unwrap();
    assert!(all_open_prs_for_spec_via_forge(root, &canonical).is_err());
    for detail in [
        "{}",
        r#"{"commits":"101"}"#,
        r#"{"commits":101,"head":{"sha":"bad"}}"#,
    ] {
        std::fs::write(root.join("detail"), detail).unwrap();
        assert!(matches!(
            detect_open_pr_for_spec_github(root, &canonical),
            PrLookup::GhFailed(_)
        ));
    }
    // No-match complete pages are known absence; inconsistent head/count is unknown.
    std::fs::write(
        root.join("commits-2"),
        serde_json::to_vec(&vec![commit(101, "ordinary work")]).unwrap(),
    )
    .unwrap();
    std::fs::write(
        root.join("detail"),
        serde_json::json!({"commits":101,"head":{"sha":format!("{:040x}",101)}}).to_string(),
    )
    .unwrap();
    assert!(matches!(
        detect_open_pr_for_spec_github(root, &canonical),
        PrLookup::NoOpenPr
    ));
    std::fs::write(
        root.join("detail"),
        serde_json::json!({"commits":102,"head":{"sha":format!("{:040x}",102)}}).to_string(),
    )
    .unwrap();
    assert!(all_open_prs_for_spec_via_forge(root, &canonical).is_err());

    // An endpoint returning fewer commits than the declared count, a moving
    // head, oversized pages, and the explicit page ceiling remain unknown.
    std::fs::write(
        root.join("detail"),
        serde_json::json!({"commits":101,"head":{"sha":format!("{:040x}",999)}}).to_string(),
    )
    .unwrap();
    assert!(all_open_prs_for_spec_via_forge(root, &canonical).is_err());
    std::fs::write(
        root.join("commits-1"),
        serde_json::to_vec(&(1..=101).map(|n| commit(n, "work")).collect::<Vec<_>>()).unwrap(),
    )
    .unwrap();
    assert!(all_open_prs_for_spec_via_forge(root, &canonical).is_err());
    std::fs::write(
        root.join("detail"),
        serde_json::json!({"commits":1001,"head":{"sha":format!("{:040x}",1001)}}).to_string(),
    )
    .unwrap();
    for page in 1..=10 {
        let rows: Vec<_> = (((page - 1) * 100 + 1)..=(page * 100))
            .map(|n| commit(n, "work"))
            .collect();
        std::fs::write(
            root.join(format!("commits-{page}")),
            serde_json::to_vec(&rows).unwrap(),
        )
        .unwrap();
    }
    assert!(matches!(
        detect_open_pr_for_spec_github(root, &canonical),
        PrLookup::GhFailed(_)
    ));
    assert!(all_open_prs_for_spec_via_forge(root, &canonical).is_err());
    let args = std::fs::read_to_string(root.join("args")).unwrap();
    assert!(!args.contains("&page=11"));

    // Keep every real duplicate for the existing canonical-selection policy.
    std::fs::write(
        root.join("search"),
        format!(
            "{noise}{}{}",
            row(3200, "fix (BUG-2392)", "first"),
            row(3204, "fix", "bug-2392-rework")
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("all"),
        format!(
            "{}{}",
            row(3204, "fix", "bug-2392-rework"),
            row(3207, "fix", "bug-desk-166-work")
        ),
    )
    .unwrap();
    assert_eq!(
        all_open_prs_for_spec_via_forge(root, &canonical)
            .ok()
            .unwrap(),
        vec![
            (3200, "first".into()),
            (3204, "bug-2392-rework".into()),
            (3207, "bug-desk-166-work".into())
        ]
    );
    // A finite list ceiling cannot prove absence or a complete duplicate set.
    let saturated = (4000..4101)
        .map(|n| row(n, "fix", "unscoped"))
        .collect::<String>();
    std::fs::write(root.join("search"), "").unwrap();
    std::fs::write(root.join("all"), saturated).unwrap();
    assert!(matches!(
        detect_open_pr_for_spec_github(root, &canonical),
        PrLookup::GhFailed(_)
    ));
    assert!(all_open_prs_for_spec_via_forge(root, &canonical).is_err());
    let args = std::fs::read_to_string(root.join("args")).unwrap();
    assert!(args
        .lines()
        .filter(|l| l.contains("--search"))
        .all(|l| l.contains("--limit 101")));

    // A literal owner of an alias wins over label expansion. Do not claim
    // that owner's PR on behalf of the canonical target.
    let mut literal = aida_core::Requirement::new("literal".into(), String::new());
    literal.spec_id = Some("BUG-HOME-166".into());
    store.requirements.push(literal);
    backend.save(&store).unwrap();
    std::fs::write(
        root.join("search"),
        row(3206, "fix (BUG-HOME-166)", "codex/bug-home-166-work"),
    )
    .unwrap();
    std::fs::write(root.join("all"), "").unwrap();
    std::fs::write(root.join("commits-1"), "[]").unwrap();
    std::fs::write(
        root.join("detail"),
        r#"{"commits":0,"head":{"sha":"0000000000000000000000000000000000000000"}}"#,
    )
    .unwrap();
    assert!(matches!(
        detect_open_pr_for_spec_github(root, &canonical),
        PrLookup::NoOpenPr
    ));
    assert!(all_open_prs_for_spec_via_forge(root, &canonical)
        .ok()
        .unwrap()
        .is_empty());

    let original_gh = std::fs::read(&gh).unwrap();
    // Exercise actual PATH resolution, without the test-binary shortcut.
    // The fallback policy is preserved and one collection probes only once.
    drop(_gh);
    let mut production_env = crate::test_env::EnvVarsGuard::set(&[
        ("PATH", root.to_str().unwrap()),
        ("AIDA_TEST_FORGE_FALLBACK_DIRS", root.to_str().unwrap()),
    ]);
    crate::test_exec::write_executable(&gh, "#!/bin/sh\nexec /bin/sleep 30\n");
    let started = std::time::Instant::now();
    assert!(matches!(
        with_forge_lookup_timeout(std::time::Duration::from_millis(100), || {
            detect_open_pr_for_spec_github(root, &canonical)
        }),
        PrLookup::GhUnreachable(_)
    ));
    assert!(
        with_forge_lookup_timeout(std::time::Duration::from_millis(100), || {
            all_open_prs_for_spec_via_forge(root, &canonical)
        })
        .is_err()
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    let fallback = root.join("fallback");
    std::fs::create_dir(&fallback).unwrap();
    crate::test_exec::write_executable(&gh, "#!/bin/sh\nexit 1\n");
    crate::test_exec::write_executable(
        &fallback.join("gh"),
        format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo probe >> '{}/probes'; fi\nexit 0\n",
            root.display()
        ),
    );
    production_env.set_key("AIDA_TEST_FORGE_FALLBACK_DIRS", &fallback);
    std::fs::write(root.join("search"), "").unwrap();
    assert!(matches!(
        detect_open_pr_for_spec_github(root, &canonical),
        PrLookup::NoOpenPr
    ));
    assert_eq!(
        std::fs::read_to_string(root.join("probes"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    drop(production_env);
    std::fs::write(&gh, original_gh).unwrap();
    let _gh = crate::test_env::EnvVarGuard::set("AIDA_TEST_GH_BINARY", &gh);

    // Malformed rows and forge failures are unknown/error, never absence.
    std::fs::write(root.join("search"), "bad row\n").unwrap();
    assert!(matches!(
        detect_open_pr_for_spec_github(root, &canonical),
        PrLookup::GhFailed(_)
    ));
    for (stderr, network) in [
        ("authentication failed", false),
        ("could not resolve host: api.github.com", true),
    ] {
        crate::test_exec::write_executable(
            &gh,
            format!("#!/bin/sh\necho '{stderr}' >&2\nexit 1\n"),
        );
        let lookup = detect_open_pr_for_spec_github(root, &canonical);
        assert!(if network {
            matches!(lookup, PrLookup::GhUnreachable(_))
        } else {
            matches!(lookup, PrLookup::GhFailed(_))
        });
    }
}
