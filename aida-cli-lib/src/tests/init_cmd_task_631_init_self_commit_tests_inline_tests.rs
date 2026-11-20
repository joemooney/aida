use super::*;
use tempfile::TempDir;

#[test]
fn commit_paths_filters_to_existing_only_and_never_bare_dot() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Nothing written yet → empty list.
    assert!(init_scaffold_commit_paths(root, crate::cli::InitFootprint::Full).is_empty());

    // Write a representative subset of init-owned paths.
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(root.join(".aida/config.toml"), "x").unwrap();
    std::fs::write(root.join(".gitignore"), "x").unwrap();
    std::fs::write(root.join("CLAUDE.md"), "x").unwrap();
    std::fs::create_dir_all(root.join(".claude/skills")).unwrap();
    std::fs::write(root.join(".claude/skills/foo.md"), "x").unwrap();
    std::fs::create_dir_all(root.join(".aida/discipline")).unwrap();

    // An UNRELATED user file must NOT appear in the staged set.
    std::fs::write(root.join("user_wip.rs"), "x").unwrap();

    let paths = init_scaffold_commit_paths(root, crate::cli::InitFootprint::Full);
    assert!(paths.contains(&".aida/config.toml".to_string()));
    assert!(paths.contains(&".gitignore".to_string()));
    assert!(paths.contains(&"CLAUDE.md".to_string()));
    assert!(paths.contains(&".claude".to_string()));
    assert!(paths.contains(&".aida/discipline".to_string()));

    // Candidates that were never written are filtered out.
    assert!(!paths.contains(&"AGENTS.md".to_string()));
    assert!(!paths.contains(&".codex".to_string()));

    // CRITICAL invariant: never a bare "." and never the user's WIP.
    assert!(!paths.iter().any(|p| p == "."));
    assert!(!paths.iter().any(|p| p == "user_wip.rs"));

    // Every staged path is one of the known candidates.
    let candidates = init_scaffold_candidate_paths();
    for p in &paths {
        assert!(
            candidates.contains(&p.as_str()),
            "staged path {p} is not in the init allow-list"
        );
    }
}

// trace:STORY-830 | ai:codex
#[test]
fn minimal_footprint_commit_allow_list_is_only_gitignore_and_config() {
    assert_eq!(
        init_scaffold_candidate_paths_for_footprint(crate::cli::InitFootprint::Minimal),
        &[".gitignore", ".aida/config.toml"]
    );

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida/discipline")).unwrap();
    std::fs::write(root.join(".aida/config.toml"), "x").unwrap();
    std::fs::write(root.join(".gitignore"), "x").unwrap();
    std::fs::write(root.join("CLAUDE.md"), "x").unwrap();
    std::fs::create_dir_all(root.join(".claude/skills")).unwrap();
    std::fs::write(root.join(".claude/skills/foo.md"), "x").unwrap();

    let paths = init_scaffold_commit_paths(root, crate::cli::InitFootprint::Minimal);
    assert_eq!(
        paths,
        vec![".gitignore".to_string(), ".aida/config.toml".to_string()]
    );
}

