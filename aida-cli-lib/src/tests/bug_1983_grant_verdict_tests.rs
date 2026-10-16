//! BUG-1983: an exported grant that fails validation is reported as WHICH
//! check failed, not "No active role"; `review record` refuses it; and the
//! verdict accepts exactly the grants `current_grant` always accepted.
// trace:BUG-1983 | ai:claude

use crate::seat_authority::{
    current_grant, grant_check, test_support::mint_grant_for, GrantCheck, GrantRejection,
    SeatGrant, GRANT_ENV,
};
use chrono::{Duration, Utc};
use std::path::Path;

/// `current_grant` exactly as it was before BUG-1983, kept verbatim as the
/// reference the new verdict must agree with (C1).
fn legacy_current_grant(project_root: &Path) -> Option<SeatGrant> {
    use crate::seat_authority::roster_allows;
    let id = std::env::var(GRANT_ENV).ok()?;
    let grant = read(&id)?;
    let now = Utc::now();
    if grant.revoked_at.is_some()
        || now >= grant.expires_at
        || grant.subject != crate::current_user_id(None)
        || !roster_allows(project_root, &grant.principal, &grant.seat)
    {
        return None;
    }
    if let Some(parent_id) = &grant.parent_grant_id {
        let parent = read(parent_id)?;
        if parent.revoked_at.is_some()
            || now >= parent.expires_at
            || parent.principal != grant.principal
            || !roster_allows(project_root, &parent.principal, &parent.seat)
            || !parent
                .delegable_seats
                .iter()
                .any(|seat| seat == &grant.seat)
        {
            return None;
        }
    }
    Some(grant)
}

fn grant_file(id: &str) -> std::path::PathBuf {
    crate::aida_home_dir()
        .unwrap()
        .join(".aida")
        .join("session-grants")
        .join(format!("{id}.json"))
}

fn read(id: &str) -> Option<SeatGrant> {
    uuid::Uuid::parse_str(id).ok()?;
    let grant: SeatGrant = serde_json::from_slice(&std::fs::read(grant_file(id)).ok()?).ok()?;
    (grant.id == id).then_some(grant)
}

fn edit(id: &str, change: impl FnOnce(&mut SeatGrant)) {
    let mut grant = read(id).unwrap();
    change(&mut grant);
    std::fs::write(grant_file(id), serde_json::to_vec(&grant).unwrap()).unwrap();
}

fn set_roster(root: &Path, subject: &str, seats: &[&str]) {
    let store = crate::detect_distributed_store_from(root).unwrap();
    let seats = seats
        .iter()
        .map(|s| format!("\"{s}\""))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        store.join("registry").join("team.toml"),
        format!("[members]\n\"{subject}\" = [{seats}]\n"),
    )
    .unwrap();
}

/// Mint a child `implementer` grant for this process, delegated from an
/// `advisor` parent, and return `(parent, child)`.
fn delegated(root: &Path, me: &str) -> (String, String) {
    let parent = mint_grant_for(root, me, "advisor", &["implementer"]);
    let child = mint_grant_for(root, me, "implementer", &[]);
    edit(&child, |g| g.parent_grant_id = Some(parent.clone()));
    (parent, child)
}

/// Run one fixture: `setup` returns the handle to export. Asserts the new
/// verdict and the legacy check agree, and returns the verdict.
fn verdict(setup: impl FnOnce(&Path, &str) -> String) -> (String, GrantCheck) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Hold the env lock across identity resolution, the mint and the check,
    // so a sibling test that pins `AIDA_USER` cannot change who `me` is
    // between minting and validating (a spurious subject mismatch).
    // trace:BUG-1983 | ai:claude
    let mut env = crate::test_env::EnvVarsGuard::snapshot(&[GRANT_ENV]);
    let me = crate::current_user_id(None);
    let id = setup(root, &me);
    env.set_key(GRANT_ENV, &id);
    let check = grant_check(root);
    assert_eq!(
        legacy_current_grant(root).map(|g| g.id),
        current_grant(root).map(|g| g.id),
        "current_grant must accept exactly what it always accepted ({check:?})"
    );
    assert_eq!(
        matches!(check, GrantCheck::Valid(_)),
        legacy_current_grant(root).is_some(),
        "{check:?}"
    );
    (id, check)
}

fn rejection(setup: impl FnOnce(&Path, &str) -> String) -> (String, GrantRejection) {
    match verdict(setup) {
        (id, GrantCheck::Rejected { reason, .. }) => (id, reason),
        (_, other) => panic!("expected a rejection, got {other:?}"),
    }
}

