//! Exercise the registered show handler with the real Doc writer and cache.
// trace:TASK-1471 | ai:codex
use aida_core::{DatabaseBackend, Requirement, RequirementType};
use clap::CommandFactory;
use std::path::Path;

// A child process keeps stdout capture and the OnceLock format pin isolated
// from other libtests. No installed CLI binary is used.
#[test]
fn task_1471_show_docs_child() {
    let Ok(root) = std::env::var("AIDA_TEST_SHOW_DOCS_ROOT") else {
        return;
    };
    let root = Path::new(&root);
    let format = std::env::var("AIDA_TEST_SHOW_DOCS_FORMAT").unwrap();
    crate::output_format::set_output_format_override(Some(match format.as_str() {
        "human" => crate::OutputFormat::Human,
        "json" => crate::OutputFormat::Json,
        "toon" => crate::OutputFormat::Toon,
        _ => panic!("unexpected format"),
    }));
    let store = root.join("store");
    let backend = aida_core::CachedGitBackend::open(&store, &root.join("cache.db")).unwrap();
    let spec = backend
        .get_requirement_by_spec_id("STORY-1472")
        .unwrap()
        .unwrap();
    if spec.relationships.is_empty() {
        // Verify opening show did not rebuild away the stale-degree repro.
        assert_eq!(backend.degrees(&spec.id).unwrap().in_degree, 0);
    }
    let matches = super::ShowArgs::command()
        .try_get_matches_from(["show", "STORY-1472", "--no-git"])
        .unwrap();
    let ctx = crate::cmdkit::DispatchCtx {
        globals: None,
        store: Some(crate::cmdkit::StoreCtx::Git {
            store_path: &store,
            backend: &backend,
        }),
    };
    println!("SHOW_DOCS_BEGIN");
    super::run_show(&matches, &ctx).unwrap();
    println!("SHOW_DOCS_END");
}

/// This build's libtest runner, via the process-wide resolver (a raw
/// executable lookup outside the resolver trips the TASK-1262 ratchet). An
/// ambient AIDA_BIN would redirect the resolver, so prove it is a test runner.
fn test_runner() -> std::path::PathBuf {
    let runner = crate::resolve_aida_exe();
    assert_eq!(
        runner
            .parent()
            .and_then(|deps| deps.file_name())
            .and_then(|name| name.to_str()),
        Some("deps"),
        "expected this build's test runner under target/<profile>/deps, got {}; \
         unset AIDA_BIN and re-run",
        runner.display()
    );
    runner
}

fn show(root: &Path, format: &str) -> String {
    let output = std::process::Command::new(test_runner())
        .args([
            "--exact",
            "requirements_cli::task_1471_show_docs_tests::task_1471_show_docs_child",
            "--nocapture",
        ])
        .current_dir(root)
        .env("AIDA_TEST_SHOW_DOCS_ROOT", root)
        .env("AIDA_TEST_SHOW_DOCS_FORMAT", format)
        .env("HOME", root)
        .env("NO_COLOR", "1")
        .env_remove("AIDA_SESSION_ROLE")
        .env_remove("AIDA_SESSION_ID")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    stdout
        .split_once("SHOW_DOCS_BEGIN\n")
        .unwrap_or_else(|| panic!("child did not run: {stdout}"))
        .1
        .split_once("SHOW_DOCS_END")
        .unwrap()
        .0
        .to_owned()
}

