//! GitHub search is candidate retrieval, including shared UUID search hits.
// trace:BUG-2400 | ai:codex
use super::*;

// Construct only after run_path_isolated_test; retain both the map and EnvLock
// guards until the isolated test finishes. Each test owns its fake store/forge.
struct Fixture {
    dir: tempfile::TempDir,
    canonical: String,
    old: String,
    unrelated: String,
    backend: aida_core::GitBackend,
    store: RequirementsStore,
    _map: aida_core::node_alias::ThreadMapGuard,
    gh_guard: Option<crate::test_env::EnvVarGuard>,
}

impl Fixture {
    fn new() -> Self {
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
base=${0%/*}
printf '%s\n' "$*" >> "$base/args"
if [ -f "$base/latency" ]; then sleep 0.2; fi
case "$*" in
  'api graphql'*)
    if [ -f "$base/graphql" ]; then exec cat "$base/graphql"; fi
    echo '{}' ;;
  api*)
    pr=${2#*/pulls/}; pr=${pr%%/*}
    if [ -f "$base/fail-$pr" ]; then cat "$base/fail-$pr" >&2; exit 1; fi
    if [ -f "$base/slow-$pr" ]; then sleep 2; fi
    case "$2" in
      *'/commits?'*)
        page=${2##*page=}
        if [ -f "$base/fail-page-$page" ]; then echo 'page failed' >&2; exit 1; fi
        if [ "$pr" = 3200 ]; then
          if [ -f "$base/detail-after-page" ]; then
            cat "$base/commits-$page"
            cp "$base/detail-after-page" "$base/detail"
          else exec cat "$base/commits-$page"; fi
        else echo '[]'; fi ;;
      *) if [ "$pr" = 3200 ]; then exec cat "$base/detail";
         else echo '{"commits":0,"head":{"sha":"0000000000000000000000000000000000000000"}}'; fi ;;
    esac ;;
  *'--search'*) exec cat "$base/search" ;;
  *) exec cat "$base/all" ;;
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
        std::fs::write(root.join("search"), "").unwrap();
        Self {
            dir,
            canonical,
            old,
            unrelated,
            backend,
            store,
            _map,
            gh_guard: Some(_gh),
        }
    }
}

fn row(n: u64, title: &str, branch: &str) -> String {
    format!("{n}\t{title}\thttps://example.test/pr/{n}\t{branch}\n")
}

fn commit(n: u64, message: &str) -> serde_json::Value {
    serde_json::json!({"sha":format!("{n:040x}"),"commit":{"message":message}})
}

// trace:BUG-2400 | ai:codex
#[test]
fn bug_2400_aliases_tokens_and_literal_ownership() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let mut f = Fixture::new();
    let root = f.dir.path();
    let canonical = f.canonical.clone();
    let old = f.old.clone();
    let noise = row(
        3177,
        &format!("fix ({})", f.unrelated),
        &format!("codex/{}", f.unrelated),
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
    // A literal owner of an alias wins over label expansion. Do not claim
    // that owner's PR on behalf of the canonical target.
    let mut literal = aida_core::Requirement::new("literal".into(), String::new());
    literal.spec_id = Some("BUG-HOME-166".into());
    f.store.requirements.push(literal);
    f.backend.save(&f.store).unwrap();
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
}

// trace:BUG-2400 | ai:codex
#[test]
fn bug_2400_commit_paging_head_binding_and_schema() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new();
    let root = f.dir.path();
    let canonical = f.canonical.clone();
    let noise = row(
        3177,
        &format!("fix ({})", f.unrelated),
        &format!("codex/{}", f.unrelated),
    );
    std::fs::write(root.join("search"), &noise).unwrap();
    std::fs::write(root.join("all"), row(3200, "fix", "unscoped")).unwrap();
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

    // A matching trailer from a different head is unknown, including when
    // it would otherwise pollute a title-visible duplicate set.
    let valid_detail = std::fs::read(root.join("detail")).unwrap();
    std::fs::write(
        root.join("detail"),
        serde_json::json!({"commits":1,"head":{"sha":format!("{:040x}",2)}}).to_string(),
    )
    .unwrap();
    assert!(matches!(
        detect_open_pr_for_spec_github(root, &canonical),
        PrLookup::GhFailed(_)
    ));
    assert!(all_open_prs_for_spec_via_forge(root, &canonical).is_err());
    std::fs::write(root.join("search"), row(3204, "fix (BUG-2392)", "visible")).unwrap();
    assert!(
        matches!(detect_open_pr_for_spec_github(root, &canonical), PrLookup::Found(pr) if pr.number == 3204)
    );
    assert!(all_open_prs_for_spec_via_forge(root, &canonical).is_err());
    std::fs::write(root.join("search"), &noise).unwrap();
    // A complete matching page cannot authorize a positive after either
    // count or head changes across requests. Missing recheck data is unknown.
    for changed in [
        serde_json::json!({"commits":1,"head":{"sha":format!("{:040x}",2)}}),
        serde_json::json!({"commits":2,"head":{"sha":format!("{:040x}",1)}}),
        serde_json::json!({}),
    ] {
        std::fs::write(root.join("detail-after-page"), changed.to_string()).unwrap();
        std::fs::write(root.join("detail"), &valid_detail).unwrap();
        assert!(matches!(
            detect_open_pr_for_spec_github(root, &canonical),
            PrLookup::GhFailed(_)
        ));
        std::fs::write(root.join("detail"), &valid_detail).unwrap();
        assert!(all_open_prs_for_spec_via_forge(root, &canonical).is_err());
    }
    std::fs::remove_file(root.join("detail-after-page")).unwrap();
    std::fs::write(root.join("detail"), &valid_detail).unwrap();

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
    // An early matching first page must still be bound to the final head.
    let mut early_page = first_page.clone();
    early_page[0] = commit(1, "early work (BUG-2392)");
    std::fs::write(
        root.join("commits-1"),
        serde_json::to_vec(&early_page).unwrap(),
    )
    .unwrap();
    std::fs::write(
        root.join("commits-2"),
        serde_json::to_vec(&vec![commit(101, "work")]).unwrap(),
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
    std::fs::write(root.join("fail-page-2"), "").unwrap();
    assert!(matches!(
        detect_open_pr_for_spec_github(root, &canonical),
        PrLookup::GhFailed(_)
    ));
    assert!(all_open_prs_for_spec_via_forge(root, &canonical).is_err());
    std::fs::remove_file(root.join("fail-page-2")).unwrap();
    let stable_detail = std::fs::read(root.join("detail")).unwrap();
    for changed in [
        serde_json::json!({"commits":101,"head":{"sha":format!("{:040x}",999)}}),
        serde_json::json!({"commits":102,"head":{"sha":format!("{:040x}",101)}}),
    ] {
        std::fs::write(root.join("detail-after-page"), changed.to_string()).unwrap();
        std::fs::write(root.join("detail"), &stable_detail).unwrap();
        assert!(matches!(
            detect_open_pr_for_spec_github(root, &canonical),
            PrLookup::GhFailed(_)
        ));
        std::fs::write(root.join("detail"), &stable_detail).unwrap();
        assert!(all_open_prs_for_spec_via_forge(root, &canonical).is_err());
    }
    std::fs::remove_file(root.join("detail-after-page")).unwrap();
    std::fs::write(root.join("detail"), &stable_detail).unwrap();
    // Exact 250 succeeds; the endpoint's 250/251 truncation is unknown even
    // with an early match, rather than a claimed positive or complete set.
    std::fs::write(
        root.join("commits-2"),
        serde_json::to_vec(&(101..=200).map(|n| commit(n, "work")).collect::<Vec<_>>()).unwrap(),
    )
    .unwrap();
    std::fs::write(
        root.join("commits-3"),
        serde_json::to_vec(&(201..=250).map(|n| commit(n, "work")).collect::<Vec<_>>()).unwrap(),
    )
    .unwrap();
    for count in [250, 251] {
        std::fs::write(
            root.join("detail"),
            serde_json::json!({"commits":count,"head":{"sha":format!("{count:040x}")}}).to_string(),
        )
        .unwrap();
        let lookup = detect_open_pr_for_spec_github(root, &canonical);
        if count == 250 {
            assert!(matches!(lookup, PrLookup::Found(pr) if pr.number == 3200));
            assert_eq!(
                all_open_prs_for_spec_via_forge(root, &canonical)
                    .ok()
                    .unwrap(),
                vec![(3200, "unscoped".into())]
            );
        } else {
            assert!(matches!(lookup, PrLookup::GhFailed(_)));
            assert!(all_open_prs_for_spec_via_forge(root, &canonical).is_err());
        }
    }
    std::fs::write(root.join("detail"), &stable_detail).unwrap();
    std::fs::write(
        root.join("commits-1"),
        serde_json::to_vec(&first_page).unwrap(),
    )
    .unwrap();

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
}

// trace:BUG-2400 | ai:codex
#[test]
fn bug_2400_saturation_and_duplicate_enumeration() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new();
    let root = f.dir.path();
    let canonical = f.canonical.clone();
    let noise = row(
        3177,
        &format!("fix ({})", f.unrelated),
        &format!("codex/{}", f.unrelated),
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
    const CAP: &str = "open PR candidate limit reached; association enumeration is incomplete";
    assert!(matches!(
        detect_open_pr_for_spec_github(root, &canonical),
        PrLookup::GhFailed(message) if message == CAP
    ));
    assert!(matches!(
        all_open_prs_for_spec_via_forge(root, &canonical),
        Err(PrLookup::GhFailed(message)) if message == CAP
    ));
    let args = std::fs::read_to_string(root.join("args")).unwrap();
    assert!(args
        .lines()
        .filter(|l| l.contains("--search"))
        .all(|l| l.contains("--limit 101")));
}

// trace:BUG-2400 | ai:codex
#[test]
fn bug_2400_real_resolver_deadlines_and_errors() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let mut f = Fixture::new();
    let root = f.dir.path();
    let canonical = f.canonical.clone();
    let noise = row(
        3177,
        &format!("fix ({})", f.unrelated),
        &format!("codex/{}", f.unrelated),
    );
    let gh = root.join("gh");
    std::fs::write(root.join("all"), row(3200, "fix", "unscoped")).unwrap();
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

    // Restore the empty-candidate state used by the resolver and list-error
    // cases in the original fixture; no prior commit match may mask an error.
    std::fs::write(root.join("search"), "").unwrap();
    std::fs::write(root.join("all"), "").unwrap();
    let original_gh = std::fs::read(&gh).unwrap();
    // Exercise actual PATH resolution, without the test-binary shortcut.
    // The fallback policy is preserved and one collection probes only once.
    drop(f.gh_guard.take());
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

fn batched_pr(head: u64, commits: &[(u64, &str)], total: usize) -> serde_json::Value {
    serde_json::json!({
        "headRefOid": format!("{head:040x}"),
        "commits": {
            "totalCount": total,
            "nodes": commits
                .iter()
                .map(|(n, m)| serde_json::json!({"commit":{"oid":format!("{n:040x}"),"message":m}}))
                .collect::<Vec<_>>(),
        }
    })
}

// Batched evidence decides only a complete, head-bound, duplicate-free page;
// everything else is left to the exact REST probe, never read as absence.
// trace:BUG-2400 | ai:claude
#[test]
fn bug_2400_batched_commit_evidence_shapes() {
    let ids = vec!["BUG-2392".to_string()];
    let numbers = [1, 2, 3, 4, 5, 6, 7, 8];
    let value = serde_json::json!({"data":{"repository":{
        "p1": batched_pr(11, &[(10, "work"), (11, "fix (BUG-2392)\n\nbody")], 2),
        "p2": batched_pr(21, &[(20, "work"), (21, "more work")], 2),
        // Head drift between commits and head: undecided.
        "p3": batched_pr(99, &[(30, "fix (BUG-2392)")], 1),
        // Declared total exceeds the page (truncated last:100): undecided.
        "p4": batched_pr(40, &[(40, "work")], 101),
        // Duplicate commit: undecided.
        "p5": batched_pr(50, &[(50, "work"), (50, "work")], 2),
        // Zero commits proves nothing here: the REST probe decides.
        "p6": batched_pr(60, &[], 0),
        "p7": {"headRefOid": "bad", "commits": {"totalCount": 1, "nodes": []}},
        // p8 missing entirely.
    }}});
    let evidence = batched_commit_evidence(&value, &numbers, &ids);
    use BatchedCommitEvidence::*;
    assert_eq!(evidence[&1], Complete(true));
    assert_eq!(evidence[&2], Complete(false));
    for n in 3..=8 {
        assert_eq!(evidence[&n], Undecided, "p{n}");
    }
    // A response of any other shape leaves every candidate undecided.
    for other in [serde_json::json!({}), serde_json::json!({"commits":0})] {
        let evidence = batched_commit_evidence(&other, &numbers, &ids);
        assert!(evidence.values().all(|e| *e == Undecided));
    }
    let query = batched_commit_query(&[3177, 3200]);
    assert!(query.contains("p3177:pullRequest(number:3177)"));
    assert!(query.contains("p3200:pullRequest(number:3200)"));
    assert!(query.contains(&format!("commits(last:{BATCHED_COMMITS_PER_PR})")));
}

// Realistic per-call latency with many unrelated open PRs: absence and the
// complete duplicate set are reachable inside the deadline because commit
// evidence is one batched read, not three REST calls per candidate.
// trace:BUG-2400 | ai:claude
#[test]
fn bug_2400_many_unrelated_open_prs_under_latency() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new();
    let root = f.dir.path();
    let canonical = f.canonical.clone();
    let noise = row(
        3177,
        &format!("fix ({})", f.unrelated),
        &format!("codex/{}", f.unrelated),
    );
    let unrelated: Vec<u64> = (5000..5013).collect();
    let all = unrelated
        .iter()
        .map(|n| row(*n, "fix", &format!("codex/other-{n}")))
        .collect::<String>();
    let mut repo = serde_json::Map::new();
    for n in unrelated.iter().chain([3177].iter()) {
        repo.insert(format!("p{n}"), batched_pr(*n, &[(*n, "ordinary work")], 1));
    }
    let write_graphql = |repo: &serde_json::Map<String, serde_json::Value>| {
        std::fs::write(
            root.join("graphql"),
            serde_json::json!({"data":{"repository":repo}}).to_string(),
        )
        .unwrap();
    };
    write_graphql(&repo);
    std::fs::write(root.join("all"), &all).unwrap();
    std::fs::write(root.join("latency"), "").unwrap();
    let logged = || std::fs::read_to_string(root.join("args")).unwrap_or_default();
    let rest_calls = || {
        logged()
            .lines()
            .filter(|l| l.starts_with("api repos/"))
            .count()
    };

    // Duplicates are still fully enumerated beside 14 unrelated open PRs.
    std::fs::write(
        root.join("search"),
        format!(
            "{noise}{}{}",
            row(3200, "fix (BUG-2392)", "first"),
            row(3204, "fix", "bug-2392-rework")
        ),
    )
    .unwrap();
    let started = std::time::Instant::now();
    assert_eq!(
        all_open_prs_for_spec_via_forge(root, &canonical)
            .ok()
            .unwrap(),
        vec![(3200, "first".into()), (3204, "bug-2392-rework".into())]
    );
    assert_eq!(rest_calls(), 0);

    // The single lookup stops at the first exact title/branch match: one
    // search list call, no open-list enumeration and no commit reads.
    std::fs::remove_file(root.join("args")).unwrap();
    assert!(
        matches!(detect_open_pr_for_spec_github(root, &canonical), PrLookup::Found(pr) if pr.number == 3200)
    );
    let calls: Vec<String> = logged()
        .lines()
        .filter(|l| l.starts_with("pr ") || l.starts_with("api "))
        .map(str::to_string)
        .collect();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(calls[0].contains("--search"));

    // Absence is reachable: NoOpenPr, not a timeout, with no REST probes.
    std::fs::write(root.join("search"), &noise).unwrap();
    assert!(matches!(
        detect_open_pr_for_spec_github(root, &canonical),
        PrLookup::NoOpenPr
    ));
    assert!(all_open_prs_for_spec_via_forge(root, &canonical)
        .ok()
        .unwrap()
        .is_empty());
    assert_eq!(rest_calls(), 0);

    // A batched commit trailer associates an otherwise unmatched PR.
    repo.insert(
        "p5005".into(),
        batched_pr(5006, &[(5005, "work"), (5006, "fix: late (BUG-2392)")], 2),
    );
    write_graphql(&repo);
    assert!(
        matches!(detect_open_pr_for_spec_github(root, &canonical), PrLookup::Found(pr) if pr.number == 5005)
    );
    assert_eq!(
        all_open_prs_for_spec_via_forge(root, &canonical)
            .ok()
            .unwrap(),
        vec![(5005, "codex/other-5005".into())]
    );
    assert_eq!(rest_calls(), 0);
    // Sequential per-PR probing would cost 3 calls x 14 PRs x 0.2s per lookup.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(6),
        "{:?}",
        started.elapsed()
    );

    // An undecidable batch entry falls back to the exact REST probe for that
    // PR alone; the probe (zero commits in this fixture) decides it.
    std::fs::remove_file(root.join("latency")).unwrap();
    repo.insert("p5005".into(), batched_pr(5005, &[(5005, "work")], 101));
    write_graphql(&repo);
    std::fs::remove_file(root.join("args")).unwrap();
    assert!(matches!(
        detect_open_pr_for_spec_github(root, &canonical),
        PrLookup::NoOpenPr
    ));
    let args = logged();
    assert!(
        args.contains("api repos/{owner}/{repo}/pulls/5005"),
        "{args}"
    );
    assert!(!args.contains("pulls/5000"), "{args}");
    // A failed batch read is not absence either: every candidate goes to the
    // exact probe, which still decides each one.
    std::fs::remove_file(root.join("graphql")).unwrap();
    assert!(matches!(
        detect_open_pr_for_spec_github(root, &canonical),
        PrLookup::NoOpenPr
    ));
    assert!(logged().contains("pulls/5000"));
}
