//! BUG-2419: process identity tokens taken inside a pid namespace (every
//! bubblewrap-sandboxed command gets one) are never compared across
//! namespaces, so an independent sandboxed reviewer is not mistaken for the
//! author of an unrelated, earlier sandboxed claim.
// trace:BUG-2419 | ai:claude

use super::*;

const NS_A: &str = "4026532901";
const NS_B: &str = "4026532902";

fn proc(pid: u32, name: &str, start: &str) -> ProcEntry {
    ProcEntry {
        pid,
        name: name.into(),
        cmd: vec![name.into()],
        start: Some(start.into()),
    }
}

/// A sandboxed command's ancestry: `aida` under the sandbox's pid-2 shell,
/// whose parent is the namespace's pid 1 (where the walk stops).
fn sandboxed(start: &str) -> IdentitySource {
    IdentitySource {
        session_ids: vec![],
        chain: vec![proc(7, "aida", start), proc(2, "bash", start)],
        session_leader: None,
    }
}

/// The PR-3277 data, verbatim: the TASK-1784 claim record (a sandboxed claim
/// by an older binary, legacy rfc3339 starts) and the independent reviewer's
/// recorded identity (another sandbox, exact starts). Pid 2 with mixed start
/// schemes made them "the same process", refusing the approval as a
/// self-approval.
#[test]
fn pr_3277_reviewer_is_not_the_author_of_an_unrelated_sandboxed_claim() {
    let record = AuthorshipRecord {
        scope: "TASK-1784".into(),
        lease_id: "01a143ac561e".into(),
        branch: String::new(),
        tokens: [
            "lease:01a143ac561e",
            "shell:2@2026-10-16T07:45:29+00:00",
            "session:01a143ac561e",
            "session:01a143ac0514",
            "session:6fac567e-792c-418e-911d-5421a958e010",
            "proc:3@2026-10-16T07:45:29+00:00",
        ]
        .map(String::from)
        .to_vec(),
        recorded_at: "2026-10-16T07:45:30.148479719+00:00".into(),
        launch: None,
    };
    let reviewer: Vec<String> = [
        "session:01a19efaf0c9",
        "session:01a19efa-06c6-79b1-a26d-c353063e4180",
        "shell:2@linux-starttime:26079813",
        "proc:2@linux-starttime:26079813",
        "lease:01a19efaf0c9",
    ]
    .map(String::from)
    .to_vec();
    // The fail-closed author set (no subject spec) holds every ledger record.
    let authors = author_sessions(&[], &[record], &["TASK-1784".into()], None, |_| None);
    assert_eq!(authors.len(), 1);
    assert!(
        !authors[0].matches(&reviewer),
        "an unrelated sandbox's pid 2 is not the reviewer"
    );
}

#[test]
fn unqualified_pid_one_and_two_never_match() {
    for pid in [0u32, 1, 2] {
        for (a, b) in [
            (format!("proc:{pid}@x"), format!("proc:{pid}@x")),
            (
                format!("shell:{pid}@"),
                format!("proc:{pid}@linux-starttime:5"),
            ),
            (
                format!("proc:{pid}@2026-10-16T07:45:29+00:00"),
                format!("proc:{pid}@linux-starttime:5"),
            ),
        ] {
            assert!(!tokens_match(&a, &b), "{a} vs {b}");
        }
    }
    // Pid 3+ keeps the pre-BUG-2419 rules, the TASK-2049 mixed-scheme
    // fail-closed match included.
    assert!(tokens_match("proc:4242@t0", "shell:4242@t0"));
    assert!(tokens_match(
        "proc:4242@2026-10-16T07:45:29+00:00",
        "proc:4242@linux-starttime:5"
    ));
}

