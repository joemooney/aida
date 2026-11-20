//! STORY-2884: the project default for an unset execution mode.
// trace:STORY-2884 | ai:claude
// mandate-gate

use super::*;
use aida_core::{ExecutionMode, Requirement, RequirementType};

fn policy(toml: &str) -> UnsetModePolicy {
    UnsetModePolicy::from_body(Some(toml))
}

fn drain_bug_task() -> UnsetModePolicy {
    policy("[gate.unset_mode]\ndefault = \"drain\"\ndrain_types = [\"bug\", \"task\"]\n")
}

fn spec(req_type: RequirementType, title: &str, tags: &[&str]) -> Requirement {
    let mut r = Requirement::new(title.into(), "d".into());
    r.req_type = req_type;
    r.tags = tags.iter().map(|t| t.to_string()).collect();
    r
}

fn plain_bug() -> Requirement {
    spec(RequirementType::Bug, "fix the list sort", &[])
}

fn paths(ps: &[&str]) -> Option<Vec<String>> {
    Some(ps.iter().map(|p| p.to_string()).collect())
}

/// The effective mode after the change-paths withdrawal, as ship applies it.
fn at_change(mut mode: EffectiveMode, changed: Option<Vec<String>>) -> EffectiveMode {
    let root = tempfile::tempdir().unwrap();
    withdraw_defaults_for_change(std::iter::once(&mut mode), root.path(), || changed);
    mode
}

// AC1: no setting, or `default = "supervised"`, changes nothing.
#[test]
fn absent_or_supervised_setting_leaves_unset_supervised() {
    for body in [
        None,
        Some(""),
        Some("[gate]\n"),
        Some("[gate.unset_mode]\n"),
        Some("[gate.unset_mode]\ndefault = \"supervised\"\ndrain_types = [\"bug\"]\n"),
    ] {
        let p = UnsetModePolicy::from_body(body);
        assert_eq!(p.default, UnsetDefault::Supervised, "{body:?}");
        assert!(!p.drains_any(), "{body:?}");
        let mode = p.resolve(None, &plain_bug());
        assert_eq!(mode, EffectiveMode::from(None), "{body:?}");
        assert!(mode.requires_supervision());
        assert_eq!(mode.label(), "unset (supervised)");
        // A set mode is untouched.
        for m in [
            ExecutionMode::Drain,
            ExecutionMode::Drive,
            ExecutionMode::Guided,
        ] {
            assert_eq!(
                p.resolve(Some(m), &plain_bug()),
                EffectiveMode::from(Some(m))
            );
        }
    }
}

// A malformed setting never fails open.
#[test]
fn malformed_setting_stays_supervised_with_a_warning() {
    for body in [
        "[gate.unset_mode]\ndefault = \"yes\"\ndrain_types = [\"bug\"]\n",
        "[gate.unset_mode]\ndefault = true\ndrain_types = [\"bug\"]\n",
        "[gate]\nunset_mode = \"drain\"\n",
        "[gate.unset_mode]\ndefault = \"drain\"\n",
        "[gate.unset_mode]\ndefault = \"drain\"\ndrain_types = \"bug\"\n",
        "[gate.unset_mode]\ndefault = \"drain\"\ndrain_types = [\"bugz\", 7]\n",
        "[gate.unset_mode\ndefault = \"drain\"\n",
    ] {
        let p = policy(body);
        assert!(!p.drains_any(), "{body}");
        assert!(!p.warnings.is_empty(), "{body}");
        assert!(
            p.resolve(None, &plain_bug()).requires_supervision(),
            "{body}"
        );
    }
}

#[test]
fn type_names_match_whatever_their_spelling() {
    let p = policy(
        "[gate.unset_mode] # trailing comment\ndefault = \" drain \" # cmt\n\
         drain_types = [\"BUG\", \"Change Request\", \"non-functional\", \"bug\", \"nope\"]\n",
    );
    assert_eq!(
        p.drain_types,
        vec![
            RequirementType::Bug,
            RequirementType::ChangeRequest,
            RequirementType::NonFunctional
        ]
    );
    assert_eq!(p.warnings.len(), 1, "{:?}", p.warnings);
    assert!(p.warnings[0].contains("nope"), "{:?}", p.warnings);
}

