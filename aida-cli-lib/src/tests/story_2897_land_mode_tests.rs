//! STORY-2897 S2: `aida land mode` reconciles the forge's required checks,
//! verifies every write (its rollback included) by read-back, journals the
//! attempt and its outcome, and is a write for stakeholder roles.
// trace:STORY-2897 | ai:claude

use super::*;
use std::collections::VecDeque;

/// What the fake forge does with one write.
#[derive(Clone)]
enum W {
    /// Takes the requested list.
    Apply,
    /// Accepts it and changes nothing.
    Swallow,
    /// Accepts it and ends up requiring this list instead.
    Garble(Vec<String>),
    /// Refuses it.
    Fail,
}

#[derive(Default)]
struct Fake {
    now: Vec<String>,
    strict: bool,
    writes: Vec<Vec<String>>,
    /// Per-write behaviour, in order; `Apply` once it runs out.
    plan: VecDeque<W>,
    /// 0-based indices of reads that fail.
    failing_reads: Vec<usize>,
    reads: usize,
    /// Runs on every write (e.g. to break the config file mid-change).
    on_write: Option<Box<dyn FnMut()>>,
}

impl Protection for Fake {
    fn read(&mut self) -> Result<Required> {
        let n = self.reads;
        self.reads += 1;
        if self.failing_reads.contains(&n) {
            bail!("read {n} timed out");
        }
        Ok(Required {
            contexts: self.now.clone(),
            strict: self.strict,
        })
    }
    fn write(&mut self, contexts: &[String], strict: bool) -> Result<()> {
        assert_eq!(strict, self.strict, "strict is written back as read");
        self.writes.push(contexts.to_vec());
        if let Some(hook) = self.on_write.as_mut() {
            hook();
        }
        match self.plan.pop_front().unwrap_or(W::Apply) {
            W::Apply => self.now = contexts.to_vec(),
            W::Swallow => {}
            W::Garble(list) => self.now = list,
            W::Fail => bail!("HTTP 502"),
        }
        Ok(())
    }
}

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

const CI: &[&str] = &["merge-hold-gate", "Build (ubuntu-latest)"];
const REVIEW: &[&str] = &["merge-hold-gate"];

fn forge(plan: Vec<W>, failing_reads: Vec<usize>) -> Fake {
    Fake {
        now: names(CI),
        strict: true,
        plan: plan.into(),
        failing_reads,
        ..Fake::default()
    }
}

#[test]
fn reconcile_sets_reads_back_and_reports_before_and_after() {
    let mut f = forge(vec![], vec![]);
    let done = reconcile(&mut f, &names(REVIEW)).unwrap();
    assert_eq!(done.outcome, Outcome::Applied);
    assert_eq!(done.before.contexts, CI);
    assert_eq!(done.after.unwrap().contexts, REVIEW);
    assert_eq!(f.writes.len(), 1);
}

#[test]
fn reconcile_leaves_a_matching_forge_untouched() {
    let mut f = forge(vec![], vec![]);
    let done = reconcile(
        &mut f,
        &names(&["Build (ubuntu-latest)", "merge-hold-gate"]),
    )
    .unwrap();
    assert_eq!(done.outcome, Outcome::Unchanged);
    assert!(f.writes.is_empty());
}

#[test]
fn a_write_that_changed_nothing_needs_no_restore() {
    let mut f = forge(vec![W::Swallow], vec![]);
    let done = reconcile(&mut f, &names(REVIEW)).unwrap();
    assert!(matches!(done.outcome, Outcome::NotApplied { .. }));
    let why = done.failure().unwrap();
    assert!(
        why.contains("after it was asked for [merge-hold-gate]"),
        "{why}"
    );
    assert!(why.contains("still requires [merge-hold-gate, Build (ubuntu-latest)], unchanged"));
    assert_eq!(f.writes.len(), 1, "no restore write");
}

#[test]
fn a_mismatched_read_back_restores_and_verifies_the_previous_checks() {
    let mut f = forge(vec![W::Garble(names(&["lint"]))], vec![]);
    let done = reconcile(&mut f, &names(REVIEW)).unwrap();
    let why = done.failure().unwrap();
    assert!(why.contains("requires [lint] after it was asked"), "{why}");
    assert!(why.contains("restored [merge-hold-gate, Build (ubuntu-latest)] (read back)"));
    assert_eq!(done.outcome_name(), "rolled-back");
    assert_eq!(f.writes.len(), 2, "one attempt, one restore");
    assert_eq!(f.now, CI);
}