#[test]
fn qualify_with_pid_ns_marks_only_process_tokens() {
    assert_eq!(
        qualify_with_pid_ns("proc:2@linux-starttime:9".into(), Some(NS_A)),
        format!("proc:2@pidns-{NS_A}/linux-starttime:9")
    );
    assert_eq!(
        qualify_with_pid_ns("shell:2@".into(), Some(NS_A)),
        format!("shell:2@pidns-{NS_A}/")
    );
    assert_eq!(
        qualify_with_pid_ns("sid:5@s".into(), Some(NS_A)),
        format!("sid:5@pidns-{NS_A}/s")
    );
    for plain in ["session:abc", "lease:01a1", "key:00"] {
        assert_eq!(qualify_with_pid_ns(plain.into(), Some(NS_A)), plain);
    }
    assert_eq!(qualify_with_pid_ns("proc:2@s".into(), None), "proc:2@s");
    // Idempotent: an already-qualified token is not qualified twice.
    let once = qualify_with_pid_ns("proc:2@s".into(), Some(NS_A));
    assert_eq!(qualify_with_pid_ns(once.clone(), Some(NS_B)), once);
}

#[test]
fn namespaced_tokens_match_only_within_their_namespace() {
    let q = |ns: &str, tok: &str| qualify_with_pid_ns(tok.into(), Some(ns));
    let a = q(NS_A, "proc:2@linux-starttime:9");
    assert!(tokens_match(&a, &q(NS_A, "shell:2@linux-starttime:9")));
    assert!(!tokens_match(&a, &q(NS_A, "proc:2@linux-starttime:8")));
    assert!(!tokens_match(&a, &q(NS_B, "proc:2@linux-starttime:9")));
    // Against an unqualified token (the host, or an older record): never.
    for other in [
        "proc:2@linux-starttime:9",
        "proc:2@2026-10-16T07:45:29+00:00",
        "proc:2@",
    ] {
        assert!(!tokens_match(&a, other), "{other}");
        assert!(!tokens_match(other, &a), "{other}");
    }
    let b = q(NS_A, "proc:3@2026-10-16T07:45:29+00:00");
    assert!(!tokens_match(&b, &q(NS_A, "proc:3@linux-starttime:5")));
}

/// A claim and a later approval from two different sandboxes share no
/// process identity; from the same sandbox (same namespace and process) the
/// self-approval is still caught.
#[test]
fn sandboxed_claimer_and_recorder_compare_by_namespace() {
    let claimer = with_pid_namespace(Some(NS_A), || {
        sandboxed("linux-starttime:9").claimer_tokens()
    });
    assert!(claimer
        .iter()
        .any(|t| t.contains(&format!("pidns-{NS_A}/"))));
    let other_sandbox = with_pid_namespace(Some(NS_B), || {
        sandboxed("linux-starttime:9").recorder_tokens()
    });
    assert!(!identities_overlap(&claimer, &other_sandbox));
    let same_sandbox = with_pid_namespace(Some(NS_A), || {
        sandboxed("linux-starttime:9").recorder_tokens()
    });
    assert!(identities_overlap(&claimer, &same_sandbox));
}

#[test]
fn host_tokens_stay_unqualified_and_launched_workers_are_qualified() {
    let host = sandboxed("linux-starttime:9");
    assert!(host.recorder_tokens().iter().all(|t| !t.contains("pidns-")));
    assert!(host
        .presenter_tokens()
        .iter()
        .all(|t| !t.contains("pidns-")));
    let worker = with_pid_namespace(Some(NS_A), || host.launched_worker_tokens());
    assert_eq!(
        worker,
        vec![format!("proc:7@pidns-{NS_A}/linux-starttime:9")]
    );
}

/// Rework item 3: a namespace inode is reused once its namespace is torn
/// down, so a qualified token with an empty start (pid + inode alone) never
/// matches — on either side, or both.
#[test]
fn qualified_tokens_with_an_empty_start_never_match() {
    let q = |tok: &str| qualify_with_pid_ns(tok.into(), Some(NS_A));
    let full = q("proc:2@linux-starttime:9");
    let empty = q("proc:2@");
    assert_eq!(empty, format!("proc:2@pidns-{NS_A}/"));
    assert!(!tokens_match(&full, &empty));
    assert!(!tokens_match(&empty, &full));
    assert!(!tokens_match(&empty, &empty));
    assert!(!tokens_match(&empty, &q("shell:2@")));
    assert!(tokens_match(&full, &full));
}