// AC2: type in/out of the list × sensitive or not × protected path or not ×
// explicit mode. Only an unset, listed, non-sensitive spec on a change that
// touches no protected path drains.
#[test]
fn drain_default_matrix() {
    let p = drain_bug_task();
    let protected_change = paths(&["aida-cli-lib/src/gate/ship.rs", "README.md"]);
    let config_change = paths(&[".aida/config.toml"]);
    let ordinary_change = paths(&["aida-cli-lib/src/list.rs", "notes/x.md"]);
    let sensitive: Vec<(&str, Requirement)> = vec![
        (
            "keystone tag",
            spec(RequirementType::Bug, "fix", &["keystone"]),
        ),
        (
            "security title",
            spec(RequirementType::Bug, "security hole", &[]),
        ),
        (
            "authority tag",
            spec(RequirementType::Task, "fix", &["review-authority"]),
        ),
        (
            "containment title",
            spec(RequirementType::Task, "containment leak", &[]),
        ),
        (
            "operator-only tag",
            spec(RequirementType::Bug, "fix", &["operator-only"]),
        ),
    ];
    let ordinary = vec![
        ("bug", plain_bug(), "bug"),
        ("task", spec(RequirementType::Task, "tidy", &[]), "task"),
    ];
    let unlisted = vec![
        spec(RequirementType::Story, "a story", &[]),
        spec(RequirementType::Epic, "an epic", &[]),
        spec(RequirementType::Spike, "a spike", &[]),
    ];
    for (name, req, word) in &ordinary {
        // Listed, not sensitive, ordinary change: drains, and says why.
        let mode = at_change(p.resolve(None, req), ordinary_change.clone());
        assert_eq!(mode.mode, Some(ExecutionMode::Drain), "{name}");
        assert!(!mode.requires_supervision(), "{name}");
        assert_eq!(
            mode.label(),
            format!("drain (project default for unset {word})"),
            "{name}"
        );
        // Same spec, change touches a protected path (or its files are
        // unknown): supervised, naming the path.
        for (change, needle) in [
            (protected_change.clone(), "aida-cli-lib/src/gate/ship.rs"),
            (config_change.clone(), ".aida/config.toml"),
            (None, "could not be read"),
        ] {
            let mode = at_change(p.resolve(None, req), change);
            assert!(mode.requires_supervision(), "{name} {needle}");
            assert!(
                mode.label().starts_with("unset (supervised: "),
                "{}",
                mode.label()
            );
            assert!(mode.label().contains(needle), "{}", mode.label());
        }
        // An explicit mode always wins, whatever the change.
        for m in [
            ExecutionMode::Drive,
            ExecutionMode::Guided,
            ExecutionMode::Operator,
            ExecutionMode::Decide,
        ] {
            for change in [ordinary_change.clone(), protected_change.clone()] {
                let mode = at_change(p.resolve(Some(m), req), change);
                assert_eq!(mode, EffectiveMode::from(Some(m)), "{name} {m}");
                assert!(mode.requires_supervision());
                assert_eq!(mode.label(), m.to_string());
            }
        }
        // An explicit drain stays drain, even on a protected path (the
        // mandate's own path gate governs that, unchanged).
        let mode = at_change(
            p.resolve(Some(ExecutionMode::Drain), req),
            protected_change.clone(),
        );
        assert_eq!(mode, EffectiveMode::from(Some(ExecutionMode::Drain)));
    }
    for (name, req) in &sensitive {
        for change in [ordinary_change.clone(), protected_change.clone()] {
            let mode = at_change(p.resolve(None, req), change);
            assert!(mode.requires_supervision(), "{name}");
            assert!(
                mode.label().starts_with("unset (supervised: "),
                "{name}: {}",
                mode.label()
            );
        }
    }
    for req in &unlisted {
        let mode = at_change(p.resolve(None, req), ordinary_change.clone());
        assert!(mode.requires_supervision(), "{:?}", req.req_type);
        assert!(
            mode.label().contains("does not cover type"),
            "{}",
            mode.label()
        );
    }
}

// The protected set is read from the gate config; an unreadable one is the
// floor, so a non-repository root still protects `.aida/config.toml`.
#[test]
fn an_unreadable_gate_config_still_protects_the_floor() {
    let root = tempfile::tempdir().unwrap();
    let floor = protected_paths(root.path());
    assert!(protected_path_reason(paths(&[".aida/config.toml"]).as_deref(), &floor).is_some());
    assert!(protected_path_reason(paths(&["src/a.rs"]).as_deref(), &floor).is_none());
    assert!(protected_path_reason(Some(&[] as &[String]), &floor).is_none());
}

