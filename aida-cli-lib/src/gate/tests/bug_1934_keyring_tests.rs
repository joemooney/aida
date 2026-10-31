//! BUG-1934 SKETCH v2: a seat grant bound to a session-keyring secret is
//! honoured for whoever possesses the key (across a sandbox's PID namespace)
//! and refused for everyone else, with a named reason. Keyring credentials
//! are per thread, so each test joins its own fresh anonymous session
//! keyring and never touches the test runner's. Tests that need the real
//! keyring return early when the kernel refuses it (a seccomp'd container).
// trace:BUG-1934 | ai:claude

use super::*;
use crate::gate::review_authority::{with_identity_source, IdentitySource, ProcEntry};
use crate::test_env::EnvVarsGuard;

const SUBJECT: &str = "bug-1934-keyring-fixture-user";
const FOREIGN: &str = "proc:999999@linux-starttime:1";

fn pinned_env() -> EnvVarsGuard {
    let mut env = EnvVarsGuard::snapshot(&["AIDA_USER", GRANT_ENV, "AIDA_SESSION_ID"]);
    env.set_key("AIDA_USER", SUBJECT);
    env.unset_key("AIDA_SESSION_ID");
    env
}

/// Give this test thread a fresh, empty session keyring. `false` when the
/// kernel refuses the keyring (the caller skips).
fn fresh_keyring() -> bool {
    let ok = crate::grant_keyring::available()
        && crate::grant_keyring::install("00000000-0000-4000-8000-000000000000", b"-", 60).is_ok();
    if !ok {
        eprintln!("SKIP: the session keyring is unavailable here");
    }
    ok
}

fn set_binding(id: &str, binding: Vec<String>) {
    update_grant(
        id,
        |g| {
            g.binding = binding.clone();
            true
        },
        |g| g.binding == binding,
    )
    .unwrap();
}

/// A presenter whose ancestry holds none of the grant's processes: an agent
/// the daemon started, `aida` under `claude` under the daemon.
fn daemon_lane() -> IdentitySource {
    let p = |pid: u32, name: &str| ProcEntry {
        pid,
        name: name.into(),
        cmd: vec![name.into()],
        start: Some(format!("linux-starttime:{pid}")),
    };
    IdentitySource {
        session_ids: vec!["daemon-session".into()],
        chain: vec![p(7000, "aida"), p(6900, "claude"), p(6800, "claude-daemon")],
        session_leader: Some(p(6800, "claude-daemon")),
    }
}

/// A grant bound to a foreign process and to `secret`'s key.
fn key_bound_grant(root: &Path, secret: &str) -> String {
    let id = test_support::mint_grant_for(root, SUBJECT, "advisor", &[]);
    set_binding(&id, vec![FOREIGN.into(), key_token(secret.as_bytes())]);
    id
}

#[test]
fn key_possession_satisfies_binding_without_proc_overlap() {
    if !fresh_keyring() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mut env = pinned_env();
    let secret = new_key_secret();
    let id = key_bound_grant(tmp.path(), &secret);
    crate::grant_keyring::install(&id, secret.as_bytes(), 600).unwrap();
    env.set_key(GRANT_ENV, &id);
    with_identity_source(daemon_lane(), || {
        let check = grant_check(tmp.path());
        assert!(matches!(check, GrantCheck::Valid(_)), "{check:?}");
        let status = serde_json::to_value(grant_status(tmp.path())).unwrap();
        assert_eq!(status["grant"]["via"], "key", "{status}");
        let grant = read_grant(&id).unwrap();
        assert_eq!(
            matched_binding(&grant, &presenter_tokens_for(&grant)),
            vec![key_token(secret.as_bytes())],
            "the attestation carries the key token to ship time"
        );
    });
}