/// A live lease whose creator shell is pid 2 and whose agent is pid 7, both
/// started at `linux-starttime:9`, recorded in pid namespace `origin`.
fn lease_from(origin: Option<&str>) -> SessionLease {
    let mut l = super::bug_1918_review_authority_tests::lease(
        "01a2b3c4d5e6",
        "BUG-24190",
        PathBuf::from("/w/bug-24190"),
    );
    l.creator_pid = Some(2);
    l.creator_pid_start_time = Some("linux-starttime:9".into());
    l.creator_pid_ns = origin.map(str::to_string);
    l.active_pid = Some(7);
    l.active_pid_start_time = Some("linux-starttime:9".into());
    l.active_pid_ns = origin.map(str::to_string);
    l
}

/// Do the live-lease authors of `lease`, read in namespace `reader`, match a
/// recorder in namespace `recorder_ns` with the same pids and starts?
fn lease_author_matches(
    lease: &SessionLease,
    reader: Option<&str>,
    recorder_ns: Option<&str>,
) -> bool {
    let spec = vec!["BUG-24190".to_string()];
    let authors = with_pid_namespace(reader, || {
        author_sessions(std::slice::from_ref(lease), &[], &spec, None, |_| None)
    });
    let recorder = with_pid_namespace(recorder_ns, || {
        sandboxed("linux-starttime:9").recorder_tokens()
    });
    // Only by process identity: no shared session or lease token.
    assert!(recorder
        .iter()
        .all(|t| t.starts_with("proc:") || t.starts_with("shell:")));
    assert_eq!(authors.len(), 1);
    authors[0].matches(&recorder)
}

/// A live lease's creator and agent pids are qualified with the namespace the
/// lease recorded them in, so a sandboxed author approving its own work from
/// the same sandbox is still refused through the live-lease path of
/// `author_sessions` — whoever reads the lease.
#[test]
fn same_sandbox_self_approval_is_refused_through_the_live_lease_path() {
    let l = lease_from(Some(NS_A));
    let authors = with_pid_namespace(Some(NS_A), || {
        author_sessions(
            std::slice::from_ref(&l),
            &[],
            &["BUG-24190".to_string()],
            None,
            |_| None,
        )
    });
    assert!(authors[0]
        .tokens
        .iter()
        .filter(|t| t.starts_with("proc:") || t.starts_with("shell:"))
        .all(|t| t.contains(&format!("@pidns-{NS_A}/"))));
    for reader in [Some(NS_A), Some(NS_B), None] {
        assert!(
            lease_author_matches(&l, reader, Some(NS_A)),
            "a same-sandbox self-approval must be refused (reader {reader:?})"
        );
    }
}

/// Rework r3 P2: the lease is qualified with its ORIGIN namespace, not the
/// reader's — a lease from namespace A with pid 7 / start 9 does not overlap
/// an independent namespace-B recorder with the same pid and start, even when
/// B is the namespace reading the lease.
#[test]
fn a_lease_from_another_namespace_never_matches_by_equal_pid_and_start() {
    let l = lease_from(Some(NS_A));
    for reader in [Some(NS_B), Some(NS_A), None] {
        assert!(
            !lease_author_matches(&l, reader, Some(NS_B)),
            "namespace-A lease vs namespace-B recorder (reader {reader:?})"
        );
        assert!(
            !lease_author_matches(&l, reader, None),
            "namespace-A lease vs host recorder (reader {reader:?})"
        );
    }
}

