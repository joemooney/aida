//! Read-only PR derivation. Decisions describe work; they grant no authority.
// trace:STORY-2688 | ai:codex
use crate::forge_observed::Observed;
use crate::gate::review_verdict::VerdictKind;
use anyhow::{Context, Result};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) enum PrState {
    Gone,
    EvidencePending,
    Held,
    InFlight,
    NeedsRuling,
    Conflict,
    BehindBase,
    NeedsRework,
    RedCi,
    CiPendingThenVerdict,
    NeedsVerdict,
    ApprovedCiPending,
    ReadyForTrain,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ci {
    Green,
    Pending,
    Red,
}
#[derive(Debug, Clone)]
pub(crate) struct Verdict {
    pub head: String,
    pub kind: VerdictKind,
    pub identity: String,
    pub attested: bool,
}
#[derive(Debug, Clone, Default)]
pub(crate) struct Owners {
    pub advisor: Vec<String>,
    pub implementer: Vec<String>,
    pub reviewer: Vec<String>,
    pub train: Vec<String>,
}
#[derive(Debug, Clone)]
pub(crate) struct PrFacts {
    pub observed_at: String,
    pub project: String,
    pub number: u64,
    pub head: String,
    pub observed_head: String,
    pub base: String,
    pub workflow: Observed<String>,
    pub workflow_fresh: bool,
    pub open: Observed<bool>,
    pub mergeable: Observed<bool>,
    pub behind: bool,
    pub ci: Observed<Ci>,
    pub held: Observed<bool>,
    /// Accepted operation receipt, possibly for an earlier head; never a bare PID.
    pub operation_owner: Option<String>,
    pub unsafe_evidence: Option<String>,
    pub policy_ruling: Option<String>,
    pub verdict: Option<Verdict>,
    pub attempts: u8,
    pub retry_backoff: bool,
    pub retry_exhausted: bool,
    pub owners: Owners,
}
impl PrFacts {
    pub(crate) fn observe_operation(&mut self, owner: Option<String>) {
        let owner = owner.unwrap_or_default();
        self.operation_owner = Some(match self.operation_owner.take() {
            Some(previous) if previous != owner => String::new(),
            _ => owner,
        });
    }
}
#[derive(Debug, Clone, Serialize)]
pub(crate) struct PrDecision {
    pub state: PrState,
    pub next_action: &'static str,
    pub owner: Option<String>,
    pub reason: String,
    pub evidence_key: String,
    pub attempt_count: u8,
}
fn unique(ids: &[String]) -> Option<String> {
    let ids: std::collections::BTreeSet<_> = ids.iter().filter(|s| !s.trim().is_empty()).collect();
    (ids.len() == 1).then(|| (*ids.first().unwrap()).clone())
}
/// First-match precedence from the signed sketch. No filesystem, clock or effects.
pub(crate) fn classify(f: PrFacts) -> PrDecision {
    use PrState::*;
    let verdict = f
        .verdict
        .as_ref()
        .filter(|v| v.head == f.head && v.kind != VerdictKind::Waiting);
    let approval = verdict.is_some_and(|v| v.kind == VerdictKind::Approved && v.attested);
    let ruling = f
        .unsafe_evidence
        .as_deref()
        .or(f.policy_ruling.as_deref())
        .or_else(|| {
            (f.retry_exhausted && verdict.is_none()).then_some("review retry policy exhausted")
        })
        .or_else(|| {
            verdict
                .filter(|v| {
                    !v.attested || matches!(v.kind, VerdictKind::Unknown | VerdictKind::Rejected)
                })
                .map(|_| "judgment or unattested verdict")
        });
    let pending = [
        &f.open as &dyn PendingReason,
        &f.mergeable,
        &f.workflow,
        &f.ci,
        &f.held,
    ]
    .into_iter()
    .find_map(|v| v.pending_reason());
    let (mut state, mut action, mut owner, mut reason) = if f.open == Observed::Known(false) {
        (
            Gone,
            "retire confirmed reservations",
            Some("reconciler".into()),
            "confirmed closed or merged".into(),
        )
    } else if let Some(why) = pending {
        (
            EvidencePending,
            "reobserve",
            Some("reconciler".into()),
            why.to_string(),
        )
    } else if f.head.is_empty()
        || f.head != f.observed_head
        || (verdict.is_none() && f.retry_backoff)
    {
        (
            EvidencePending,
            "reobserve",
            Some("reconciler".into()),
            "moving head or review retry backoff".into(),
        )
    } else if f.held == Observed::Known(true) {
        (
            Held,
            "request authorized clearance",
            unique(&f.owners.advisor),
            "effective hold or punt".into(),
        )
    } else if let Some(owner) = &f.operation_owner {
        (
            InFlight,
            "observe recorded operation",
            Some(owner.clone()),
            "accepted operation or ambiguous receipt".into(),
        )
    } else if let Some(why) = ruling {
        (
            NeedsRuling,
            "request decision",
            unique(&f.owners.advisor),
            why.to_string(),
        )
    } else if f.mergeable == Observed::Known(false) {
        (
            Conflict,
            "queue conflict resolution",
            unique(&f.owners.implementer),
            "confirmed merge conflict".into(),
        )
    } else if f.behind || !f.workflow_fresh {
        (
            BehindBase,
            "request guarded branch update",
            unique(&f.owners.implementer),
            "base or workflow is stale".into(),
        )
    } else if verdict.is_some_and(|v| v.kind == VerdictKind::RequestChanges) {
        (
            NeedsRework,
            "enqueue findings-bound rework",
            unique(&f.owners.implementer),
            "current-head findings".into(),
        )
    } else if f.ci == Observed::Known(Ci::Red) {
        (
            RedCi,
            "route CI diagnosis",
            unique(&f.owners.implementer),
            "blocking CI failure".into(),
        )
    } else if !approval && f.ci == Observed::Known(Ci::Pending) {
        (
            CiPendingThenVerdict,
            "dispatch review now",
            unique(&f.owners.reviewer),
            "no current-head verdict; CI pending".into(),
        )
    } else if !approval {
        (
            NeedsVerdict,
            "dispatch review",
            unique(&f.owners.reviewer),
            "no current-head verdict; CI green".into(),
        )
    } else if f.ci == Observed::Known(Ci::Pending) {
        (
            ApprovedCiPending,
            "observe CI",
            Some("reconciler".into()),
            "current-head approval; CI pending".into(),
        )
    } else {
        (
            ReadyForTrain,
            "nominate exact head",
            unique(&f.owners.train),
            "approval, CI and freshness confirmed".into(),
        )
    };
    if owner.as_deref().is_none_or(|o| o.trim().is_empty()) {
        state = NeedsRuling;
        action = "request decision";
        owner = unique(&f.owners.advisor);
        reason = "missing or ambiguous operation/role owner".into();
    }
    if state == NeedsRuling && owner.is_none() {
        reason.push_str("; configuration refusal: advisor seat is missing or ambiguous");
    }
    // Include round identity and update target; this is diagnostic, never a ledger key.
    let evidence_key = serde_json::json!([
        f.project,
        f.number,
        f.head,
        f.base,
        f.workflow.as_known(),
        verdict.map(|v| &v.identity),
        action
    ])
    .to_string();
    PrDecision {
        state,
        next_action: action,
        owner,
        reason,
        evidence_key,
        attempt_count: f.attempts,
    }
}
trait PendingReason {
    fn pending_reason(&self) -> Option<&str>;
}
impl<T> PendingReason for Observed<T> {
    fn pending_reason(&self) -> Option<&str> {
        self.unsettled_reason()
    }
}

#[derive(clap::Args, Debug)]
pub(crate) struct StateArgs {}
pub(crate) fn run(_args: &StateArgs) -> Result<()> {
    let root = crate::find_main_worktree_root()?;
    let facts = collect(&root)?;
    let decisions: Vec<_> = facts
        .into_iter()
        .map(|facts| {
            let number = facts.number;
            let head = facts.head.clone();
            serde_json::json!({"number": number, "head": head, "observed_at": facts.observed_at,
                "base": facts.base, "workflow_digest": facts.workflow.as_known(),
                "verdict_head": facts.verdict.as_ref().map(|v| &v.head),
                "verdict_kind": facts.verdict.as_ref().map(|v| v.kind.label()), "decision": classify(facts.clone())})
        })
        .collect();
    if crate::output_format::output_format_is_json() {
        println!("{}", serde_json::to_string_pretty(&decisions)?);
    } else {
        println!("PR\tSTATE\tNEXT ACTION\tOWNER\tATTEMPTS\tREASON");
        for row in decisions {
            let d = &row["decision"];
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}",
                row["number"],
                d["state"].as_str().unwrap_or(""),
                d["next_action"].as_str().unwrap_or(""),
                d["owner"].as_str().unwrap_or("UNRESOLVED"),
                d["attempt_count"],
                d["reason"].as_str().unwrap_or("")
            );
        }
    }
    Ok(())
}