// trace:STORY-1093 | ai:codex
#[test]
fn memory_lane_footprint_scaffolds_only_reflex_lane_files() {
    assert_eq!(
        init_scaffold_candidate_paths_for_footprint(crate::cli::InitFootprint::MemoryLane),
        &[
            ".gitignore",
            ".aida/config.toml",
            "CLAUDE.md",
            "AGENTS.md",
            ".claude/skills/aida-capture/SKILL.md",
            ".claude/skills/aida-learn/SKILL.md",
            ".agents/skills/aida-capture/SKILL.md",
            ".agents/skills/aida-learn/SKILL.md",
            ".codex/skills/aida-capture/SKILL.md",
            ".codex/skills/aida-learn/SKILL.md",
            ".claude/skills/.aida-delivered",
            ".agents/skills/.aida-delivered",
            ".codex/skills/.aida-delivered",
        ]
    );

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(root.join(".gitignore"), ".aida/*\n!.aida/config.toml\n").unwrap();
    let store = aida_core::models::RequirementsStore::new();

    complete_init_scaffolding(
        root,
        &store,
        None,
        false, // with_mcp
        false, // no_skills
        false, // no_hooks
        false, // force
        root.join(".aida-store"),
        "test store",
        false, // verbose
        crate::cli::InitFootprint::MemoryLane,
        true, // suppress scaffold commit
    )
    .unwrap();

    for rel in [
        ".gitignore",
        ".aida/config.toml",
        "CLAUDE.md",
        "AGENTS.md",
        ".claude/skills/aida-capture/SKILL.md",
        ".claude/skills/aida-learn/SKILL.md",
        // trace:BUG-1639 | ai:claude
        ".agents/skills/aida-capture/SKILL.md",
        ".agents/skills/aida-learn/SKILL.md",
    ] {
        assert!(
            root.join(rel).exists(),
            "expected memory-lane artifact {rel}"
        );
    }

    for rel in [
        ".mcp.json",
        ".aida/project.toml",
        ".aida/agents.toml",
        ".aida/discipline",
        "docs/agents",
        "docs/extending-skills.md",
        ".claude/commands",
        ".claude/settings.json",
        ".claude/hooks",
        ".claude/skills/aida-queue.md",
        ".claude/skills/aida-drain-queue.md",
        ".claude/skills/aida-implement.md",
        ".codex/config.toml",
        ".codex/hooks.json",
        ".codex/hooks",
        ".codex/skills/aida-queue/SKILL.md",
        ".codex/skills/aida-drain-queue/SKILL.md",
        ".codex/skills/aida-implement/SKILL.md",
        // A legacy per-vendor pack is never created.
        ".codex/skills",
        ".agents/skills/aida-queue/SKILL.md",
        ".agents/skills/aida-implement/SKILL.md",
        // trace:TASK-1744 | ai:antigravity
        ".agents/hooks.json",
        ".agents/hooks",
        ".antigravity",
    ] {
        assert!(
            !root.join(rel).exists(),
            "memory-lane footprint must not scaffold machinery artifact {rel}"
        );
    }

    let agents = std::fs::read_to_string(root.join("AGENTS.md")).unwrap();
    assert!(agents.contains("<!-- AIDA-AUTOGEN-BEGIN -->"));
    assert!(agents.contains("/aida-capture"));
    assert!(agents.contains("/aida-learn"));
}

// trace:STORY-1093 | ai:codex
#[test]
fn memory_lane_no_skills_skips_reflex_skill_files() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(root.join(".gitignore"), ".aida/*\n!.aida/config.toml\n").unwrap();
    let store = aida_core::models::RequirementsStore::new();

    complete_init_scaffolding(
        root,
        &store,
        None,
        false, // with_mcp
        true,  // no_skills
        false, // no_hooks
        false, // force
        root.join(".aida-store"),
        "test store",
        false, // verbose
        crate::cli::InitFootprint::MemoryLane,
        true, // suppress scaffold commit
    )
    .unwrap();

    for rel in ["CLAUDE.md", "AGENTS.md"] {
        assert!(
            root.join(rel).exists(),
            "memory-lane no-skills still writes guidance artifact {rel}"
        );
    }

    for rel in [
        ".claude/skills/aida-capture/SKILL.md",
        ".claude/skills/aida-learn/SKILL.md",
        ".codex/skills/aida-capture/SKILL.md",
        ".codex/skills/aida-learn/SKILL.md",
        ".agents/skills/aida-capture/SKILL.md",
        ".agents/skills/aida-learn/SKILL.md",
    ] {
        assert!(
            !root.join(rel).exists(),
            "memory-lane --no-skills must not scaffold reflex skill artifact {rel}"
        );
    }
}

// BUG-789: init reports the orphan-store push, then makes a second commit
// on the code branch and does not push it. These pin the wording that stops
// "AIDA initialized ✓" reading as "everything is pushed".
// trace:BUG-789 | ai:claude
// ── BUG-794: --agent is a strict allow-list ──

#[test]
fn single_agent_selections_are_strict() {
    let c = parse_agent_selection("claude").unwrap();
    assert!(c.claude && !c.codex && !c.antigravity);
    let x = parse_agent_selection("codex").unwrap();
    assert!(
        !x.claude && x.codex && !x.antigravity,
        "codex must NOT drag antigravity along — the original bug"
    );
    let a = parse_agent_selection("antigravity").unwrap();
    assert!(!a.claude && !a.codex && a.antigravity);
}

#[test]
fn both_means_exactly_claude_and_codex() {
    let b = parse_agent_selection("both").unwrap();
    assert!(b.claude && b.codex);
    assert!(
        !b.antigravity,
        "the name says two; a silent third agent is the bug"
    );
}

#[test]
fn all_is_every_profile_and_matches_the_default() {
    let a = parse_agent_selection("all").unwrap();
    assert!(a.claude && a.codex && a.antigravity);
}