/// v2 C4: a lane the agent daemon started does not descend from the role
/// shell, so it holds neither the key nor a bound process: the exported grant
/// (e.g. leaked through the daemon's env, STORY-1662) is refused and confers
/// no advisor seat and no commit authority.
#[test]
fn daemon_lane_without_the_key_is_refused_for_advisor_gates() {
    if !fresh_keyring() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mut env = pinned_env();
    let secret = new_key_secret();
    let id = key_bound_grant(tmp.path(), &secret);
    // The role shell holds the key; this thread is the daemon lane.
    let holder_id = id.clone();
    let holder_secret = secret.clone();
    std::thread::spawn(move || {
        crate::grant_keyring::install(&holder_id, holder_secret.as_bytes(), 600).unwrap();
        assert!(matches!(
            crate::grant_keyring::read(&holder_id),
            crate::grant_keyring::KeyRead::Held(_)
        ));
    })
    .join()
    .unwrap();
    env.set_key(GRANT_ENV, &id);
    with_identity_source(daemon_lane(), || {
        assert_eq!(
            crate::grant_keyring::read(&id),
            crate::grant_keyring::KeyRead::NotHeld
        );
        let check = grant_check(tmp.path());
        let GrantCheck::Rejected { reason, .. } = &check else {
            panic!("a daemon lane must not present the grant: {check:?}");
        };
        assert!(
            matches!(reason, GrantRejection::PresenterNotBound { .. }),
            "{reason:?}"
        );
        assert_eq!(current_seat(tmp.path()), None, "no advisor seat");
        assert!(
            !matches!(
                resolve_commit_authority(&tmp.path().join(".aida-store")),
                Some(aida_core::lifecycle::CommitAuthority::Grant { .. })
            ),
            "no advisor commit authority"
        );
        let sentence = check.rejection_sentence().unwrap();
        assert!(sentence.contains("agent daemon"), "{sentence}");
    });
}

/// The sandbox case: no key, in a foreign PID namespace, names the fix for a
/// foreground and for a background/relay session (v1 C1, v2 C6).
#[test]
fn foreign_namespace_without_key_names_the_fix() {
    if !fresh_keyring() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mut env = pinned_env();
    let id = key_bound_grant(tmp.path(), &new_key_secret());
    env.set_key(GRANT_ENV, &id);
    test_support::FOREIGN_NAMESPACE.with(|f| f.set(Some(true)));
    let check = with_identity_source(daemon_lane(), || grant_check(tmp.path()));
    test_support::FOREIGN_NAMESPACE.with(|f| f.set(Some(false)));
    let GrantCheck::Rejected { reason, .. } = &check else {
        panic!("{check:?}");
    };
    assert!(
        matches!(reason, GrantRejection::ForeignNamespace { .. }),
        "{reason:?}"
    );
    let sentence = check.rejection_sentence().unwrap();
    for needle in [
        "sandboxed process namespace",
        "eval \"$(aida role enter advisor)\"",
        "a foreground agent directly",
        "a background or relay agent",
        "--bind-pid",
    ] {
        assert!(sentence.contains(needle), "missing {needle:?}: {sentence}");
    }
}

/// v2 C5: a refusal is only ever re-labelled, never turned into a pass, and
/// an unavailable keyring is named.
#[test]
fn keyring_unavailable_never_fails_open() {
    let bound_to = "process 999999".to_string();
    let base = || GrantRejection::PresenterNotBound {
        bound_to: bound_to.clone(),
    };
    assert_eq!(
        classify_refusal(base(), true, false, false),
        GrantRejection::KeyringUnavailable {
            bound_to: bound_to.clone()
        }
    );
    assert_eq!(
        classify_refusal(base(), true, false, true),
        GrantRejection::KeyringUnavailable {
            bound_to: bound_to.clone()
        }
    );
    assert_eq!(
        classify_refusal(base(), false, false, true),
        GrantRejection::ForeignNamespace {
            bound_to: bound_to.clone()
        }
    );
    assert_eq!(classify_refusal(base(), true, true, false), base());
    assert_eq!(
        classify_refusal(GrantRejection::Unbound, true, false, true),
        GrantRejection::Unbound
    );
    let sentence = GrantCheck::Rejected {
        id: uuid::Uuid::new_v4().to_string(),
        seat: Some("reviewer".into()),
        reason: GrantRejection::KeyringUnavailable { bound_to },
    }
    .rejection_sentence()
    .unwrap();
    assert!(
        sentence.contains("session keyring is unavailable"),
        "{sentence}"
    );
    assert!(sentence.contains("aida role enter reviewer"), "{sentence}");
}

