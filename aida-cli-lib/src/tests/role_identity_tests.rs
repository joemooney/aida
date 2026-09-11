use super::{
    canonical_role_name, default_role_guidance, is_human_route, load_role,
    role_restore_prompt_enabled_from, role_restore_prompt_marker_name, roleless_recovery_line_for,
    save_role_at, scaffold_starter_roles, RoleState, STARTER_ROLES,
};

// TASK-747: `human` is a first-class route target (the escalation-cascade
// terminus), canonicalized to the lowercase form regardless of input
// casing so `--for Human` / `--for HUMAN` route identically.
// trace:TASK-747 | ai:claude
#[test]
fn human_route_recognized_case_insensitively() {
    assert!(is_human_route("human"));
    assert!(is_human_route("Human"));
    assert!(is_human_route("HUMAN"));
    assert!(!is_human_route("implementer"));
    assert!(!is_human_route("advisor"));
}

#[test]
fn human_canonicalizes_to_lowercase() {
    assert_eq!(canonical_role_name("human"), "human");
    assert_eq!(canonical_role_name("Human"), "human");
    assert_eq!(canonical_role_name("HUMAN"), "human");
}

// TASK-747: a `--for human` routed spec counts toward the view only while
// open; archived or terminal specs that once carried the route drop out.
// trace:TASK-747 | ai:claude
#[test]
fn human_route_open_predicate() {
    use super::human_route_is_open;
    use aida_core::RequirementStatus::*;
    // Open statuses → routed spec is a live bottleneck.
    assert!(human_route_is_open(false, &Draft));
    assert!(human_route_is_open(false, &Approved));
    assert!(human_route_is_open(false, &InProgress));
    assert!(human_route_is_open(false, &Done));
    // Terminal statuses → no longer a bottleneck.
    assert!(!human_route_is_open(false, &Completed));
    assert!(!human_route_is_open(false, &Rejected));
    // Archived drops out regardless of status.
    assert!(!human_route_is_open(true, &Approved));
}

// TASK-586: `advisor` is canonical; `dialog` is a deprecated alias that
// still resolves (case-insensitively). Other role names pass through.
#[test]
fn dialog_canonicalizes_to_advisor() {
    assert_eq!(canonical_role_name("dialog"), "advisor");
    assert_eq!(canonical_role_name("Dialog"), "advisor");
    assert_eq!(canonical_role_name("DIALOG"), "advisor");
    assert_eq!(canonical_role_name("advisor"), "advisor");
}

// Doer roles (and anything else) are unchanged by canonicalization.
#[test]
fn other_roles_pass_through_canonicalization() {
    for name in ["implementer", "reviewer", "architect", "triage"] {
        assert_eq!(canonical_role_name(name), name, "role {name}");
    }
}

// The advisor starter role exists with advisor framing, and the old
// `dialog` starter name + "Captain / PO hat" framing are both gone.
#[test]
fn starter_set_uses_advisor_not_dialog() {
    assert!(
        STARTER_ROLES.iter().any(|(name, _)| *name == "advisor"),
        "advisor must be a starter role"
    );
    assert!(
        !STARTER_ROLES.iter().any(|(name, _)| *name == "dialog"),
        "dialog must no longer be a starter role name"
    );
    let (_, purpose) = STARTER_ROLES
        .iter()
        .find(|(name, _)| *name == "advisor")
        .expect("advisor is a starter role");
    assert!(
        !purpose.contains("PO hat"),
        "old captain/PO-hat framing should be removed: {purpose}"
    );
}

// TASK-608: the scaffold default set is the agent-wired role taxonomy
// (implementer, advisor, reviewer). architect/triage have no
// orchestrator phase and are opt-in via `aida role add` — they must NOT
// ship as starter roles. trace:TASK-608 | ai:claude
#[test]
fn starter_set_is_agent_wired_only() {
    let names: Vec<&str> = STARTER_ROLES.iter().map(|(name, _)| *name).collect();
    assert!(
        names.contains(&"implementer"),
        "implementer must be scaffolded"
    );
    // TASK-1200: product is the starter intake / PO seat. It is not an
    // orchestrator phase, but it must ship on a fresh machine so role pickers
    // expose requirement-capture work out of the box. trace:TASK-1200 | ai:codex
    assert!(names.contains(&"product"), "product must be scaffolded");
    assert!(names.contains(&"advisor"), "advisor must be scaffolded");
    assert!(names.contains(&"reviewer"), "reviewer must be scaffolded");
    // trace:STORY-460 | ai:claude
    assert!(
        names.contains(&"integrator"),
        "integrator must be scaffolded"
    );
    assert!(
        !names.contains(&"architect"),
        "architect must be opt-in, not a default starter role"
    );
    assert!(
        !names.contains(&"triage"),
        "triage must be opt-in, not a default starter role"
    );
}

