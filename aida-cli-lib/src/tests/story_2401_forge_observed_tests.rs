//! STORY-2401 slice 2: the `Observed<T>` read type, the gh reply
//! classifier, bounded settling, and the TASK-2794 mergeable fix.
//! trace:STORY-2401 trace:TASK-2794 | ai:claude

use super::*;
use crate::forge::{ChangeLookup, ChangeRef, ChangeState, ChangeStatus, ReviewDecision};
use std::cell::Cell;

// trace:STORY-2401 | ai:codex
#[test]
fn story_2401_github_status_requires_complete_recognized_evidence() {
    use crate::forge::parse_github_change_status;
    for malformed in [
        "",
        "OPEN",
        "OPEN\tMERGEABLE\tAPPROVED\t",
        "BOGUS\tMERGEABLE\t\tabc",
    ] {
        assert!(matches!(
            parse_github_change_status(malformed),
            Observed::Transient(_)
        ));
    }
    assert_eq!(
        parse_github_change_status("OPEN\tMERGEABLE\t\tabc\n"),
        Observed::Known(status(Observed::Known(true)))
    );
    let Observed::Known(pending) = parse_github_change_status("OPEN\tUNKNOWN\t\tabc\n") else {
        panic!("a complete status with unsettled mergeability remains usable");
    };
    assert!(matches!(pending.mergeable, Observed::Pending(_)));
    for (state, expected) in [
        ("MERGED", ChangeState::Merged),
        ("CLOSED", ChangeState::Closed),
    ] {
        let Observed::Known(parsed) =
            parse_github_change_status(&format!("{state}\tUNKNOWN\tAPPROVED\tabc"))
        else {
            panic!("complete terminal status must be known");
        };
        assert_eq!(parsed.state, expected);
        assert_eq!(parsed.review, ReviewDecision::Approved);
    }
}

