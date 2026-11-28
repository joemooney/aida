//! `aida pr train --check`: would each PR ride a train right now, and if not,
//! which step of the member gate stops it and what clears that step.
//!
//! The answer comes from the member gate `aida pr train` itself runs
//! ([`typed_gate_member_with`] behind the two rider checks), in read-only
//! mode, so the report cannot drift from the train. Read-only means no label,
//! hold, journal, gate-ledger, event or activity write; the gate still
//! refreshes remote-tracking refs, as any fetch does.
// trace:STORY-2824 | ai:claude

use super::*;
use crate::gate::mandate::MandateRefusal;
use std::time::{Duration, Instant};

/// `aida pr train --check` options.
// trace:STORY-2824 | ai:claude
// trace:STORY-2824 | ai:codex
pub(crate) struct CheckOptions<'a> {
    pub prs: &'a [u64],
    /// Every open PR on the default branch, instead of `prs`.
    pub all_open: bool,
    pub json: bool,
    /// Override full-tier admission for this read-only report only.
    pub skip_full_check: bool,
    /// Shared budget for discovery and member checks.
    pub timeout: Duration,
}

/// How far a PR's check got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Evidence {
    /// The gate reached a verdict.
    Read,
    /// A read failed; the PR is reported not eligible, fail closed.
    Unavailable,
    /// The time budget ran out before this PR's verdict.
    NotChecked,
}

impl Evidence {
    fn token(self) -> &'static str {
        match self {
            Evidence::Read => "read",
            Evidence::Unavailable => "unavailable",
            Evidence::NotChecked => "not-checked",
        }
    }
}

/// One PR's line of the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub pr: u64,
    pub eligible: bool,
    /// The head the train would carry, when eligible.
    pub head: Option<String>,
    /// The first step that stopped it.
    pub gate: Option<&'static str>,
    pub reason: Option<String>,
    pub remedy: Option<String>,
    pub evidence: Evidence,
}

/// The gate's name in the report.
pub(crate) fn gate_token(gate: Gate) -> &'static str {
    match gate {
        Gate::PrState => "pr-state",
        Gate::Riding => "riding-train",
        Gate::Drive => "live-drive",
        Gate::Hold => "merge-hold",
        Gate::HoldLabels => "hold-labels",
        Gate::ReviewClaim => "review-in-progress",
        Gate::Head => "head",
        Gate::Approval => "approval",
        Gate::FullCi => "full-ci",
        Gate::Corpus => "verdict-corpus",
        Gate::Message => "commit-message",
        Gate::SpecPolicy => "spec-policy",
        Gate::Mandate => "mandate",
    }
}

/// What clears a refusal at `gate` for PR `pr`.
pub(crate) fn remedy(gate: Gate, pr: u64) -> String {
    match gate {
        Gate::PrState => format!(
            "only an open, ready, same-repository PR on the default branch rides a train; \
             mark a draft ready with `gh pr ready {pr}`"
        ),
        Gate::Riding => "wait for that train to land or leave it out, then check again".into(),
        Gate::Drive => "let the drive finish; its reviewer gates the merge".into(),
        Gate::Hold => format!(
            "resolve the hold's reason, then a human clears it with `aida merge-hold clear {pr}`"
        ),
        Gate::HoldLabels => format!(
            "`aida merge-hold list` shows the label state; `aida merge-hold clear {pr}` \
             records the clearance"
        ),
        Gate::ReviewClaim => "wait for the running review to record its verdict".into(),
        Gate::Head => "check again; the head branch could not be fetched".into(),
        Gate::Approval => {
            format!("get an approval recorded at the current head: `aida pr review {pr}`")
        }
        Gate::FullCi => "wait for full-tier CI on this head to finish green".into(),
        Gate::Corpus => format!(
            "address the blocking verdict, then get a fresh approval at the head: \
             `aida pr review {pr}`"
        ),
        Gate::Message => "check again; the PR's title and body could not be read".into(),
        Gate::SpecPolicy => {
            format!("a human lands it from a terminal: `aida pr train {pr}` or `aida pr ship {pr}`")
        }
        Gate::Mandate => {
            format!("address the reported mandate refusal, then rerun `aida pr train --check {pr}`")
        }
    }
}

