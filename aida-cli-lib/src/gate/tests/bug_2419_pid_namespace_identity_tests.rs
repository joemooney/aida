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

/// Rework item 2: a live lease's creator and agent pids are qualified like
/// the recorder tokens they meet, so a sandboxed author approving its own
/// work from the same sandbox is still refused through the live-lease path
/// of `author_sessions` — while the same lease seen from the host never
/// matches the host's unqualified tokens.
#[test]
fn same_sandbox_self_approval_is_refused_through_the_live_lease_path() {
    let mut l = super::bug_1918_review_authority_tests::lease(
        "01a2b3c4d5e6",
        "BUG-24190",
        PathBuf::from("/w/bug-24190"),
    );
    l.creator_pid = Some(2);
    l.creator_pid_start_time = Some("linux-starttime:9".into());
    l.active_pid = Some(7);
    l.active_pid_start_time = Some("linux-starttime:9".into());
    let spec = vec!["BUG-24190".to_string()];

    let (authors, recorder) = with_pid_namespace(Some(NS_A), || {
        (
            author_sessions(std::slice::from_ref(&l), &[], &spec, None, |_| None),
            sandboxed("linux-starttime:9").recorder_tokens(),
        )
    });
    assert_eq!(authors.len(), 1);
    assert!(authors[0]
        .tokens
        .iter()
        .filter(|t| t.starts_with("proc:") || t.starts_with("shell:"))
        .all(|t| t.contains(&format!("@pidns-{NS_A}/"))));
    // Only by process identity: no shared session or lease token.
    assert!(recorder
        .iter()
        .all(|t| t.starts_with("proc:") || t.starts_with("shell:")));
    assert!(
        authors[0].matches(&recorder),
        "a same-sandbox self-approval must still be refused"
    );

    let host_authors = author_sessions(std::slice::from_ref(&l), &[], &spec, None, |_| None);
    let host_recorder = with_pid_namespace(Some(NS_A), || {
        sandboxed("linux-starttime:9").recorder_tokens()
    });
    assert!(
        !host_authors[0].matches(&host_recorder),
        "unqualified lease pids never match a namespaced recorder"
    );
}

/// Rework item 4: grant presenter/binding tokens taken inside a sandbox are
/// qualified, so two sandboxes whose processes started in the same jiffy do
/// not present each other's binding; within one sandbox they still bind.
#[test]
fn presenter_tokens_of_two_sandboxes_started_in_the_same_jiffy_do_not_bind() {
    let src = sandboxed("linux-starttime:9");
    let in_a = with_pid_namespace(Some(NS_A), || src.presenter_tokens());
    let in_a_again = with_pid_namespace(Some(NS_A), || src.presenter_tokens());
    let in_b = with_pid_namespace(Some(NS_B), || src.presenter_tokens());
    assert!(in_a.iter().all(|t| t.contains(&format!("@pidns-{NS_A}/"))));
    assert!(!binding_overlaps(&in_a, &in_b));
    for (a, b) in in_a.iter().zip(&in_b) {
        assert!(!binding_tokens_match(a, b), "{a} vs {b}");
    }
    assert!(binding_overlaps(&in_a, &in_a_again));
    // Before the fix both sandboxes presented the same unqualified token.
    let unqualified = src.presenter_tokens();
    assert!(binding_tokens_match(&unqualified[0], &unqualified[0]));
    assert!(!binding_overlaps(&in_a, &unqualified));
}
