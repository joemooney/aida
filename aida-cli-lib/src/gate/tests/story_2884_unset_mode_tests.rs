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
    withdraw_defaults_for_change(std::iter::once(&mut mode), || changed);
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

// The floor always protects `.aida/config.toml`.
#[test]
fn the_floor_protects_the_gate_config() {
    let floor = crate::gate::mandate_gates::ProtectedPaths::floor();
    assert!(protected_path_reason(paths(&[".aida/config.toml"]).as_deref(), &floor).is_some());
    assert!(protected_path_reason(paths(&["src/a.rs"]).as_deref(), &floor).is_none());
    assert!(protected_path_reason(Some(&[] as &[String]), &floor).is_none());
}

const DRAIN_BUG: &str = "[gate.unset_mode]\ndefault = \"drain\"\ndrain_types = [\"bug\"]\n";
const PAYMENTS: &str = "[mandate.human_gates]\nprotected_paths = [\"src/payments/\"]\n";

fn withheld(mode: &EffectiveMode) -> String {
    match &mode.source {
        ModeSource::Unset {
            withheld: Some(why),
        } => why.clone(),
        other => panic!("not withheld: {other:?} {mode:?}"),
    }
}

// Round 2, finding 1: the committed protected paths come from the same body
// as the setting, so a custom protected path withdraws the default.
#[test]
fn committed_custom_protection_withdraws_the_default() {
    let p = policy(&format!("{DRAIN_BUG}{PAYMENTS}"));
    let mode = p.resolve(None, &plain_bug());
    assert!(mode.is_project_default());
    let hit = at_change(mode.clone(), paths(&["src/payments/charge.rs"]));
    assert!(hit.requires_supervision());
    assert!(withheld(&hit).contains("src/payments/"), "{hit:?}");
    assert!(!at_change(mode, paths(&["src/list.rs"])).requires_supervision());
}

// Finding 1: a committed or operator gate layer that cannot be read leaves
// the protection unknown, and no spec takes the default (fail closed).
#[test]
fn unreadable_protection_withdraws_every_default() {
    // A committed `[mandate]` layer that does not parse.
    let p = policy(&format!("{DRAIN_BUG}[mandate.human_gates]\nbogus = 1\n"));
    assert!(p.protected.is_err());
    let mode = p.resolve(None, &plain_bug());
    assert!(mode.requires_supervision());
    assert!(
        withheld(&mode).contains("protected gate paths could not be read"),
        "{mode:?}"
    );
    // An operator layer that cannot be read, with committed custom paths.
    let p = policy(&format!("{DRAIN_BUG}{PAYMENTS}")).with_operator_layer(Err(anyhow::anyhow!(
        "~/.aida/config.toml is not valid TOML"
    )));
    let mode = p.resolve(None, &plain_bug());
    assert!(mode.requires_supervision());
    assert!(withheld(&mode).contains("not valid TOML"), "{mode:?}");
    // A readable operator layer adds its paths.
    let operator =
        crate::gate::mandate_gates::parse_layer(PAYMENTS.replace("payments", "ops").as_str(), "op")
            .unwrap();
    let p = policy(DRAIN_BUG).with_operator_layer(Ok(operator));
    let mode = p.resolve(None, &plain_bug());
    assert!(mode.is_project_default());
    assert!(at_change(mode.clone(), paths(&["src/ops/a.rs"])).requires_supervision());
    assert!(!at_change(mode, paths(&["src/a.rs"])).requires_supervision());
}