/// A host lease keeps unqualified tokens: it matches a host recorder and
/// never a sandboxed one, wherever it is read.
#[test]
fn host_lease_matches_only_a_host_recorder() {
    // Host pids 0-2 never name an author, so the host shell is pid 4242.
    let mut l = lease_from(Some(INITIAL_PID_NAMESPACE_INODE));
    l.creator_pid = Some(4242);
    let recorder_in = |ns: Option<&str>| {
        with_pid_namespace(ns, || {
            IdentitySource {
                session_ids: vec![],
                chain: vec![
                    proc(7, "aida", "linux-starttime:9"),
                    proc(4242, "bash", "linux-starttime:9"),
                ],
                session_leader: None,
            }
            .recorder_tokens()
        })
    };
    let spec = vec!["BUG-24190".to_string()];
    for reader in [None, Some(NS_A)] {
        let authors = with_pid_namespace(reader, || {
            author_sessions(std::slice::from_ref(&l), &[], &spec, None, |_| None)
        });
        assert!(authors[0].matches(&recorder_in(None)), "{reader:?}");
        assert!(!authors[0].matches(&recorder_in(Some(NS_A))), "{reader:?}");
    }
    // The same lease with no recorded origin matches neither.
    l.creator_pid_ns = None;
    l.active_pid_ns = None;
    let authors = author_sessions(std::slice::from_ref(&l), &[], &spec, None, |_| None);
    assert!(!authors[0].matches(&recorder_in(None)));
    assert!(!authors[0].matches(&recorder_in(Some(NS_A))));
}

/// A lease with no recorded origin (written before BUG-2419) is an unknown
/// namespace: it is never relabelled as the reader's and its pids never
/// match — only its lease id remains as an identity token.
#[test]
fn a_lease_with_no_recorded_origin_never_matches_by_pid() {
    let l = lease_from(None);
    assert_eq!(
        lease_tokens(&l, None),
        vec!["lease:01a2b3c4d5e6".to_string()]
    );
    for ns in [None, Some(NS_A)] {
        assert!(!lease_author_matches(&l, ns, ns), "{ns:?}");
    }
}

/// Lease writers record their own namespace beside each pid they write.
#[test]
fn lease_writers_record_the_origin_namespace() {
    assert_eq!(recorded_pid_ns(None), None);
    assert_eq!(
        recorded_pid_ns(Some(7)).as_deref(),
        Some(INITIAL_PID_NAMESPACE_INODE)
    );
    let in_a = with_pid_namespace(Some(NS_A), || recorded_pid_ns(Some(7)));
    assert_eq!(in_a.as_deref(), Some(NS_A));
    assert_eq!(
        lease_process_token("proc", 7, Some("linux-starttime:9"), Some(NS_A)).as_deref(),
        Some(format!("proc:7@pidns-{NS_A}/linux-starttime:9").as_str())
    );
    assert_eq!(
        lease_process_token("proc", 7, Some("s"), Some(INITIAL_PID_NAMESPACE_INODE)).as_deref(),
        Some("proc:7@s")
    );
    assert_eq!(lease_process_token("proc", 7, Some("s"), Some(" ")), None);
}

/// Rework r3 P1: the exact reproduction — a namespaced binding token whose
/// underlying start is empty is a pid plus a reusable inode, never a binding,
/// while the unqualified `proc:7@` was already refused.
#[test]
fn namespaced_binding_with_an_empty_start_never_binds() {
    let empty = format!("proc:7@pidns-{NS_B}/");
    assert!(!binding_tokens_match(&empty, &empty));
    assert!(!binding_tokens_match("proc:7@", "proc:7@"));
    for bad in [
        format!("proc:7@pidns-{NS_B}/  "),
        format!("sid:7@pidns-{NS_B}/"),
        format!("proc:7@pidns-{NS_B}"),
        "proc:7@pidns-/linux-starttime:9".to_string(),
    ] {
        assert!(!binding_tokens_match(&bad, &bad), "{bad}");
    }
    let full = format!("proc:7@pidns-{NS_B}/linux-starttime:9");
    assert!(binding_tokens_match(&full, &full));
    assert!(!binding_tokens_match(
        &full,
        &format!("proc:7@pidns-{NS_A}/linux-starttime:9")
    ));
    assert!(!binding_overlaps(&[empty.clone()], &[empty]));
}

