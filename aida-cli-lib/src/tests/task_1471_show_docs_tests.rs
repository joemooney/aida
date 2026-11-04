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
    let ctx = crate::command_registry::DispatchCtx {
        globals: None,
        store: Some(crate::command_registry::StoreCtx::Git {
            store_path: &store,
            backend: &backend,
        }),
    };
    println!("SHOW_DOCS_BEGIN");
    super::run_show(&matches, &ctx).unwrap();
    println!("SHOW_DOCS_END");
}

fn show(root: &Path, format: &str) -> String {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
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
    for format in ["json", "toon"] {
        assert!(!show(root, format).contains("Documentation:"));
    }
}