#[test]
fn comma_lists_select_exactly_what_they_name() {
    let cc = parse_agent_selection("claude,codex").unwrap();
    assert!(cc.claude && cc.codex && !cc.antigravity);
    let ca = parse_agent_selection("codex, antigravity").unwrap();
    assert!(
        !ca.claude && ca.codex && ca.antigravity,
        "whitespace tolerated"
    );
    let agy = parse_agent_selection("agy").unwrap();
    assert!(agy.antigravity, "agy is the accepted short form");
}

#[test]
fn garbage_and_empty_selections_fail_loudly() {
    assert!(parse_agent_selection("cursor").is_err());
    assert!(
        parse_agent_selection("claude,cursor").is_err(),
        "one bad entry fails the whole list"
    );
    assert!(
        parse_agent_selection(",,").is_err(),
        "no profiles selected is an error"
    );
    assert!(parse_agent_selection("CLAUDE").is_ok(), "case-insensitive");
}

#[test]
fn enabled_agent_selection_round_trips_project_config() {
    let dir = tempfile::tempdir().unwrap();
    let selection = AgentSelection {
        claude: false,
        codex: true,
        antigravity: false,
    };
    write_enabled_agent_selection(dir.path(), selection).unwrap();

    let body = std::fs::read_to_string(dir.path().join(".aida/config.toml")).unwrap();
    assert!(
        body.contains("[agents]") && body.contains("enabled = [\"codex\"]"),
        "canonical enabled array written: {body}"
    );
    assert_eq!(read_enabled_agent_selection(dir.path()), Some(selection));
}

#[test]
fn enabled_agent_selection_prefers_project_agents_toml_over_config() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    std::fs::write(
        dir.path().join(".aida/config.toml"),
        "[agents]\nenabled = [\"claude\"]\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join(".aida/agents.toml"),
        "[agents]\nenabled = [\"codex\"]\n",
    )
    .unwrap();

    let resolved = read_enabled_agent_selection_with_source(dir.path()).unwrap();

    assert_eq!(
        resolved.selection,
        AgentSelection {
            claude: false,
            codex: true,
            antigravity: false,
        }
    );
    assert_eq!(resolved.path, dir.path().join(".aida/agents.toml"));
}

// trace:BUG-2513 | ai:codex
#[test]
fn init_interactive_selection_survives_conflicting_saved_settings() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    for (saved, chosen) in [("claude", "codex"), ("codex", "claude")] {
        std::fs::write(
            dir.path().join(".aida/agents.toml"),
            format!("[agents]\nenabled = [\"{saved}\"]\nvendor = \"codex\"\n"),
        )
        .unwrap();
        let selection =
            prompt_init_agent_selection(AgentSelection::all(), |_| Ok(chosen.to_string())).unwrap();
        // This is the same explicit allow-list passed by run_init into
        // every storage handler, so scaffolding cannot prompt again.
        let allow_list = selection.names().join(",");
        let scaffold_selection =
            resolve_init_agent_selection_for_root(dir.path(), Some(&allow_list)).unwrap();
        write_enabled_agent_selection(dir.path(), scaffold_selection).unwrap();
        assert_eq!(selection, scaffold_selection);
        assert_eq!(selection.codex, chosen == "codex");
        let offered = std::cell::Cell::new(false);
        config_cmd::offer_init_codex_native(dir.path(), selection.codex, true, || {
            offered.set(true);
            Ok(false)
        })
        .unwrap();
        assert_eq!(offered.get(), chosen == "codex");
        assert_eq!(
            read_enabled_agent_selection(dir.path()).unwrap().codex,
            saved == "codex"
        );
    }
}

#[test]
fn codex_vendor_config_selects_codex_skill_surface() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    std::fs::write(
        dir.path().join(".aida/config.toml"),
        "[agents]\nvendor = \"codex\"\n",
    )
    .unwrap();

    let selection = resolve_init_agent_selection_for_root(dir.path(), None).unwrap();
    assert!(
        selection.codex,
        "codex-vendor init must select the Codex skill pack"
    );

    let mut config = ScaffoldConfig::default();
    selection.apply_to_scaffold_config(&mut config, false);
    assert!(config.generate_codex_skills);
    assert!(!config.generate_codex_config);
    assert!(config.include_aida_capture_skill);
}

