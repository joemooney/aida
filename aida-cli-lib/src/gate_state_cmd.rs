//! `aida gate state PR-N [--compare] [--json]`: the diagnostic surface over
//! [`crate::gate::effective`]. Presentation and I/O only — the state itself
//! is the pure `effective_gate_state`. Decides nothing.
// trace:STORY-2324 | ai:claude

use anyhow::{Context, Result};

use crate::gate::effective::{self, Decision, GateState, Hold, Mode, Refusal, Verdict};
use crate::gate::ledger::{ChangeForge, ChangeKey};

/// What today's merge gates decide for the same change, read through their
/// own readers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Today {
    pub verdict: String,
    pub hold_open: bool,
    pub supervised: Vec<(String, String)>,
    pub draft_only: Vec<String>,
}

/// One compared column.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Column {
    pub column: &'static str,
    pub today: String,
    pub canonical: String,
    pub differs: bool,
}

fn parse_change(raw: &str) -> Result<ChangeKey> {
    let key = ChangeKey::parse(raw)
        .or_else(|| raw.trim().parse::<u64>().ok().map(ChangeKey::pr))
        .filter(|k| k.number > 0)
        .with_context(|| format!("{raw:?} is not a pull request (PR-<N> or <N>)"))?;
    if key.forge != ChangeForge::Pr {
        anyhow::bail!("aida gate state reads pull requests (PR-<N>)");
    }
    Ok(key)
}

/// The category a verdict compares on.
pub(crate) fn verdict_category(v: &Verdict) -> &'static str {
    match v {
        Verdict::Absent => "absent",
        Verdict::Approved { .. } => "approved",
        Verdict::ChangesRequested { .. } => "changes-requested",
        Verdict::Stale { .. } => "stale",
        Verdict::Unrecognized { .. } => "unrecognized",
        Verdict::Unsafe { .. } => "unsafe",
    }
}