/// Rework r3 P1: a sandboxed process without a start identity presents no
/// qualified token at all, so it can neither bind nor be bound by one.
#[test]
fn missing_start_presenter_emits_no_qualified_token() {
    let src = IdentitySource {
        session_ids: vec![],
        chain: vec![
            ProcEntry {
                pid: 7,
                name: "aida".into(),
                cmd: vec!["aida".into()],
                start: None,
            },
            proc(5, "bash", "linux-starttime:9"),
        ],
        session_leader: Some(ProcEntry {
            pid: 4,
            start: Some(" ".into()),
            ..ProcEntry::default()
        }),
    };
    let in_b = with_pid_namespace(Some(NS_B), || src.presenter_tokens());
    assert_eq!(in_b, vec![format!("proc:5@pidns-{NS_B}/linux-starttime:9")]);
    assert!(in_b.iter().all(|t| binding_tokens_match(t, t)));
    // On the host the start-less tokens stay unqualified (and unbindable).
    let host = src.presenter_tokens();
    assert!(host.contains(&"proc:7@".to_string()));
    assert!(host.iter().all(|t| !t.contains("pidns-")));
    assert!(!binding_tokens_match("proc:7@", "proc:7@"));
}

/// Rework item 4 (r4): the REAL binding writer and a presenter in the same
/// pid namespace agree, and two sandboxes whose processes started in the same
/// jiffy do not present each other's binding. No test-only presenter token is
/// involved: both sides come from one [`IdentitySource`].
#[test]
fn direct_issue_binding_matches_its_namespace_only() {
    let src = IdentitySource {
        session_leader: Some(proc(3, "", "linux-starttime:9")),
        ..sandboxed("linux-starttime:9")
    };
    let bound = with_pid_namespace(Some(NS_A), || {
        crate::gate::seat_authority::direct_issue_binding(&src).unwrap()
    });
    assert!(
        bound.iter().all(|t| t.contains(&format!("@pidns-{NS_A}/"))),
        "{bound:?}"
    );
    let in_a = with_pid_namespace(Some(NS_A), || src.presenter_tokens());
    let in_b = with_pid_namespace(Some(NS_B), || src.presenter_tokens());
    assert!(binding_overlaps(&bound, &in_a));
    assert!(!binding_overlaps(&bound, &in_b));
    assert!(!binding_overlaps(&bound, &src.presenter_tokens()));
    // On the host the writer and presenter forms are identical, unqualified.
    let host = crate::gate::seat_authority::direct_issue_binding(&src).unwrap();
    assert_eq!(host[0], "proc:2@linux-starttime:9");
    assert!(binding_overlaps(&host, &src.presenter_tokens()));
    assert_eq!(
        author_process_token("proc", 2, Some("linux-starttime:9")),
        process_token("proc", 2, Some("linux-starttime:9"))
    );
}

/// `binding_tokens_match` compares parsed `(pid, namespace, start)`.
#[test]
fn binding_tokens_match_compares_namespace_and_start() {
    let a = format!("proc:9@pidns-{NS_A}/linux-starttime:5");
    let b = format!("proc:9@pidns-{NS_B}/linux-starttime:5");
    assert!(binding_tokens_match(&a, &a));
    assert!(!binding_tokens_match(&a, &b));
    assert!(!binding_tokens_match(&a, "proc:9@linux-starttime:5"));
    assert!(!binding_tokens_match(
        &a,
        &format!("proc:9@pidns-{NS_A}/linux-starttime:6")
    ));
    assert!(binding_tokens_match(
        &a,
        &format!("sid:9@pidns-{NS_A}/linux-starttime:5")
    ));
}