#[test]
fn codex_agent_init_writes_discoverable_aida_capture_skill() {
    let tmp = TempDir::new().unwrap();
    let (root, _) = setup_clone_like_repo(&tmp);
    let store = aida_core::models::RequirementsStore::new();

    complete_init_scaffolding(
        &root,
        &store,
        Some("codex"),
        false, // with_mcp
        false, // no_skills
        true,  // no_hooks
        false, // force
        root.join(".aida-store"),
        "test store",
        false, // verbose
        crate::cli::InitFootprint::Full,
        true, // suppress bootstrap commit
    )
    .unwrap();

    assert!(
        root.join(".agents/skills/aida-capture/SKILL.md").is_file(), // trace:BUG-1639 | ai:claude
        "codex init must scaffold the discoverable $aida-capture skill"
    );
    assert!(
        !root.join(".claude/skills/aida-capture/SKILL.md").exists(),
        "--agent codex remains a strict agent allow-list"
    );
}

/// BUG-1587: `.codex/hooks/` is intentionally ignored machine-local
/// output, so it can disappear while the wiring remains. Re-running init
/// must restore missing handlers without overwriting customized wiring.
// trace:BUG-1587 | ai:codex
#[test]
fn reinit_repairs_missing_codex_hook_handlers_without_force() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    aida_core::scaffolding::codex_hooks::scaffold_codex_hooks(root, false).unwrap();
    let wiring = root.join(".codex/hooks.json");
    let customized = format!("{}\n", std::fs::read_to_string(&wiring).unwrap());
    std::fs::write(&wiring, &customized).unwrap();
    std::fs::remove_dir_all(root.join(".codex/hooks")).unwrap();

    maybe_repair_codex_hooks_on_reinit(root, Some("codex"), false).unwrap();

    for name in aida_core::scaffolding::codex_hooks::handler_names() {
        assert!(
            root.join(".codex/hooks").join(&name).is_file(),
            "re-init must restore missing handler {name}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(wiring).unwrap(),
        customized,
        "non-force repair must preserve customized wiring"
    );
}

#[test]
fn reinit_codex_hook_repair_honors_agent_allow_list_and_no_hooks() {
    for (agent, no_hooks) in [(Some("claude"), false), (Some("codex"), true)] {
        let dir = tempfile::tempdir().unwrap();
        maybe_repair_codex_hooks_on_reinit(dir.path(), agent, no_hooks).unwrap();
        assert!(
            !dir.path().join(".codex/hooks.json").exists(),
            "agent={agent:?}, no_hooks={no_hooks} must not scaffold Codex hooks"
        );
    }
}

// trace:TASK-1744 | ai:antigravity
#[test]
fn reinit_repairs_missing_antigravity_hook_handlers_without_force() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    aida_core::scaffolding::codex_hooks::scaffold_antigravity_hooks(root, false).unwrap();
    let wiring = root.join(".agents/hooks.json");
    let customized = format!("{}\n", std::fs::read_to_string(&wiring).unwrap());
    std::fs::write(&wiring, &customized).unwrap();
    std::fs::remove_dir_all(root.join(".agents/hooks")).unwrap();

    maybe_repair_codex_hooks_on_reinit(root, None, false).unwrap();

    for name in aida_core::scaffolding::codex_hooks::antigravity_handler_names() {
        assert!(
            root.join(".agents/hooks").join(&name).is_file(),
            "re-init must restore missing Antigravity handler {name}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(wiring).unwrap(),
        customized,
        "non-force repair must preserve customized Antigravity wiring"
    );
}

// trace:TASK-1744 | ai:antigravity
#[test]
fn reinit_antigravity_hook_repair_honors_agent_allow_list_no_hooks_and_third_party_hooks_json() {
    for (agent, no_hooks) in [
        (Some("claude"), false),
        (Some("codex"), false),
        (Some("antigravity"), true),
    ] {
        let dir = tempfile::tempdir().unwrap();
        maybe_repair_codex_hooks_on_reinit(dir.path(), agent, no_hooks).unwrap();
        assert!(
            !dir.path().join(".agents/hooks.json").exists(),
            "agent={agent:?}, no_hooks={no_hooks} must not scaffold Antigravity hooks"
        );
        assert!(
            !dir.path().join(".agents/hooks").exists(),
            "agent={agent:?}, no_hooks={no_hooks} must not drop Antigravity handlers"
        );
    }

    // Pre-existing third-party .agents/hooks.json without AIDA's key must not
    // trigger handler installation on re-init, even when antigravity is enabled.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_enabled_agent_selection(
        root,
        AgentSelection {
            claude: true,
            codex: true,
            antigravity: true,
        },
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".agents")).unwrap();
    let third_party =
        "{\"third-party\":{\"PreInvocation\":[{\"type\":\"command\",\"command\":\"true\"}]}}\n";
    std::fs::write(root.join(".agents/hooks.json"), third_party).unwrap();

    maybe_repair_codex_hooks_on_reinit(root, None, false).unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join(".agents/hooks.json")).unwrap(),
        third_party
    );
    assert!(
        !root.join(".agents/hooks").exists(),
        "third-party .agents/hooks.json without AIDA wiring must not receive unreferenced handlers"
    );
}