fn join_pairs(pairs: &[(String, String)]) -> String {
    if pairs.is_empty() {
        return "none".into();
    }
    pairs
        .iter()
        .map(|(id, mode)| format!("{id} ({mode})"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether two requirement-ID lists name the same set (order and ID case
/// ignored), so a draft-only member swapped for another is a diff.
fn same_ids(a: &[String], b: &[String]) -> bool {
    let set = |ids: &[String]| {
        ids.iter()
            .map(|id| id.trim().to_ascii_uppercase())
            .collect::<std::collections::BTreeSet<_>>()
    };
    set(a) == set(b)
}

fn join(ids: &[String]) -> String {
    if ids.is_empty() {
        "none".into()
    } else {
        ids.join(", ")
    }
}

/// The column-by-column comparison of today's answer and the canonical one.
// trace:STORY-2324 | ai:claude
pub(crate) fn compare(today: &Today, state: &GateState) -> Vec<Column> {
    let canonical_supervised = match &state.mode {
        Mode::Drain => Vec::new(),
        Mode::Supervised { members } => members.clone(),
    };
    let mut sorted_today = today.supervised.clone();
    sorted_today.sort();
    let mut sorted_new = canonical_supervised.clone();
    sorted_new.sort();
    let hold_open = matches!(state.hold, Hold::Open { .. });
    let col = |column, today: String, canonical: String, differs| Column {
        column,
        today,
        canonical,
        differs,
    };
    vec![
        col(
            "verdict",
            today.verdict.clone(),
            verdict_category(&state.verdict).to_string(),
            today.verdict != verdict_category(&state.verdict),
        ),
        col(
            "hold",
            if today.hold_open { "open" } else { "none" }.into(),
            if hold_open { "open" } else { "none" }.into(),
            today.hold_open != hold_open,
        ),
        col(
            "supervised",
            join_pairs(&today.supervised),
            join_pairs(&canonical_supervised),
            sorted_today.len() != sorted_new.len()
                || sorted_today
                    .iter()
                    .zip(&sorted_new)
                    .any(|(a, b)| !a.0.eq_ignore_ascii_case(&b.0) || a.1 != b.1),
        ),
        col(
            "draft-only",
            join(&today.draft_only),
            join(&state.draft_only),
            !same_ids(&today.draft_only, &state.draft_only),
        ),
    ]
}

/// How today's TASK-1960 carry-over answered a head refusal: `None` when it
/// does not apply (not a moved head), `Some(true)` carried over, `Some(false)`
/// refused.
pub(crate) type CarryAnswer = Option<bool>;

/// Today's verdict column over the merge-gate candidates, exactly as the
/// merge paths decide it: an open refusal among the candidates outranks a
/// stale approval (ship refuses a refused PR whatever its approval covers),
/// then the approval-covers-head gate with the carry-over applied
/// (`approval.rs` `pr_ship_approval_gate` / `merge_path_approval_decision`
/// via `carry_over_for_refusal`). Pure; `carry` is the carry-over read.
// trace:STORY-2324 | ai:claude
pub(crate) fn today_verdict(
    candidates: &[crate::gate::review_verdict::RecordedVerdict],
    head: &str,
    carry: impl FnOnce(&crate::gate::ship::ApprovalHeadRefusal) -> CarryAnswer,
) -> &'static str {
    use crate::gate::ship;
    let approved = candidates
        .iter()
        .any(|v| v.kind.approves() && !v.is_closed());
    let refused = candidates
        .iter()
        .any(|v| v.kind.blocks_done() && !v.is_closed());
    if refused {
        return "changes-requested";
    }
    match ship::approval_head_refusal(candidates, Some(head)) {
        // An implementer-seat approval is refused by today's authority
        // check, not by its head: its own category, never `stale`. The
        // canonical state leaves authority to the later stage, so this
        // column shows as an expected diff.
        Some(ship::ApprovalHeadRefusal::Unauthorized { .. }) => "unauthorized",
        Some(refusal) => match carry(&refusal) {
            Some(true) => "approved",
            Some(false) | None => "stale",
        },
        None if approved => "approved",
        None => "absent",
    }
}

/// Today's answer, read through the readers ship and the train use now.
/// The carry-over is read through a read-only site: a stored ledger answer,
/// else a proof that leaves no state behind.
fn today_answer(
    project_root: &std::path::Path,
    main: &std::path::Path,
    pr: u64,
    ids: &[String],
    head: &str,
    base: &str,
) -> Today {
    use crate::gate::ship;
    let candidates =
        ship::merge_gate_verdict_candidates(&[project_root, main], pr, ids, Some(head));
    let base = Some(base.trim()).filter(|b| !b.is_empty());
    let site = crate::gate::review_carryover::CarryOverSite::read_only(main, base);
    let verdict = today_verdict(&candidates, head, |refusal| {
        crate::gate::review_carryover::carry_over_for_refusal(&site, pr, refusal).map(|r| r.is_ok())
    });
    let records = crate::pr_cmd::ship_gate_spec_records_for_ids(main, ids.to_vec());
    let modes: Vec<(String, Option<aida_core::ExecutionMode>)> = records
        .iter()
        .map(|(id, _, mode)| (id.clone(), *mode))
        .collect();
    Today {
        verdict: verdict.to_string(),
        hold_open: crate::gate::ledger::hold::state(main, pr).open.is_some(),
        supervised: ship::supervised_merge_holds(&modes, false),
        draft_only: crate::pr_cmd::draft_only_specs_in(&records),
    }
}

fn describe_verdict(v: &Verdict) -> String {
    match v {
        Verdict::Absent => "absent".into(),
        Verdict::Approved {
            seq,
            reviewed_sha,
            carried_over,
        } => format!(
            "approved at {} (event {seq}{})",
            short(reviewed_sha),
            if *carried_over { ", carried over" } else { "" }
        ),
        Verdict::ChangesRequested { seq, findings } => {
            format!(
                "changes requested (event {seq}, {} findings)",
                findings.len()
            )
        }
        Verdict::Stale {
            seq,
            reviewed,
            reason,
            ..
        } => format!(
            "stale: {reason} (event {seq}, reviewed {})",
            reviewed.as_deref().map(short).unwrap_or("none")
        ),
        Verdict::Unrecognized { seq, raw } => format!("unrecognized verdict {raw:?} (event {seq})"),
        Verdict::Unsafe { reason } => format!("unsafe evidence: {reason}"),
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

fn describe_refusal(r: &Refusal) -> String {
    match r {
        Refusal::Unresolved { labels } => {
            format!("cannot prove what this PR ships: {}", labels.join(", "))
        }
        Refusal::UnsafeEvidence { reason } => format!("unsafe evidence: {reason}"),
        Refusal::HoldOpen => "a merge hold is open".into(),
        Refusal::ChangesRequested => "changes requested".into(),
        Refusal::UnrecognizedVerdict => "an unrecognized verdict answers".into(),
        Refusal::StaleApproval => "the approval does not cover the head".into(),
        Refusal::NoApproval => "no approval recorded".into(),
        Refusal::Supervised { members } => {
            format!("supervised: {}", join_pairs(members))
        }
        Refusal::DraftOnly { members } => format!("draft-only: {}", members.join(", ")),
    }
}

/// The human rendering.
// trace:STORY-2324 | ai:claude
pub(crate) fn render(
    change: ChangeKey,
    store_head: &str,
    set: &crate::gate::canon::CanonicalSet,
    state: &GateState,
    columns: Option<&[Column]>,
) -> String {
    let mut out = format!(
        "{} at {} (requirements store at {})\n",
        change.as_key(),
        short(&state.head),
        short(store_head)
    );
    out.push_str("Requirements shipped:\n");
    if state.members.is_empty() {
        out.push_str("  none resolved\n");
    }
    for m in &state.members {
        out.push_str(&format!(
            "  {:<24} mode {:<20} {}\n",
            m.native,
            m.mode,
            describe_verdict(&m.verdict)
        ));
    }
    for label in &set.unresolved {
        out.push_str(&format!("  {label:<24} names no requirement\n"));
    }
    for (label, why) in &set.ambiguous {
        out.push_str(&format!("  {label:<24} ambiguous: {why}\n"));
    }
    if !state.foreign_labels.is_empty() {
        out.push_str(&format!(
            "Verdicts on this PR for labels it does not ship: {} ({})\n",
            state.foreign_labels.join(", "),
            match &state.foreign_verdict {
                Verdict::Absent => "none blocks".to_string(),
                v => format!("blocks: {}", describe_verdict(v)),
            }
        ));
    }
    out.push_str(&format!(
        "PR-level verdict: {}\n",
        describe_verdict(&state.change_verdict)
    ));
    out.push_str(&format!("Verdict:  {}\n", describe_verdict(&state.verdict)));
    out.push_str(&format!(
        "Hold:     {}\n",
        match &state.hold {
            Hold::None => "none".to_string(),
            Hold::Released => "none (last hold released)".to_string(),
            Hold::Open {
                reason_kind,
                detail,
                authorized,
                ..
            } => format!(
                "open ({reason_kind}{}): {detail}",
                if *authorized {
                    ", release authorized"
                } else {
                    ""
                }
            ),
        }
    ));
    out.push_str(&format!(
        "Mode:     {}\n",
        match &state.mode {
            Mode::Drain => "drain".to_string(),
            Mode::Supervised { members } => format!("supervised: {}", join_pairs(members)),
        }
    ));
    out.push_str(&format!(
        "Decision (unattended merge): {}\n",
        match &state.decision {
            Decision::Mergeable { pin } => format!("mergeable, pinned to {}", short(pin)),
            Decision::Refused { refusals } => format!(
                "refused — {}",
                refusals
                    .iter()
                    .map(describe_refusal)
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        }
    ));
    if let Some(columns) = columns {
        out.push_str("\nCompared with today's merge gates (today | canonical):\n");
        for c in columns {
            out.push_str(&format!(
                "  {} {:<11} {} | {}\n",
                if c.differs { "≠" } else { " " },
                c.column,
                c.today,
                c.canonical
            ));
        }
        let n = columns.iter().filter(|c| c.differs).count();
        out.push_str(&format!("  {n} difference(s)\n"));
    }
    out.push_str("Diagnostic only: no merge gate reads this yet.\n");
    out
}

/// `aida gate state`.
// trace:STORY-2324 | ai:claude
pub(crate) fn run_state(raw: &str, with_compare: bool, json: bool) -> Result<()> {
    let change = parse_change(raw)?;
    let pr = change.number;
    let project_root = crate::find_project_root()?;
    let main = crate::main_worktree_root_from(&project_root);
    let info = crate::pr_cmd::fetch_change_info_via_resolved_forge(
        &main,
        pr,
        crate::forge::resolve_open_change_forge_kind(&main),
    )?;
    let head = info.head_oid.trim().to_string();
    let (ids, _branch, _forge_resolved, _authors) =
        crate::pr_cmd::pr_gate_subject(&main, pr, &[], Some(&info.head_ref));
    let loaded = effective::load(&main, change, &ids)?;
    let state = effective::effective_gate_state(
        &head,
        &loaded.set,
        &loaded.view,
        &effective::GatePolicy::default(),
    );
    let columns = with_compare.then(|| {
        let today = today_answer(&project_root, &main, pr, &ids, &head, &info.base_ref);
        (compare(&today, &state), today)
    });
    if json {
        let body = serde_json::json!({
            "change": change.as_key(),
            "store_head": loaded.store_head,
            "subject_labels": ids,
            "unresolved": loaded.set.unresolved,
            "ambiguous": loaded.set.ambiguous,
            "state": state,
            "compare": columns.as_ref().map(|(c, today)| serde_json::json!({
                "today": today,
                "columns": c,
                "differences": c.iter().filter(|c| c.differs).count(),
            })),
        });
        println!("{}", serde_json::to_string_pretty(&body)?);
    } else {
        print!(
            "{}",
            render(
                change,
                &loaded.store_head,
                &loaded.set,
                &state,
                columns.as_ref().map(|(c, _)| c.as_slice())
            )
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/story_2324_gate_state_cmd_tests.rs"]
mod tests;
