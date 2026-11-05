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
        Observed::Known(status(Observed::Known(true)))
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
        status: std::cell::RefCell::new(Vec::new()),
        rows: Vec::new(),
        lookup: ChangeLookup::NoChange,
    };
    assert_eq!(
        settled.observe_change_for_branch("topic"),
        Observed::Known(ChangeLookup::NoChange)
    );
}