// trace:TASK-1744 | ai:antigravity
#[test]
fn fresh_init_scaffolds_antigravity_hooks_only_when_selected_and_hooks_enabled() {
    let store = aida_core::models::RequirementsStore::new();

    // 1. `--agent antigravity` with hooks enabled scaffolds `.agents/hooks.json` and handler.
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(root.join(".gitignore"), ".aida/*\n!.aida/config.toml\n").unwrap();
        complete_init_scaffolding(
            root,
            &store,
            Some("antigravity"),
            false, // with_mcp
            true,  // no_skills
            false, // no_hooks
            false, // force
            root.join(".aida-store"),
            "test store",
            false, // verbose
            crate::cli::InitFootprint::Full,
            true, // suppress scaffold commit
        )
        .unwrap();
        assert!(
            root.join(".agents/hooks.json").is_file(),
            "fresh init with --agent antigravity must scaffold .agents/hooks.json"
        );
        assert!(
            root.join(".agents/hooks/aida-grant-expiry-notice.sh")
                .is_file(),
            "fresh init with --agent antigravity must scaffold handler"
        );
    }

    // 2. `--agent antigravity --no-hooks` or non-antigravity allow-list (`--agent codex`) does not.
    for (agent, no_hooks) in [(Some("antigravity"), true), (Some("codex"), false)] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(root.join(".gitignore"), ".aida/*\n!.aida/config.toml\n").unwrap();
        complete_init_scaffolding(
            root,
            &store,
            agent,
            false, // with_mcp
            true,  // no_skills
            no_hooks,
            false, // force
            root.join(".aida-store"),
            "test store",
            false, // verbose
            crate::cli::InitFootprint::Full,
            true, // suppress scaffold commit
        )
        .unwrap();
        assert!(
            !root.join(".agents/hooks.json").exists(),
            "fresh init with agent={agent:?}, no_hooks={no_hooks} must not scaffold .agents/hooks.json"
        );
        assert!(
            !root.join(".agents/hooks").exists(),
            "fresh init with agent={agent:?}, no_hooks={no_hooks} must not scaffold .agents/hooks"
        );
    }
}

#[test]
fn sibling_reinit_uses_the_config_written_by_initialization_as_its_marker() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    std::fs::write(dir.path().join(".aida/config.toml"), "[deployment]\n").unwrap();

    assert!(sibling_init_marker_exists(dir.path()));
    assert!(
        !dir.path().join(".aida/node.toml").exists(),
        "a standard sibling init does not write the obsolete marker"
    );
}

// trace:STORY-830 | ai:codex
#[test]
fn init_footprint_round_trips_project_config() {
    let dir = tempfile::tempdir().unwrap();

    write_init_footprint(dir.path(), crate::cli::InitFootprint::Minimal).unwrap();
    let body = std::fs::read_to_string(dir.path().join(".aida/config.toml")).unwrap();
    assert!(
        body.contains("[scaffold]") && body.contains("footprint = \"minimal\""),
        "canonical footprint persisted: {body}"
    );
    assert_eq!(
        read_init_footprint(dir.path()),
        Some(crate::cli::InitFootprint::Minimal)
    );

    write_init_footprint(dir.path(), crate::cli::InitFootprint::Full).unwrap();
    assert_eq!(
        read_init_footprint(dir.path()),
        Some(crate::cli::InitFootprint::Full)
    );

    write_init_footprint(dir.path(), crate::cli::InitFootprint::MemoryLane).unwrap();
    let body = std::fs::read_to_string(dir.path().join(".aida/config.toml")).unwrap();
    assert!(
        body.contains("footprint = \"memory-lane\""),
        "memory-lane footprint persisted: {body}"
    );
    assert_eq!(
        read_init_footprint(dir.path()),
        Some(crate::cli::InitFootprint::MemoryLane)
    );
}

#[test]
fn no_origin_means_nothing_to_say_about_pushing() {
    assert_eq!(unpushed_scaffold_note("main", false, false, 3), None);
}

#[test]
fn an_in_sync_branch_says_nothing() {
    assert_eq!(unpushed_scaffold_note("main", true, true, 0), None);
}