fn refused(pr: u64, gate: Gate, reason: String, evidence: Evidence) -> Row {
    Row {
        pr,
        eligible: false,
        head: None,
        gate: Some(gate_token(gate)),
        remedy: Some(match evidence {
            Evidence::Read => remedy(gate, pr),
            _ => "check again; the evidence for this step could not be read".into(),
        }),
        reason: Some(reason),
        evidence,
    }
}

/// One PR through the train's admission: `rider` is the already-riding
/// refusal the train checks first, then the member gate.
pub(crate) fn check_one(
    checks: &mut dyn MemberChecks,
    base: &str,
    pr: u64,
    rider: std::result::Result<Option<String>, String>,
) -> Row {
    match rider {
        Ok(None) => {}
        Ok(Some(reason)) => return refused(pr, Gate::Riding, reason, Evidence::Read),
        Err(why) => return refused(pr, Gate::Riding, why, Evidence::Unavailable),
    }
    match typed_gate_member_with(checks, base, pr) {
        // A red train's culprit at an unchanged head. trace:BUG-2767 | ai:claude
        Ok(member) => match culprit::hold(checks.marker_root(), pr, &member.head) {
            // trace:TASK-3923 | ai:claude
            Some(why) if why.starts_with(culprit::FIX_FIRST) => Row {
                pr,
                eligible: false,
                head: None,
                gate: Some("fix-first"),
                reason: Some(why),
                remedy: Some("push the rework as a new head; review then applies to it".into()),
                evidence: Evidence::Read,
            },
            Some(why) => Row {
                pr,
                eligible: false,
                head: None,
                gate: Some("culprit-hold"),
                reason: Some(why),
                remedy: Some(format!("push a new head, or name it: `aida pr train {pr}`")),
                evidence: Evidence::Read,
            },
            None => Row {
                pr,
                eligible: true,
                head: Some(member.head),
                gate: None,
                reason: None,
                remedy: None,
                evidence: Evidence::Read,
            },
        },
        Err(MemberGateError::Refused(reason)) => refused(pr, last_gate(), reason, Evidence::Read),
        Err(MemberGateError::Unavailable(reason)) => {
            refused(pr, last_gate(), reason, Evidence::Unavailable)
        }
    }
}

fn not_checked(pr: u64) -> Row {
    Row {
        pr,
        eligible: false,
        head: None,
        gate: None,
        reason: Some("not checked: the time budget ran out".into()),
        remedy: Some("check again with fewer PRs or a longer --timeout".into()),
        evidence: Evidence::NotChecked,
    }
}

/// Every PR in order, each once. Discard a verdict if its check used up the budget.
// trace:TASK-3792 | ai:codex
pub(crate) fn check_all(
    prs: &[u64],
    mut expired: impl FnMut() -> bool,
    mut one: impl FnMut(u64) -> Row,
) -> Vec<Row> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out_of_time = false;
    prs.iter()
        .copied()
        .filter(|pr| seen.insert(*pr))
        .map(|pr| {
            out_of_time = out_of_time || expired();
            if out_of_time {
                return not_checked(pr);
            }
            let row = one(pr);
            out_of_time = expired();
            if out_of_time {
                not_checked(pr)
            } else {
                row
            }
        })
        .collect()
}

/// The report as text: one line per PR.
pub(crate) fn render_text(base: Option<&str>, rows: &[Row]) -> String {
    let mut out = format!(
        "train eligibility on {} ({} PR(s); nothing was changed)\n",
        base.map_or_else(|| "an unknown base".into(), |base| format!("`{base}`")),
        rows.len()
    );
    for row in rows {
        if row.eligible {
            out.push_str(&format!(
                "  #{}  yes  head {}\n",
                row.pr,
                short(row.head.as_deref().unwrap_or(""))
            ));
            continue;
        }
        let gate = row.gate.map(|g| format!("{g}: ")).unwrap_or_default();
        out.push_str(&format!(
            "  #{}  no   {gate}{} — fix: {}\n",
            row.pr,
            row.reason.as_deref().unwrap_or("").trim(),
            row.remedy.as_deref().unwrap_or("")
        ));
    }
    let eligible = rows.iter().filter(|r| r.eligible).count();
    out.push_str(&format!("{eligible} of {} eligible\n", rows.len()));
    out
}

/// The report as JSON.
pub(crate) fn render_json(base: Option<&str>, rows: &[Row]) -> serde_json::Value {
    serde_json::json!({
        "base": base,
        "eligible": rows.iter().all(|r| r.eligible),
        "prs": rows.iter().map(|r| serde_json::json!({
            "pr": r.pr,
            "eligible": r.eligible,
            "head": r.head,
            "gate": r.gate,
            "reason": r.reason,
            "remedy": r.remedy,
            "evidence": r.evidence.token(),
        })).collect::<Vec<_>>(),
    })
}