fn past() -> chrono::DateTime<Utc> {
    Utc::now() - Duration::hours(1)
}

#[test]
fn a_valid_grant_and_a_valid_delegation_stay_valid() {
    let (_, check) = verdict(|root, me| mint_grant_for(root, me, "advisor", &[]));
    assert!(matches!(check, GrantCheck::Valid(ref g) if g.seat == "advisor"));
    let (_, check) = verdict(|root, me| delegated(root, me).1);
    assert!(matches!(check, GrantCheck::Valid(ref g) if g.seat == "implementer"));
}

#[test]
fn no_exported_grant_is_absent_not_rejected() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[(GRANT_ENV, None)]);
    let tmp = tempfile::tempdir().unwrap();
    assert!(matches!(grant_check(tmp.path()), GrantCheck::Absent));
    drop(_env);
    let _env = crate::test_env::EnvVarsGuard::apply(&[(GRANT_ENV, Some("  "))]);
    assert!(matches!(grant_check(tmp.path()), GrantCheck::Absent));
}

/// C3 regression pin: an env-only advisor label with no grant has no
/// advisor authority.
///
/// Hermetic: the guard also clears `AIDA_AUTO_COMPLETE`,
/// `AIDA_AUTO_COMPLETE_TOKEN` and `AIDA_ROLE_INSTANCE` and pins the ambient
/// root and a non-TTY stdin, so a live drain or an interactive `cargo test`
/// cannot flip the answer.
// trace:BUG-1983 | ai:claude
#[test]
fn env_only_advisor_label_has_no_authority() {
    let tmp = tempfile::tempdir().unwrap();
    let _ambient = crate::test_env::AmbientGuard::hermetic(tmp.path(), Some("advisor"));
    assert!(std::env::var_os(GRANT_ENV).is_none());
    assert!(!crate::role::has_advisor_authority());
}

/// Every rejection variant, each reached on its own, plus fixtures where two
/// checks fail at once so the reported reason pins the check order (C1).
#[test]
fn every_rejection_names_its_check_in_the_legacy_order() {
    let unknown = uuid::Uuid::new_v4().to_string();
    let cases: Vec<(&str, Box<dyn FnOnce(&Path, &str) -> String>, GrantRejection)> = vec![
        (
            "bad handle",
            Box::new(|_, _| "not-a-grant".into()),
            GrantRejection::BadHandle,
        ),
        (
            "unknown id",
            Box::new(move |_, _| unknown),
            GrantRejection::Unreadable,
        ),
        (
            "malformed",
            Box::new(|root, me| {
                let id = mint_grant_for(root, me, "advisor", &[]);
                std::fs::write(grant_file(&id), b"{").unwrap();
                id
            }),
            GrantRejection::Malformed,
        ),
        (
            "handle mismatch",
            Box::new(|root, me| {
                let id = mint_grant_for(root, me, "advisor", &[]);
                edit(&id, |g| g.id = uuid::Uuid::new_v4().to_string());
                id
            }),
            GrantRejection::Malformed,
        ),
        (
            "revoked and expired: revoked wins",
            Box::new(|root, me| {
                let id = mint_grant_for(root, me, "advisor", &[]);
                edit(&id, |g| {
                    g.revoked_at = Some(past());
                    g.expires_at = past();
                });
                id
            }),
            GrantRejection::Revoked { at: Utc::now() },
        ),
        (
            "expired and foreign: expired wins",
            Box::new(|root, me| {
                let id = mint_grant_for(root, me, "advisor", &[]);
                edit(&id, |g| {
                    g.expires_at = past();
                    g.subject = "someone-else".into();
                });
                id
            }),
            GrantRejection::Expired { at: Utc::now() },
        ),
        (
            "foreign and off-roster: subject wins",
            Box::new(|root, me| {
                let id = mint_grant_for(root, me, "advisor", &[]);
                edit(&id, |g| g.subject = "someone-else".into());
                set_roster(root, me, &[]);
                id
            }),
            GrantRejection::SubjectMismatch {
                grant_subject: "someone-else".into(),
                caller: String::new(),
            },
        ),
        (
            "off-roster",
            Box::new(|root, me| {
                let id = mint_grant_for(root, me, "advisor", &[]);
                set_roster(root, me, &["reviewer"]);
                id
            }),
            GrantRejection::RosterDenied {
                principal: String::new(),
                seat: "advisor".into(),
            },
        ),
        (
            "parent missing",
            Box::new(|root, me| {
                let (parent, child) = delegated(root, me);
                std::fs::remove_file(grant_file(&parent)).unwrap();
                child
            }),
            GrantRejection::Parent(Box::new(GrantRejection::Unreadable)),
        ),
        (
            "parent revoked and expired: revoked wins",
            Box::new(|root, me| {
                let (parent, child) = delegated(root, me);
                edit(&parent, |g| {
                    g.revoked_at = Some(past());
                    g.expires_at = past();
                });
                child
            }),
            GrantRejection::Parent(Box::new(GrantRejection::Revoked { at: Utc::now() })),
        ),
        (
            "parent expired and foreign: expired wins",
            Box::new(|root, me| {
                let (parent, child) = delegated(root, me);
                edit(&parent, |g| {
                    g.expires_at = past();
                    g.principal = "someone-else".into();
                });
                child
            }),
            GrantRejection::Parent(Box::new(GrantRejection::Expired { at: Utc::now() })),
        ),
        (
            "parent foreign and off-roster: principal wins",
            Box::new(|root, me| {
                let (parent, child) = delegated(root, me);
                edit(&parent, |g| g.principal = "someone-else".into());
                set_roster(root, me, &["implementer"]);
                child
            }),
            GrantRejection::ParentPrincipalMismatch,
        ),
        (
            "parent off-roster and not delegating: roster wins",
            Box::new(|root, me| {
                let (parent, child) = delegated(root, me);
                edit(&parent, |g| g.delegable_seats.clear());
                set_roster(root, me, &["implementer"]);
                child
            }),
            GrantRejection::Parent(Box::new(GrantRejection::RosterDenied {
                principal: String::new(),
                seat: "advisor".into(),
            })),
        ),
        (
            "parent not delegating",
            Box::new(|root, me| {
                let (parent, child) = delegated(root, me);
                edit(&parent, |g| g.delegable_seats.clear());
                child
            }),
            GrantRejection::ParentNotDelegating {
                parent_seat: "advisor".into(),
            },
        ),
    ];
    let mut seen = std::collections::BTreeSet::new();
    for (name, setup, expected) in cases {
        let (_, got) = rejection(setup);
        assert_eq!(
            same_check(&got),
            same_check(&expected),
            "{name}: got {got:?}"
        );
        seen.insert(same_check(&got));
    }
    // Every distinct check is reached once, so none can be dropped silently.
    assert_eq!(seen.len(), 13, "{seen:?}");
}