#[cfg(unix)]
fn output(code: i32, stdout: &str, stderr: &str) -> Output {
    use std::os::unix::process::ExitStatusExt;
    Output {
        status: std::process::ExitStatus::from_raw(code << 8),
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

#[cfg(unix)]
#[test]
fn story_2401_observe_gh_classifies_ok_refused_and_transient() {
    assert_eq!(
        observe_gh(Some(output(0, "MERGED\n", ""))),
        Observed::Known(GhReply::Ok("MERGED".into()))
    );
    assert_eq!(
        observe_gh(Some(output(
            1,
            "",
            "no pull requests found for branch \"x\"\n"
        ))),
        Observed::Known(GhReply::Refused(
            "no pull requests found for branch \"x\"".into()
        ))
    );
    for transient in [
        "error connecting to api.github.com",
        "dial tcp: lookup api.github.com: Temporary failure in name resolution",
        "HTTP 502: 502 Bad Gateway",
        "HTTP 500: Internal Server Error",
        "HTTP/2 507 Insufficient Storage",
        "HTTP 429: Too Many Requests",
        "context deadline exceeded (Client.Timeout exceeded while awaiting headers)",
        "API rate limit exceeded",
    ] {
        assert!(
            matches!(
                observe_gh(Some(output(1, "", transient))),
                Observed::Transient(_)
            ),
            "{transient} must be transient"
        );
    }
    assert!(matches!(observe_gh(None), Observed::Transient(_)));
    assert!(matches!(
        observe_gh(Some(output(1, "", ""))),
        Observed::Transient(_)
    ));
    use std::os::unix::process::ExitStatusExt;
    let mut killed = output(1, "", "killed");
    killed.status = std::process::ExitStatus::from_raw(9);
    assert!(matches!(observe_gh(Some(killed)), Observed::Transient(_)));
    // A refusal is not a stdout value.
    assert!(matches!(
        observe_gh_stdout(Some(output(1, "", "not found"))),
        Observed::Transient(_)
    ));
    assert_eq!(
        observe_gh_stdout(Some(output(0, "3\n", ""))),
        Observed::Known("3".to_string())
    );
}

#[test]
fn story_2401_observed_combinators_keep_the_unsettled_reason() {
    let known: Observed<u32> = Observed::Known(2);
    assert_eq!(known.clone().map(|n| n * 2), Observed::Known(4));
    assert!(known.is_settled());
    assert_eq!(known.unsettled_reason(), None);
    let pending: Observed<u32> = Observed::Pending("UNKNOWN".into());
    assert_eq!(
        pending.clone().map(|n| n + 1),
        Observed::Pending("UNKNOWN".into())
    );
    assert_eq!(pending.unsettled_reason(), Some("UNKNOWN"));
    assert_eq!(pending.known(), None);
    assert_eq!(
        Observed::<u32>::from_result(Err(anyhow::anyhow!("dns"))),
        Observed::Transient("dns".into())
    );
    assert!(Observed::Known(false).is_known_false());
    assert!(!Observed::<bool>::Pending("x".into()).is_known_false());
    assert!(!Observed::<bool>::Transient("x".into()).is_known_true());
}

#[test]
fn story_2401_settle_rereads_until_known_and_is_bounded() {
    let calls = Cell::new(0);
    let got = settle(5, Duration::ZERO, || {
        calls.set(calls.get() + 1);
        if calls.get() < 3 {
            Observed::Pending("computing".into())
        } else {
            Observed::Known(calls.get())
        }
    });
    assert_eq!(got, Observed::Known(3));
    assert_eq!(calls.get(), 3, "stops at the first settled read");

    calls.set(0);
    let got: Observed<()> = settle(3, Duration::ZERO, || {
        calls.set(calls.get() + 1);
        Observed::Transient("timeout".into())
    });
    assert_eq!(got, Observed::Transient("timeout".into()));
    assert_eq!(calls.get(), 3, "never reads past the bound");

    calls.set(0);
    let _ = settle(0, Duration::ZERO, || {
        calls.set(calls.get() + 1);
        Observed::Known(())
    });
    assert_eq!(calls.get(), 1, "always reads at least once");
}

/// TASK-2794: GitHub's UNKNOWN mergeable is pending, not a "no".
#[test]
fn story_2401_github_unknown_mergeable_is_pending_not_false() {
    use crate::forge::github_mergeable;
    assert_eq!(github_mergeable("MERGEABLE"), Observed::Known(true));
    assert_eq!(github_mergeable("CONFLICTING"), Observed::Known(false));
    assert!(matches!(github_mergeable("UNKNOWN"), Observed::Pending(_)));
    assert!(matches!(github_mergeable(""), Observed::Pending(_)));
}

#[test]
fn story_2401_behind_by_parse_is_pending_on_a_non_count() {
    use crate::forge::parse_behind_by;
    assert_eq!(parse_behind_by("0"), Observed::Known(0));
    assert_eq!(parse_behind_by("4\n"), Observed::Known(4));
    assert!(matches!(parse_behind_by("null"), Observed::Pending(_)));
    assert!(matches!(parse_behind_by(""), Observed::Pending(_)));
}

/// The integration gate reads a settled GitHub "no" as a conflict and
/// everything unsettled (or GitLab's non-conflict "no") as unknown.
#[test]
fn story_2401_wave_mergeable_state_only_conflicts_on_a_settled_github_no() {
    use crate::burndown_run::wave_mergeable_state;
    use crate::forge::ForgeKind;
    use crate::integrate::MergeableState;
    assert_eq!(
        wave_mergeable_state(ForgeKind::GitHub, &Observed::Known(true)),
        MergeableState::Mergeable
    );
    assert_eq!(
        wave_mergeable_state(ForgeKind::GitHub, &Observed::Known(false)),
        MergeableState::Conflicting
    );
    assert_eq!(
        wave_mergeable_state(ForgeKind::GitHub, &Observed::Pending("UNKNOWN".into())),
        MergeableState::Unknown
    );
    assert_eq!(
        wave_mergeable_state(ForgeKind::GitLab, &Observed::Known(false)),
        MergeableState::Unknown
    );
}

/// A forge stub whose `Result` reads drive the default `observe_*` bridges.
struct StubForge {
    metadata: std::cell::RefCell<Vec<Observed<Result<crate::forge::ChangeMetadata, String>>>>,
    status: std::cell::RefCell<Vec<anyhow::Result<ChangeStatus>>>,
    rows: Vec<crate::ci_gate::CheckRow>,
    lookup: ChangeLookup,
}

fn status(mergeable: Observed<bool>) -> ChangeStatus {
    ChangeStatus {
        state: ChangeState::Open,
        mergeable,
        review: ReviewDecision::None,
        head_sha: "abc".into(),
    }
}

fn change() -> ChangeRef {
    ChangeRef {
        id: 7,
        url: String::new(),
        branch: "topic".into(),
        base: "main".into(),
        title: None,
    }
}

impl crate::forge::Forge for StubForge {
    fn kind(&self) -> crate::forge::ForgeKind {
        crate::forge::ForgeKind::GitHub
    }
    fn open_change(&self, _: crate::forge::OpenChange) -> anyhow::Result<ChangeRef> {
        unimplemented!()
    }
    fn change_for_branch(&self, _: &str) -> anyhow::Result<ChangeLookup> {
        Ok(self.lookup.clone())
    }
    fn change_for_spec(&self, _: &str) -> anyhow::Result<ChangeLookup> {
        unimplemented!()
    }
    fn merged_change_for_branch(&self, _: &str) -> anyhow::Result<ChangeLookup> {
        unimplemented!()
    }
    fn change_status(&self, _: &ChangeRef) -> anyhow::Result<ChangeStatus> {
        self.status.borrow_mut().remove(0)
    }
    fn observe_change_metadata(
        &self,
        _: u64,
        _: &mut dyn crate::network_retry::RetrySink,
    ) -> Observed<Result<crate::forge::ChangeMetadata, String>> {
        let mut metadata = self.metadata.borrow_mut();
        if metadata.is_empty() {
            Observed::Transient("no metadata".into())
        } else {
            metadata.remove(0)
        }
    }
    fn change_metadata(
        &self,
        _: u64,
        _: &mut dyn crate::network_retry::RetrySink,
    ) -> anyhow::Result<crate::forge::ChangeMetadata> {
        anyhow::bail!("no metadata")
    }
    fn authenticated_user_login(&self) -> anyhow::Result<String> {
        unimplemented!()
    }
    fn change_commit_headlines(
        &self,
        _: u64,
        _: &mut dyn crate::network_retry::RetrySink,
    ) -> anyhow::Result<Vec<String>> {
        unimplemented!()
    }
    fn diff_change(&self, _: u64) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn change_reviews(
        &self,
        _: u64,
        _: &mut dyn crate::network_retry::RetrySink,
    ) -> anyhow::Result<crate::forge::ChangeReviews> {
        unimplemented!()
    }
    fn ci_status(&self, _: crate::forge::CiTarget) -> anyhow::Result<crate::forge::CiStatus> {
        unimplemented!()
    }
    fn ci_probe_for_branch(&self, _: &str) -> anyhow::Result<crate::forge::CiProbeResult> {
        unimplemented!()
    }
    fn watch_ci(&self, _: &ChangeRef) -> anyhow::Result<crate::gate::dispatch_types::CiState> {
        unimplemented!()
    }
    fn stream_ci_for_branch(
        &self,
        _: &str,
        _: bool,
    ) -> anyhow::Result<crate::forge::CiProbeResult> {
        unimplemented!()
    }
    fn check_rows(&self, _: &ChangeRef) -> anyhow::Result<Vec<crate::ci_gate::CheckRow>> {
        Ok(self.rows.clone())
    }
    fn merge_change(
        &self,
        _: &ChangeRef,
        _: &crate::forge::MergeOptions,
        _: &mut dyn crate::network_retry::RetrySink,
    ) -> anyhow::Result<crate::forge::MergeResult> {
        unimplemented!()
    }
    fn comment(&self, _: &ChangeRef, _: &str) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn close_change(&self, _: &ChangeRef, _: &str) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn mark_change_ready(&self, _: &ChangeRef) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn checkout_change(&self, _: &ChangeRef) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn list_changes(&self, _: crate::forge::ChangeFilter) -> anyhow::Result<Vec<ChangeRef>> {
        unimplemented!()
    }
}

#[test]
fn story_2401_default_observe_reads_bridge_result_reads() {
    use crate::forge::Forge;
    let forge = StubForge {
        metadata: Default::default(),
        status: std::cell::RefCell::new(vec![
            Err(anyhow::anyhow!("dns")),
            Ok(status(Observed::Known(true))),
        ]),
        rows: Vec::new(),
        lookup: ChangeLookup::Unreachable("timeout".into()),
    };
    assert_eq!(
        forge.observe_change_status(&change()),
        Observed::Transient("dns".into())
    );
    assert_eq!(
        forge.observe_change_status(&change()),
        Observed::Known(Ok(status(Observed::Known(true))))
    );
    assert!(
        matches!(forge.observe_check_rows(&change()), Observed::Pending(_)),
        "an empty rollup has not registered yet"
    );
    assert_eq!(
        forge.observe_change_for_branch("topic"),
        Observed::Transient("timeout".into())
    );
    let mut sink = crate::network_retry::StderrSink;
    assert!(matches!(
        forge.observe_change_metadata(7, &mut sink),
        Observed::Transient(_)
    ));
    let settled = StubForge {
        metadata: Default::default(),
        status: std::cell::RefCell::new(Vec::new()),
        rows: Vec::new(),
        lookup: ChangeLookup::NoChange,
    };
    assert_eq!(
        settled.observe_change_for_branch("topic"),
        Observed::Known(ChangeLookup::NoChange)
    );
}

// trace:STORY-2401 trace:TASK-2794 | ai:codex
#[test]
fn story_2401_ship_mergeability_decisions_and_retry_bound() {
    use crate::gate::ship::{ShipFailure, ShipRefusal};
    use crate::pr_cmd::ship_mergeability_preflight;
    for (reads, expected, remaining) in [
        // Settled false refuses immediately, leaving the next read unused.
        (
            vec![
                Ok(status(Observed::Known(false))),
                Ok(status(Observed::Known(true))),
            ],
            "conflict",
            1,
        ),
        // UNKNOWN must settle within the actual ship loop, not only the parser.
        (
            vec![
                Ok(status(Observed::Pending("UNKNOWN".into()))),
                Ok(status(Observed::Known(true))),
                Err(anyhow::anyhow!("unused")),
            ],
            "mergeable",
            1,
        ),
    ] {
        let forge = StubForge {
            metadata: Default::default(),
            status: std::cell::RefCell::new(reads),
            rows: vec![],
            lookup: ChangeLookup::NoChange,
        };
        let result = ship_mergeability_preflight(&forge, &change(), Duration::ZERO);
        match expected {
            "conflict" => assert_eq!(
                result
                    .unwrap_err()
                    .downcast_ref::<ShipFailure>()
                    .unwrap()
                    .reason,
                ShipRefusal::NotMergeable
            ),
            "mergeable" => assert_eq!(result.unwrap(), status(Observed::Known(true))),
            _ => unreachable!(),
        }
        assert_eq!(forge.status.borrow().len(), remaining);
    }
    let pending = StubForge {
        metadata: Default::default(),
        status: std::cell::RefCell::new(
            (0..4)
                .map(|_| Ok(status(Observed::Pending("UNKNOWN".into()))))
                .collect(),
        ),
        rows: vec![],
        lookup: ChangeLookup::NoChange,
    };
    let got = ship_mergeability_preflight(&pending, &change(), Duration::ZERO).unwrap();
    assert!(
        matches!(got.mergeable, Observed::Pending(_)),
        "persistent Pending proceeds"
    );
    assert_eq!(pending.status.borrow().len(), 1, "exactly three reads");
    let transient = StubForge {
        metadata: Default::default(),
        status: std::cell::RefCell::new(
            (0..4)
                .map(|_| Err(anyhow::anyhow!("DNS unavailable")))
                .collect(),
        ),
        rows: vec![],
        lookup: ChangeLookup::NoChange,
    };
    let err = ship_mergeability_preflight(&transient, &change(), Duration::ZERO).unwrap_err();
    assert!(err.to_string().contains("DNS unavailable"));
    assert!(
        err.downcast_ref::<ShipFailure>().is_none(),
        "unavailable read is not a known conflict"
    );
    assert_eq!(transient.status.borrow().len(), 1, "exactly three reads");
}

// trace:STORY-2401 | ai:codex
#[cfg(unix)]
#[test]
fn story_2401_status_refusals_are_settled_and_not_retried() {
    use crate::forge::observe_github_change_status;
    for why in [
        "no pull requests found",
        "HTTP 404: Not Found",
        "HTTP 401: authentication refused",
    ] {
        let calls = Cell::new(0);
        let got = settle(3, Duration::ZERO, || {
            calls.set(calls.get() + 1);
            observe_github_change_status(Some(output(1, "", why)))
        });
        assert_eq!(got, Observed::Known(Err(why.into())));
        assert_eq!(calls.get(), 1, "a definitive refusal needs no retry");
    }
    assert!(matches!(
        observe_github_change_status(Some(output(1, "", "HTTP 500: Internal Server Error"))),
        Observed::Transient(_)
    ));
    assert_eq!(
        observe_github_change_status(Some(output(0, "OPEN\tMERGEABLE\t\tabc", ""))),
        Observed::Known(Ok(status(Observed::Known(true))))
    );
}

// trace:STORY-2401 | ai:codex
#[test]
fn story_2401_negative_needs_second_read_and_git_truth() {
    use Confirmed::*;
    let cases = [
        (
            vec![Observed::Known(false), Observed::Known(false)],
            Observed::Known(false),
            Negative,
            2,
        ),
        (
            vec![Observed::Known(false), Observed::Known(false)],
            Observed::Known(true),
            Unconfirmed,
            2,
        ),
        (
            vec![Observed::Known(false), Observed::Known(false)],
            Observed::Transient("fetch failed".into()),
            Unconfirmed,
            2,
        ),
        (
            vec![Observed::Transient("DNS".into()), Observed::Known(true)],
            Observed::Known(false),
            Positive,
            2,
        ),
        (
            vec![Observed::Transient("DNS".into()); 3],
            Observed::Known(false),
            Unconfirmed,
            3,
        ),
        (
            vec![
                Observed::Known(false),
                Observed::Pending("index".into()),
                Observed::Known(false),
            ],
            Observed::Known(false),
            Unconfirmed,
            3,
        ),
    ];
    for (reads, git, expected, count) in cases {
        let mut reads = std::collections::VecDeque::from(reads);
        let calls = Cell::new(0);
        let pauses = Cell::new(0);
        assert_eq!(
            confirm_negative(
                3,
                || {
                    calls.set(calls.get() + 1);
                    reads.pop_front().unwrap()
                },
                || git,
                || pauses.set(pauses.get() + 1)
            ),
            expected
        );
        assert_eq!(calls.get(), count);
        assert_eq!(pauses.get(), count - 1);
    }
}

// trace:STORY-2401 | ai:codex
#[test]
fn story_2401_snapshot_requires_matching_head_and_independent_evidence() {
    for (values, git, known) in [
        (vec!["closed@a", "closed@a"], true, true),
        (vec!["closed@a", "closed@b", "closed@c"], true, false),
        (vec!["closed@a", "closed@a"], false, false),
    ] {
        let mut values = values.into_iter();
        let reads = Cell::new(0);
        let observed = confirm_snapshot(
            || {
                reads.set(reads.get() + 1);
                Observed::Known(values.next().unwrap())
            },
            |_| Observed::Known(git),
            || {},
        );
        assert_eq!(observed.is_settled(), known);
        assert!(reads.get() >= 2);
    }
}

// trace:STORY-2401 | ai:codex
#[test]
fn story_2401_metadata_cannot_invent_open_from_unknown_or_partial_reply() {
    let mut value = serde_json::json!({"state": "CLOSED", "headRefOid": "a".repeat(40),
        "baseRefName": "main", "headRefName": "topic"});
    assert!(
        matches!(crate::forge::parse_github_metadata_observation(&value.to_string()), Observed::Known(m) if m.state == ChangeState::Closed)
    );
    value["state"] = "UNKNOWN".into();
    assert!(matches!(
        crate::forge::parse_github_metadata_observation(&value.to_string()),
        Observed::Transient(_)
    ));
    value["state"] = "OPEN".into();
    value["headRefOid"] = "".into();
    assert!(matches!(
        crate::forge::parse_github_metadata_observation(&value.to_string()),
        Observed::Transient(_)
    ));
}

// trace:STORY-2401 | ai:codex
fn metadata_forge(state: &str, head: &str, copies: usize) -> StubForge {
    let body = serde_json::json!({"state":state, "headRefOid":head,
        "baseRefName":"main", "headRefName":"topic"});
    let meta = crate::forge::parse_github_metadata_observation(&body.to_string())
        .known()
        .unwrap();
    StubForge {
        metadata: std::cell::RefCell::new(vec![Observed::Known(Ok(meta)); copies]),
        status: Default::default(),
        rows: vec![],
        lookup: ChangeLookup::NoChange,
    }
}

#[test]
fn story_2401_keep_answers_and_refusals_need_one_read_and_no_git() {
    let root = std::path::Path::new("/no-such-confirmation-repo");
    for terminal_only in [true, false] {
        let forge = metadata_forge("OPEN", &"a".repeat(40), 2);
        let result = crate::forge_confirmation::metadata_for(
            root,
            &forge,
            42,
            &mut crate::network_retry::NoopSink,
            |m| {
                if terminal_only {
                    m.state != ChangeState::Open
                } else {
                    m.state == ChangeState::Merged
                }
            },
        );
        assert!(matches!(result, Observed::Known(m) if m.state == ChangeState::Open));
        assert_eq!(forge.metadata.borrow().len(), 1);
    }
    // Retraction/abandoned-train callers only authorize on Open. Terminal
    // observations must also skip both the confirmation pause and Git read.
    // trace:STORY-2401 | ai:codex
    for state in ["CLOSED", "MERGED"] {
        let forge = metadata_forge(state, &"a".repeat(40), 2);
        assert!(matches!(crate::forge_confirmation::metadata_for(
            root, &forge, 42, &mut crate::network_retry::NoopSink,
            |m| m.state == ChangeState::Open), Observed::Known(m) if m.state != ChangeState::Open));
        assert_eq!(forge.metadata.borrow().len(), 1);
    }
    let forge = metadata_forge("OPEN", &"a".repeat(40), 0);
    *forge.metadata.borrow_mut() = vec![
        Observed::Known(Err("HTTP 404".into())),
        Observed::Transient("must not read".into()),
    ];
    assert_eq!(
        crate::forge_confirmation::metadata(root, &forge, 42, &mut crate::network_retry::NoopSink),
        Observed::Pending("HTTP 404".into())
    );
    assert_eq!(forge.metadata.borrow().len(), 1);
}

#[test]
fn story_2401_metadata_refusals_do_not_retry_and_transients_notify_sink() {
    #[derive(Default)]
    struct Sink(Vec<crate::network_retry::RetryEvent>);
    impl crate::network_retry::RetrySink for Sink {
        fn on_retry(&mut self, event: &crate::network_retry::RetryEvent) {
            self.0.push(event.clone());
        }
    }
    let cfg = crate::network_retry::RetryConfig {
        base_delay: Duration::ZERO,
        ..Default::default()
    };
    for why in [
        "HTTP 404: Not Found",
        "authentication failed",
        "no pull requests found",
    ] {
        let mut calls = 0;
        let mut sink = Sink::default();
        assert_eq!(
            crate::forge::observe_metadata_with_retry(42, &cfg, &mut sink, || {
                calls += 1;
                Some(output(1, "", why))
            }),
            Observed::Known(Err(why.into()))
        );
        assert_eq!(calls, 1);
        assert!(sink.0.is_empty());
    }
    let body = serde_json::json!({"state":"OPEN", "headRefOid":"a".repeat(40), "baseRefName":"main", "headRefName":"topic"}).to_string();
    let mut calls = 0;
    let mut sink = Sink::default();
    assert!(matches!(
        crate::forge::observe_metadata_with_retry(42, &cfg, &mut sink, || {
            calls += 1;
            Some(if calls == 1 {
                output(1, "", "HTTP 503: unavailable")
            } else {
                output(0, &body, "")
            })
        }),
        Observed::Known(Ok(_))
    ));
    assert_eq!(calls, 2);
    assert_eq!(sink.0.len(), 1);
    assert_eq!(sink.0[0].attempt, 1);
}

fn confirmation_git(root: &std::path::Path, args: &[&str]) -> String {
    let output = crate::test_git::git_command()
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

#[test]
fn story_2401_closed_train_receipt_and_absent_squash_head_real_git() {
    use crate::forge_confirmation::{metadata, on_base, train_landed, Sweep};
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path();
    confirmation_git(r, &["init", "-b", "main"]);
    confirmation_git(r, &["config", "user.name", "Confirmation"]);
    confirmation_git(r, &["config", "user.email", "confirmation@example.com"]);
    confirmation_git(r, &["config", "commit.gpgsign", "false"]);
    confirmation_git(r, &["remote", "add", "origin", r.to_str().unwrap()]);
    // The forge head was created remotely and was never fetched here.
    let head = "a".repeat(40);
    let msg = format!("land member (#42)\n\nAida-Train-Member: #42 at {head}");
    confirmation_git(r, &["commit", "--allow-empty", "-m", &msg]);
    assert!(!crate::test_git::git_command()
        .current_dir(r)
        .args(["cat-file", "-e", &head])
        .output()
        .unwrap()
        .status
        .success());
    let forge = metadata_forge("CLOSED", &head, 2);
    assert!(
        matches!(metadata(r, &forge, 42, &mut crate::network_retry::NoopSink), Observed::Known(m) if m.state == ChangeState::Merged)
    );
    {
        let _sweep = Sweep::new();
        assert_eq!(on_base(r, "main", &head, 42), Observed::Known(true));
        assert_eq!(train_landed(r, "main", &head, 42), Observed::Known(true));
        assert_eq!(
            train_landed(r, "main", &"b".repeat(40), 42),
            Observed::Known(false)
        );
        // Removing origin proves subsequent candidates reuse this sweep's fetch.
        confirmation_git(r, &["remote", "remove", "origin"]);
        assert_eq!(on_base(r, "main", &head, 99), Observed::Known(false));
    }
    // A later sweep may never borrow the prior successful tip.
    assert!(matches!(
        on_base(r, "main", &head, 42),
        Observed::Transient(_)
    ));
    confirmation_git(r, &["remote", "add", "origin", r.to_str().unwrap()]);
    confirmation_git(r, &["commit", "--allow-empty", "-m", "squash only (#43)"]);
    assert_eq!(on_base(r, "main", &head, 43), Observed::Known(true));
    assert_eq!(train_landed(r, "main", &head, 43), Observed::Known(false));
    let forge = metadata_forge("CLOSED", &head, 2);
    assert!(
        matches!(metadata(r, &forge, 43, &mut crate::network_retry::NoopSink), Observed::Known(m) if m.state == ChangeState::Closed)
    );
    // A transient fetch failure is not cached for the rest of the sweep: the
    // next candidate fetches again. trace:STORY-2401 | ai:claude
    {
        let _sweep = Sweep::new();
        confirmation_git(r, &["remote", "remove", "origin"]);
        assert!(matches!(
            on_base(r, "main", &head, 43),
            Observed::Transient(_)
        ));
        confirmation_git(r, &["remote", "add", "origin", r.to_str().unwrap()]);
        assert_eq!(on_base(r, "main", &head, 43), Observed::Known(true));
    }
    // A GitHub merge commit is a receipt even when the PR head was never
    // fetched locally. trace:STORY-2401 | ai:claude
    assert_eq!(on_base(r, "main", &head, 44), Observed::Known(false));
    confirmation_git(
        r,
        &[
            "commit",
            "--allow-empty",
            "-m",
            "Merge pull request #44 from fork/topic\n\nTopic",
        ],
    );
    assert_eq!(on_base(r, "main", &head, 44), Observed::Known(true));
    assert_eq!(on_base(r, "main", &head, 4), Observed::Known(false));
    assert_eq!(train_landed(r, "main", &head, 44), Observed::Known(false));
}