/// Refuse incomplete discovery before train reads or eligibility reporting.
// trace:STORY-2824 | ai:codex
pub(crate) fn all_open_candidates(root: &Path, base: &str) -> Result<Vec<u64>> {
    // The listing's own 15s ceiling, tightened to what is left of a scoped
    // `--timeout` budget. trace:TASK-3792 | ai:claude
    let ceiling =
        crate::forge_lookup::forge_lookup_deadline().map_or(Duration::from_secs(15), |deadline| {
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(15))
        });
    let rows = crate::forge::pr_state_batch(root, ceiling)?;
    let mut ids = Vec::new();
    for row in rows {
        let id = row["number"]
            .as_u64()
            .filter(|id| *id > 0)
            .context("open PR listing row has no valid number")?;
        let branch = row["headRefName"]
            .as_str()
            .filter(|s| !s.is_empty())
            .context("open PR listing row has no head branch")?;
        let target = row["baseRefName"]
            .as_str()
            .filter(|s| !s.is_empty())
            .context("open PR listing row has no base branch")?;
        anyhow::ensure!(
            row["state"].as_str() == Some("OPEN"),
            "open PR listing row has invalid state"
        );
        if target == base && !branch.starts_with(BRANCH_PREFIX) {
            ids.push(id);
        }
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

/// `aida pr train --check`. Fails unless every PR is eligible.
// trace:STORY-2824 | ai:claude
// trace:STORY-2824 | ai:codex
pub(crate) fn pr_train_check_handler(opts: &CheckOptions<'_>) -> Result<()> {
    with_skip_full_check(opts.skip_full_check, || {
        #[cfg(test)]
        if let Some(probe) = TEST_CHECK_DISPATCH.with(|hook| hook.take()) {
            return probe(opts);
        }
        check_report(opts, || {
            let project_root = crate::find_project_root()?;
            let main_worktree = crate::main_worktree_root_from(&project_root);
            Ok((project_root, main_worktree))
        })
    })
}

fn expired() -> bool {
    crate::forge_lookup::forge_lookup_deadline().is_some_and(|deadline| Instant::now() >= deadline)
}

// trace:TASK-3792 | ai:codex
fn check_report(
    opts: &CheckOptions<'_>,
    roots: impl FnOnce() -> Result<(PathBuf, PathBuf)>,
) -> Result<()> {
    let mut main = None;
    let (base, rows) = collect_report(opts, || {
        let roots = roots()?;
        main = Some(roots.1.clone());
        Ok(roots)
    })?;
    // Manual trains the auto-train yields its next train to.
    // trace:BUG-2771 TASK-3924 | ai:claude
    let waiters = main
        .as_deref()
        .map(crate::admission_owner::waiter_lines)
        .unwrap_or_default();
    print!(
        "{}",
        render_check(base.as_deref(), &rows, &waiters, opts.json)?
    );
    let not_eligible = rows.iter().filter(|r| !r.eligible).count();
    anyhow::ensure!(
        not_eligible == 0,
        "{not_eligible} of {} PR(s) would not ride a train right now",
        rows.len()
    );
    Ok(())
}

/// `--check`'s output: the report plus one `admission:` line (text) or a
/// `manual_waiters` array (JSON, only when any wait). trace:TASK-3924 | ai:claude
fn render_check(
    base: Option<&str>,
    rows: &[Row],
    waiters: &[String],
    json: bool,
) -> Result<String> {
    if json {
        let mut value = render_json(base, rows);
        if !waiters.is_empty() {
            value["manual_waiters"] = serde_json::json!(waiters);
        }
        return Ok(format!("{}\n", serde_json::to_string_pretty(&value)?));
    }
    let mut text = render_text(base, rows);
    for line in waiters {
        text.push_str(&format!("admission: {line}\n"));
    }
    Ok(text)
}

#[cfg(test)]
pub(crate) fn check_at(
    opts: &CheckOptions<'_>,
    project_root: &Path,
    main_worktree: &Path,
) -> Result<()> {
    check_report(opts, || {
        Ok((project_root.to_owned(), main_worktree.to_owned()))
    })
}

pub(crate) fn collect_report(
    opts: &CheckOptions<'_>,
    roots: impl FnOnce() -> Result<(PathBuf, PathBuf)>,
) -> Result<(Option<String>, Vec<Row>)> {
    let (base, rows, _) = collect_checks(opts, roots, false)?;
    Ok((base, rows))
}

type Checks = (
    Option<String>,
    Vec<Row>,
    std::collections::BTreeMap<u64, ActorProbe>,
);

// Every caller, the next-actor probe included, collects under the deadline
// scope. trace:TASK-3792 STORY-2893 | ai:claude
fn collect_checks(
    opts: &CheckOptions<'_>,
    roots: impl FnOnce() -> Result<(PathBuf, PathBuf)>,
    observe_actors: bool,
) -> Result<Checks> {
    crate::forge_lookup::with_forge_lookup_deadline(opts.timeout, || {
        collect_before_deadline(opts, roots, observe_actors)
    })
}

fn collect_before_deadline(
    opts: &CheckOptions<'_>,
    roots: impl FnOnce() -> Result<(PathBuf, PathBuf)>,
    observe_actors: bool,
) -> Result<Checks> {
    let mut base = None;
    let mut probes = std::collections::BTreeMap::new();
    let mut candidates = (!opts.all_open).then(|| opts.prs.to_vec());
    let result: Result<Vec<Row>> = (|| {
        anyhow::ensure!(!expired(), "train discovery timed out");
        let (project_root, main_worktree) = roots()?;
        // Root and origin reads run under the deadline too: one it cut off
        // fell back to a guess, which must not steer the rest of the check.
        // trace:TASK-3792 | ai:claude
        anyhow::ensure!(!expired(), "worktree discovery timed out");
        let (project_root, main_worktree) = (project_root.as_path(), main_worktree.as_path());
        let kind = crate::forge::peek_forge_kind(project_root);
        anyhow::ensure!(!expired(), "forge discovery timed out");
        if let Some(why) = forge_refusal(kind) {
            anyhow::bail!(why);
        }
        let forge = crate::forge::forge_for_kind(project_root, kind);
        let protected =
            crate::gate::review_carryover::protected_base(main_worktree).map_err(|e| {
                anyhow::anyhow!("could not read origin's default branch: {}", e.reason())
            })?;
        let base = base.insert(protected).as_str();
        if opts.all_open {
            let ids = all_open_candidates(project_root, base)?;
            anyhow::ensure!(!expired(), "open PR discovery timed out");
            candidates = Some(ids);
        }
        let (open_trains, leased_trains, _abandoned) = live_trains(main_worktree)?;
        anyhow::ensure!(!expired(), "live train discovery timed out");
        let ids = candidates
            .as_deref()
            .context("open PR discovery is incomplete")?;
        Ok(check_all(ids, expired, |pr| {
            let rider = ownership_rider_refusal(main_worktree, pr, true, forge.as_ref())
                .map(|own| own.or_else(|| train_rider_refusal(pr, &open_trains, &leased_trains)))
                .map_err(|e| format!("its train ownership could not be read ({e:#})"));
            let mut checks = LiveChecks {
                project_root,
                main_worktree,
                forge: forge.as_ref(),
                base: &base,
                named: !opts.all_open,
                read_only: true,
                actor_probe: observe_actors.then(ActorProbe::default),
                request_full: false,
                released_member: None,
            };
            let row = check_one(&mut checks, &base, pr, rider);
            if let Some(probe) = checks.actor_probe {
                probes.insert(pr, probe);
            }
            row
        }))
    })();
    let rows = match result {
        Err(error) if expired() => {
            let ids = candidates.with_context(|| {
                format!("train check timed out before open PR discovery completed: {error:#}")
            })?;
            anyhow::ensure!(
                !ids.is_empty(),
                "train check discovery timed out: {error:#}"
            );
            check_all(&ids, || true, not_checked)
        }
        other => other?,
    };
    Ok((base, rows, probes))
}

// trace:STORY-2893 | ai:codex
#[derive(Default)]
pub(super) struct ActorProbe {
    pub(super) head: Option<String>,
    pub(super) mandate_refusal: Option<MandateRefusal>,
    pub(super) human_clearance_label: bool,
}

#[derive(serde::Serialize)]
pub(crate) struct NextActor {
    pub actor: &'static str,
    pub gate: &'static str,
    pub reason: String,
    pub head: Option<String>,
}

impl NextActor {
    pub(crate) fn unverified(reason: &str) -> Self {
        Self {
            actor: "unverified",
            gate: "train-check",
            reason: reason.into(),
            head: None,
        }
    }
}

fn mandate_actor(refusal: &MandateRefusal) -> &'static str {
    use MandateRefusal::*;
    match refusal {
        AlwaysHuman { .. }
        | Excluded { .. }
        | HumanGated { .. }
        | TouchesGates(_)
        | SupervisedNotIncluded { .. }
        | ScopeNotCovered { .. } => "operator",
        HoldNotReleasable { kind } if matches!(kind.as_str(), "recusal" | "untyped legacy") => {
            "operator"
        }
        Approval(_) | StaleApproval(_) | ApproverIsShipper | SoloSelfApproval => "reviewer",
        NotAdvisor { .. }
        | PrincipalMismatch { .. }
        | AdvisorAuthored { .. }
        | ShipperUnanchored => "advisor",
        _ => "unverified",
    }
}

