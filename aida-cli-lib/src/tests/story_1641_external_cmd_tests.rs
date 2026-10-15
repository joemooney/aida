//! Unit coverage for git-style external subcommands (`aida-<name>` on PATH):
//! the dispatch decision, discovery, shadowing, and the extension context.
// trace:STORY-1641 | ai:claude

use super::*;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

// trace:STORY-1641 | ai:claude
#[test]
fn dispatch_name_skips_builtins_flags_help_and_companions() {
    let shadowed = shadowed_names();
    assert_eq!(dispatch_name(&s(&["aida"]), &shadowed), None);
    assert_eq!(dispatch_name(&s(&["aida", "list"]), &shadowed), None);
    assert_eq!(dispatch_name(&s(&["aida", "show", "X-1"]), &shadowed), None);
    assert_eq!(dispatch_name(&s(&["aida", "--json"]), &shadowed), None);
    assert_eq!(dispatch_name(&s(&["aida", "help"]), &shadowed), None);
    assert_eq!(dispatch_name(&s(&["aida", "server"]), &shadowed), None);
    assert_eq!(dispatch_name(&s(&["aida", "../evil"]), &shadowed), None);
    assert_eq!(dispatch_name(&s(&["aida", ".hidden"]), &shadowed), None);
    assert_eq!(
        dispatch_name(&s(&["aida", "hello", "a", "b c"]), &shadowed),
        Some("hello")
    );
}

// trace:STORY-1641 | ai:claude
#[cfg(unix)]
#[test]
fn discover_in_first_on_path_wins_and_hides_shadowed_and_non_executable() {
    let tmp = tempfile::tempdir().unwrap();
    let d1 = tmp.path().join("one");
    let d2 = tmp.path().join("two");
    std::fs::create_dir_all(&d1).unwrap();
    std::fs::create_dir_all(&d2).unwrap();
    let script = "#!/bin/sh\nexit 0\n";
    crate::test_exec::write_executable(&d1.join("aida-hello"), script);
    crate::test_exec::write_executable(&d1.join("aida-list"), script);
    crate::test_exec::write_executable(&d2.join("aida-hello"), script);
    crate::test_exec::write_executable(&d2.join("aida-zed"), script);
    crate::test_exec::write_executable(&d2.join("aida-server"), script);
    std::fs::write(d1.join("aida-plain"), script).unwrap();
    std::fs::write(d1.join("unrelated"), script).unwrap();

    let path_var = std::env::join_paths([&d1, &d2]).unwrap();
    let found = discover_in(Some(path_var.as_os_str()), &shadowed_names());
    let names: Vec<&str> = found.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["hello", "zed"]);
    assert_eq!(found[0].path, d1.join("aida-hello"), "first PATH dir wins");

    assert_eq!(
        find_in("hello", Some(path_var.as_os_str())),
        Some(d1.join("aida-hello"))
    );
    assert_eq!(find_in("plain", Some(path_var.as_os_str())), None);
    assert_eq!(find_in("missing", Some(path_var.as_os_str())), None);
    assert_eq!(find_in("hello", None), None);

    let empty = tmp.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let empty_path = std::env::join_paths([&empty]).unwrap();
    assert!(discover_in(Some(empty_path.as_os_str()), &shadowed_names()).is_empty());
}

// trace:STORY-1641 | ai:claude
#[cfg(unix)]
#[test]
fn name_from_file_name_accepts_only_plain_command_words() {
    assert_eq!(name_from_file_name("aida-hello"), Some("hello".to_string()));
    assert_eq!(
        name_from_file_name("aida-my_tool"),
        Some("my_tool".to_string())
    );
    assert_eq!(name_from_file_name("aida-"), None);
    assert_eq!(name_from_file_name("aida-.x"), None);
    assert_eq!(name_from_file_name("aida--x"), None);
    assert_eq!(name_from_file_name("aidahello"), None);
}