/// Review P2: an unreadable read-back is a failed verification, not an
/// early exit that leaves the change in place.
#[test]
fn an_unreadable_read_back_restores_the_previous_checks() {
    // Read 0 = before, read 1 = the read-back (fails), read 2 = restore check.
    let mut f = forge(vec![], vec![1]);
    let done = reconcile(&mut f, &names(REVIEW)).unwrap();
    let why = done.failure().unwrap();
    assert!(
        why.contains("could not be read back (read 1 timed out)"),
        "{why}"
    );
    assert!(why.contains("restored ["), "{why}");
    assert_eq!(f.now, CI, "the change does not stay in place");
}

/// Review P2: a restore the forge accepts but does not apply is reported as
/// unresolved, never as restored.
#[test]
fn an_ineffective_restore_is_unresolved_not_restored() {
    let mut f = forge(vec![W::Garble(names(&["lint"])), W::Swallow], vec![]);
    let done = reconcile(&mut f, &names(REVIEW)).unwrap();
    let why = done.failure().unwrap();
    assert!(!why.contains("restored ["), "{why}");
    assert!(
        why.contains("could not be confirmed (the forge requires [lint])"),
        "{why}"
    );
    assert!(why.contains("UNRESOLVED"), "{why}");
    assert_eq!(done.outcome_name(), "unresolved");

    let mut f = forge(vec![], vec![1, 2]);
    let done = reconcile(&mut f, &names(REVIEW)).unwrap();
    let why = done.failure().unwrap();
    assert!(why.contains("its read-back failed"), "{why}");
    assert!(why.contains("UNRESOLVED"), "{why}");

    let mut f = forge(vec![W::Garble(names(&["lint"])), W::Fail], vec![]);
    let why = reconcile(&mut f, &names(REVIEW))
        .unwrap()
        .failure()
        .unwrap();
    assert!(why.contains("restoring [merge-hold-gate, Build (ubuntu-latest)] also failed"));
    assert!(why.contains("UNRESOLVED"), "{why}");
}

#[test]
fn a_refused_write_is_checked_by_read_back() {
    let mut f = forge(vec![W::Fail], vec![]);
    let done = reconcile(&mut f, &names(REVIEW)).unwrap();
    let why = done.failure().unwrap();
    assert!(
        why.contains("the forge refused [merge-hold-gate] (HTTP 502)"),
        "{why}"
    );
    assert!(why.contains("unchanged"), "{why}");
}

#[test]
fn drift_names_missing_and_extra_checks() {
    let policy = LandingPolicy {
        mode: LandingMode::ReviewGated,
        ..LandingPolicy::default()
    };
    let req = |c: &[&str]| Required {
        contexts: names(c),
        strict: false,
    };
    assert!(drift(&policy, &req(REVIEW)).is_empty());
    let lines = drift(&policy, &req(&["Build (ubuntu-latest)"]));
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].contains("does not require merge-hold-gate"));
    assert!(lines[1].contains("also requires Build (ubuntu-latest)"));
}

#[test]
fn the_forge_response_parses() {
    let r =
        parse_required(r#"{"strict":true,"contexts":["merge-hold-gate"],"checks":[]}"#).unwrap();
    assert_eq!(r.contexts, ["merge-hold-gate"]);
    assert!(r.strict);
    assert!(parse_required(r#"{"strict":true}"#).is_err());
}

fn actor() -> Actor {
    Actor {
        user: "joe".into(),
        floor: false,
        seat: Some("advisor".into()),
        grant_id: Some("grant-123".into()),
        principal: Some("joe".into()),
        mandate: Some("mandate-9".into()),
    }
}

fn set_change(config: &Path) -> Change<'_> {
    Change {
        action: "set",
        from: Some(LandingMode::CiGated),
        to: LandingMode::ReviewGated,
        want: names(REVIEW),
        config: Some(config),
    }
}

fn audit_lines(root: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(root.join(AUDIT))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// Review P2: the signed audit contract — the attempt and the outcome,
/// with the validated grant, the mandate, the forge before/after and the
/// read-back; plus the `LandModeChanged` event.
#[test]
fn a_set_journals_the_attempt_and_outcome_with_the_grant_and_emits_the_event() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = crate::test_env::AmbientGuard::hermetic_with_seat(tmp.path(), "advisor", &[]);
    let config = tmp.path().join(".aida/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "[forge]\nprovider = \"github\"\n").unwrap();
    let mut f = forge(vec![], vec![]);
    apply_change(tmp.path(), &mut f, &actor(), &set_change(&config)).unwrap();

    let lines = audit_lines(tmp.path());
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0]["phase"], "attempt");
    let out = &lines[1];
    assert_eq!(out["phase"], "outcome");
    assert_eq!(out["outcome"], "applied");
    assert_eq!(out["grant_id"], "grant-123");
    assert_eq!(out["mandate"], "mandate-9");
    assert_eq!(out["seat"], "advisor");
    assert_eq!(out["from"], "ci-gated");
    assert_eq!(out["to"], "review-gated");
    assert_eq!(out["forge_before"], serde_json::json!(CI));
    assert_eq!(out["forge_after"], serde_json::json!(REVIEW));
    assert_eq!(out["read_back_ok"], true);
    assert_eq!(out["config_written"], true);
    let body = std::fs::read_to_string(&config).unwrap();
    assert!(body.contains("mode = \"review-gated\""), "{body}");

    let events = crate::events::read_all(tmp.path());
    assert!(
        events.iter().any(|e| matches!(
            &e.kind,
            crate::events::EventKind::LandModeChanged { outcome, grant_id, to, .. }
                if outcome == "applied" && grant_id.as_deref() == Some("grant-123")
                    && to == "review-gated"
        )),
        "{events:?}"
    );
}