pub(in crate::pr_train) fn next_actor(row: Row, probe: ActorProbe) -> NextActor {
    let (actor, gate) = match (row.evidence, row.eligible, row.gate) {
        (Evidence::Read, true, _) => ("train", "train-eligible"),
        (Evidence::Read, false, Some(gate)) => {
            let actor = match gate {
                "riding-train" => "train",
                "live-drive" | "review-in-progress" | "approval" | "verdict-corpus" => "reviewer",
                "full-ci" => "CI",
                "mandate" => probe
                    .mandate_refusal
                    .as_ref()
                    .map_or("unverified", mandate_actor),
                "spec-policy" => "operator",
                "hold-labels" if probe.human_clearance_label => "operator",
                _ => "unverified",
            };
            (actor, gate)
        }
        (_, _, gate) => ("unverified", gate.unwrap_or("train-check")),
    };
    NextActor {
        actor,
        gate,
        reason: row
            .reason
            .unwrap_or_else(|| "train admission passed".into()),
        head: probe.head,
    }
}

pub(crate) fn next_actors(
    project_root: &Path,
    prs: &[u64],
) -> Result<std::collections::BTreeMap<u64, NextActor>> {
    if prs.is_empty() {
        return Ok(Default::default());
    }
    let main_worktree = crate::main_worktree_root_from(project_root);
    let opts = CheckOptions {
        prs,
        all_open: false,
        json: false,
        skip_full_check: false,
        timeout: Duration::from_secs(9),
    };
    let (_, rows, mut probes) =
        collect_checks(&opts, || Ok((project_root.to_owned(), main_worktree)), true)?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let pr = row.pr;
            (pr, next_actor(row, probes.remove(&pr).unwrap_or_default()))
        })
        .collect())
}