#[test]
fn an_ahead_branch_names_the_count_and_the_command() {
    let n = unpushed_scaffold_note("main", true, true, 1).expect("should warn");
    assert!(n.contains("1 commit on `main` is not pushed"), "got: {n}");
    assert!(
        n.contains("git push -u origin main"),
        "must give the exact command: {n}"
    );
    assert!(
        n.contains("scaffolding"),
        "must say what is being stranded: {n}"
    );
}

#[test]
fn plural_reads_correctly() {
    let n = unpushed_scaffold_note("main", true, true, 4).unwrap();
    assert!(n.contains("4 commits on `main` are not pushed"), "got: {n}");
}

#[test]
fn a_never_pushed_branch_is_reported_even_though_ahead_is_unknown() {
    // origin exists but origin/<branch> does not resolve locally, so
    // ahead/behind is unanswerable. That is still "unpushed", and the
    // earlier version of this check silently said nothing.
    let n = unpushed_scaffold_note("main", true, false, 0).expect("should warn");
    assert!(n.contains("has not been pushed yet"), "got: {n}");
    assert!(n.contains("git push -u origin main"), "got: {n}");
}

#[test]
fn the_branch_name_is_never_assumed_to_be_main() {
    let n = unpushed_scaffold_note("trunk", true, true, 2).unwrap();
    assert!(n.contains("`trunk`"), "got: {n}");
    assert!(n.contains("git push -u origin trunk"), "got: {n}");
    assert!(!n.contains("main"), "must not hardcode main: {n}");
}

#[test]
fn auto_vs_prompt_decision() {
    // Non-interactive (no TTY) → auto-commit.
    assert_eq!(should_auto_commit_scaffold(false, None), Some(true));
    // Interactive (TTY) → prompt (None).
    assert_eq!(should_auto_commit_scaffold(true, None), None);

    // Env override forces auto even on a TTY.
    assert_eq!(
        should_auto_commit_scaffold(true, Some("1")),
        Some(true),
        "env=1 should force auto"
    );
    assert_eq!(should_auto_commit_scaffold(true, Some("true")), Some(true));
    assert_eq!(should_auto_commit_scaffold(true, Some("YES")), Some(true));
    assert_eq!(should_auto_commit_scaffold(true, Some(" on ")), Some(true));

    // Env override forces never even when non-interactive.
    assert_eq!(
        should_auto_commit_scaffold(false, Some("0")),
        Some(false),
        "env=0 should force never"
    );
    assert_eq!(
        should_auto_commit_scaffold(false, Some("false")),
        Some(false)
    );
    assert_eq!(should_auto_commit_scaffold(false, Some("no")), Some(false));
    assert_eq!(should_auto_commit_scaffold(false, Some("off")), Some(false));

    // Unrecognized env value falls through to the TTY heuristic.
    assert_eq!(
        should_auto_commit_scaffold(false, Some("maybe")),
        Some(true)
    );
    assert_eq!(should_auto_commit_scaffold(true, Some("maybe")), None);
}

fn git_in(root: &std::path::Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .current_dir(root)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?} failed to spawn: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// BUG-565: the resilient-staging partition must drop the gitignored path
/// and keep the rest — never abort the whole batch on one ignored path.
/// `.claude` is the canonical local-only gitignore entry that used to
// strand the onboarding commit. trace:BUG-565 | ai:claude
#[test]
fn partition_drops_gitignored_paths_keeps_remainder() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git_in(root, &["init", "-q"]);
    // The user's .gitignore covers .claude (a very common local-only entry).
    std::fs::write(root.join(".gitignore"), ".claude/\n").unwrap();
    std::fs::create_dir_all(root.join(".claude")).unwrap();
    std::fs::write(root.join(".claude/x"), "x").unwrap();
    std::fs::write(root.join("CLAUDE.md"), "x").unwrap();

    let paths = vec![
        ".gitignore".to_string(),
        "CLAUDE.md".to_string(),
        ".claude".to_string(),
    ];
    let (to_stage, ignored) = partition_scaffold_paths_by_gitignore(root, &paths);

    // The ignored path is filtered out (NOT force-added — respect the
    // user's ignore choice) and surfaced for a warning.
    assert_eq!(ignored, vec![".claude".to_string()]);
    // The remainder is staged, in input order.
    assert_eq!(
        to_stage,
        vec![".gitignore".to_string(), "CLAUDE.md".to_string()]
    );
}

