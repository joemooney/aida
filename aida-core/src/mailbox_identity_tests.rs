//! Mailbox identity policy over explicit facts (STORY-1843 B2b; the cases
//! moved here from the CLI's TASK-818/TASK-2345 policy tests).
// trace:STORY-1843 | ai:claude
use super::*;
use crate::mailbox_logical::ResolvedAudience;

fn unbound(user: &str) -> MailIdentityFacts {
    MailIdentityFacts::fixture(None, None, user, false, &[], None, None)
}

#[test]
fn unions_stable_name_and_agent_type() {
    let facts = MailIdentityFacts::fixture(
        None,
        None,
        "codex-implementer-1",
        false,
        &[],
        None,
        Some("codex"),
    );
    assert_eq!(
        inbox_identities(&facts),
        vec!["codex-implementer-1", "codex"]
    );
}

#[test]
fn stable_name_only_when_no_agent_type() {
    assert_eq!(inbox_identities(&unbound("solo-user")), vec!["solo-user"]);
}

#[test]
fn bound_child_excludes_inherited_parent_aliases() {
    let facts = MailIdentityFacts::fixture(
        Some("codex-implementer-03"),
        Some("codex"),
        "claude-advisor-01",
        false,
        &["claude-advisor-01", "parent-launch"],
        None,
        Some("claude"),
    );
    assert_eq!(
        inbox_identities(&facts),
        vec!["codex", "codex-implementer-03"]
    );
}

#[test]
fn bound_session_keeps_known_human_and_dedups() {
    let facts = MailIdentityFacts::fixture(
        Some("Claude-Implementer-02"),
        Some("claude"),
        "joe",
        true,
        &["Claude-Implementer-02"],
        Some("implementer"),
        None,
    );
    assert_eq!(
        classified_identities(&facts),
        vec![
            ("joe".to_string(), IdentityClass::Human),
            ("Claude-Implementer-02".to_string(), IdentityClass::Private),
            ("implementer".to_string(), IdentityClass::Shared),
            ("claude".to_string(), IdentityClass::Shared),
        ]
    );
}

#[test]
fn same_session_and_role_alias() {
    let facts = MailIdentityFacts::fixture(
        Some("claude-advisor-01"),
        Some("claude"),
        "CLAUDE-ADVISOR-01",
        false,
        &[],
        Some("dialog"),
        None,
    );
    assert_eq!(
        inbox_identities(&facts),
        vec!["CLAUDE-ADVISOR-01", "advisor", "claude"]
    );
}

#[test]
fn observed_takes_handle_and_vendor_from_the_binding_only() {
    let env = IdentityEnv::fixture("joe", &["parent-launch"], Some("reviewer"));
    let catalog = PeopleCatalogView::fixture(&["Joe"]);
    let facts = MailIdentityFacts::observed(
        SessionBinding::Bound(SessionBindingFacts::fixture("seat-a", "codex")),
        &catalog,
        env.clone(),
    );
    assert_eq!(facts.own_handle(), Some("seat-a"));
    assert_eq!(
        inbox_identities(&facts),
        vec!["joe", "reviewer", "codex", "seat-a"]
    );
    // Without a binding the user is the private reader.
    let unbound = MailIdentityFacts::observed(SessionBinding::Unbound, &catalog, env.clone());
    assert_eq!(
        inbox_identities(&unbound),
        vec!["joe", "parent-launch", "reviewer"]
    );
    // A selected row that fails verification grants no private inbox: not
    // the inherited user's (unless a catalog person) nor a launch alias.
    let refused = SessionBinding::Unverified(BindingRefusal::NotThisProcess);
    let unverified = MailIdentityFacts::observed(refused.clone(), &catalog, env.clone());
    assert!(unverified.binding_unverified());
    assert_eq!(inbox_identities(&unverified), vec!["joe", "reviewer"]);
    let stranger = MailIdentityFacts::observed(refused, &PeopleCatalogView::default(), env);
    assert_eq!(inbox_identities(&stranger), vec!["reviewer"]);
}

#[test]
fn audience_from_facts_classes_each_key() {
    let facts = MailIdentityFacts::fixture(
        Some("seat-a"),
        Some("claude"),
        "joe",
        true,
        &[],
        Some("advisor"),
        None,
    );
    let audience = ResolvedAudience::from_facts(&facts);
    assert_eq!(audience.private_keys(), ["seat-a"]);
    assert_eq!(audience.human_keys(), ["joe"]);
    assert_eq!(audience.shared_keys(), ["advisor", "claude"]);
    // Narrowing only ever selects a key the facts already authorized.
    assert!(audience.narrowed_to("ADVISOR").is_some());
    assert!(audience.narrowed_to("someone-else").is_none());
}

