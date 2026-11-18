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
    /// Stop starting new PR checks after this long.
    pub timeout: Duration,
}

/// How far a PR's check got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Evidence {
    /// The gate reached a verdict.
    Read,
    /// A read failed; the PR is reported not eligible, fail closed.
    Unavailable,
    /// The time budget ran out before this PR's turn.
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
            format!("`aida pr train {pr} --dry-run` asks the mandate and reports its answer")
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
        Ok(member) => Row {
            pr,
            eligible: true,
            head: Some(member.head),
            gate: None,
            reason: None,
            remedy: None,
            evidence: Evidence::Read,
        },
        Err(MemberGateError::Refused(reason)) => refused(pr, last_gate(), reason, Evidence::Read),
        Err(MemberGateError::Unavailable(reason)) => {
            refused(pr, last_gate(), reason, Evidence::Unavailable)
        }
    }
}

/// Every PR in order, each once. A PR whose turn comes after `expired()`
/// first says so is reported unchecked instead of being gated, so the run
/// ends within the budget plus one PR's check.
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
                Row {
                    pr,
                    eligible: false,
                    head: None,
                    gate: None,
                    reason: Some("not checked: the time budget ran out".into()),
                    remedy: Some("check again with fewer PRs or a longer --timeout".into()),
                    evidence: Evidence::NotChecked,
                }
            } else {
                one(pr)
            }
        })
        .collect()
}

/// The report as text: one line per PR.
pub(crate) fn render_text(base: &str, rows: &[Row]) -> String {
    let mut out = format!(
        "train eligibility on `{base}` ({} PR(s); nothing was changed)\n",
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
pub(crate) fn render_json(base: &str, rows: &[Row]) -> serde_json::Value {
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
    let rows = crate::forge::pr_state_batch(root, Duration::from_secs(15))?;
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
        let project_root = crate::find_project_root()?;
        let main_worktree = crate::main_worktree_root_from(&project_root);
        check_at(opts, &project_root, &main_worktree)
    })
}

// trace:STORY-2824 | ai:codex
pub(crate) fn check_at(
    opts: &CheckOptions<'_>,
    project_root: &Path,
    main_worktree: &Path,
) -> Result<()> {
    let started = Instant::now();
    let kind = crate::forge::peek_forge_kind(project_root);
    if let Some(why) = forge_refusal(kind) {
        anyhow::bail!(why);
    }
    let forge = crate::forge::forge_for_kind(project_root, kind);
    let base = crate::gate::review_carryover::protected_base(main_worktree)
        .map_err(|e| anyhow::anyhow!("could not read origin's default branch: {}", e.reason()))?;
    let candidates: Vec<u64> = if opts.all_open {
        all_open_candidates(project_root, &base)?
    } else {
        opts.prs.to_vec()
    };
    // The trains a candidate may already ride. An abandoned train does not
    // block: the next `aida pr train` closes it.
    let (open_trains, leased_trains, _abandoned) = live_trains(main_worktree)?;
    let rows = check_all(
        &candidates,
        || started.elapsed() >= opts.timeout,
        |pr| {
            let rider = ownership_rider_refusal(main_worktree, pr, true, forge.as_ref())
                .map(|own| own.or_else(|| train_rider_refusal(pr, &open_trains, &leased_trains)))
                .map_err(|e| format!("its train ownership could not be read ({e:#})"));
            check_one(
                &mut LiveChecks {
                    project_root,
                    main_worktree,
                    forge: forge.as_ref(),
                    base: &base,
                    named: !opts.all_open,
                    read_only: true,
                    request_full: false,
                },
                &base,
                pr,
                rider,
            )
        },
    );
    if opts.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&render_json(&base, &rows))?
        );
    } else {
        print!("{}", render_text(&base, &rows));
    }
    let not_eligible = rows.iter().filter(|r| !r.eligible).count();
    anyhow::ensure!(
        not_eligible == 0,
        "{not_eligible} of {} PR(s) would not ride a train right now",
        rows.len()
    );
    Ok(())
}

// Exercise command dispatch and its scoped override without live forge I/O.
// trace:STORY-2824 | ai:codex
#[cfg(test)]
thread_local! {
    pub(crate) static TEST_CHECK_DISPATCH: std::cell::Cell<Option<fn(&CheckOptions<'_>) -> Result<()>>> = const { std::cell::Cell::new(None) };
}