/// v2 C2: revoking a grant invalidates its key where reachable, so the key
/// no longer satisfies even a check that skipped the revocation.
#[test]
fn revoked_grant_key_no_longer_satisfies() {
    if !fresh_keyring() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let _env = pinned_env();
    let secret = new_key_secret();
    let id = key_bound_grant(tmp.path(), &secret);
    crate::grant_keyring::install(&id, secret.as_bytes(), 600).unwrap();
    let grant = read_grant(&id).unwrap();
    assert!(possession_token(&grant).is_some());
    assert!(revoke_by_id(&id).unwrap());
    assert_eq!(
        crate::grant_keyring::read(&id),
        crate::grant_keyring::KeyRead::NotHeld,
        "the revoked grant's key is invalidated"
    );
    assert_eq!(possession_token(&grant), None);
}

/// A child grant minted by a launcher carries a key the launcher (and so
/// every worker it spawns or execs, sandboxed or not) holds.
#[test]
fn child_grant_key_is_held_by_the_launcher() {
    if !fresh_keyring() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mut env = pinned_env();
    let parent = test_support::mint_grant_for(tmp.path(), SUBJECT, "advisor", &["reviewer"]);
    env.set_key(GRANT_ENV, &parent);
    let child = issue_child(tmp.path(), "reviewer", SUBJECT).unwrap();
    let token = possession_token(&child).expect("the launcher holds the child's key");
    assert!(child.binding.contains(&token), "{:?}", child.binding);
    assert!(
        child.binding.iter().any(|b| b.starts_with("proc:")),
        "the launcher's process binding stays"
    );
}

/// TASK-2195 (round 3): a long-lived launcher keeps ONE session keyring.
/// Every child launch adds its key to the keyring the first launch joined,
/// so launches never pin one keyring each against the kernel key quota.
#[test]
fn launcher_reuses_one_session_keyring_across_child_launches() {
    if !fresh_keyring() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mut env = pinned_env();
    let parent = test_support::mint_grant_for(tmp.path(), SUBJECT, "advisor", &["reviewer"]);
    env.set_key(GRANT_ENV, &parent);
    let ring = crate::grant_keyring::session_keyring_id().expect("a session keyring");
    let children: Vec<SeatGrant> = (0..3)
        .map(|_| issue_child(tmp.path(), "reviewer", SUBJECT).unwrap())
        .collect();
    assert_eq!(
        crate::grant_keyring::session_keyring_id(),
        Some(ring),
        "no keyring is joined per launch"
    );
    for child in &children {
        assert!(possession_token(child).is_some(), "{}", child.id);
    }
    // Revoking a child invalidates its key in the shared keyring only.
    assert!(revoke_by_id(&children[0].id).unwrap());
    assert_eq!(
        crate::grant_keyring::read(&children[0].id),
        crate::grant_keyring::KeyRead::NotHeld
    );
    assert!(possession_token(&children[1]).is_some());
}

/// TASK-2195 (round 3): an exhausted kernel key quota refuses the launch with
/// a named reason instead of silently minting a proc-only child grant.
#[test]
fn child_launch_refuses_on_key_quota_exhaustion() {
    let tmp = tempfile::tempdir().unwrap();
    let mut env = pinned_env();
    let parent = test_support::mint_grant_for(tmp.path(), SUBJECT, "advisor", &["reviewer"]);
    env.set_key(GRANT_ENV, &parent);
    crate::grant_keyring::FORCE_INSTALL_ERRNO.with(|e| e.set(Some(libc::EDQUOT)));
    let err = format!(
        "{:#}",
        issue_child(tmp.path(), "reviewer", SUBJECT).unwrap_err()
    );
    for needle in [
        "cannot launch the `reviewer` worker",
        "kernel key quota",
        &format!("errno {}", libc::EDQUOT),
        "kernel.keys.maxkeys",
    ] {
        assert!(err.contains(needle), "missing {needle:?}: {err}");
    }
    // A keyring that is merely unavailable keeps the documented proc-only
    // fallback (warned on stderr), which a sandboxed worker cannot present.
    crate::grant_keyring::FORCE_INSTALL_ERRNO.with(|e| e.set(Some(libc::ENOSYS)));
    let child = issue_child(tmp.path(), "reviewer", SUBJECT).unwrap();
    assert!(
        child.binding.iter().all(|b| b.starts_with("proc:")),
        "{:?}",
        child.binding
    );
}