#[test]
fn audience_drops_unbounded_keys() {
    let facts = MailIdentityFacts::fixture(None, None, "a/b", false, &["ok-name"], None, None);
    assert_eq!(
        ResolvedAudience::from_facts(&facts).private_keys(),
        ["ok-name"]
    );
}

fn row(dir: &Path, id: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join(format!("{id}.toml"));
    std::fs::write(&path, body).unwrap();
    path
}

fn row_body(id: &str, pid: u32, extra: &str) -> String {
    format!(
        "id = \"{id}\"\nagent_type = \"claude\"\npid = {pid}\nname = \"seat-a\"\n\
         started_at = \"2026-10-26T00:00:00Z\"\nlast_active_at = \"2026-10-26T00:00:00Z\"\n\
         worktree_path = \"/x\"\nsource = \"session-start-hook\"\n{extra}"
    )
}

#[test]
fn binding_verifies_only_rows_this_process_owns() {
    let dir = tempfile::tempdir().unwrap();
    let env = IdentityEnv::fixture("u", &[], None);
    // This process's own pid: bound by ancestry (depth 0).
    let own = row(dir.path(), "own", &row_body("own", std::process::id(), ""));
    assert_eq!(
        SessionBindingFacts::verify(&own, &env),
        Ok(SessionBindingFacts::fixture("seat-a", "claude"))
    );
    // A pid that is not this process or an ancestor binds nothing.
    let other = row(dir.path(), "other", &row_body("other", u32::MAX - 7, ""));
    assert_eq!(
        SessionBindingFacts::verify(&other, &env),
        Err(BindingRefusal::NotThisProcess)
    );
    // Its session id binds once this process carries it in its payload.
    let mut with_payload = env.clone();
    with_payload.payload_session = Some("sid-1".into());
    let by_sid = row(
        dir.path(),
        "sid",
        &row_body("sid", u32::MAX - 7, "native_session_id = \"sid-1\"\n"),
    );
    assert!(SessionBindingFacts::verify(&by_sid, &with_payload).is_ok());
    assert_eq!(
        SessionBindingFacts::verify(&by_sid, &env),
        Err(BindingRefusal::NotThisProcess)
    );
    // Another vendor's session variable never binds a Claude row.
    let mut codex_env = env.clone();
    codex_env.client_sessions = vec![("codex", "sid-1".into())];
    assert_eq!(
        SessionBindingFacts::verify(&by_sid, &codex_env),
        Err(BindingRefusal::NotThisProcess)
    );
    // Ended, unnamed, mismatched-id and recycled-pid rows are refused.
    let ended = row(
        dir.path(),
        "ended",
        &row_body(
            "ended",
            std::process::id(),
            "ended_at = \"2026-10-26T01:00:00Z\"\n",
        ),
    );
    assert_eq!(
        SessionBindingFacts::verify(&ended, &env),
        Err(BindingRefusal::Ended)
    );
    let renamed = row(
        dir.path(),
        "renamed",
        &row_body("not-renamed", std::process::id(), ""),
    );
    assert_eq!(
        SessionBindingFacts::verify(&renamed, &env),
        Err(BindingRefusal::Corrupt)
    );
    let recycled = row(
        dir.path(),
        "recycled",
        &row_body(
            "recycled",
            std::process::id(),
            "pid_start = \"linux-starttime:1\"\n",
        ),
    );
    assert_eq!(
        SessionBindingFacts::verify(&recycled, &env),
        Err(BindingRefusal::NotThisProcess)
    );
    let unnamed = row(
        dir.path(),
        "unnamed",
        &row_body("unnamed", std::process::id(), "").replace("name = \"seat-a\"\n", ""),
    );
    assert_eq!(
        SessionBindingFacts::verify(&unnamed, &env),
        Err(BindingRefusal::NoHandle)
    );
}

#[test]
fn binding_refuses_a_symlinked_row() {
    let dir = tempfile::tempdir().unwrap();
    let env = IdentityEnv::fixture("u", &[], None);
    let real = row(
        dir.path(),
        "real",
        &row_body("link", std::process::id(), ""),
    );
    let link = dir.path().join("link.toml");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(
        SessionBindingFacts::verify(&link, &env),
        Err(BindingRefusal::Unreadable)
    );
}