/// The variant with its check-identifying fields, ignoring timestamps and
/// identities that vary per fixture.
fn same_check(reason: &GrantRejection) -> String {
    match reason {
        GrantRejection::BadHandle => "BadHandle".into(),
        GrantRejection::Unreadable => "Unreadable".into(),
        GrantRejection::Malformed => "Malformed".into(),
        GrantRejection::Revoked { .. } => "Revoked".into(),
        GrantRejection::Expired { .. } => "Expired".into(),
        GrantRejection::SubjectMismatch { grant_subject, .. } => {
            format!("SubjectMismatch({grant_subject})")
        }
        GrantRejection::RosterDenied { seat, .. } => format!("RosterDenied({seat})"),
        GrantRejection::Parent(inner) => format!("Parent({})", same_check(inner)),
        GrantRejection::ParentPrincipalMismatch => "ParentPrincipalMismatch".into(),
        GrantRejection::ParentNotDelegating { parent_seat } => {
            format!("ParentNotDelegating({parent_seat})")
        }
    }
}

/// AC4 + C2: an expired grant says so, by an 8-character prefix only.
#[test]
fn an_expired_grant_reads_as_expired_not_roleless() {
    let (id, check) = verdict(|root, me| {
        let id = mint_grant_for(root, me, "advisor", &[]);
        edit(&id, |g| g.expires_at = past());
        id
    });
    let sentence = check.rejection_sentence().expect("rejected grant");
    assert!(sentence.contains(&id[..8]), "{sentence}");
    assert!(!sentence.contains(&id), "full handle leaked: {sentence}");
    assert!(
        sentence.contains("(advisor)") && sentence.contains("expired at"),
        "{sentence}"
    );
    assert!(sentence.contains("aida role enter advisor"), "{sentence}");
    assert!(!sentence.contains("No active role"), "{sentence}");
    assert!(GrantCheck::Absent.rejection_sentence().is_none());
}

