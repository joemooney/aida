//! GitHub search is candidate retrieval, including shared UUID search hits.
// trace:BUG-2400 | ai:codex
use super::*;

#[test]
fn bug_2400_search_candidates_require_exact_store_association() {
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
  *'pr view'*) cat "$base/commits" ;;
  *'--search'*) cat "$base/search" ;;
  *) cat "$base/all" ;;
esac
"#,
    );
    let _gh = crate::test_env::EnvVarGuard::set("AIDA_TEST_GH_BINARY", &gh);
    std::fs::write(root.join("commits"), "").unwrap();
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
                all_open_prs_for_spec_via_forge(root, requested),
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
    assert!(all_open_prs_for_spec_via_forge(root, &canonical).is_empty());

    // Alias searches may miss; an exact branch or commit trailer still protects work.
    std::fs::write(root.join("all"), row(3200, "fix", "codex/bug-2392-work")).unwrap();
    assert!(
        matches!(detect_open_pr_for_spec_github(root, &canonical), PrLookup::Found(pr) if pr.number == 3200)
    );
    std::fs::write(root.join("all"), row(3200, "fix", "unscoped")).unwrap();
    std::fs::write(root.join("commits"), "fix: actual work (BUG-2392)\n").unwrap();
    assert!(
        matches!(detect_open_pr_for_spec_github(root, &canonical), PrLookup::Found(pr) if pr.number == 3200)
    );

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
    assert_eq!(
        all_open_prs_for_spec_via_forge(root, &canonical),
        vec![(3200, "first".into()), (3204, "bug-2392-rework".into())]
    );
    let args = std::fs::read_to_string(root.join("args")).unwrap();
    assert!(args
        .lines()
        .filter(|l| l.contains("--search"))
        .all(|l| l.contains("--limit 100")));

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
    std::fs::write(root.join("commits"), "").unwrap();
    assert!(matches!(
        detect_open_pr_for_spec_github(root, &canonical),
        PrLookup::NoOpenPr
    ));
    assert!(all_open_prs_for_spec_via_forge(root, &canonical).is_empty());

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