/// BUG-565 end-to-end: a scaffold set containing one gitignored path must
/// still COMMIT the non-ignored remainder (init no longer aborts the whole
/// commit on one ignored path) and must NOT commit the ignored path.
// trace:BUG-565 | ai:claude
#[test]
fn commit_scaffolding_commits_remainder_when_one_path_is_gitignored() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git_in(root, &["init", "-q"]);
    git_in(root, &["config", "user.email", "t@t"]);
    git_in(root, &["config", "user.name", "t"]);
    git_in(root, &["commit", "--allow-empty", "-q", "-m", "root"]);

    // .gitignore covers .claude; init writes both .claude (ignored) and the
    // non-ignored scaffolding (.aida/config.toml, CLAUDE.md, .gitignore).
    std::fs::write(root.join(".gitignore"), ".claude/\n").unwrap();
    std::fs::write(root.join("CLAUDE.md"), "x").unwrap();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(root.join(".aida/config.toml"), "x").unwrap();
    std::fs::create_dir_all(root.join(".claude/skills")).unwrap();
    std::fs::write(root.join(".claude/skills/foo.md"), "x").unwrap();

    // Force the auto-commit branch (env override beats the TTY heuristic).
    // The guard holds the shared env lock and restores the prior value.
    // trace:BUG-1666 | ai:claude
    let env = crate::test_env::EnvVarGuard::set("AIDA_INIT_COMMIT_SCAFFOLD", "1");
    let committed = commit_init_scaffolding(root, crate::cli::InitFootprint::Full).unwrap();
    drop(env);

    // The remainder was committed → onboarding task is de-stranded.
    assert!(
        committed,
        "non-ignored scaffolding must commit even when one path is gitignored"
    );

    // The committed tree carries the non-ignored paths but NOT .claude.
    let tree = crate::test_git::git_command()
        .current_dir(root)
        .args(["ls-tree", "-r", "--name-only", "HEAD"])
        .output()
        .unwrap();
    let tracked = String::from_utf8_lossy(&tree.stdout);
    assert!(tracked.contains("CLAUDE.md"), "CLAUDE.md must be committed");
    assert!(
        tracked.contains(".aida/config.toml"),
        ".aida/config.toml must be committed"
    );
    assert!(
        !tracked.contains(".claude"),
        "the gitignored .claude path must NOT be committed (intent-respecting): {tracked}"
    );
}