/// TASK-2211: a launch whose `add_key` fails after the keyring JOIN (EDQUOT)
/// still records the joined keyring, so the next launch on the thread adds
/// to it instead of joining and chaining another one against the quota.
// trace:TASK-1955 | ai:claude (TASK-2211)
#[test]
fn a_failed_add_key_after_join_keeps_the_joined_keyring() {
    // No `fresh_keyring()`: this thread must start without a launcher
    // keyring so the first install joins one.
    if !crate::grant_keyring::available() {
        eprintln!("SKIP: the session keyring is unavailable here");
        return;
    }
    let before = crate::grant_keyring::session_keyring_id();
    crate::grant_keyring::FORCE_ADD_KEY_ERRNO.with(|e| e.set(Some(libc::EDQUOT)));
    let err = crate::grant_keyring::install("00000000-0000-4000-8000-000000002211", b"-", 60)
        .unwrap_err();
    assert!(err.quota_exhausted(), "{err}");
    let joined = crate::grant_keyring::session_keyring_id();
    assert_ne!(joined, before, "the failing install joined a keyring");
    crate::grant_keyring::install("00000000-0000-4000-8000-000000002212", b"-", 60).unwrap();
    assert_eq!(
        crate::grant_keyring::session_keyring_id(),
        joined,
        "the next install reuses the joined keyring instead of joining another"
    );
}

/// TASK-2212: a structurally unavailable keyring (off Linux, or refused by
/// the kernel) makes every child proc-only, so no per-launch warning prints;
/// an available keyring that failed still warns.
// trace:TASK-1955 | ai:claude (TASK-2212)
#[test]
fn proc_only_child_warning_is_silent_without_a_keyring() {
    let err = crate::grant_keyring::InstallError::new(
        "the session keyring is only available on Linux",
        0,
    );
    assert_eq!(proc_only_child_warning("reviewer", &err, false), None);
    let warning = proc_only_child_warning("reviewer", &err, true).expect("a warning");
    assert!(
        warning.contains("child `reviewer` grant is bound to this launcher's process only"),
        "{warning}"
    );
}

/// `aida role adopt-key` accepts only the secret whose digest the grant
/// carries. (The success path installs into the PARENT process's keyring, so
/// it is exercised by the binary-level bwrap test, never in-process.)
#[test]
fn adopt_key_refuses_a_secret_the_grant_does_not_carry() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = pinned_env();
    let id = key_bound_grant(tmp.path(), &new_key_secret());
    let err = adopt_key(&id, &new_key_secret()).unwrap_err().to_string();
    assert!(err.contains("does not belong"), "{err}");
    let err = adopt_key(&id, "").unwrap_err().to_string();
    assert!(err.contains("does not belong"), "{err}");
}

/// v2 C3: the eval text switches xtrace off before the secret, and the
/// secret appears only inside the quoted heredoc: never on a command line.
#[test]
fn role_enter_eval_keeps_the_secret_out_of_argv_and_xtrace() {
    let secret = new_key_secret();
    let lines = crate::role_cmd::adopt_key_eval_lines(
        "1234abcd-0000-4000-8000-000000000000",
        Some(&secret),
    );
    assert!(
        lines[0].ends_with("{ set +x; } 2>/dev/null"),
        "{}",
        lines[0]
    );
    assert!(
        lines[0].starts_with("case $- in *x*)"),
        "xtrace state saved"
    );
    assert_eq!(
        lines.last().unwrap(),
        "[ -n \"$_aida_xtrace\" ] && set -x; unset _aida_xtrace",
        "xtrace state restored after the heredoc"
    );
    let open = lines
        .iter()
        .position(|l| l.ends_with("<<'AIDA_GRANT_KEY'"))
        .expect("a quoted heredoc");
    assert!(lines[open].contains(" role adopt-key "), "{}", lines[open]);
    let close = lines
        .iter()
        .position(|l| l == "AIDA_GRANT_KEY")
        .expect("heredoc terminator");
    for (i, line) in lines.iter().enumerate() {
        if line.contains(&secret) {
            assert!(open < i && i < close, "secret outside the heredoc: {line}");
        }
    }
    assert_eq!(lines[open + 1], secret);
    assert!(crate::role_cmd::adopt_key_eval_lines("id", None).is_empty());
}

#[test]
fn foreign_namespace_signals() {
    assert!(!foreign_namespace_from(4200, "systemd"));
    assert!(!foreign_namespace_from(4200, "init"));
    assert!(
        foreign_namespace_from(0, "systemd"),
        "session leader outside"
    );
    assert!(foreign_namespace_from(1, "bwrap"), "bwrap's init");
}