// The changed paths are read only when some spec takes the default.
#[test]
fn change_paths_are_read_only_for_a_project_default() {
    let root = tempfile::tempdir().unwrap();
    let mut modes = vec![
        EffectiveMode::from(None),
        EffectiveMode::from(Some(ExecutionMode::Drive)),
    ];
    withdraw_defaults_for_change(modes.iter_mut(), root.path(), || {
        panic!("no project default: the change's files are not read")
    });
    let mut read = false;
    let mut modes = vec![drain_bug_task().resolve(None, &plain_bug())];
    withdraw_defaults_for_change(modes.iter_mut(), root.path(), || {
        read = true;
        paths(&["src/a.rs"])
    });
    assert!(read);
    assert_eq!(modes[0].mode, Some(ExecutionMode::Drain));
}

// AC2/AC3 at the ship gate: the hold names the effective mode and its source.
#[test]
fn ship_holds_name_the_effective_mode() {
    let p = drain_bug_task();
    let records = vec![
        (
            "BUG-1".to_string(),
            Vec::new(),
            p.resolve(None, &plain_bug()),
        ),
        (
            "STORY-2".to_string(),
            Vec::new(),
            p.resolve(None, &spec(RequirementType::Story, "s", &[])),
        ),
        (
            "TASK-3".to_string(),
            Vec::new(),
            EffectiveMode::from(Some(ExecutionMode::Guided)),
        ),
    ];
    let holds = crate::gate::ship::effective_supervised_merge_holds(&records, false);
    assert_eq!(
        holds,
        vec![
            (
                "STORY-2".to_string(),
                "unset (supervised: the project's drain default does not cover type story)"
                    .to_string()
            ),
            ("TASK-3".to_string(), "guided".to_string()),
        ]
    );
    assert!(crate::gate::ship::effective_supervised_merge_holds(&records, true).is_empty());
    // With the default withdrawn for a protected path, the bug holds too.
    let mut records = records;
    let root = tempfile::tempdir().unwrap();
    withdraw_defaults_for_change(records.iter_mut().map(|(_, _, m)| m), root.path(), || {
        paths(&["aida-cli-lib/src/gate/mod.rs"])
    });
    let holds = crate::gate::ship::effective_supervised_merge_holds(&records, false);
    assert_eq!(holds.len(), 3, "{holds:?}");
    assert!(
        holds[0].1.contains("aida-cli-lib/src/gate/mod.rs"),
        "{holds:?}"
    );
}

// The evidence a train records keeps its old shape: the mode alone.
#[test]
fn effective_mode_serializes_as_the_mode() {
    let drained = drain_bug_task().resolve(None, &plain_bug());
    assert_eq!(
        serde_json::to_value(&drained).unwrap(),
        serde_json::json!("drain")
    );
    assert_eq!(
        serde_json::to_value(EffectiveMode::from(None)).unwrap(),
        serde_json::Value::Null
    );
}

// The setting is read from the default branch only: a branch-local edit has
// no effect, and no repository means the default.
#[test]
fn only_the_default_branch_config_counts() {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .args(args)
            .current_dir(dir.path())
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    assert!(!UnsetModePolicy::load(dir.path()).drains_any());
    git(&["init", "-q", "-b", "main"]);
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    std::fs::write(
        dir.path().join(".aida/config.toml"),
        "[agents]\nvendor = \"x\"\n",
    )
    .unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "init"]);
    git(&["checkout", "-qb", "feature"]);
    std::fs::write(
        dir.path().join(".aida/config.toml"),
        "[gate.unset_mode]\ndefault = \"drain\"\ndrain_types = [\"bug\"]\n",
    )
    .unwrap();
    git(&["commit", "-qam", "branch-local drain"]);
    assert!(
        !UnsetModePolicy::load(dir.path()).drains_any(),
        "a feature branch cannot opt itself into the drain default"
    );
    assert_eq!(show_label(dir.path(), &plain_bug()), None);
    git(&["checkout", "-q", "main"]);
    git(&["merge", "-q", "--ff-only", "feature"]);
    assert!(UnsetModePolicy::load(dir.path()).drains_any());
    assert_eq!(
        show_label(dir.path(), &plain_bug()).as_deref(),
        Some("drain (project default for unset bug)")
    );
    let mut set = plain_bug();
    set.execution_mode = Some(ExecutionMode::Drive);
    assert_eq!(
        show_label(dir.path(), &set),
        None,
        "a set mode prints as before"
    );
}