/// Set up a temp git repo standing in for a fresh CLONE: an initial commit
/// exists on `main`, and a fake `origin` remote points HEAD at it so HEAD ==
/// origin/main at the start. Returns the repo root + the HEAD sha. The
// scaffold files are written but NOT committed by the helper. trace:BUG-570
fn setup_clone_like_repo(tmp: &TempDir) -> (std::path::PathBuf, String) {
    let root = tmp.path().to_path_buf();
    git_in(&root, &["init", "-q", "-b", "main"]);
    git_in(&root, &["config", "user.email", "t@t"]);
    git_in(&root, &["config", "user.name", "t"]);
    git_in(
        &root,
        &["commit", "--allow-empty", "-q", "-m", "upstream root"],
    );
    // A bare "origin" so origin/main resolves to the same commit (a clone
    // starts with HEAD == origin/main).
    let origin = tmp.path().join("origin.git");
    git_in(&root, &["init", "-q", "--bare", origin.to_str().unwrap()]);
    git_in(
        &root,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git_in(&root, &["push", "-q", "origin", "main"]);
    let head = git_head(&root);
    (root, head)
}

fn git_head(root: &std::path::Path) -> String {
    let out = crate::test_git::git_command()
        .current_dir(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Write the scaffold files a bootstrap-clone would land locally, then drive
/// `complete_init_scaffolding`. Returns the HEAD sha afterward.
fn run_complete_scaffolding(root: &std::path::Path, suppress: bool) -> String {
    // The Claude profile keeps the scaffold set small + deterministic.
    let store = aida_core::models::RequirementsStore::new();
    let db_path = root.join(".aida-store");
    complete_init_scaffolding(
        root,
        &store,
        Some("claude"),
        false, // with_mcp
        false, // no_skills
        true,  // no_hooks — keep the scaffold set lean for the test
        false, // force
        db_path,
        "test store",
        false, // verbose
        crate::cli::InitFootprint::Full,
        suppress,
    )
    .unwrap();
    git_head(root)
}

/// BUG-570 criterion 1+5: a bootstrap-clone init (suppress=true) must NOT
/// create a code-branch commit — HEAD stays exactly where origin/main is.
/// The scaffold files are written (working tree dirty) but uncommitted.
// trace:BUG-570 | ai:claude
#[test]
fn bootstrap_clone_init_leaves_head_unchanged() {
    let tmp = TempDir::new().unwrap();
    let (root, head_before) = setup_clone_like_repo(&tmp);

    let head_after = run_complete_scaffolding(&root, /* suppress */ true);

    assert_eq!(
        head_before, head_after,
        "bootstrap-clone init must not create a new code-branch commit"
    );
    // And it actually wrote local scaffold files (uncommitted).
    assert!(
        root.join("CLAUDE.md").exists(),
        "scaffold files should be written locally even when the commit is suppressed"
    );
    let status = crate::test_git::git_command()
        .current_dir(&root)
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "the written scaffold files should leave the working tree dirty (uncommitted)"
    );
}

/// BUG-838: init must not silently skip a user-owned AGENTS.md. The
/// AIDA-AUTOGEN block is AIDA-owned, so non-force init appends it while
/// preserving the existing user content.
// trace:BUG-838 | ai:codex
#[test]
fn init_appends_aida_block_to_existing_agents_md() {
    let tmp = TempDir::new().unwrap();
    let (root, _) = setup_clone_like_repo(&tmp);
    let original = "# AGENTS.md\n\nExisting Codex instructions.\n";
    std::fs::write(root.join("AGENTS.md"), original).unwrap();

    let store = aida_core::models::RequirementsStore::new();
    complete_init_scaffolding(
        &root,
        &store,
        None,  // default all-agent profile generates AGENTS.md
        false, // with_mcp
        false, // no_skills
        true,  // no_hooks
        false, // force
        root.join(".aida-store"),
        "test store",
        false, // verbose
        crate::cli::InitFootprint::Full,
        true, // suppress bootstrap commit
    )
    .unwrap();

    let content = std::fs::read_to_string(root.join("AGENTS.md")).unwrap();
    assert!(content.starts_with(original));
    assert!(content.contains("<!-- AIDA-AUTOGEN-BEGIN -->"));
    assert!(content.contains("<!-- AIDA-AUTOGEN-END -->"));
    assert!(content.contains("# AIDA Conventions"));
    // Only the delimited block may be appended — the generated seed's own
    // framing (its `# AGENTS.md` heading, orientation prose) must not be
    // spliced into a user-owned file. `original` carries the only H1.
    assert_eq!(content.matches("# AGENTS.md").count(), 1);
    assert!(!content.contains("Guidance for Codex and MCP-compatible coding agents"));
}

/// BUG-570 criterion 2: non-interactive (no TTY, the test harness) init on a
/// bootstrap-clone must NOT auto-commit. Same HEAD-unchanged guarantee as the
/// TTY case — suppression is independent of the TTY heuristic. The test
/// process has no TTY, so this exercises exactly the dangerous silent path.
// trace:BUG-570 | ai:claude
#[test]
fn bootstrap_clone_init_no_tty_does_not_autocommit() {
    // Belt-and-suspenders: even if some env tried to force auto-commit, the
    // bootstrap-clone suppression must win. Held for the whole test.
    // trace:BUG-1666 | ai:claude
    let _env = crate::test_env::EnvVarGuard::unset("AIDA_INIT_COMMIT_SCAFFOLD");
    let tmp = TempDir::new().unwrap();
    let (root, head_before) = setup_clone_like_repo(&tmp);

    let head_after = run_complete_scaffolding(&root, /* suppress */ true);

    assert_eq!(
        head_before, head_after,
        "non-TTY bootstrap-clone init must not silently auto-commit a scaffold dump"
    );
}

/// BUG-570 criterion 3: a genuinely-new first-init (suppress=false) STILL
/// commits its scaffolding — no regression. In the no-TTY test harness the
/// auto-commit branch fires, so HEAD must advance past the root commit.
// trace:BUG-570 | ai:claude
#[test]
fn genuinely_new_init_still_commits_scaffolding() {
    // trace:BUG-1666 | ai:claude
    let _env = crate::test_env::EnvVarGuard::unset("AIDA_INIT_COMMIT_SCAFFOLD");
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git_in(root, &["init", "-q", "-b", "main"]);
    git_in(root, &["config", "user.email", "t@t"]);
    git_in(root, &["config", "user.name", "t"]);
    git_in(root, &["commit", "--allow-empty", "-q", "-m", "root"]);
    let head_before = git_head(root);

    let head_after = run_complete_scaffolding(root, /* suppress */ false);

    assert_ne!(
        head_before, head_after,
        "genuinely-new first-init must still commit its scaffolding (no BUG-570 regression)"
    );
}