// The changed paths are read only when some spec takes the default.
#[test]
fn change_paths_are_read_only_for_a_project_default() {
    let mut modes = vec![
        EffectiveMode::from(None),
        EffectiveMode::from(Some(ExecutionMode::Drive)),
    ];
    withdraw_defaults_for_change(modes.iter_mut(), || {
        panic!("no project default: the change's files are not read")
    });
    let mut read = false;
    let mut modes = vec![drain_bug_task().resolve(None, &plain_bug())];
    withdraw_defaults_for_change(modes.iter_mut(), || {
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
    withdraw_defaults_for_change(records.iter_mut().map(|(_, _, m)| m), || {
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
    let home = tempfile::tempdir().unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_HOME", home.path().to_str())]);
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
    assert_eq!(show_label(dir.path(), &plain_bug(), || None), None);
    git(&["checkout", "-q", "main"]);
    git(&["merge", "-q", "--ff-only", "feature"]);
    assert!(UnsetModePolicy::load(dir.path()).drains_any());
    assert_eq!(
        show_label(dir.path(), &plain_bug(), || None).as_deref(),
        Some("drain (project default for unset bug)")
    );
    let mut set = plain_bug();
    set.execution_mode = Some(ExecutionMode::Drive);
    assert_eq!(
        show_label(dir.path(), &set, || None),
        None,
        "a set mode prints as before"
    );
}

fn git_in(dir: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .args(args)
        .current_dir(dir)
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
}

/// A repository whose default branch is `branch`, committing `config`.
fn repo_on(branch: &str, config: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git_in(dir.path(), &["init", "-q", "-b", branch]);
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    std::fs::write(dir.path().join(".aida/config.toml"), config).unwrap();
    git_in(dir.path(), &["add", "."]);
    git_in(dir.path(), &["commit", "-qm", "init"]);
    dir
}

// Finding 1: a repository whose default branch is `master` reads the
// committed protected paths from the same commit as the setting, and an
// unreadable operator layer withdraws the default there too.
#[test]
fn a_master_default_branch_reads_its_protection_and_fails_closed() {
    let home = tempfile::tempdir().unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_HOME", home.path().to_str())]);
    let dir = repo_on("master", &format!("{DRAIN_BUG}{PAYMENTS}"));
    let p = UnsetModePolicy::load(dir.path());
    let mode = p.resolve(None, &plain_bug());
    assert!(mode.is_project_default(), "{p:?}");
    assert!(at_change(mode.clone(), paths(&["src/payments/a.rs"])).requires_supervision());
    assert!(!at_change(mode, paths(&["src/a.rs"])).requires_supervision());
    // A malformed operator config: protection unknown, no default.
    std::fs::create_dir_all(home.path().join(".aida")).unwrap();
    std::fs::write(home.path().join(".aida/config.toml"), "[mandate\n").unwrap();
    let mode = UnsetModePolicy::load(dir.path()).resolve(None, &plain_bug());
    assert!(mode.requires_supervision());
    assert!(
        withheld(&mode).contains("protected gate paths could not be read"),
        "{mode:?}"
    );
}

// Finding 2: the default is admitted for the head its files were read at;
// a different (or unknown) preflight head is refused.
#[test]
fn a_head_moved_after_the_default_check_is_refused() {
    let (h1, h2) = ("1".repeat(40), "2".repeat(40));
    let heads = CheckedHeads::default();
    assert_eq!(
        heads.moved_refusal(7, Some(&h2)),
        None,
        "no default checked"
    );
    assert_eq!(heads.moved_refusal(7, None), None);
    heads.record(&h1);
    heads.record(&h1);
    assert_eq!(heads.moved_refusal(7, Some(&h1)), None);
    // H1 was ordinary; H2, pushed before preflight, touches a protected path
    // the default never saw.
    let refusal = heads
        .moved_refusal(7, Some(&h2))
        .expect("moved head refused");
    assert!(
        refusal.contains("PR-7") && refusal.contains(&h1[..12]),
        "{refusal}"
    );
    assert!(
        heads.moved_refusal(7, None).is_some(),
        "unknown head refused"
    );
    heads.record(&h2);
    assert!(
        heads.moved_refusal(7, Some(&h2)).is_some(),
        "H1 still unpinned"
    );
}

fn superseded_into(child: &mut Requirement, carrier: &Requirement) {
    child.relationships.push(aida_core::Relationship {
        rel_type: aida_core::RelationshipType::SupersededBy,
        target_id: carrier.id,
        created_at: None,
        created_by: None,
    });
}

// Finding 3: `aida show` resolves a superseded unset spec through its
// carrier, as the merge gate does, and names the carrier.
#[test]
fn show_label_follows_the_superseding_carrier() {
    let home = tempfile::tempdir().unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_HOME", home.path().to_str())]);
    let dir = repo_on("main", DRAIN_BUG);
    let carrier = |mode, id: &str| {
        let mut c = spec(RequirementType::Story, "carrier", &[]);
        c.spec_id = Some(id.into());
        c.execution_mode = Some(mode);
        c
    };
    for (mode, child_type, want) in [
        (ExecutionMode::Guided, RequirementType::Bug, "guided"),
        (ExecutionMode::Operator, RequirementType::Bug, "operator"),
        (ExecutionMode::Drain, RequirementType::Story, "drain"),
    ] {
        let c = carrier(mode, "STORY-90");
        let mut child = spec(child_type, "bundled child", &[]);
        superseded_into(&mut child, &c);
        let store = vec![child.clone(), c.clone()];
        let label = show_label(dir.path(), &child, || Some(store)).unwrap();
        assert!(label.starts_with(want), "{mode:?}: {label}");
        assert!(label.contains("from its carrier"), "{label}");
        assert!(!label.contains("project default"), "{label}");
        // The merge gate's mode agrees.
        let reqs = vec![child.clone(), c];
        assert_eq!(
            crate::gate::ship::gating_execution_mode(&reqs, &child),
            Some(mode)
        );
    }
    // An unreadable store: the carrier is unknown, so supervised.
    let c = carrier(ExecutionMode::Drain, "STORY-91");
    let mut child = plain_bug();
    superseded_into(&mut child, &c);
    let label = show_label(dir.path(), &child, || None).unwrap();
    assert!(label.starts_with("unset (supervised"), "{label}");
    // A carrier without a mode: the child takes the project default itself.
    let mut c = c;
    c.execution_mode = None;
    let label = show_label(dir.path(), &child, || Some(vec![c])).unwrap();
    assert_eq!(label, "drain (project default for unset bug)");
}
