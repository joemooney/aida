//! `aida merge-hold`/`merge-lock` handlers and stale-hold partitioning (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

pub(crate) fn slugify(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut last_dash = false;
    for c in lower.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Walk up from cwd looking for a .git directory — the project root. We
/// don't use git_ops::is_git_repo here because we need the *path*, not
/// just a yes/no.
// trace:STORY-1171 | ai:claude
pub(crate) fn handle_merge_lock_status(json: bool) -> Result<()> {
    let root = find_project_root()?;
    let leases = merge_lock::status(&root);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if json {
        let items: Vec<String> = leases
            .iter()
            .map(|(b, h)| {
                let pr = h.pr.map(|p| p.to_string()).unwrap_or_else(|| "null".to_string());
                format!(
                    "{{\"branch\":{b:?},\"pr\":{pr},\"host\":{:?},\"pid\":{},\"acquired_at\":{},\"age_secs\":{},\"note\":{:?}}}",
                    h.host, h.pid, h.acquired_at, now.saturating_sub(h.acquired_at), h.note
                )
            })
            .collect();
        println!("{{\"merge_locks\":[{}]}}", items.join(","));
        return Ok(());
    }
    if leases.is_empty() {
        println!("No active merge-leases.");
        return Ok(());
    }
    println!("Active merge-leases:");
    for (branch, h) in leases {
        let pr =
            h.pr.map(|p| p.to_string())
                .unwrap_or_else(|| "-".to_string());
        println!(
            "  {branch}  pr={pr}  host={}  pid={}  age={}s  {}",
            h.host,
            h.pid,
            now.saturating_sub(h.acquired_at),
            h.note
        );
    }
    Ok(())
}

pub(crate) type StaleHold = (u64, String, merge_hold::TerminalState);

/// Partition merge-hold markers into (stale, live) by a terminal-state
/// predicate: a marker whose PR can never merge (again) is stale — merged (a
/// phantom left by a merge AIDA did not perform, e.g. a raw `gh pr merge`) OR
/// closed without merging (BUG-1541). Pure so the list/sweep logic is
/// testable without a live forge.
// trace:TASK-161 trace:BUG-1541 | ai:claude
pub(crate) fn partition_stale_holds(
    holds: Vec<(u64, String)>,
    mut terminal_state: impl FnMut(u64) -> Option<merge_hold::TerminalState>,
) -> (Vec<StaleHold>, Vec<(u64, String)>) {
    let mut stale = Vec::new();
    let mut live = Vec::new();
    for (pr, reason) in holds {
        // Only a definite terminal state makes a marker stale; an unknown
        // (offline / gh error / unrecognised state word) is treated as live so
        // we never clear a hold we could not confirm is safe to drop.
        // trace:TASK-161 trace:BUG-1541 | ai:claude
        match terminal_state(pr) {
            Some(terminal) => stale.push((pr, reason, terminal)),
            None => live.push((pr, reason)),
        }
    }
    (stale, live)
}

/// The terminal state behind a marker, from one pinned forge read (or the
/// pure-git fallback). Fails CLOSED: anything but a definite merged/closed
/// answer is `None` (live).
// trace:BUG-1541 | ai:claude
pub(crate) fn hold_terminal_state(
    root: &std::path::Path,
    pr: u64,
    fetched: Option<&Result<Option<merge_hold::PinnedChange>, String>>,
) -> Option<merge_hold::TerminalState> {
    match fetched {
        Some(Ok(Some(change))) => change.state.terminal(),
        // pure-git: no forge repo to pin; keep the forge-routed check.
        Some(Ok(None)) => {
            let mut sink = network_retry::StderrSink;
            (pr_is_merged_with_sink(root, pr as u32, &mut sink) == Some(true))
                .then_some(merge_hold::TerminalState::Merged)
        }
        // unreadable / unpinnable: cannot confirm terminal → stays live.
        _ => None,
    }
}

#[cfg(test)]
mod merge_hold_cli_tests {
    use super::partition_stale_holds;
    use crate::merge_hold::TerminalState;

    #[test]
    fn stale_partition_splits_merged_from_open_and_unknown() {
        let holds = vec![
            (1u64, "drive".to_string()),
            (2u64, "drive".to_string()),
            (3u64, "drive".to_string()),
        ];
        // PR 1 merged -> stale; PR 2 still open -> live; PR 3 unknown (offline /
        // gh error) -> live, because an unconfirmed hold is never swept.
        let (stale, live) = partition_stale_holds(holds, |pr| match pr {
            1 => Some(TerminalState::Merged),
            _ => None,
        });
        assert_eq!(
            stale,
            vec![(1u64, "drive".to_string(), TerminalState::Merged)]
        );
        assert_eq!(
            live,
            vec![(2u64, "drive".to_string()), (3u64, "drive".to_string())]
        );
    }

    // BUG-1541: a PR closed WITHOUT merging is equally phantom — it can never
    // merge, so its hold releases nothing. It is swept and reported distinctly
    // from a merged one; an unknown forge state stays live (fail closed).
    // trace:BUG-1541 | ai:claude
    #[test]
    fn stale_partition_sweeps_closed_unmerged_and_fails_closed_on_unknown() {
        use crate::merge_hold::ChangeState;
        let states = [
            (10u64, ChangeState::parse(Some("MERGED"))),
            (11, ChangeState::parse(Some("CLOSED"))),
            (12, ChangeState::parse(Some("OPEN"))),
            (13, ChangeState::parse(None)),
            (14, ChangeState::parse(Some("weird"))),
            (15, ChangeState::parse(Some("closed"))), // GitLab spelling
            (16, ChangeState::parse(Some("opened"))),
        ];
        let holds = states
            .iter()
            .map(|(pr, _)| (*pr, "r".to_string()))
            .collect();
        let (stale, live) = partition_stale_holds(holds, |pr| {
            states
                .iter()
                .find(|(n, _)| *n == pr)
                .and_then(|(_, s)| s.terminal())
        });
        assert_eq!(
            stale.iter().map(|(pr, _, t)| (*pr, *t)).collect::<Vec<_>>(),
            vec![
                (10, TerminalState::Merged),
                (11, TerminalState::ClosedUnmerged),
                (15, TerminalState::ClosedUnmerged),
            ]
        );
        assert_eq!(
            live.iter().map(|(pr, _)| *pr).collect::<Vec<_>>(),
            vec![12, 13, 14, 16],
            "open and unknown states are never swept"
        );
        assert_eq!(TerminalState::Merged.describe(), "PR merged");
        assert_eq!(
            TerminalState::ClosedUnmerged.describe(),
            "PR closed without merging"
        );
    }
}

// `aida merge-hold list|clear` — the human surface for supervised merge-hold
// markers the docs + CI templates already reference. trace:TASK-161 | ai:claude
pub(crate) fn handle_merge_hold(action: &crate::cli::MergeHoldAction) -> Result<()> {
    // trace:BUG-1188 | ai:codex
    let root = find_main_worktree_root()?;
    match action {
        crate::cli::MergeHoldAction::Labels {
            create_missing,
            json,
        } => {
            let kind = forge::resolve_forge_kind(&root);
            if kind == forge::ForgeKind::None {
                print_merge_hold_no_forge(*json);
                return Ok(());
            }
            let definitions = merge_hold::label_definitions(&root);
            let missing = match definitions {
                merge_hold::LabelDefinitions::Read { missing, .. } => missing,
                merge_hold::LabelDefinitions::NoForge => {
                    print_merge_hold_no_forge(*json);
                    return Ok(());
                }
                merge_hold::LabelDefinitions::Unknown(detail) => {
                    if *json {
                        println!(
                            "{}",
                            serde_json::json!({"status":"unknown", "labels":{}, "error":detail})
                        );
                    } else {
                        println!("Could not determine merge-hold label definitions: {detail}");
                    }
                    anyhow::bail!("could not determine merge-hold label definitions");
                }
            };
            let pin = merge_hold::resolve_pinned_repo(&root, kind)
                .map_err(|err| anyhow::anyhow!(err))?
                .ok_or_else(|| anyhow::anyhow!("could not determine forge repository"))?;
            let mut statuses: std::collections::BTreeMap<String, String> =
                merge_hold::MERGE_HOLD_LABELS
                    .iter()
                    .map(|label| {
                        (
                            label.name.to_string(),
                            if missing.contains(&label.name) {
                                "missing".to_string()
                            } else {
                                "present".to_string()
                            },
                        )
                    })
                    .collect();
            let mut failures = Vec::new();
            if *create_missing {
                for (name, result) in merge_hold::provision_label_definitions(&root, &pin, &missing)
                {
                    match result {
                        Ok(()) => {
                            statuses.insert(name.into(), "created".into());
                        }
                        Err(error) => {
                            statuses.insert(name.into(), "failed".into());
                            failures.push(format!("{name}: {error}"));
                        }
                    }
                }
            }
            if *json {
                let status = if failures.is_empty() { "ok" } else { "unknown" };
                if failures.is_empty() {
                    println!(
                        "{}",
                        serde_json::json!({"status":status, "labels":statuses})
                    );
                } else {
                    println!(
                        "{}",
                        serde_json::json!({"status":status, "labels":statuses, "error":failures.join("; ")})
                    );
                }
            } else {
                for (name, status) in &statuses {
                    println!("{name}: {status}");
                }
                if !missing.is_empty() && !*create_missing {
                    println!("Provision missing definitions with `aida merge-hold labels --create-missing`.");
                }
                for failure in &failures {
                    println!("Could not provision {failure}");
                }
            }
            if !failures.is_empty() {
                anyhow::bail!("one or more merge-hold label definitions could not be provisioned");
            }
            Ok(())
        }
        crate::cli::MergeHoldAction::List { json, fix } => {
            let holds = merge_hold::list_holds(&root);
            // TASK-1455 / TASK-189: ONE pinned forge read per hold answers
            // both "is the PR merged" (stale sweep) and "does the forge carry
            // the label" (the column). The label column reports the FORGE,
            // not the marker's recorded claim, so it can see marker/label
            // divergence. trace:TASK-1455 trace:TASK-189 | ai:claude
            let forge_kind = forge::resolve_forge_kind(&root);
            let fetch_all = |holds: &[(u64, String)]| {
                holds
                    .iter()
                    .map(|(pr, _)| (*pr, merge_hold::fetch_pinned_change(&root, forge_kind, *pr)))
                    .collect::<std::collections::HashMap<_, _>>()
            };
            let mut facts = fetch_all(&holds);
            // BUG-1541: merged AND closed-unmerged are both terminal.
            let (stale, live) =
                partition_stale_holds(holds, |pr| hold_terminal_state(&root, pr, facts.get(&pr)));
            // BUG-1236: `--fix` re-syncs the Layer-2 label on every LIVE hold
            // whose FORGE label is not confirmed present, then reports.
            // TASK-189: keyed off the forge read, not the recorded state, so a
            // label removed at the forge after a `synced` record is repaired.
            if *fix {
                let mut fixed = 0usize;
                for (pr, _) in &live {
                    let forge_label = facts
                        .get(pr)
                        .map(merge_hold::ForgeLabel::from_fetch)
                        .unwrap_or(merge_hold::ForgeLabel::Unknown(String::new()));
                    if matches!(
                        forge_label,
                        merge_hold::ForgeLabel::Present | merge_hold::ForgeLabel::NoForge
                    ) {
                        continue;
                    }
                    match merge_hold::sync_label(&root, *pr, true) {
                        Ok(()) => {
                            fixed += 1;
                            println!("Re-synced `aida:merge-hold` label on PR #{pr}.");
                        }
                        Err(err) => println!("PR #{pr}: label still not applied — {err}"),
                    }
                }
                if fixed == 0 {
                    println!("No live hold needed a label re-sync.");
                } else {
                    // Re-read so the listing below shows the post-fix forge.
                    let all: Vec<(u64, String)> = live
                        .iter()
                        .cloned()
                        .chain(stale.iter().map(|(pr, r, _)| (*pr, r.clone())))
                        .collect();
                    facts = fetch_all(&all);
                }
                // STORY-1397: recusal routing is reconciled HERE, on an
                // explicit command, never on the awaiting render. It adopts a
                // moved head, re-selects a live independent reader, and
                // writes/retires the routing brief. trace:STORY-1397 | ai:claude
                let recusals: Vec<u64> = live
                    .iter()
                    .map(|(pr, _)| *pr)
                    .filter(|pr| {
                        merge_hold::read_hold_record(&root, *pr).is_some_and(|r| {
                            r.reason_kind == merge_hold::HoldReasonKind::Recusal && !r.legacy
                        })
                    })
                    .collect();
                if !recusals.is_empty() {
                    let heads: std::collections::HashMap<u64, String> = collect_open_prs(&root)
                        .by_branch
                        .into_values()
                        .filter_map(|pr| pr.head_sha.map(|sha| (pr.number, sha)))
                        .collect();
                    for pr in recusals {
                        if let Some(record) = merge_hold::reconcile_recusal_hold(
                            &root,
                            pr,
                            heads.get(&pr).map(String::as_str),
                        ) {
                            let routed = record
                                .routed_to
                                .iter()
                                .map(merge_hold::PrincipalIdentity::key)
                                .collect::<Vec<_>>()
                                .join(", ");
                            println!(
                                "PR #{pr}: recusal routing {}{}",
                                record.routing_state.as_str(),
                                if routed.is_empty() {
                                    String::new()
                                } else {
                                    format!(" → {routed}")
                                }
                            );
                        }
                    }
                }
            }
            let forge_label_of = |pr: u64| {
                facts
                    .get(&pr)
                    .map(merge_hold::ForgeLabel::from_fetch)
                    .unwrap_or(merge_hold::ForgeLabel::Unknown(String::new()))
            };
            // BUG-1469: a hold placed as a bare forge label has no marker, yet
            // blocks the merge exactly like one. Enumerate open labeled PRs in
            // ONE pinned query and list the label-only ones too; a scan that
            // could not run is REPORTED, never rendered as "no such holds".
            // trace:BUG-1469 | ai:claude
            let label_scan = merge_hold::list_labeled_open_changes(&root, forge_kind);
            let marker_prs: Vec<u64> = live
                .iter()
                .map(|(pr, _)| *pr)
                .chain(stale.iter().map(|(pr, _, _)| *pr))
                .collect();
            let label_only: Vec<u64> = match &label_scan {
                Ok(Some(labeled)) => merge_hold::label_only_holds(labeled, &marker_prs),
                _ => Vec::new(),
            };
            // A live marker the per-PR read saw labeled but the scan did not
            // list is a disagreement between two forge answers — surfaced,
            // not silently resolved toward the smaller set.
            let scan_disagrees: Vec<u64> = match &label_scan {
                Ok(Some(labeled)) => live
                    .iter()
                    .map(|(pr, _)| *pr)
                    .filter(|pr| {
                        forge_label_of(*pr) == merge_hold::ForgeLabel::Present
                            && !labeled.contains(pr)
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let label_scan_state = match &label_scan {
                Ok(Some(_)) => "ok".to_string(),
                Ok(None) => "no-forge".to_string(),
                Err(e) => format!("unknown: {}", e.lines().next().unwrap_or("")),
            };
            let unrecorded_removals = merge_hold::unrecorded_marker_removals(&root);
            // BUG-1562: re-evaluate each live marker's premise at read time
            // (PR head vs the sha it cites; a rework hold's verdict now
            // approved or closed). FLAG only — never auto-clear.
            // trace:BUG-1562 | ai:claude
            // BUG-1562: "is the spec this hold was placed for still
            // running?" — one cache-backed local read for every live hold,
            // never a forge call. trace:BUG-1562 | ai:claude
            let spec_status: std::collections::HashMap<String, String> = if live.is_empty() {
                std::collections::HashMap::new()
            } else {
                let mut map = std::collections::HashMap::new();
                for summary in all_requirement_summaries(&root) {
                    for id in [summary.agreed_id.as_ref(), summary.spec_id.as_ref()]
                        .into_iter()
                        .flatten()
                    {
                        map.insert(id.to_ascii_uppercase(), summary.status.clone());
                    }
                }
                map
            };
            let head_of = |pr: u64| match facts.get(&pr) {
                Some(Ok(Some(change))) => change.head_sha.clone(),
                _ => None,
            };
            let premise_of = |pr: u64| -> Option<String> {
                let record = merge_hold::read_hold_record(&root, pr)?;
                merge_hold::premise_stale(
                    &record,
                    head_of(pr).as_deref(),
                    |spec| review_verdict::read_recorded_verdict(&root, spec),
                    |spec| spec_status.get(&spec.to_ascii_uppercase()).cloned(),
                )
            };
            // BUG-1532 criterion 4: for a refusal hold, whether its release
            // condition (a fresh APPROVED verdict at the current head) is met.
            // Reported only — the release itself stays behind the human floor.
            // trace:BUG-1532 | ai:claude
            let refusal_state_of = |pr: u64| -> Option<merge_hold::RefusalRelease> {
                let record = merge_hold::read_hold_record(&root, pr)?;
                record.is_refusal().then(|| {
                    crate::pr_cmd::ship_refusal_release(
                        &[root.as_path()],
                        &record,
                        head_of(pr).as_deref(),
                    )
                })
            };
            let source_of = |pr: u64| {
                if forge_label_of(pr) == merge_hold::ForgeLabel::Present {
                    merge_hold::HoldSource::MarkerAndLabel
                } else {
                    merge_hold::HoldSource::Marker
                }
            };
            let recorded_of = |pr: u64| match merge_hold::read_label_state(&root, pr) {
                merge_hold::LabelState::Synced => "synced".to_string(),
                merge_hold::LabelState::Unsynced(e) => format!("unsynced: {e}"),
                merge_hold::LabelState::Unknown => "unknown".to_string(),
            };
            if *json {
                let row = |pr: u64, reason: &str, terminal: Option<merge_hold::TerminalState>| {
                    let is_stale = terminal.is_some();
                    let forge_label = forge_label_of(pr);
                    let record = merge_hold::read_hold_record(&root, pr);
                    let kind = record
                        .as_ref()
                        .map(|r| format!("{:?}", r.reason_kind).to_ascii_lowercase())
                        .unwrap_or_else(|| "unknown".into());
                    let routing = record
                        .as_ref()
                        .map(|r| r.routing_state.as_str().to_string())
                        .unwrap_or_else(|| "pending".into());
                    let recused = record
                        .as_ref()
                        .map(|r| r.recused_principals.clone())
                        .unwrap_or_default();
                    let condition = record.as_ref().map(|r| r.release_condition_or_default());
                    let refusal = if is_stale { None } else { refusal_state_of(pr) };
                    serde_json::json!({
                        "pr": pr,
                        "reason": reason,
                        "reason_kind": kind,
                        "routing_state": routing,
                        "recused_principals": recused,
                        "source": source_of(pr).as_str(),
                        "stale": is_stale,
                        "stale_reason": terminal.map(merge_hold::TerminalState::as_str),
                        "premise_stale": if is_stale { None } else { premise_of(pr) },
                        "label": forge_label.as_str(),
                        "label_recorded": recorded_of(pr),
                        "label_diverged": if is_stale {
                            None
                        } else {
                            merge_hold::label_diverged(&forge_label)
                        },
                        "release_condition": condition.as_ref().map(|(c, _)| c.clone()),
                        "release_condition_default": condition.as_ref().map(|(_, d)| *d),
                        "verdict_ref": record.as_ref().and_then(|r| r.verdict_ref.clone()),
                        "spec": record.as_ref().and_then(|r| r.spec.clone()),
                        "refusal_released": refusal.as_ref().map(|r| matches!(r, merge_hold::RefusalRelease::Released(_))),
                        "refusal_detail": refusal.as_ref().and_then(|r| match r {
                            merge_hold::RefusalRelease::Held(why) => Some(why.clone()),
                            merge_hold::RefusalRelease::Released(_) => None,
                        }),
                    })
                };
                let mut items: Vec<serde_json::Value> = Vec::new();
                for (pr, reason) in &live {
                    items.push(row(*pr, reason, None));
                }
                // BUG-1469: label-only holds are rows too, naming the missing half.
                for pr in &label_only {
                    items.push(serde_json::json!({
                        "pr": pr,
                        "reason": merge_hold::LABEL_ONLY_REASON,
                        "reason_kind": "unknown",
                        "routing_state": "pending",
                        "recused_principals": [],
                        "source": merge_hold::HoldSource::LabelOnly.as_str(),
                        "stale": false,
                        "stale_reason": null,
                        "premise_stale": null,
                        "label": merge_hold::ForgeLabel::Present.as_str(),
                        "label_recorded": "none",
                        "label_diverged": null,
                    }));
                }
                for (pr, reason, terminal) in &stale {
                    items.push(row(*pr, reason, Some(*terminal)));
                }
                println!(
                    "{}",
                    serde_json::json!({
                        "merge_holds": items,
                        "label_scan": label_scan_state,
                        "label_scan_disagrees": scan_disagrees,
                        "unrecorded_marker_removals": unrecorded_removals,
                    })
                );
                return Ok(());
            }
            if let Err(err) = &label_scan {
                println!(
                    "{}",
                    format!(
                        "Label-only holds UNKNOWN — the `aida:merge-hold` label scan did not run ({}); PRs held by the label alone may be missing below.",
                        err.lines().next().unwrap_or("")
                    )
                    .yellow()
                );
            }
            for pr in &unrecorded_removals {
                println!(
                    "{}",
                    format!("PR #{pr}: TAMPERING — AIDA recorded a hold placement, but its marker is missing and no clearance record exists.").red()
                );
            }
            for pr in &scan_disagrees {
                println!(
                    "{}",
                    format!(
                        "PR #{pr}: the per-PR read shows the hold label but the label scan did not list it — the forge answers disagree."
                    )
                    .yellow()
                );
            }
            if live.is_empty()
                && stale.is_empty()
                && label_only.is_empty()
                && unrecorded_removals.is_empty()
            {
                println!("No active merge-holds.");
                return Ok(());
            }
            println!("Active merge-holds:");
            for (pr, reason) in &live {
                let forge_label = forge_label_of(*pr);
                let recorded = recorded_of(*pr);
                let rendered = match &forge_label {
                    merge_hold::ForgeLabel::Present => "label: present".green().to_string(),
                    merge_hold::ForgeLabel::NoForge => "label: n/a (no forge)".dimmed().to_string(),
                    merge_hold::ForgeLabel::Absent => "label: ABSENT — DIVERGED: marker holds but the forge gate is not armed (`aida merge-hold list --fix`)"
                    .red()
                    .to_string(),
                    merge_hold::ForgeLabel::Unknown(err) => if err.is_empty() {
                        "label: unknown (forge not read)".to_string()
                    } else {
                        format!("label: unknown (forge not read: {err})")
                    }
                    .yellow()
                    .to_string(),
                };
                println!("  PR #{pr}  {reason}  [{rendered} | recorded: {recorded}]");
                // BUG-1562 / BUG-1532: the release condition (a future check)
                // and, for a refusal, the verdict it points at and whether a
                // fresh verdict at the head has met it. The reason above is a
                // placement-time summary. trace:BUG-1532 trace:BUG-1562 | ai:claude
                if let Some(record) = merge_hold::read_hold_record(&root, *pr) {
                    let (cond, is_default) = record.release_condition_or_default();
                    println!(
                        "      release when: {cond}{}",
                        if is_default {
                            " (default for this hold kind)"
                        } else {
                            ""
                        }
                    );
                    if let Some(v) = &record.verdict_ref {
                        println!(
                            "      verdict: {} (owner and current state are read from that record, not from the reason)",
                            v.path()
                        );
                    }
                    match refusal_state_of(*pr) {
                        // TASK-1582: clear → ship, never ship alone (the two
                        // messages used to point at each other).
                        // trace:TASK-1582 | ai:antigravity
                        Some(merge_hold::RefusalRelease::Released(v)) => println!(
                            "      {}",
                            merge_hold::release_met_next_action(
                                *pr,
                                &v.key,
                                v.reviewed_sha
                                    .as_deref()
                                    .map(review_verdict::short_sha)
                                    .unwrap_or("?"),
                            )
                            .green()
                        ),
                        Some(merge_hold::RefusalRelease::Held(why)) => {
                            println!("      release condition not met: {why}")
                        }
                        None => {}
                    }
                }
                if let Some(why) = premise_of(*pr) {
                    println!(
                        "      {}",
                        format!(
                            "premise stale: {why} — the hold is still armed; a human decides whether to `aida merge-hold clear {pr}`"
                        )
                        .yellow()
                    );
                }
            }
            for pr in &label_only {
                println!(
                    "  PR #{pr}  {}  [{} | recorded: none — no marker, no reason; `aida merge-hold clear {pr}` releases it]",
                    merge_hold::LABEL_ONLY_REASON,
                    "label: present".green()
                );
            }
            for (pr, reason, terminal) in &stale {
                println!(
                    "  PR #{pr}  {reason}  {}",
                    format!(
                        "[stale — {}; `aida merge-hold clear --stale` to sweep]",
                        terminal.describe()
                    )
                    .yellow()
                );
            }
            Ok(())
        }
        // BUG-1236: the symmetric hand-hold — marker + label together.
        crate::cli::MergeHoldAction::Add {
            pr,
            reason,
            reason_kind,
            recused_principals,
            routed_to,
            head,
            release_condition,
            verdict,
            replace,
        } => {
            let reason = reason
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .unwrap_or("held by hand — merge requires human/advisor review")
                .to_string();
            let kind = merge_hold::HoldReasonKind::parse(reason_kind).ok_or_else(|| {
                anyhow::anyhow!("--reason-kind must be supervision, recusal, rework, or decision")
            })?;
            let recused_principals: Vec<merge_hold::PrincipalIdentity> = recused_principals
                .iter()
                .map(|p| merge_hold::PrincipalIdentity::parse(p))
                .filter(|p| !p.principal_id.is_empty())
                .collect();
            let routed_to: Vec<merge_hold::PrincipalIdentity> = routed_to
                .iter()
                .map(|p| merge_hold::PrincipalIdentity::parse(p))
                .filter(|p| !p.principal_id.is_empty())
                .collect();
            let routing_state = if kind == merge_hold::HoldReasonKind::Recusal {
                if routed_to.is_empty() {
                    merge_hold::HoldRoutingState::NoIndependentReader
                } else {
                    merge_hold::HoldRoutingState::Routed
                }
            } else {
                merge_hold::HoldRoutingState::Pending
            };
            // BUG-1532 criterion 10: a rework hold REFERENCES the verdict
            // record it stands on (key + the sha and seat currently recorded
            // there) instead of quoting it. trace:BUG-1532 | ai:claude
            let verdict_ref = match verdict.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
                None => None,
                Some(_) if kind != merge_hold::HoldReasonKind::Rework => {
                    anyhow::bail!("--verdict applies to a rework hold (`--reason-kind rework`)");
                }
                Some(key) => {
                    let current = review_verdict::read_recorded_verdict(&root, key);
                    if current.is_none() {
                        eprintln!(
                            "  {} no verdict is recorded under {} yet — the hold references it anyway and releases only once an approving verdict is recorded at the PR's head",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                            review_verdict::verdict_path(&root, key).display()
                        );
                    }
                    Some(merge_hold::VerdictRef::new(
                        key,
                        Some(*pr),
                        current.as_ref().and_then(|v| v.reviewed_sha.clone()),
                        current.as_ref().and_then(|v| v.recorded_by.clone()),
                    ))
                }
            };
            let release_condition = release_condition
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(str::to_string);
            let record = merge_hold::MergeHoldRecord {
                schema_version: 2,
                pr: *pr,
                reason_kind: kind,
                detail: reason,
                recused_principals,
                routed_to,
                routing_state,
                target_head_sha: head.clone(),
                label_state: None,
                legacy: false,
                verdict_ref,
                release_condition,
                spec: None,
                placed_by: Some(merge_hold::placing_seat()),
                absorbed: Vec::new(),
            };
            // STORY-1416 criterion 1a: before the marker lands, show what is
            // already on record for this PR (marker + verdicts, with seat and
            // sha). Informational only — never blocks the write.
            // trace:STORY-1416 | ai:claude
            {
                let extra: Vec<&str> = verdict.as_deref().into_iter().collect();
                let standing = pr_claim_surface::read_pr_record(&root, *pr, &extra);
                pr_claim_surface::print(
                    *pr,
                    &pr_claim_surface::lines_for_marker_write(&standing, &record),
                );
            }
            // BUG-1562: never silently overwrite an existing marker's body.
            // trace:BUG-1562 | ai:claude
            merge_hold::place_hand_hold(&root, &record, *replace)
                .map_err(|e| anyhow::anyhow!(e))?;
            if record.release_condition.is_none() {
                println!(
                    "  hint: state what must be true to release it with `--release-condition \"1. ... 2. ...\"` — a condition is checked when read; a reason goes stale."
                );
            }
            match merge_hold::sync_label(&root, *pr, true) {
                Ok(()) => println!(
                    "Merge-hold placed on PR #{pr} (marker written, `aida:merge-hold` label applied). Release with `aida merge-hold clear {pr}`."
                ),
                Err(err) if err.is_definition_missing() => {
                    println!("Merge-hold marker written for PR #{pr} and Layer 1 is armed, but Layer 2 is not armed because the forge label definitions are missing: {err}");
                    return Err(anyhow::anyhow!(err));
                }
                Err(err) => println!(
                    "Merge-hold marker written for PR #{pr}, but the label did not apply: {err} — retry with `aida merge-hold list --fix`."
                ),
            }
            Ok(())
        }
        crate::cli::MergeHoldAction::Clear { pr, stale } => {
            // Clearing a supervised hold is a non-grantable human integrity
            // floor, independent of dispatch/disposition roles.
            // trace:STORY-1353 | ai:codex
            if !has_integrity_floor_authority() {
                let refusal = "clearing a merge-hold requires a human at an interactive terminal; \
                     dispatch or advisor authority cannot override this integrity floor";
                // STORY-1436: the floor HELD — record it (PR, refused seat,
                // reason, ts) so floor refusals can be counted against
                // releases. Best-effort; never changes the refusal.
                // trace:STORY-1436 | ai:claude
                events::record_gate_held(
                    &root,
                    events::GATE_MERGE_HOLD_CLEAR_FLOOR,
                    None,
                    *pr,
                    refusal,
                );
                anyhow::bail!(refusal);
            }
            match (pr, stale) {
                (Some(_), true) => {
                    anyhow::bail!(
                        "give a PR number OR --stale, not both: `aida merge-hold clear <pr>` clears one, `aida merge-hold clear --stale` sweeps every marker whose PR merged or closed unmerged"
                    );
                }
                (None, false) => {
                    anyhow::bail!(
                        "nothing to clear: `aida merge-hold clear <pr>` clears one hold, `aida merge-hold clear --stale` sweeps every marker whose PR merged or closed unmerged"
                    );
                }
                (Some(pr), false) => {
                    // STORY-1397: the integrity floor above already proved a
                    // human is at an interactive terminal. That human clears
                    // WITHOUT an agent id and is recorded as `human:<user>`:
                    // the independence rule restricts the recused author
                    // agent, and a human clearance is independent of it by
                    // definition. A self-declared AIDA_AGENT_ID can only
                    // refuse (when it names a recused principal); it never
                    // satisfies independence. trace:STORY-1397 | ai:claude
                    let cleared = match merge_hold::read_hold_record(&root, *pr) {
                        Some(record) => {
                            let actor = merge_hold::human_clear_actor(
                                &record,
                                &current_user_id(None),
                                merge_hold::current_principal().as_ref(),
                            )
                            .map_err(|e| anyhow::anyhow!(e))?;
                            Some((record, actor))
                        }
                        None => None,
                    };
                    let existed = cleared.is_some();
                    // BUG-1499: no marker may still mean a LABEL-ONLY hold (a
                    // seat applied the label by hand). The same human floor
                    // above already applies; read the forge label (pinned) so
                    // the clearance is recorded against a hold that really
                    // existed, never against a guess.
                    // trace:BUG-1499 | ai:claude
                    let label_only_forge = if existed {
                        None
                    } else {
                        Some(merge_hold::ForgeLabel::from_fetch(
                            &merge_hold::fetch_pinned_change(
                                &root,
                                forge::resolve_forge_kind(&root),
                                *pr,
                            ),
                        ))
                    };
                    let label_only_record = match &label_only_forge {
                        Some(merge_hold::ForgeLabel::Present) => {
                            let record = merge_hold::typed_hold(
                                *pr,
                                merge_hold::HoldReasonKind::Unknown,
                                merge_hold::LABEL_ONLY_REASON,
                                None,
                            );
                            let actor = merge_hold::human_clear_actor(
                                &record,
                                &current_user_id(None),
                                merge_hold::current_principal().as_ref(),
                            )
                            .map_err(|e| anyhow::anyhow!(e))?;
                            Some((record, actor))
                        }
                        _ => None,
                    };
                    // BUG-1693: a marker deleted outside the human-gated path
                    // leaves AIDA fail-closed on that PR — `read_hold` reports
                    // tampering, so every AIDA merge refuses it. The human at
                    // the terminal needs a door that RECORDS the release rather
                    // than one that reopens the hole: without this the PR is
                    // unmergeable forever and no clearance names who released
                    // it, which is the same silence this spec exists to end.
                    // trace:BUG-1693 | ai:claude
                    let tampering_record = if cleared.is_none()
                        && label_only_record.is_none()
                        && merge_hold::unrecorded_marker_removals(&root).contains(pr)
                    {
                        let record = merge_hold::typed_hold(
                            *pr,
                            merge_hold::HoldReasonKind::Unknown,
                            merge_hold::MARKER_MISSING_REASON,
                            None,
                        );
                        let actor = merge_hold::human_clear_actor(
                            &record,
                            &current_user_id(None),
                            merge_hold::current_principal().as_ref(),
                        )
                        .map_err(|e| anyhow::anyhow!(e))?;
                        Some((record, actor))
                    } else {
                        None
                    };
                    // BUG-1532 criterion 4: a reviewer REFUSAL is normally
                    // released by a fresh APPROVED verdict at the head. The
                    // human at the terminal may still override — this is the
                    // documented, recorded escape — but is told when the
                    // condition is not met, and the clearance records which
                    // verdict (if any) met it. trace:BUG-1532 | ai:claude
                    let released_by_verdict = match &cleared {
                        Some((record, _)) if record.is_refusal() => {
                            let head = merge_hold::fetch_pinned_change(
                                &root,
                                forge::resolve_forge_kind(&root),
                                *pr,
                            )
                            .ok()
                            .flatten()
                            .and_then(|change| change.head_sha);
                            match crate::pr_cmd::ship_refusal_release(
                                &[root.as_path()],
                                record,
                                head.as_deref(),
                            ) {
                                merge_hold::RefusalRelease::Released(v) => Some(v),
                                merge_hold::RefusalRelease::Held(why) => {
                                    eprintln!(
                                        "  {} PR #{pr} is a reviewer refusal whose release condition is NOT met: {why}. Clearing it anyway as a recorded human override.",
                                        crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                                    );
                                    None
                                }
                            }
                        }
                        _ => None,
                    };
                    // BUG-1693: persist the local clearance before removing
                    // the marker or changing forge labels. A failed audit
                    // write must leave the hold in place.
                    if let Some((record, actor)) = &cleared {
                        merge_hold::record_clearance_with_verdict(
                            &root,
                            record,
                            actor,
                            released_by_verdict.clone(),
                        )?;
                    }
                    if let Some((record, actor)) = &tampering_record {
                        merge_hold::record_clearance(&root, record, actor)?;
                    }
                    if let Some((record, actor)) = &label_only_record {
                        merge_hold::record_clearance(&root, record, actor)?;
                    }
                    merge_hold::clear_hold(&root, *pr)?;
                    if let Some((record, actor)) = &cleared {
                        if record.reason_kind == merge_hold::HoldReasonKind::Recusal {
                            println!(
                                "Recusal hold on PR #{pr} cleared by {} (independent of the recused author).",
                                actor.key()
                            );
                        }
                    }
                    // STORY-1436: a human clear is the floor's complement —
                    // record the release so floor refusals have a denominator.
                    // trace:STORY-1436 | ai:claude
                    let emit_release = |detail: &str| {
                        let mut ev = events::Event::new(
                            None,
                            "",
                            events::EventKind::MergeHoldChanged {
                                pr: *pr as u32,
                                placed: false,
                                reason: Some(format!("merge-hold clear: {detail}")),
                            },
                        );
                        ev.seat = events::active_seat();
                        events::emit(&root, &ev);
                    };
                    if matches!(
                        label_only_forge,
                        Some(merge_hold::ForgeLabel::Absent | merge_hold::ForgeLabel::NoForge)
                    ) {
                        if let Some((record, actor)) = &tampering_record {
                            emit_release(merge_hold::MARKER_MISSING_REASON);
                            println!(
                                "PR #{pr} had a recorded hold placement whose marker went missing with no clearance. Recorded the release as {} ({}); `aida merge-hold list` and `aida doctor` stop reporting it, and the clearance record keeps the evidence.",
                                actor.key(),
                                record.detail
                            );
                        } else {
                            println!(
                                "No merge-hold on PR #{pr}: no marker, and the forge carries no `aida:merge-hold` label."
                            );
                        }
                        return Ok(());
                    }
                    let unlabel = merge_hold::sync_label(&root, *pr, false);
                    if let Err(err) = &unlabel {
                        eprintln!(
                        "  {} label not dropped on PR #{pr}: {err} — drop it by hand or re-run `aida merge-hold clear {pr}`",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                    );
                    }
                    if existed {
                        if let Some((record, _)) = &cleared {
                            emit_release(&record.detail);
                        }
                        println!(
                            "Cleared merge-hold on PR #{pr} (marker removed, `aida:merge-hold` label dropped)."
                        );
                    } else if let Some((record, actor)) = &label_only_record {
                        if unlabel.is_ok() {
                            emit_release(&record.detail);
                            println!(
                                "Cleared label-only merge-hold on PR #{pr} (no marker; `aida:merge-hold` label dropped, clearance recorded as {}).",
                                actor.key()
                            );
                        } else {
                            anyhow::bail!(
                                "label-only merge-hold on PR #{pr} is still in place: the label could not be removed"
                            );
                        }
                    } else {
                        println!(
                            "No merge-hold marker for PR #{pr} and the forge label could not be read; dropped the `aida:merge-hold` label anyway in case it lingered."
                        );
                    }
                    Ok(())
                }
                (None, true) => {
                    let holds = merge_hold::list_holds(&root);
                    // TASK-1455: the "is it merged" read that authorises
                    // removing a marker is pinned to this project's repo — a
                    // same-numbered merged PR elsewhere must not sweep a live
                    // hold here. trace:TASK-1455 | ai:claude
                    // BUG-1541: a PR closed WITHOUT merging is swept too —
                    // it can never merge, so its hold releases nothing. An
                    // unknown forge state stays (fail closed).
                    // trace:BUG-1541 | ai:claude
                    let forge_kind = forge::resolve_forge_kind(&root);
                    let (stale, live) = partition_stale_holds(holds, |pr| {
                        let fetched = merge_hold::fetch_pinned_change(&root, forge_kind, pr);
                        hold_terminal_state(&root, pr, Some(&fetched))
                    });
                    if stale.is_empty() {
                        println!(
                            "No stale merge-holds ({} marker(s) on PRs that are open or whose state could not be confirmed).",
                            live.len()
                        );
                        return Ok(());
                    }
                    for (pr, _reason, terminal) in &stale {
                        let _ = merge_hold::clear_hold(&root, *pr);
                        if let Err(err) = merge_hold::sync_label(&root, *pr, false) {
                            eprintln!(
                                "  {} label not dropped on PR #{pr}: {err}",
                                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                            );
                        }
                        println!(
                            "Cleared stale merge-hold on PR #{pr} ({}).",
                            terminal.describe()
                        );
                    }
                    println!("Swept {} stale merge-hold(s).", stale.len());
                    Ok(())
                }
            }
        }
    }
}