mod grant_writers {
    use super::*;
    use crate::gate::seat_authority::{
        bind_grant_to_pid, issue_child, presenter_rejection, read_grant, test_support, SeatGrant,
        GRANT_ENV,
    };
    use crate::test_env::EnvVarsGuard;

    const SUBJECT: &str = "bug-2419-fixture-user";

    fn pinned_env() -> EnvVarsGuard {
        let mut env =
            EnvVarsGuard::snapshot(&["AIDA_USER", GRANT_ENV, "AIDA_SESSION_ID", "AIDA_HOME"]);
        env.set_key("AIDA_USER", SUBJECT);
        env.unset_key("AIDA_SESSION_ID");
        env
    }

    /// This very process as a presenter in `ns` sees it: no `cfg(test)` help.
    fn me_presenting(ns: Option<&str>) -> Vec<String> {
        let src = IdentitySource {
            session_ids: vec![],
            chain: vec![ProcEntry {
                pid: std::process::id(),
                name: "aida".into(),
                cmd: vec!["aida".into()],
                start: aida_core::liveness::process_start_identity(std::process::id()),
            }],
            session_leader: None,
        };
        with_pid_namespace(ns, || src.presenter_tokens())
    }

    fn proc_binding_only(grant: SeatGrant, keep: impl Fn(&String) -> bool) -> SeatGrant {
        SeatGrant {
            binding: grant.binding.iter().filter(|b| keep(b)).cloned().collect(),
            ..grant
        }
    }

    /// F2: a grant stamped by `bind_grant_to_pid` inside namespace A is
    /// presented by its own process in A and refused from B (and the host).
    #[test]
    fn grant_bound_to_pid_inside_namespace_a_is_refused_from_b() {
        let tmp = tempfile::tempdir().unwrap();
        let _env = pinned_env();
        // Minted on the host: its original binding is the unqualified form.
        let id = test_support::mint_grant_for(tmp.path(), SUBJECT, "reviewer", &[]);
        let host_binding = read_grant(&id).unwrap().binding;
        with_pid_namespace(Some(NS_A), || bind_grant_to_pid(&id, std::process::id())).unwrap();
        let only_a = proc_binding_only(read_grant(&id).unwrap(), |b| !host_binding.contains(b));
        assert_eq!(only_a.binding.len(), 1, "{:?}", only_a.binding);
        assert!(
            only_a.binding[0].contains(&format!("@pidns-{NS_A}/")),
            "{:?}",
            only_a.binding
        );
        assert_eq!(
            presenter_rejection(&only_a, &me_presenting(Some(NS_A))),
            None
        );
        assert!(presenter_rejection(&only_a, &me_presenting(Some(NS_B))).is_some());
        assert!(presenter_rejection(&only_a, &me_presenting(None)).is_some());
    }

    /// F2: a child grant minted (`issue_child_from`) inside namespace A binds
    /// the launcher's qualified token, which its process presents in A only.
    #[test]
    fn child_grant_minted_inside_namespace_a_is_presented_there_only() {
        let tmp = tempfile::tempdir().unwrap();
        let mut env = pinned_env();
        let child = with_pid_namespace(Some(NS_A), || {
            let advisor =
                test_support::mint_grant_for(tmp.path(), SUBJECT, "advisor", &["reviewer"]);
            env.set_key(GRANT_ENV, &advisor);
            issue_child(tmp.path(), "reviewer", SUBJECT).unwrap()
        });
        assert!(
            child.binding[0].starts_with(&format!("proc:{}@pidns-{NS_A}/", std::process::id())),
            "{:?}",
            child.binding
        );
        let procs = proc_binding_only(child, |b| !b.starts_with("key:"));
        assert_eq!(
            presenter_rejection(&procs, &me_presenting(Some(NS_A))),
            None
        );
        assert!(presenter_rejection(&procs, &me_presenting(Some(NS_B))).is_some());
    }
}