#[test]
fn product_starter_purpose_is_intake_not_advisor() {
    let (_, purpose) = STARTER_ROLES
        .iter()
        .find(|(name, _)| *name == "product")
        .expect("product is a starter role");

    assert!(purpose.contains("Intake"), "{purpose}");
    assert!(purpose.contains("requirements"), "{purpose}");
    assert!(purpose.contains("Distinct from advisor"), "{purpose}");
    assert!(
        purpose.contains("advisor owns strategic counsel"),
        "{purpose}"
    );
}

#[test]
fn product_default_guidance_is_available_without_role_file() {
    let guidance = default_role_guidance("product");

    assert!(guidance.contains("product seat"), "{guidance}");
    assert!(guidance.contains("capture requirements"), "{guidance}");
    assert!(guidance.contains("advisor"), "{guidance}");
}

#[test]
fn roleless_recovery_uses_last_used_role_and_copyable_command() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = chrono::Utc::now();
    let old = RoleState {
        name: "implementer".to_string(),
        purpose: None,
        created_at: now - chrono::Duration::hours(2),
        last_active_at: now - chrono::Duration::hours(2),
        working_directory: None,
        notes: None,
        global: false,
        activity: Vec::new(),
        scope_tags: Vec::new(),
        scope_status: None,
        system_prompt: None,
    };
    let recent = RoleState {
        name: "advisor".to_string(),
        last_active_at: now,
        ..old.clone()
    };
    save_role_at(&old, &root.join(".aida/roles/implementer.toml")).unwrap();
    save_role_at(&recent, &root.join(".aida/roles/advisor.toml")).unwrap();

    let line = roleless_recovery_line_for(root, false).unwrap();

    assert!(line.starts_with("No active role."), "{line}");
    assert!(line.contains("Last-used role: advisor"), "{line}");
    assert!(line.contains("`aida role enter advisor`"), "{line}");
}

#[test]
fn roleless_recovery_silent_when_role_is_active() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = chrono::Utc::now();
    let role = RoleState {
        name: "advisor".to_string(),
        purpose: None,
        created_at: now,
        last_active_at: now,
        working_directory: None,
        notes: None,
        global: false,
        activity: Vec::new(),
        scope_tags: Vec::new(),
        scope_status: None,
        system_prompt: None,
    };
    save_role_at(&role, &root.join(".aida/roles/advisor.toml")).unwrap();

    assert!(roleless_recovery_line_for(root, true).is_none());
}

#[test]
fn role_restore_prompt_config_defaults_off_and_can_enable() {
    assert!(!role_restore_prompt_enabled_from(""));
    assert!(!role_restore_prompt_enabled_from("[role]\n"));
    assert!(role_restore_prompt_enabled_from(
        "[role]\nrestore_prompt = true\n"
    ));
    assert!(!role_restore_prompt_enabled_from(
        "[role]\nrestore_prompt = false\n"
    ));
}

#[test]
fn role_restore_prompt_marker_name_is_filesystem_safe() {
    let name = role_restore_prompt_marker_name("/dev/pts/12");
    assert_eq!(name, ".role-restore-prompt--dev-pts-12");
    assert!(!name.contains('/'));
}

#[test]
fn starter_scaffold_adds_product_and_preserves_existing_product_role() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let roles_dir = home.path().join(".aida/roles");
    std::fs::create_dir_all(&roles_dir).unwrap();

    let product_path = roles_dir.join("product.toml");
    let custom_product = r#"name = "product"
purpose = "My custom product role"
created_at = "2026-09-01T00:00:00Z"
last_active_at = "2026-09-01T00:00:00Z"
global = true
"#;
    std::fs::write(&product_path, custom_product).unwrap();

    // trace:BUG-1021 | ai:claude — AIDA_TEST_HOME: `$HOME` is not honored on Windows.
    let home_str = home.path().to_str().unwrap();
    let _env =
        crate::test_env::EnvVarsGuard::set(&[("HOME", home_str), ("AIDA_TEST_HOME", home_str)]);
    let (created, skipped) = scaffold_starter_roles(project.path()).unwrap();

    assert!(created.contains(&"implementer"));
    assert!(
        !created.contains(&"product"),
        "existing product role must not be overwritten"
    );
    assert!(skipped.contains(&"product"));
    assert_eq!(
        std::fs::read_to_string(&product_path).unwrap(),
        custom_product
    );

    let (product, _) = load_role(project.path(), "product").unwrap();
    assert_eq!(product.purpose.as_deref(), Some("My custom product role"));
}