/// D3: `review record` refuses a rejected grant with the same sentence.
#[test]
fn review_record_refuses_a_rejected_grant() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Same env-lock discipline as `verdict` (identity stable across mint
    // and check). trace:BUG-1983 | ai:claude
    let mut env = crate::test_env::EnvVarsGuard::snapshot(&[GRANT_ENV]);
    let me = crate::current_user_id(None);
    let id = mint_grant_for(root, &me, "reviewer", &[]);
    edit(&id, |g| g.revoked_at = Some(past()));
    env.set_key(GRANT_ENV, &id);
    let err = crate::handle_review_record_at(
        root.to_path_buf(),
        "BUG-1",
        "approved",
        Some("0123456789abcdef0123456789abcdef01234567"),
        Some("some-branch"),
        None,
        &[],
        &[],
        None,
    )
    .expect_err("a rejected grant must not record a verdict");
    let message = format!("{err:#}");
    assert!(message.starts_with("refused: review record."), "{message}");
    assert!(
        message.contains(&id[..8]) && !message.contains(&id),
        "{message}"
    );
    assert!(message.contains("revoked at"), "{message}");
}

// ---------------------------------------------------------------------------
// Review fixes on PR #2587: the real gates, driven end to end.
// trace:BUG-1983 | ai:claude
// ---------------------------------------------------------------------------

use crate::test_env::{AmbientGuard, HERMETIC_TEST_USER};

const SPEC: &str = "TASK-1";
const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

/// A throwaway project: the seat-grant scaffolding (`.aida/config.toml`,
/// `.aida-store/registry`), a git-initialised store, and one Draft spec.
/// Returns the tempdir and the store path.
// trace:BUG-1983 | ai:claude
fn draft_project() -> (tempfile::TempDir, std::path::PathBuf) {
    use aida_core::db::DatabaseBackend;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Scaffolding only; each case mints the grant it exports.
    mint_grant_for(root, HERMETIC_TEST_USER, "advisor", &[]);
    let store = crate::detect_distributed_store_from(root).unwrap();
    aida_core::git_ops::init(&store).unwrap();
    aida_core::git_ops::configure_user(&store, "fixture", "fixture@localhost").unwrap();
    let backend = aida_core::CachedGitBackend::open(
        &store,
        &aida_core::CachedGitBackend::default_cache_path(&store),
    )
    .unwrap();
    let mut req = aida_core::models::Requirement::new("fixture".into(), "desc".into());
    req.spec_id = Some(SPEC.into());
    req.status = aida_core::models::RequirementStatus::Draft;
    backend.add_requirement(req).unwrap();
    (tmp, store)
}

fn spec_status(store: &Path) -> aida_core::models::RequirementStatus {
    use aida_core::db::DatabaseBackend;
    aida_core::CachedGitBackend::open(
        store,
        &aida_core::CachedGitBackend::default_cache_path(store),
    )
    .unwrap()
    .get_requirement_by_spec_id(SPEC)
    .unwrap()
    .unwrap()
    .status
}

/// `aida edit TASK-1 --status approved`, parsed by the real CLI and run
/// through the real command handler: the Draft->Approved advisor gate.
// trace:BUG-1983 | ai:claude
fn edit_gate_refusal(store: &Path) -> String {
    use clap::Parser;
    let cli = crate::Cli::try_parse_from(["aida", "edit", SPEC, "--status", "approved"]).unwrap();
    let err = crate::git_backend_cmd::handle_git_backend_command(store, &cli.command)
        .expect_err("the Draft->Approved gate must refuse");
    assert_eq!(
        spec_status(store),
        aida_core::models::RequirementStatus::Draft,
        "a refused promotion must leave the spec Draft"
    );
    format!("{err:#}")
}

/// `aida queue add TASK-1 --for implementer`: the queue's dispatch gate,
/// which shares the edit gate's refusal tail (`roleless_recovery_sentence`).
// trace:BUG-1983 | ai:claude
fn queue_gate_refusal(store: &Path) -> String {
    let cmd = crate::QueueCommand::Add {
        id: SPEC.into(),
        top: false,
        bottom: false,
        user: None,
        note: None,
        r#for: Some("implementer".into()),
        scope: None,
        for_session: None,
        no_scope: true,
        global: false,
        force: false,
    };
    let storage = crate::Storage::new(store);
    let err = crate::queue_cmd::handle_queue_command(&cmd, &storage, store)
        .expect_err("queuing for execution without dispatch authority must refuse");
    format!("{err:#}")
}