/// A user alias resolves for an interactive human before the external stage
/// sees argv, so an `aida-<alias>` never runs for that caller; an agent's argv
/// is not expanded, so the same name reaches the external stage.
// trace:STORY-1641 | ai:claude
#[test]
fn human_user_alias_wins_over_external_but_agent_reaches_it() {
    let dir = tempfile::tempdir().unwrap();
    let aliases = dir.path().join("home").join(".aida").join("aliases.toml");
    std::fs::create_dir_all(aliases.parent().unwrap()).unwrap();
    std::fs::write(&aliases, "[alias]\nfoo = \"list --status draft\"\n").unwrap();
    let home = dir.path().to_str().unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_TEST_HOME", Some(home)),
        ("AIDA_AGENT_TYPE", None),
        ("AIDA_HEADLESS", None),
    ]);
    let shadowed = shadowed_names();
    let argv = s(&["aida", "foo", "--json"]);

    // Human path: the alias expands first, the head is a built-in.
    let human = crate::user_alias::expand_inner(&argv, false);
    assert_eq!(human, s(&["aida", "list", "--status", "draft", "--json"]));
    assert_eq!(dispatch_name(&human, &shadowed), None);

    // Agent path: argv is unchanged, so the external stage sees `foo`.
    assert_eq!(dispatch_name(&argv, &shadowed), Some("foo"));
}

// trace:STORY-1641 | ai:claude
#[test]
fn caller_kind_matches_the_alias_gate() {
    assert_eq!(caller_kind_from(None, None, true), "human");
    assert_eq!(caller_kind_from(None, None, false), "agent");
    assert_eq!(caller_kind_from(Some("codex"), None, true), "agent");
    assert_eq!(caller_kind_from(Some(""), None, true), "human");
    assert_eq!(caller_kind_from(None, Some("1"), true), "headless");
    assert_eq!(
        caller_kind_from(Some("claude"), Some("1"), false),
        "headless"
    );
    assert_eq!(caller_kind_from(None, Some("0"), true), "human");
}

// trace:STORY-1641 | ai:claude
#[test]
fn output_format_label_resolves_pin_then_agent_default() {
    assert_eq!(output_format_label(Some("json"), None, true), "json");
    assert_eq!(output_format_label(Some("TOON"), None, true), "toon");
    assert_eq!(
        output_format_label(Some("human"), Some("1"), false),
        "human"
    );
    assert_eq!(output_format_label(None, None, false), "toon");
    assert_eq!(output_format_label(None, None, true), "human");
    assert_eq!(output_format_label(None, Some("0"), false), "human");
    assert_eq!(output_format_label(Some("bogus"), None, true), "human");
}

// trace:STORY-1641 | ai:claude
#[test]
fn project_and_store_resolve_from_root_and_subdir_and_not_outside() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(proj.join(".git")).unwrap();
    std::fs::create_dir_all(proj.join(".aida")).unwrap();
    std::fs::create_dir_all(proj.join(".aida-store")).unwrap();
    std::fs::create_dir_all(proj.join("sub").join("deeper")).unwrap();
    std::fs::write(
        proj.join(".aida").join("config.toml"),
        "store_path = \".aida-store\"\n",
    )
    .unwrap();

    let expected = (Some(proj.clone()), Some(proj.join(".aida-store")));
    assert_eq!(project_and_store_from(&proj), expected);
    assert_eq!(
        project_and_store_from(&proj.join("sub").join("deeper")),
        expected
    );

    // A git checkout that is not an AIDA project yields neither.
    let plain = tmp.path().join("plain");
    std::fs::create_dir_all(plain.join(".git")).unwrap();
    assert_eq!(project_and_store_from(&plain), (None, None));
}

// trace:STORY-1641 | ai:claude
#[test]
fn near_matches_suggest_close_installed_names_only() {
    let installed = vec![
        External {
            name: "hallo".to_string(),
            path: PathBuf::from("/x/aida-hallo"),
        },
        External {
            name: "deploy".to_string(),
            path: PathBuf::from("/x/aida-deploy"),
        },
    ];
    let hits = near_matches("hello", &installed);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name, "hallo");
    assert!(near_matches("zzzzzz", &installed).is_empty());
    assert!(
        near_matches("hallo", &installed[..1]).is_empty(),
        "exact name is not a hint"
    );
}

// trace:STORY-1641 | ai:claude
#[test]
fn same_binary_compares_resolved_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    std::fs::write(&a, "x").unwrap();
    assert!(same_binary(&a, &a));
    assert!(same_binary(&a, &tmp.path().join(".").join("a")));
    assert!(!same_binary(&a, &tmp.path().join("b")));
}