/// F3 / hypothesis 1: a reviewer lease on the PR's own head branch never
/// counts as an author of the PR it reviews.
#[test]
fn reviewer_lease_on_the_pr_branch_is_never_an_author() {
    let head = "chore/codex-workspace-write";
    let mut by_pr = super::bug_1918_review_authority_tests::lease(
        "01a19efaf0c9",
        "PR-3277",
        std::env::temp_dir().join("aida-pr-3277"),
    );
    by_pr.branch = head.into();
    by_pr.role = Some("reviewer".into());
    let mut by_verb = by_pr.clone();
    by_verb.id = "01a19efaf0ca".into();
    by_verb.scope = "TASK-2877".into();
    by_verb.review_verb = true;
    for reviewer in [by_pr.clone(), by_verb] {
        let authors = author_sessions(
            std::slice::from_ref(&reviewer),
            &[],
            &["TASK-2877".into()],
            Some(head),
            |_| None,
        );
        assert!(authors.is_empty(), "{reviewer:?}");
    }
    // Control: an implementer's work lease on the same branch is an author.
    let mut author = by_pr;
    author.scope = "TASK-2877".into();
    author.role = Some("implementer".into());
    assert_eq!(
        author_sessions(&[author], &[], &[], Some(head), |_| None).len(),
        1
    );
}

/// TASK-2799: the detached-worker path (STORY-2254) stamps and records its
/// worker exactly as the attached path does: the strict lease stamp records
/// the origin pid namespace beside `active_pid` (so `lease_tokens` keeps the
/// worker's proc token), and the strict authorship record names the worker
/// with its namespace-qualified token.
// trace:TASK-2799 | ai:claude
#[test]
fn detached_worker_stamp_and_record_carry_the_pid_namespace() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let grant = "33333333-3333-3333-3333-333333333333";
    let l = super::bug_1918_review_authority_tests::lease(
        "01a2b3c4d5f7",
        "TASK-27990",
        root.join("wt"),
    );
    super::bug_1918_review_authority_tests::write_lease(root, &l);
    let path = crate::lease_path(root, &l.id);
    let mut doc: toml_edit::DocumentMut = std::fs::read_to_string(&path).unwrap().parse().unwrap();
    doc[crate::session::LEASE_CHILD_GRANT_KEY] = toml_edit::value(grant);
    std::fs::write(&path, doc.to_string()).unwrap();

    let me = std::process::id();
    let start = "linux-starttime:4242";
    with_pid_namespace(Some(NS_A), || {
        crate::session::stamp_lease_detached_worker_at(root, &l.id, grant, me, start).unwrap();
        record_detached_worker_strict(root, &l.id, me, start).unwrap();
    });

    let stamped = toml::from_str::<SessionLease>(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(stamped.active_pid_ns.as_deref(), Some(NS_A));
    let qualified = format!("proc:{me}@{PID_NS_PREFIX}{NS_A}/{start}");
    assert!(
        lease_tokens(&stamped, None).contains(&qualified),
        "{:?}",
        lease_tokens(&stamped, None)
    );
    let ledger = load_ledger(root);
    assert!(
        ledger
            .iter()
            .any(|r| r.lease_id == l.id && r.tokens.contains(&qualified)),
        "{ledger:?}"
    );
    assert!(
        !ledger
            .iter()
            .any(|r| r.tokens.contains(&process_token("proc", me, Some(start)))),
        "the unqualified token is never recorded inside a namespace"
    );

    // On the host the stamp records the host namespace and the token stays
    // unqualified.
    crate::session::stamp_lease_detached_worker_at(root, &l.id, grant, me, start).unwrap();
    let stamped = toml::from_str::<SessionLease>(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        stamped.active_pid_ns.as_deref(),
        Some(INITIAL_PID_NAMESPACE_INODE)
    );
    assert!(lease_tokens(&stamped, None).contains(&process_token("proc", me, Some(start))));
}