/// Review P2: an audit or config that cannot be written refuses before the
/// forge is touched.
#[test]
fn an_unwritable_audit_or_bad_config_refuses_before_any_forge_write() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join(".aida/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "[landing\nmode = ").unwrap();
    let mut f = forge(vec![], vec![]);
    let err = apply_change(tmp.path(), &mut f, &actor(), &set_change(&config)).unwrap_err();
    assert!(format!("{err:#}").contains("failed to parse"), "{err:#}");
    assert!(f.writes.is_empty(), "nothing written to the forge");

    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(AUDIT)).unwrap();
    let mut f = forge(vec![], vec![]);
    let change = Change {
        config: None,
        action: "reconcile",
        ..set_change(Path::new("unused"))
    };
    let err = apply_change(tmp.path(), &mut f, &actor(), &change).unwrap_err();
    assert!(
        format!("{err:#}").contains("could not open the audit"),
        "{err:#}"
    );
    assert!(f.writes.is_empty(), "nothing written to the forge");
}

/// Review P2: a config write that fails after the forge changed is recorded,
/// with the recovery steps.
#[test]
fn a_config_write_failure_after_the_forge_changed_is_journaled_with_recovery() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = crate::test_env::AmbientGuard::hermetic_with_seat(tmp.path(), "advisor", &[]);
    let config = tmp.path().join(".aida/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "").unwrap();
    let broken = config.clone();
    let mut f = forge(vec![], vec![]);
    // Something else breaks the file while the forge is being changed.
    f.on_write = Some(Box::new(move || {
        std::fs::write(&broken, "[landing\n").unwrap();
    }));
    let err = apply_change(tmp.path(), &mut f, &actor(), &set_change(&config)).unwrap_err();
    let text = format!("{err:#}");
    assert!(
        text.contains("the forge now requires [merge-hold-gate]"),
        "{text}"
    );
    assert!(text.contains("aida land mode reconcile"), "{text}");
    let lines = audit_lines(tmp.path());
    let out = lines.last().unwrap();
    assert_eq!(out["outcome"], "applied");
    assert_eq!(out["config_written"], false);
    assert!(
        out["config_error"].as_str().unwrap().contains("parse"),
        "{out}"
    );
}

/// A failed rollback is journaled, never silent.
#[test]
fn an_unresolved_change_is_journaled() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = crate::test_env::AmbientGuard::hermetic_with_seat(tmp.path(), "advisor", &[]);
    let mut f = forge(vec![W::Garble(names(&["lint"])), W::Fail], vec![]);
    let change = Change {
        config: None,
        action: "reconcile",
        ..set_change(Path::new("unused"))
    };
    let err = apply_change(tmp.path(), &mut f, &actor(), &change).unwrap_err();
    assert!(format!("{err:#}").contains("UNRESOLVED"));
    let out = audit_lines(tmp.path()).pop().unwrap();
    assert_eq!(out["outcome"], "unresolved");
    assert_eq!(out["read_back_ok"], false);
    assert!(
        out["error"].as_str().unwrap().contains("also failed"),
        "{out}"
    );
}

/// Review P2: `land mode set|reconcile` are writes for stakeholder roles;
/// status and metrics stay reads.
#[test]
fn a_guest_may_read_but_not_change_the_landing_mode() {
    let enforce = |args: &[&str]| {
        let mut cli = crate::TEST_CLI
            .try_parse_from(args)
            .expect("valid CLI command");
        crate::stakeholder_role::enforce_stakeholder_role_capabilities_for_role(
            &mut cli.command,
            "guest",
        )
    };
    for args in [
        &["aida", "land", "mode", "set", "review-gated"][..],
        &["aida", "land", "mode", "reconcile"][..],
    ] {
        assert!(enforce(args).is_err(), "{args:?} must be refused");
    }
    for args in [
        &["aida", "land", "mode", "status"][..],
        &["aida", "land", "status"][..],
    ] {
        enforce(args).unwrap_or_else(|e| panic!("{args:?} is a read: {e:#}"));
    }
}
