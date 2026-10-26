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

/// A registry row. A row for this process records its live start identity
/// (what registration writes) unless `extra` records one itself.
fn row_body(id: &str, pid: u32, extra: &str) -> String {
    let start = (pid == std::process::id() && !extra.contains("pid_start"))
        .then(|| crate::liveness::process_start_identity(pid))
        .flatten()
        .map(|start| format!("pid_start = \"{start}\"\n"))
        .unwrap_or_default();
    format!(
        "id = \"{id}\"\nagent_type = \"claude\"\npid = {pid}\nname = \"seat-a\"\n\
         started_at = \"2026-10-26T00:00:00Z\"\nlast_active_at = \"2026-10-26T00:00:00Z\"\n\
         worktree_path = \"/x\"\nsource = \"session-start-hook\"\n{start}{extra}"
    )
}

/// [`row_body`] with no recorded start identity (a legacy or crashed row).
fn row_body_without_start(id: &str, pid: u32) -> String {
    format!(
        "id = \"{id}\"\nagent_type = \"claude\"\npid = {pid}\nname = \"seat-a\"\n\
         started_at = \"2026-10-26T00:00:00Z\"\nlast_active_at = \"2026-10-26T00:00:00Z\"\n\
         worktree_path = \"/x\"\nsource = \"session-start-hook\"\n"
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

#[test]
fn ancestry_binding_compares_the_recorded_identity_by_scheme() {
    // A legacy RFC3339 row whose pid was recycled: the live legacy start
    // differs, so the pid alone never binds.
    let legacy = "2026-10-26T00:00:00+00:00";
    let strict = |live_legacy: Option<&'static str>, live_exact: Option<&'static str>| {
        move |pid: u32, recorded: &str| {
            crate::liveness::process_identity_matches_strict_with(
                pid,
                recorded,
                |_| true,
                |_| live_legacy.map(str::to_string),
                |_| live_exact.map(str::to_string),
            )
        }
    };
    let chain = || [10u32, 7, 1].into_iter();
    assert!(!row_binds_chain(
        7,
        Some(legacy),
        chain(),
        strict(Some("2026-10-26T05:00:00+00:00"), None)
    ));
    // The same legacy row binds when the live process really is the one recorded.
    assert!(row_binds_chain(
        7,
        Some(legacy),
        chain(),
        strict(Some(legacy), None)
    ));
    // A record in another or an unknown scheme cannot be compared: refused.
    assert!(!row_binds_chain(
        7,
        Some("sysinfo-start:2026-10-26T00:00:00+00:00"),
        chain(),
        strict(None, Some("linux-starttime:42"))
    ));
    assert!(!row_binds_chain(
        7,
        Some("mystery:42"),
        chain(),
        strict(Some(legacy), Some("linux-starttime:42"))
    ));
    // Exact match binds; a pid outside the chain never does.
    assert!(row_binds_chain(
        7,
        Some("linux-starttime:42"),
        chain(),
        strict(None, Some("linux-starttime:42"))
    ));
    assert!(!row_binds_chain(
        8,
        Some("linux-starttime:42"),
        chain(),
        strict(None, Some("linux-starttime:42"))
    ));
}

#[test]
fn binding_accepts_this_process_under_its_own_recorded_identity() {
    let dir = tempfile::tempdir().unwrap();
    let env = IdentityEnv::fixture("u", &[], None);
    let me = std::process::id();
    let Some(start) = crate::liveness::process_start_identity(me) else {
        return;
    };
    let own = row(
        dir.path(),
        "exact",
        &row_body("exact", me, &format!("pid_start = \"{start}\"\n")),
    );
    assert!(SessionBindingFacts::verify(&own, &env).is_ok());
    let legacy = row(
        dir.path(),
        "legacy",
        &row_body("legacy", me, "pid_start = \"2001-01-01T00:00:00+00:00\"\n"),
    );
    assert_eq!(
        SessionBindingFacts::verify(&legacy, &env),
        Err(BindingRefusal::NotThisProcess)
    );
}

#[test]
fn lookup_binds_the_nearest_registered_ancestor_and_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let env = IdentityEnv::fixture("parent-user", &["parent-handle"], None);
    let me = std::process::id();
    // No registry: unbound.
    assert_eq!(
        SessionBinding::lookup_in_chain(&dir.path().join("none"), &env, &[me]),
        SessionBinding::Unbound
    );
    // A row for this process wins over one for a more distant ancestor.
    row(
        dir.path(),
        "near",
        &row_body("near", me, "").replace("seat-a", "near-seat"),
    );
    row(dir.path(), "far", &row_body("far", u32::MAX - 3, ""));
    assert_eq!(
        SessionBinding::lookup_in_chain(dir.path(), &env, &[me, u32::MAX - 3]),
        SessionBinding::Bound(SessionBindingFacts::fixture("near-seat", "claude"))
    );
    // Ended rows are not candidates; a candidate that fails verification
    // (the far pid is not really an ancestor) binds nothing.
    let only_far = tempfile::tempdir().unwrap();
    row(only_far.path(), "far", &row_body("far", u32::MAX - 3, ""));
    row(
        only_far.path(),
        "gone",
        &row_body("gone", me, "ended_at = \"2026-10-26T01:00:00Z\"\n"),
    );
    assert_eq!(
        SessionBinding::lookup_in_chain(only_far.path(), &env, &[me, u32::MAX - 3]),
        SessionBinding::Unverified(BindingRefusal::NotThisProcess)
    );
}

#[test]
fn a_row_with_no_recorded_start_never_binds_by_pid_alone() {
    // trace:STORY-1843 | ai:claude — a legacy or crashed row whose pid an
    // ancestor shell now reuses: the pid matches, nothing proves the process.
    let chain = || [10u32, 7, 1].into_iter();
    assert!(!row_binds_chain(7, None, chain(), |_, _| true));
    let dir = tempfile::tempdir().unwrap();
    let me = std::process::id();
    let recycled = row(
        dir.path(),
        "recycled",
        &row_body_without_start("recycled", me),
    );
    let env = IdentityEnv::fixture("u", &[], None);
    assert_eq!(
        SessionBindingFacts::verify(&recycled, &env),
        Err(BindingRefusal::NotThisProcess)
    );
    assert_eq!(
        SessionBinding::lookup_in_chain(dir.path(), &env, &[me]),
        SessionBinding::Unverified(BindingRefusal::NotThisProcess)
    );
    // The same row still binds by the session evidence it names.
    let claude = IdentityEnv {
        claude_pid: Some(me),
        ..IdentityEnv::fixture("u", &[], None)
    };
    assert!(SessionBindingFacts::verify(&recycled, &claude).is_ok());
}