#[test]
fn task_1471_show_docs_immediate_reverse_and_direct_links_preserve_machine_output() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ROLE", None)]);
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let store = root.join("store");
    let backend =
        crate::test_env::home_fixture_cached_backend(&store, &root.join("cache.db")).unwrap();
    let mut spec = Requirement::new("Explained feature".into(), "body".into());
    spec.req_type = RequirementType::Story;
    spec.spec_id = Some("STORY-1472".into());
    let mut spec = backend.add_requirement(spec).unwrap();
    assert_eq!(backend.degrees(&spec.id).unwrap().in_degree, 0);
    assert!(!show(root, "human").contains("Documentation:"));
    let json_before = show(root, "json");
    let toon_before = show(root, "toon");

    crate::doc_cmd::handle_doc_command(
        &crate::cli::DocCommand::Add {
            title: "Reverse-linked exposition".into(),
            about: vec!["STORY-1472".into()],
            scenario: Some("first use".into()),
            audience: vec!["human".into()],
            description: Some("explanation".into()),
            description_from_file: None,
            description_stdin: false,
            tags: None,
        },
        &store,
        &backend,
    )
    .unwrap();
    // This is the stale degree that used to suppress the reverse scan. No
    // cache rebuild may occur between doc add and show.
    assert_eq!(backend.degrees(&spec.id).unwrap().in_degree, 0);
    let human = show(root, "human");
    assert!(human.contains("Documentation:"), "{human}");
    assert!(human.contains("Reverse-linked exposition"), "{human}");
    assert!(human.contains("aida doc show DOC-"), "{human}");
    assert_eq!(show(root, "json"), json_before);
    assert_eq!(show(root, "toon"), toon_before);

    let mut direct = Requirement::new("Direct-linked explanation".into(), "body".into());
    direct.req_type = RequirementType::Doc;
    direct.spec_id = Some("DOC-1-174".into());
    let direct = backend.add_requirement(direct).unwrap();
    // Capture machine output BEFORE the direct link is written, so the
    // comparison below measures what adding the link changed.
    let json_before_direct = show(root, "json");
    let toon_before_direct = show(root, "toon");
    spec.relationships.push(aida_core::models::Relationship {
        target_id: direct.id,
        rel_type: aida_core::models::RelationshipType::References,
        created_at: None,
        created_by: None,
    });
    backend.update_requirement(&spec).unwrap();
    let human = show(root, "human");
    assert!(human.contains("Direct-linked explanation"), "{human}");
    assert!(human.contains("Reverse-linked exposition"), "{human}");

    // JSON: the only change is the new edge itself (plus write bookkeeping).
    // No key disappears, and any added or changed fields must be edge-delta
    // fields. The relationships key is always present, even for an empty
    // list; adding the first edge changes its value, not its presence.
    // This ensures no Documentation projection leaked in.
    // trace:TASK-1471 | ai:claude ai:codex
    const EDGE_DELTA: &[&str] = &[
        "relationships",
        "out_degree",
        "heft",
        "modified_at",
        "updated_at",
        "history",
    ];
    let json_after_direct = show(root, "json");
    let before: serde_json::Value = serde_json::from_str(&json_before_direct).unwrap();
    let after: serde_json::Value = serde_json::from_str(&json_after_direct).unwrap();
    let (before, after) = (before.as_object().unwrap(), after.as_object().unwrap());
    assert_eq!(before["relationships"], serde_json::json!([]));
    let keys = |m: &serde_json::Map<String, serde_json::Value>| {
        m.keys().cloned().collect::<std::collections::BTreeSet<_>>()
    };
    let (keys_before, keys_after) = (keys(before), keys(after));
    assert!(
        keys_before.is_subset(&keys_after),
        "JSON lost keys: {:?}",
        keys_before.difference(&keys_after).collect::<Vec<_>>()
    );
    let added: Vec<&String> = keys_after.difference(&keys_before).collect();
    assert!(
        added.iter().all(|k| EDGE_DELTA.contains(&k.as_str())),
        "unexpected JSON keys added: {added:?}"
    );
    let changed: std::collections::BTreeSet<&str> = after
        .iter()
        .filter(|(k, v)| before.get(*k) != Some(*v))
        .map(|(k, _)| k.as_str())
        .collect();
    assert!(changed.contains("relationships"), "{changed:?}");
    assert!(
        changed.iter().all(|k| EDGE_DELTA.contains(k)),
        "unexpected JSON delta: {changed:?}"
    );
    // The show projection resolves relationship IDs to display IDs, not UUIDs.
    // Assert the fields themselves so a title match cannot hide a regression.
    // trace:TASK-1471 | ai:codex
    let rels = after["relationships"].as_array().unwrap();
    assert_eq!(rels.len(), 1, "{rels:?}");
    assert_eq!(rels[0]["id"], direct.display_id());
    assert_eq!(rels[0]["title"], direct.title);

    // TOON: same top-level fields before and after; the new edge is visible
    // and the human-only section is not.
    let toon_after_direct = show(root, "toon");
    let toon_keys = |s: &str| {
        s.lines()
            .filter(|l| !l.starts_with(char::is_whitespace))
            .filter_map(|l| l.split(|c| c == ':' || c == '[').next())
            .map(str::to_owned)
            .collect::<std::collections::BTreeSet<_>>()
    };
    let (tk_before, tk_after) = (
        toon_keys(&toon_before_direct),
        toon_keys(&toon_after_direct),
    );
    assert!(tk_before.is_subset(&tk_after), "TOON lost fields");
    // A first edge may introduce the relationships table; nothing else may.
    assert!(
        tk_after
            .difference(&tk_before)
            .all(|k| k.starts_with("relationships")),
        "unexpected TOON fields: {:?}",
        tk_after.difference(&tk_before).collect::<Vec<_>>()
    );
    assert_ne!(toon_before_direct, toon_after_direct);
    assert!(
        toon_after_direct.contains(&direct.friendly_id()),
        "{toon_after_direct}"
    );
    for out in [&json_after_direct, &toon_after_direct] {
        assert!(!out.contains("Documentation:"), "{out}");
        assert!(!out.contains("read: aida doc show"), "{out}");
    }
}