// Exercise command dispatch and its scoped override without live forge I/O.
// trace:STORY-2824 | ai:codex
#[cfg(test)]
thread_local! {
    pub(crate) static TEST_CHECK_DISPATCH: std::cell::Cell<Option<fn(&CheckOptions<'_>) -> Result<()>>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
mod task3924_waiter_tests {
    use super::*;

    /// TASK-3924 AC3: `--check` appends each queued manual train as an
    /// `admission:` line (text) and a `manual_waiters` array (JSON), and
    /// adds nothing when none wait. trace:TASK-3924 trace:TASK-3924.AC3 | ai:claude
    #[test]
    fn task3924_check_report_shows_waiters_in_text_and_json() {
        let temp = tempfile::tempdir().unwrap();
        let id = crate::admission_owner::record_test_waiter(temp.path(), &[4464]);
        let waiters = crate::admission_owner::waiter_lines(temp.path());
        assert_eq!(waiters.len(), 1);
        let text = render_check(None, &[], &waiters, false).unwrap();
        let line = text.lines().last().unwrap();
        assert!(
            line.starts_with("admission: operator train for #4464 waiting since "),
            "{text}"
        );
        assert!(line.contains(&format!("release-waiter {id}")), "{text}");
        let json: serde_json::Value =
            serde_json::from_str(&render_check(None, &[], &waiters, true).unwrap()).unwrap();
        assert_eq!(json["manual_waiters"][0], waiters[0]);
        assert!(render_check(None, &[], &[], false)
            .unwrap()
            .find("admission:")
            .is_none());
        let json: serde_json::Value =
            serde_json::from_str(&render_check(None, &[], &[], true).unwrap()).unwrap();
        assert!(json.get("manual_waiters").is_none(), "{json}");
    }
}