/// `aida review record TASK-1 --verdict approved` through its production
/// handler, rooted at the same project.
// trace:BUG-1983 | ai:claude
fn review_record(root: &Path) -> anyhow::Result<()> {
    // BUG-1918: an approval must name its recorder (here, an independent
    // session id) or the anchor rule refuses it; a rejected grant still
    // refuses first. trace:BUG-1918 | ai:claude
    let reviewer = crate::review_authority::IdentitySource {
        session_ids: vec!["independent-reviewer".into()],
        ..Default::default()
    };
    crate::review_authority::with_identity_source(reviewer, || {
        crate::handle_review_record_at(
            root.to_path_buf(),
            SPEC,
            "approved",
            Some(SHA),
            Some("some-branch"),
            None,
            &[],
            &[],
            None,
        )
    })
}

/// The refusal names the grant by its 8-character prefix only (C2), the
/// failed check, and never the roleless "No active role" line (AC4).
// trace:BUG-1983 | ai:claude
fn assert_names_rejection(surface: &str, message: &str, id: &str, sentence: &str, phrase: &str) {
    assert!(
        message.contains(sentence),
        "{surface}: missing `{sentence}` in: {message}"
    );
    assert!(
        message.contains(&id[..8]),
        "{surface}: no prefix in: {message}"
    );
    assert!(
        !message.contains(id),
        "{surface}: full handle leaked: {message}"
    );
    assert!(
        message.contains(phrase),
        "{surface}: no `{phrase}` in: {message}"
    );
    assert!(!message.contains("No active role"), "{surface}: {message}");
}

/// Blocking review item 1 (AC4, the edit side of AC2): the REAL Draft->Approved
/// edit gate, given an expired advisor grant (the BUG-1983 incident), says
/// which grant and why, not "No active role". The queue dispatch gate shares
/// the same tail and is pinned too.
// trace:BUG-1983 | ai:claude
#[test]
fn the_edit_gate_names_an_expired_grant_not_no_active_role() {
    let (tmp, store) = draft_project();
    let root = tmp.path();
    let id = mint_grant_for(root, HERMETIC_TEST_USER, "advisor", &[]);
    edit(&id, |g| g.expires_at = past());
    let _ambient = AmbientGuard::hermetic_with_grant(root, Some("advisor"), &id);
    let sentence = grant_check(root).rejection_sentence().expect("rejected");

    let message = edit_gate_refusal(&store);
    assert!(message.contains("needs advisor authority"), "{message}");
    assert_names_rejection("edit", &message, &id, &sentence, "expired at");

    let message = queue_gate_refusal(&store);
    assert!(message.contains("needs dispatch authority"), "{message}");
    assert_names_rejection("queue add", &message, &id, &sentence, "expired at");
}