#[test]
fn key_tokens_match_exactly() {
    use crate::gate::review_authority::binding_tokens_match as m;
    let a = key_token(b"one");
    assert!(m(&a, &a));
    assert!(!m(&a, &key_token(b"two")));
    assert!(!m("key:abc", "key:abc"), "a short digest never matches");
    assert!(!m(&a, "proc:1@x"));
}

/// v2 design 7: doctor warns about yama ptrace_scope 0 only while a grant is
/// held.
#[test]
fn doctor_warns_on_ptrace_scope_zero_with_a_grant() {
    let f = ptrace_scope_finding(true, Some("0\n")).expect("scope 0 warns");
    assert_eq!(f.category, DOCTOR_CATEGORY);
    assert!(f.summary.contains("ptrace_scope is 0"), "{}", f.summary);
    assert!(ptrace_scope_finding(false, Some("0")).is_none());
    assert!(ptrace_scope_finding(true, Some("1")).is_none());
    assert!(ptrace_scope_finding(true, None).is_none());
}

/// BUG-HOME-045: a contained (sandboxed) worker whose child grant got no
/// keyring key is refused before launch: its sandboxed `aida review record`
/// could never present the grant. An unsandboxed worker only warns, and a
/// structurally unavailable keyring never refuses.
// trace:BUG-b6b1278890774ef993d2ebc327862470-045 | ai:claude
#[test]
fn sandboxed_child_without_a_keyring_key_is_refused_before_launch() {
    let err = crate::grant_keyring::InstallError::new(
        "cannot hold the previous session keyring",
        libc::ELOOP,
    );
    let refusal =
        sandboxed_child_key_refusal("reviewer", &err, true, true).expect("a contained refusal");
    assert!(
        refusal.contains("cannot launch the contained `reviewer` worker"),
        "{refusal}"
    );
    assert!(
        refusal.contains("cannot hold the previous session keyring"),
        "{refusal}"
    );
    assert_eq!(
        sandboxed_child_key_refusal("reviewer", &err, true, false),
        None
    );
    assert_eq!(
        sandboxed_child_key_refusal("reviewer", &err, false, true),
        None
    );
}

/// BUG-HOME-045 round 2: the launch issuer refuses a CONTAINED child whose
/// key install failed (with a [`ChildGrantRefused`], which releases the review
/// lease) and keeps the warn-only proc-only fallback for an uncontained one;
/// the launch preflight refuses the same condition without minting a grant.
// trace:BUG-b6b1278890774ef993d2ebc327862470-045 | ai:claude
#[test]
fn bug_045_contained_launch_child_without_a_keyring_key_is_refused() {
    if !crate::grant_keyring::available() {
        eprintln!("SKIP: the session keyring is unavailable here");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mut env = pinned_env();
    let parent = test_support::mint_grant_for(tmp.path(), SUBJECT, "advisor", &["reviewer"]);
    env.set_key(GRANT_ENV, &parent);

    crate::grant_keyring::FORCE_INSTALL_ERRNO.with(|e| e.set(Some(libc::ELOOP)));
    let err = issue_launch_child(tmp.path(), "reviewer", SUBJECT, true).unwrap_err();
    assert!(ChildGrantRefused::is_in(&err), "{err:#}");
    assert!(
        format!("{err:#}").contains("cannot launch the contained `reviewer` worker"),
        "{err:#}"
    );

    crate::grant_keyring::FORCE_INSTALL_ERRNO.with(|e| e.set(Some(libc::ELOOP)));
    let child = issue_launch_child(tmp.path(), "reviewer", SUBJECT, false).unwrap();
    assert!(
        child.binding.iter().all(|b| b.starts_with("proc:")),
        "{:?}",
        child.binding
    );

    crate::grant_keyring::FORCE_INSTALL_ERRNO.with(|e| e.set(Some(libc::ELOOP)));
    let err = preflight_sandboxed_child_key("reviewer").unwrap_err();
    assert!(ChildGrantRefused::is_in(&err), "{err:#}");
    // Off the forced failure the probe passes and leaves no key behind.
    preflight_sandboxed_child_key("reviewer").expect("a holdable key passes the preflight");
}