fn collect(root: &std::path::Path) -> Result<Vec<PrFacts>> {
    let started = std::time::Instant::now();
    let mut facts = collect_with(
        root,
        crate::forge::pr_state_batch,
        crate::forge::pr_state_workflows,
    )?;
    for f in &mut facts {
        if f.behind {
            continue;
        }
        if started.elapsed() >= std::time::Duration::from_secs(9) {
            f.mergeable = Observed::Pending("observation budget exhausted".into());
            continue;
        }
        let mut cmd = std::process::Command::new("git");
        cmd.current_dir(root)
            .args(["merge-base", "--is-ancestor", &f.base, &f.head]);
        match crate::command_output_with_timeout(cmd, std::time::Duration::from_millis(100))
            .and_then(|out| out.status.code())
        {
            Some(0) => f.workflow_fresh = true,
            Some(1) => f.behind = true,
            _ => {
                f.mergeable = Observed::Pending(
                    "base ancestry unavailable locally; no fetch in read-only collection".into(),
                )
            }
        }
    }
    Ok(facts)
}
fn collect_with(
    root: &std::path::Path,
    mut batch: impl FnMut(&std::path::Path, std::time::Duration) -> Result<Vec<serde_json::Value>>,
    workflows: impl FnOnce(
        &std::path::Path,
        &[serde_json::Value],
        std::time::Duration,
    ) -> Result<serde_json::Value>,
) -> Result<Vec<PrFacts>> {
    let start = std::time::Instant::now();
    let first = batch(root, std::time::Duration::from_secs(2))?;
    let owners = collect_owners(root);
    let agents = crate::agent_registry::load_live_identity_entries(root);
    let leases = crate::list_leases(root);
    let train = crate::pr_train::pr_state_members(root);
    let runtime = crate::review_watch::pr_state_snapshot(root)?;
    let required = crate::review_watch::required_checks(root)?;
    let config = crate::ci_gate::read_ci_gate_config(root);
    let workflows = if first.is_empty() {
        Ok(serde_json::json!({}))
    } else {
        workflows(root, &first, std::time::Duration::from_secs(2))
    };
    let mut facts = Vec::new();
    for (index, row) in first.iter().enumerate() {
        let mut required = required.clone();
        if let Ok(w) = &workflows {
            if let Some(names) = w[format!("r{index}")]["baseRef"]["branchProtectionRule"]
                ["requiredStatusCheckContexts"]
                .as_array()
            {
                required.extend(names.iter().filter_map(|v| v.as_str().map(str::to_owned)));
            }
        }
        let mut f = parse_row(row, &required, &config)?;
        if start.elapsed() >= std::time::Duration::from_millis(5500) {
            f.ci = Observed::Pending("local evidence observation budget exhausted".into());
            facts.push(f);
            continue;
        }
        f.owners = owners.clone();
        match &workflows {
            Ok(w) => {
                let head = w[format!("p{index}h")]["oid"].as_str();
                let base = w[format!("p{index}b")]["oid"].as_str();
                match (head, base) {
                    (Some(h), Some(b)) => {
                        f.workflow = Observed::Known(h.into());
                        f.workflow_fresh = h == b;
                    }
                    (None, None)
                        if w.get(format!("p{index}h")).is_some()
                            && w.get(format!("p{index}b")).is_some() =>
                    {
                        f.workflow = Observed::Known("absent".into());
                        f.workflow_fresh = true;
                    }
                    _ => f.workflow = Observed::Pending("workflow tree unavailable".into()),
                }
            }
            Err(e) => f.workflow = Observed::Transient(format!("workflow observation: {e}")),
        }
        crate::review_watch::apply_pr_state_snapshot(&runtime, &mut f);
        let key = format!("PR-{}", f.number);
        let mut specs = crate::extract_spec_ids_from_commit(row["title"].as_str().unwrap_or(""));
        specs.extend(crate::extract_spec_ids_from_commit(
            row["headRefName"].as_str().unwrap_or(""),
        ));
        specs.sort();
        specs.dedup();
        let branch = row["headRefName"].as_str().unwrap_or("");
        let branch_leases: Vec<_> = leases
            .iter()
            .filter(|l| l.branch == branch || l.scope == key)
            .collect();
        let implementers: Vec<_> = agents
            .iter()
            .filter(|a| {
                a.role.as_deref() == Some("implementer")
                    && (a.current_spec.as_ref().is_some_and(|id| specs.contains(id))
                        || branch_leases
                            .iter()
                            .any(|l| l.worktree_path == a.worktree_path))
            })
            .map(|a| a.id.clone())
            .collect();
        f.owners.implementer = implementers;
        for lease in &branch_leases {
            if let Some(holder) = crate::leases::lease_holder_record(lease) {
                match holder.observe() {
                    Ok(aida_core::proc_identity::Verdict::Dead(_)) => (),
                    _ => {
                        let ids: Vec<_> = agents
                            .iter()
                            .filter(|a| {
                                holder.identity().is_some()
                                    && a.process_identity().as_ref() == holder.identity()
                            })
                            .map(|a| a.id.clone())
                            .collect();
                        f.observe_operation(unique(&ids));
                    }
                }
            }
        }
        match &train {
            Ok(members) if members.contains(&f.number) => {
                f.observe_operation(unique(&f.owners.train))
            }
            Err(e) => f.unsafe_evidence = Some(format!("train receipts unreadable: {e}")),
            _ => (),
        }
        for spec in &specs {
            match (
                crate::punt::read_seat_signal(root, spec),
                crate::punt::read_seat_hold_signal(root, spec),
            ) {
                (Ok(None), Ok(None)) => (),
                (Ok(_), Ok(_)) => f.held = Observed::Known(true),
                _ => f.unsafe_evidence = Some("unreadable punt or hold evidence".into()),
            }
        }
        let path = crate::gate::review_verdict::verdict_path(root, &key);
        if crate::gate::review_verdict::verdict_file_refused(&path) {
            f.unsafe_evidence = Some("refused review evidence".into());
        } else if let Some(body) = crate::gate::review_verdict::current_verdict_body_at(&path) {
            if let Some(v) = crate::gate::review_verdict::parse_recorded_verdict(&body) {
                if v.reviewed_sha.as_deref().is_none_or(|sha| sha.is_empty()) {
                    f.unsafe_evidence = Some("review record has no head binding".into());
                }
                let attested = v
                    .attestation
                    .as_ref()
                    .is_some_and(|a| a.authorizes(v.reviewed_sha.as_deref()))
                    && v.attestation
                        .as_ref()
                        .and_then(|a| a.authority_key(v.recorded_at.as_deref()))
                        .is_some_and(|key| {
                            crate::gate::review_authority::verified_authority_keys(
                                root,
                                std::slice::from_ref(&v),
                            )
                            .contains_key(&key)
                        });
                let attested = if v.kind.approves()
                    && v.reviewed_sha.as_deref() == Some(f.head.as_str())
                {
                    let mut cmd = std::process::Command::new("git");
                    cmd.current_dir(root)
                        .args(["show", "-s", "--format=%cI", &f.head]);
                    let at = crate::command_output_with_timeout(
                        cmd,
                        std::time::Duration::from_millis(100),
                    )
                    .filter(|out| out.status.success())
                    .and_then(|out| String::from_utf8(out.stdout).ok())
                    .and_then(|text| chrono::DateTime::parse_from_rfc3339(text.trim()).ok())
                    .map(|at| at.with_timezone(&chrono::Utc));
                    if at.is_none() {
                        f.ci = Observed::Pending("reviewed commit date unavailable locally".into());
                    }
                    let candidates = std::slice::from_ref(&v);
                    let policy = crate::gate::ship::ShipApprovalPolicy {
                        author_sessions: crate::gate::review_authority::all_author_sessions(
                            root,
                            Some(branch),
                        ),
                        verified_authority: crate::gate::review_authority::verified_authority_keys(
                            root, candidates,
                        ),
                        reviewed_committed_at: at,
                        shipper: Vec::new(),
                        solo_human: false,
                        verified_solo: Default::default(),
                    };
                    crate::gate::ship::approval_authority_refusal(candidates, &policy).is_none()
                } else {
                    attested
                };
                f.verdict = Some(Verdict {
                    head: v.reviewed_sha.clone().unwrap_or_default(),
                    kind: v.kind,
                    identity: format!(
                        "{}:{:?}",
                        v.recorded_at.as_deref().unwrap_or(""),
                        v.recorded_by
                    ),
                    attested: attested && !v.is_closed(),
                });
            } else {
                f.unsafe_evidence = Some("malformed review evidence".into());
            }
        }
        // Existing read-only hold policy includes deleted-marker evidence.
        if let Some(why) = crate::gate::merge_hold_core::read_hold(root, f.number).or_else(|| {
            crate::gate::merge_hold_core::read_hold_record(root, f.number).map(|r| r.detail)
        }) {
            f.held = Observed::Known(true);
            f.policy_ruling.get_or_insert(why);
        }
        facts.push(f);
    }
    let remaining = std::time::Duration::from_secs(7).saturating_sub(start.elapsed());
    anyhow::ensure!(
        !remaining.is_zero(),
        "PR state collection exceeded observation budget"
    );
    match batch(root, remaining.min(std::time::Duration::from_secs(2))) {
        Ok(second) => {
            for f in &mut facts {
                let row = second
                    .iter()
                    .find(|r| r["number"].as_u64() == Some(f.number));
                f.observed_head = row
                    .and_then(|r| r["headRefOid"].as_str())
                    .unwrap_or("")
                    .to_string();
                if row.and_then(|r| r["baseRefOid"].as_str()) != Some(f.base.as_str()) {
                    f.workflow = Observed::Pending("base moved during collection".into());
                }
            }
        }
        Err(e) => {
            for f in &mut facts {
                f.open = Observed::Transient(format!("head recheck failed: {e}"));
            }
        }
    }
    Ok(facts)
}
fn collect_owners(root: &std::path::Path) -> Owners {
    let mut out = Owners::default();
    for entry in crate::agent_registry::load_live_identity_entries(root) {
        let list = match entry.role.as_deref() {
            Some("advisor") => &mut out.advisor,
            Some("implementer") => &mut out.implementer,
            Some("reviewer") => &mut out.reviewer,
            Some("integrator") => &mut out.train,
            _ => continue,
        };
        list.push(entry.id);
    }
    out
}
fn parse_row(
    row: &serde_json::Value,
    required: &[String],
    config: &crate::ci_gate::CiGateConfig,
) -> Result<PrFacts> {
    let string = |key: &str| {
        row[key]
            .as_str()
            .map(str::to_owned)
            .with_context(|| format!("missing {key} in PR snapshot"))
    };
    let head = string("headRefOid")?;
    let base = string("baseRefOid")?;
    let status = string("mergeStateStatus")?;
    let held = row["labels"]
        .as_array()
        .context("missing labels")?
        .iter()
        .any(|l| {
            matches!(
                l["name"].as_str(),
                Some("aida:merge-hold" | "aida:merge-hold-recorded")
            )
        });
    let checks = row["statusCheckRollup"]
        .as_array()
        .context("missing check rollup")?;
    let rows: Vec<_> = checks
        .iter()
        .map(|c| {
            let result = c["conclusion"]
                .as_str()
                .or_else(|| c["state"].as_str())
                .unwrap_or("");
            let bucket = match result {
                "SUCCESS" | "NEUTRAL" | "SKIPPED" => "pass",
                "FAILURE" | "ERROR" | "TIMED_OUT" | "ACTION_REQUIRED" | "CANCELLED" => "fail",
                _ => "pending",
            };
            crate::ci_gate::CheckRow {
                name: c["name"]
                    .as_str()
                    .or_else(|| c["context"].as_str())
                    .unwrap_or("")
                    .into(),
                workflow: c["workflowName"].as_str().unwrap_or("").into(),
                bucket: bucket.into(),
                run_id: None,
            }
        })
        .collect();
    let classified = crate::ci_gate::classify_red(&rows, required, config, held, held);
    let missing_required = required.iter().any(|n| {
        !rows
            .iter()
            .any(|c| crate::ci_gate::check_name_eq(n, &c.name))
    });
    let ci = if classified.is_real() {
        Ci::Red
    } else if checks.is_empty() || missing_required || classified.has_pending() {
        Ci::Pending
    } else {
        Ci::Green
    };
    Ok(PrFacts {
        observed_at: chrono::Utc::now().to_rfc3339(),
        project: string("url")?.split("/pull/").next().unwrap_or("").into(),
        number: row["number"].as_u64().context("missing PR number")?,
        observed_head: head.clone(),
        head,
        base: base.clone(),
        // Filled from the batched immutable workflow-tree observation before classification.
        workflow: if matches!(
            status.as_str(),
            "CLEAN" | "BEHIND" | "DIRTY" | "BLOCKED" | "UNSTABLE" | "HAS_HOOKS" | "DRAFT"
        ) {
            Observed::Known(base)
        } else {
            Observed::Pending("forge freshness has not settled".into())
        },
        workflow_fresh: status != "BEHIND",
        behind: status == "BEHIND",
        open: match string("state")?.as_str() {
            "OPEN" => Observed::Known(true),
            "CLOSED" | "MERGED" => Observed::Known(false),
            _ => Observed::Pending("lifecycle unknown".into()),
        },
        mergeable: match (status.as_str(), string("mergeable")?.as_str()) {
            ("UNKNOWN", _) => Observed::Pending("freshness unknown".into()),
            (_, "MERGEABLE") => Observed::Known(true),
            (_, "CONFLICTING") => Observed::Known(false),
            _ => Observed::Pending("mergeability unknown".into()),
        },
        ci: Observed::Known(ci),
        held: Observed::Known(held),
        operation_owner: None,
        unsafe_evidence: None,
        policy_ruling: row["isDraft"]
            .as_bool()
            .unwrap_or(true)
            .then(|| "draft or unsupported policy".into()),
        verdict: None,
        attempts: 0,
        retry_backoff: false,
        retry_exhausted: false,
        owners: Owners::default(),
    })
}

#[cfg(test)]
#[path = "pr_state_tests.rs"]
mod tests;