/// Blocking review item 2 (AC3 / sketch T2): one rejection table drives BOTH
/// the edit status-promotion gate and `review record` from the SAME grant
/// fixture, and both refuse with the same reason sentence.
///
/// Designed divergence (sketch D3, advisor ruling Q1): `review record`
/// refuses only a REJECTED grant. A VALID grant for another seat (reviewer)
/// is not forged; the edit gate refuses it with the team-role clause (it is
/// not an advisor), while `review record` records the reviewer's verdict.
/// That row asserts the designed outcomes rather than identical ones.
// trace:BUG-1983 | ai:claude
#[test]
fn one_rejection_table_drives_the_edit_gate_and_review_record() {
    type Setup = fn(&Path) -> String;
    // (case, AIDA_SESSION_ROLE label, grant fixture, reason phrase; `None`
    // marks the valid rows).
    let cases: Vec<(&str, &str, Setup, Option<&str>)> = vec![
        (
            "unknown grant id",
            "advisor",
            |_| uuid::Uuid::new_v4().to_string(),
            Some("no grant record exists"),
        ),
        (
            "expired",
            "advisor",
            |root| {
                let id = mint_grant_for(root, HERMETIC_TEST_USER, "advisor", &[]);
                edit(&id, |g| g.expires_at = past());
                id
            },
            Some("expired at"),
        ),
        (
            "revoked",
            "advisor",
            |root| {
                let id = mint_grant_for(root, HERMETIC_TEST_USER, "advisor", &[]);
                edit(&id, |g| g.revoked_at = Some(past()));
                id
            },
            Some("was revoked at"),
        ),
        (
            "subject mismatch",
            "advisor",
            |root| {
                let id = mint_grant_for(root, HERMETIC_TEST_USER, "advisor", &[]);
                edit(&id, |g| g.subject = "someone-else".into());
                id
            },
            Some("was issued to `someone-else`"),
        ),
        (
            "roster denied",
            "advisor",
            |root| {
                let id = mint_grant_for(root, HERMETIC_TEST_USER, "advisor", &[]);
                set_roster(root, HERMETIC_TEST_USER, &["reviewer"]);
                id
            },
            Some("team roster no longer allows"),
        ),
        (
            "parent no longer delegates",
            "implementer",
            |root| {
                let (parent, child) = delegated(root, HERMETIC_TEST_USER);
                edit(&parent, |g| g.delegable_seats.clear());
                child
            },
            Some("no longer delegates this seat"),
        ),
        // trace:TASK-1741 | ai:codex
        (
            "valid advisor grant",
            "advisor",
            |root| mint_grant_for(root, HERMETIC_TEST_USER, "advisor", &[]),
            None,
        ),
        (
            "valid grant for another seat",
            "reviewer",
            |root| mint_grant_for(root, HERMETIC_TEST_USER, "reviewer", &[]),
            None,
        ),
    ];
    for (case, label, setup, phrase) in cases {
        let (tmp, store) = draft_project();
        let root = tmp.path();
        // The valid-advisor guard mints the fixture and pins agent attribution
        // under the same env lock, with both TTY inputs false.
        // trace:TASK-1741 | ai:codex
        if phrase.is_none() && label == "advisor" {
            let _ambient = AmbientGuard::hermetic_with_seat_and(
                root,
                "advisor",
                &[],
                &[
                    ("AIDA_AGENT_NAME", Some("valid-advisor-fixture")),
                    ("AIDA_AGENT_TYPE", Some("codex")),
                ],
            );
            assert!(!crate::authority_stdin_is_terminal());
            assert!(!crate::authority_stdout_is_terminal());
            assert!(
                matches!(grant_check(root), GrantCheck::Valid(ref g) if g.seat == "advisor"),
                "{case}: healthy advisor must stay Valid"
            );
            use clap::Parser;
            let cli =
                crate::Cli::try_parse_from(["aida", "edit", SPEC, "--status", "approved"]).unwrap();
            crate::git_backend_cmd::handle_git_backend_command(&store, &cli.command)
                .unwrap_or_else(|e| panic!("{case}: Draft->Approved must succeed: {e:#}"));
            assert_eq!(
                spec_status(&store),
                aida_core::models::RequirementStatus::Approved,
                "{case}: the real edit must persist the approval"
            );
            review_record(root)
                .unwrap_or_else(|e| panic!("{case}: advisor must record its verdict: {e:#}"));
            let recorded = crate::review_verdict::read_recorded_verdict(root, SPEC)
                .expect("the real review handler must persist a verdict");
            assert_eq!(recorded.kind, crate::review_verdict::VerdictKind::Approved);
            assert_eq!(recorded.reviewed_sha.as_deref(), Some(SHA));
            assert_eq!(
                recorded.recorded_by.as_deref(),
                Some("valid-advisor-fixture (codex advisor seat)"),
                "{case}: persisted attribution must use the advisor seat"
            );
            continue;
        }
        let id = setup(root);
        let _ambient = AmbientGuard::hermetic_with_grant(root, Some(label), &id);
        let edit_message = edit_gate_refusal(&store);
        assert!(
            edit_message.contains("needs advisor authority"),
            "{case}: {edit_message}"
        );
        let review = review_record(root);
        match phrase {
            Some(phrase) => {
                let sentence = grant_check(root)
                    .rejection_sentence()
                    .unwrap_or_else(|| panic!("{case}: the fixture must be rejected"));
                assert_names_rejection(case, &edit_message, &id, &sentence, phrase);
                let review_message = format!(
                    "{:#}",
                    review.expect_err("review record must refuse a rejected grant")
                );
                assert!(
                    review_message.starts_with("refused: review record."),
                    "{case}: {review_message}"
                );
                assert_names_rejection(case, &review_message, &id, &sentence, phrase);
            }
            None => {
                assert!(matches!(grant_check(root), GrantCheck::Valid(_)), "{case}");
                assert!(
                    edit_message.contains("Your active session seat is `reviewer`"),
                    "{case}: {edit_message}"
                );
                assert!(
                    !edit_message.contains("was rejected"),
                    "{case}: {edit_message}"
                );
                assert!(
                    !edit_message.contains("No active role"),
                    "{case}: {edit_message}"
                );
                review.unwrap_or_else(|e| {
                    panic!("{case}: a valid reviewer seat records its verdict: {e:#}")
                });
            }
        }
    }
}
