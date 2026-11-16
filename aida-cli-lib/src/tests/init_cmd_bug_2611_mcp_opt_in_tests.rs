use super::*;

// Exercise both global registration paths through full init scaffolding, with
// executable stand-ins writing exactly the two user configuration files.
// trace:BUG-2611 | ai:codex
#[test]
fn bug_2611_global_mcp_requires_opt_in() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    for (name, script) in [
        ("codex", "#!/bin/sh\ncase \"$1 $2\" in\n'--version ') exit 0;;\n'mcp list') exit 0;;\n'mcp add') mkdir -p \"$HOME/.codex\"; printf 'registered' > \"$HOME/.codex/config.toml\";;\nesac\n"),
        ("antigravity", "#!/bin/sh\nif [ \"$1\" = '--add-mcp' ]; then\nmkdir -p \"$HOME/.gemini/antigravity\"\nprintf 'registered' > \"$HOME/.gemini/antigravity/mcp_config.json\"\nfi\n"),
    ] {
        let path = bin.join(name);
        std::fs::write(&path, script).unwrap();
        aida_testkit::test_exec::mark_executable(&path);
    }
    let search_path = format!("{}:/usr/bin:/bin", bin.display());
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("HOME", home.to_str().unwrap()),
        ("AIDA_HOME", home.to_str().unwrap()),
        ("PATH", &search_path),
        ("AIDA_TEST_ANTIGRAVITY_BINARY", "1"),
        ("AIDA_TEST_ANTIGRAVITY_MCP_REGISTERED", "0"),
        ("AIDA_TEST_ANTIGRAVITY_MCP_TTY", "1"),
        ("AIDA_TEST_ANTIGRAVITY_MCP_ACCEPT", "1"),
    ]);
    let configs = [
        home.join(".codex/config.toml"),
        home.join(".gemini/antigravity/mcp_config.json"),
    ];
    for (iteration, with_mcp) in [false, true, false].into_iter().enumerate() {
        let root = tmp.path().join(format!("project-{iteration}"));
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        complete_init_scaffolding(
            &root,
            &RequirementsStore::new(),
            Some("codex,antigravity"),
            with_mcp,
            true,
            true,
            false,
            root.join(".aida-store"),
            "test store",
            false,
            crate::cli::InitFootprint::Full,
            true,
        )
        .unwrap();
        for path in &configs {
            if iteration == 0 {
                assert!(
                    !path.exists(),
                    "unexpected global write: {}",
                    path.display()
                );
            } else if with_mcp {
                assert_eq!(std::fs::read_to_string(path).unwrap(), "registered");
                std::fs::write(path, "existing user config").unwrap();
            } else {
                assert_eq!(
                    std::fs::read_to_string(path).unwrap(),
                    "existing user config"
                );
            }
        }
    }
}
