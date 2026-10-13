//! next-N drain driver: RealNextNDriver, next-keyword parsing, drain summary and cap stop, auto-complete run records and failure-bug drafting (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// TASK-293 — a parsed `next` / `nextN` keyword positional for `aida queue
/// work`. trace:TASK-293 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NextKeyword {
    /// The positional is not a `next*` keyword — a spec-id, cluster anchor,
    /// or (already rerouted) batch positional.
    NotNext,
    /// `next`, `nextN`, or `next N` — drain `N` specs from the queue head.
    /// `N >= 1`; `N == 1` is the plain head pickup.
    Count(usize),
}

/// Parse the `aida queue work` positional(s) for the `next` / `nextN` keyword
/// (TASK-293). `id` is the first positional, `count` the optional second (the
/// spaced `next 3` form). `next` and `nextN` / `next N` resolve to a drain
/// count; anything else is `NotNext`. Malformed forms (`next0`, `nextfoo`, a
/// count with no `next`, a count alongside the compact `nextN`) are rejected.
/// The keyword matches case-insensitively — a real spec-id always carries a
/// `TYPE-N` hyphen, so `next` / `nextN` never collide with one.
/// trace:TASK-293 | ai:claude
pub(crate) fn parse_next_keyword(id: Option<&str>, count: Option<&str>) -> Result<NextKeyword> {
    let Some(raw) = id else {
        // No positional id. A trailing count with no `next` before it is not
        // reachable through clap's positional fill, but guard rather than
        // silently ignore it.
        if count.is_some() {
            anyhow::bail!(
                "a count positional needs the `next` keyword before it (e.g. `aida queue work next 3`)"
            );
        }
        return Ok(NextKeyword::NotNext);
    };
    let lower = raw.to_ascii_lowercase();
    if !lower.starts_with("next") {
        if count.is_some() {
            anyhow::bail!(
                "`{}` is not the `next` keyword — a trailing count only follows `next`",
                raw
            );
        }
        return Ok(NextKeyword::NotNext);
    }
    let suffix = &lower["next".len()..];
    // `next` exact: the count comes from the optional second positional.
    if suffix.is_empty() {
        let n = match count {
            None => 1,
            Some(c) => parse_next_count(c)?,
        };
        return Ok(NextKeyword::Count(n));
    }
    // `nextN` compact: the suffix must be all digits, and there must be no
    // separate count positional (`next3 5` is contradictory).
    if suffix.chars().all(|c| c.is_ascii_digit()) {
        if let Some(c) = count {
            anyhow::bail!(
                "`{}` already carries its count — drop the extra `{}`",
                raw,
                c
            );
        }
        return Ok(NextKeyword::Count(parse_next_count(suffix)?));
    }
    // `nextfoo` — starts with `next` but is not a valid form.
    anyhow::bail!(
        "`{}` is not a valid pickup target — use `next`, `nextN` (e.g. `next3`), or a SPEC id",
        raw
    )
}

/// Parse + validate the `N` of a `nextN` form: a positive whole number.
/// trace:TASK-293 | ai:claude
pub(crate) fn parse_next_count(raw: &str) -> Result<usize> {
    let n: usize = raw.trim().parse().map_err(|_| {
        anyhow::anyhow!(
            "`{}` is not a whole number — `nextN` needs a count like `next3`",
            raw
        )
    })?;
    if n == 0 {
        anyhow::bail!("`next0` drains nothing — use `next` (or `next1`) for a single pickup");
    }
    Ok(n)
}

/// Non-erroring queue-head resolver for the `nextN` drain (TASK-293): the
/// drivable head of the active role's queue, or `None` when nothing drivable
/// remains (the drain is then complete). A store/queue read failure is fatal —
/// it is not "drained" and would recur on every iteration. trace:TASK-293
/// Resolve the next `nextN` / queue-wide drain head. Returns the pick plus
/// the entries skipped to reach it: `role_skipped` as `(id, routed role)` and
/// (BUG-1608) `blocked_skipped` as `(id, pickability reason)` — dependents
/// whose `BlockedBy` prerequisite is not Completed. The candidates are
/// re-read from storage on every call, so a blocker shelved by the previous
/// member is seen by the very next pick.
#[allow(clippy::type_complexity)]
pub(crate) fn resolve_next_n_head(
    storage: &Storage,
    user_id: &str,
    role_override: Option<&str>,
) -> (
    Option<AutoCompleteHeadPick>,
    Vec<(String, String)>,
    Vec<(String, String)>,
) {
    let effective_role = effective_auto_complete_role(role_override);
    match auto_complete_head_candidates_with_roles(storage, user_id, Some(&effective_role)) {
        Ok(candidates) => {
            let pick = pick_auto_complete_head_for_role(&candidates, &effective_role).unwrap();
            let (role_skipped, blocked_skipped) = match &pick {
                Some(pick) => (pick.role_skipped.clone(), pick.blocked_skipped.clone()),
                None => (
                    candidates
                        .iter()
                        .filter_map(|candidate| {
                            let routed = candidate.for_role.as_deref().map(canonical_role_name)?;
                            (routed != effective_role).then(|| (candidate.id.clone(), routed))
                        })
                        .collect(),
                    // trace:BUG-1608 | ai:claude
                    candidates
                        .iter()
                        .filter(|candidate| {
                            candidate
                                .for_role
                                .as_deref()
                                .map(|r| canonical_role_name(r) == effective_role)
                                .unwrap_or(true)
                                && auto_complete_head_drivable(&candidate.status)
                        })
                        .filter_map(|candidate| {
                            candidate
                                .blocked
                                .clone()
                                .map(|reason| (candidate.id.clone(), reason))
                        })
                        .collect(),
                ),
            };
            (pick, role_skipped, blocked_skipped)
        }
        Err(e) => {
            eprintln!(
                "{} could not resolve the queue head: {}",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                e
            );
            std::process::exit(1);
        }
    }
}

/// Count the orchestrator-drivable items queued for the active role — used to
/// surface the "only K items queued" note when a `nextN` asks for more than
/// the queue holds. trace:TASK-293 | ai:claude
pub(crate) fn drivable_queued_count(
    storage: &Storage,
    user_id: &str,
    role_override: Option<&str>,
) -> Result<usize> {
    let effective_role = effective_auto_complete_role(role_override);
    Ok(
        auto_complete_head_candidates_with_roles(storage, user_id, Some(&effective_role))?
            .iter()
            .filter(|candidate| {
                candidate
                    .for_role
                    .as_deref()
                    .map(|r| canonical_role_name(r) == effective_role)
                    .unwrap_or(true)
                    && auto_complete_head_drivable(&candidate.status)
            })
            .count(),
    )
}

/// TASK-966: process-exit code a drain uses when a hard budget cap
/// (`--max-tokens` / `--max-iterations` / `--max-runtime`) stopped it. Distinct
/// from the orchestrator's phase-failure codes (1-6) and the shelved-drain code
/// (2) so an outer loop like `scripts/drain-loop.sh` can recognise "budget
/// exhausted — stop the loop" vs "chunk drained, keep going".
// trace:TASK-966 | ai:claude
pub(crate) const DRAIN_CAP_EXIT_CODE: i32 = 7;

/// TASK-966: sum the cumulative reported tokens across the headless-phase logs a
/// drain has produced since `since`. Each `claude -p --output-format
/// stream-json` phase writes a `*.jsonl` log under `.aida/headless-logs/`; the
/// terminal `result` event carries that phase's cumulative usage. We sum
/// per-log totals over the logs touched at/after the drain start so the running
/// figure reflects only THIS drain's spend. Best-effort: an unreadable dir /
/// file contributes nothing rather than erroring.
// trace:TASK-966 | ai:claude
pub(crate) fn sum_headless_log_tokens(
    project_root: &std::path::Path,
    since: std::time::SystemTime,
) -> u64 {
    let dir = project_root.join(".aida").join("headless-logs");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return 0;
    };
    let mut total: u64 = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        // Only count logs written during this drain.
        let touched_in_window = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|m| m >= since)
            .unwrap_or(true);
        if !touched_in_window {
            continue;
        }
        if let Ok(contents) = std::fs::read_to_string(&path) {
            total = total.saturating_add(drain_caps::tokens_from_log(&contents));
        }
    }
    total
}

/// Strict drain-exit accounting. Live caps keep using the best-effort scanner
/// above; persisted cost evidence is exact or explicitly unknown.
// trace:BUG-1418 | ai:codex
pub(crate) fn measure_completed_headless_logs(
    project_root: &std::path::Path,
    since: std::time::SystemTime,
) -> Option<u64> {
    let dir = project_root.join(".aida").join("headless-logs");
    let entries = std::fs::read_dir(&dir).ok()?;
    let mut total = 0_u64;
    let mut found = false;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let touched_in_window = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|m| m >= since)
            .unwrap_or(false);
        if !touched_in_window {
            continue;
        }
        found = true;
        let contents = std::fs::read_to_string(&path).ok()?;
        match drain_caps::completed_log_tokens(&contents) {
            drain_caps::CompletedLogTokens::Measured(tokens) => {
                total = total.saturating_add(tokens);
            }
            drain_caps::CompletedLogTokens::Unrecognized { shape } => {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("<non-utf8>");
                eprintln!("{}", unrecognized_usage_diagnostic(name, &shape));
                return None;
            }
            drain_caps::CompletedLogTokens::Truncated => return None,
        }
    }
    found.then_some(total)
}

// trace:BUG-1418 | ai:codex
pub(crate) fn unrecognized_usage_diagnostic(file_name: &str, shape: &str) -> String {
    fn safe_label(value: &str, limit: usize) -> String {
        value
            .chars()
            .take(limit)
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '=' | ',') {
                    ch
                } else {
                    '_'
                }
            })
            .collect()
    }
    format!(
        "token usage unrecognized in headless log {} (shape: {})",
        safe_label(file_name, 80),
        safe_label(shape, 96)
    )
}

/// TASK-967: the `BatchDrainOutcome` → machine label used in the drain exit
/// summary's `outcome` field (mirrors the per-emitter JSON mapping).
// trace:TASK-967 | ai:claude
pub(crate) fn batch_outcome_label(outcome: &auto_complete::BatchDrainOutcome) -> &'static str {
    use auto_complete::BatchDrainOutcome;
    match outcome {
        BatchDrainOutcome::Drained => "drained",
        BatchDrainOutcome::MaxReached => "max-reached",
        BatchDrainOutcome::Failed(_) => "failed",
        BatchDrainOutcome::Stalled => "stalled",
        BatchDrainOutcome::Mismatched { .. } => "mismatched",
        BatchDrainOutcome::Inconclusive => "inconclusive",
        BatchDrainOutcome::Held => "held",
        BatchDrainOutcome::DrainedWithShelved => "drained-with-shelved",
        BatchDrainOutcome::EnvironmentBlocked { .. } => "environment-blocked", // trace:BUG-1930 | ai:claude
    }
}

/// BUG-1930: the machine `halt_reason` of a drain summary: the environmental
/// cause slug when the drain halted on a blocked environment, else `null`.
// trace:BUG-1930 | ai:claude
pub(crate) fn batch_halt_reason(outcome: &auto_complete::BatchDrainOutcome) -> serde_json::Value {
    match outcome {
        auto_complete::BatchDrainOutcome::EnvironmentBlocked { cause } => {
            serde_json::Value::String((*cause).to_string())
        }
        _ => serde_json::Value::Null,
    }
}

/// BUG-1930: the human summary lines for an environment-blocked drain,
/// shared by the `nextN` and `--batch` summaries.
// trace:BUG-1930 | ai:claude
pub(crate) fn emit_environment_blocked_lines(cause: &str, stopped: &str, shipped_list: &str) {
    eprintln!(
        "{} {}",
        crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
        auto_complete::environment_blocked_message(cause)
    );
    eprintln!(
        "  {} halted after {} — already shipped: {}",
        "→".dimmed(),
        stopped.bold(),
        shipped_list
    );
    eprintln!(
        "  {} no further specs were started; a member already running in a pipelined drain is not interrupted",
        "→".dimmed()
    );
}

/// TASK-967: `git diff --numstat <base>..HEAD` in the drain's main worktree —
/// the cumulative line/file churn the drain landed (each shipped spec merged +
/// pulled advances HEAD on the integration branch). Best-effort: any git error
/// yields an empty body, which `DrainDiffStats::from_numstat` reads as all-zero.
// trace:TASK-967 | ai:claude
pub(crate) fn drain_diff_numstat(root: &std::path::Path, base_sha: &str) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "--numstat", &format!("{base_sha}..HEAD")])
        .output();
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => String::new(),
    }
}

/// TASK-967: assemble + emit + record the permanent drain EXIT SUMMARY.
///
/// Gathers the impure inputs — cumulative headless-log tokens (reusing
/// [`sum_headless_log_tokens`] / [`drain_caps::tokens_from_log`]), the
/// drain-wide `git diff --numstat`, and the wall time — then hands them to the
/// pure [`drain_summary::DrainSummary`]. The block prints to stderr (human) or
/// emits the structured record as a JSON line (machine), and the same record is
/// appended to `~/.aida/usage.jsonl` (gated on telemetry) so `aida usage` can
/// chart cost-per-drain. Works for every exit family — drained, cap-hit (the
/// `cap_stop` label overrides the outcome), shelved, or failed.
///
/// TASK-1165: the project root is the caller-supplied `drain_root` — every
/// caller already resolves it as `find_main_worktree_root().ok()` for the token
/// / diff inputs, so the terminal `QueueDrained` emit (and the `.aida/last-drain.json`
/// write beside it) reuses that one injected value rather than re-deriving a
/// second root from the process cwd at the emit site. `None` skips those
/// side-effects entirely, which is the fail-safe BUG-770 established.
// trace:TASK-967 trace:TASK-1165 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn finalize_drain_summary(
    kind: &str,
    label: String,
    outcome_outcome: &auto_complete::BatchDrainOutcome,
    cap_stop: Option<&drain_caps::CapStop>,
    tallies: drain_summary::DrainTallies,
    drain_root: Option<&std::path::Path>,
    drain_base_sha: Option<&str>,
    started: std::time::SystemTime,
    elapsed: std::time::Duration,
    drain_invocation: &last_drain::DrainInvocation,
    json: bool,
    // TASK-1297: "M other approved specs routed to this role are not in this
    // batch" — 0 for every non-batch drain kind (single, next-n). Echoed onto
    // the terminal QueueDrained event so a monitor can alarm on it without
    // scraping the human closing line. trace:TASK-1297 | ai:claude
    excluded_from_batch: Option<usize>,
    ineligible: &[events::IneligibleBatchMember],
) {
    // A budget-cap stop reports the cap that fired; otherwise the drain outcome.
    let outcome = match cap_stop {
        Some(stop) => stop.flag().to_string(),
        None => batch_outcome_label(outcome_outcome).to_string(),
    };
    let cumulative_tokens =
        drain_root.and_then(|root| measure_completed_headless_logs(root, started));
    let diff = match (drain_root, drain_base_sha) {
        (Some(root), Some(base)) => {
            drain_summary::DrainDiffStats::from_numstat(&drain_diff_numstat(root, base))
        }
        _ => drain_summary::DrainDiffStats::default(),
    };
    // TASK-997: tally what the cheap event classifier absorbed vs surfaced over
    // this drain's window, so the exit summary PROVES the event-driven
    // supervision lever with real numbers instead of asserting it. Read off the
    // same `.aida/events.jsonl` the drain emitted into, classified with the same
    // `is_actionable` predicate `aida watch` wakes on. Best-effort — an
    // unreadable stream yields an all-zero tally that renders as "none
    // recorded", never a failed exit. trace:TASK-997 | ai:claude
    let events_tally = drain_root
        .map(|root| events::tally_window(root, started))
        .unwrap_or_default();
    // TASK-1334: PRs this drain opened and did not merge in its own window —
    // shelved members' open PRs. Without them the summary reads "0 shipped ·
    // diff +0 -0" for a wave that produced real PRs. Derived from the same
    // event window the tally above reads. trace:TASK-1334 | ai:claude
    let open_prs = drain_root
        .map(|root| events::open_prs_window(root, started))
        .unwrap_or_default();
    let summary = drain_summary::DrainSummary {
        kind: kind.to_string(),
        label,
        outcome,
        tallies,
        cumulative_tokens,
        diff,
        open_prs,
        elapsed_secs: elapsed.as_secs(),
        events: events_tally,
    };
    let ts = chrono::Utc::now().to_rfc3339();
    let sha = build_sha_short();
    let role = std::env::var("AIDA_SESSION_ROLE")
        .ok()
        .filter(|s| !s.is_empty());
    let vendor = drain_root
        .map(session::resolve_headless_vendor)
        .unwrap_or(session::HeadlessVendor::Claude)
        .as_str();
    let run_key = drain_root
        .map(|root| drain_state::current_context(root).1)
        .filter(|key| !key.is_empty())
        .unwrap_or_else(|| {
            format!(
                "invocation:{}:{}",
                drain_invocation.pid,
                drain_invocation
                    .process_started_at
                    .as_deref()
                    .unwrap_or("unknown")
            )
        });
    let record = summary.to_usage_value(&ts, sha.as_deref(), role.as_deref(), vendor, &run_key);
    if json {
        // Machine consumers read JSONL — the record is a distinct
        // `"event":"drain_summary"` line alongside the per-emitter object.
        println!("{record}");
    } else {
        eprint!("\n{}", summary.render());
    }
    // Persist to ~/.aida/usage.jsonl so `aida usage` can chart cost-per-drain.
    // TASK-1165: the caller's injected root, not a cwd-derived one.
    // trace:TASK-1165 | ai:claude
    let project_root = drain_root;
    if usage::is_enabled(project_root) {
        usage::append_value(&record);
    }
    // STORY-730: persist the compact outcome to `.aida/last-drain.json` so the
    // next bare `aida status` can LEAD with a "since you were away" banner —
    // otherwise this tally dies on the ephemeral stderr render above. Sibling of
    // the live drain-state file (kept distinct so the "presence ⇒ live-or-crashed"
    // invariant of `drain-state.json` is unaffected). Best-effort. trace:STORY-730
    if let Some(root) = project_root {
        let previous = last_drain::LastDrainOutcome::read(root);
        let _ = last_drain::LastDrainOutcome::from_summary_with_previous(
            &summary,
            &ts,
            previous.as_ref(),
            Some(drain_invocation.clone()),
        )
        .write(root);
    }
    // STORY-712: emit the terminal QueueDrained wake — the "agent is done" an
    // overnight loop waits on. Drain-level, so no spec. Best-effort, not
    // telemetry-gated (supervision substrate). trace:TASK-988 | ai:claude
    if let Some(root) = project_root {
        let (_, run_uuid) = drain_state::current_context(root);
        events::emit(
            root,
            &events::Event::new(
                None,
                run_uuid,
                events::EventKind::QueueDrained {
                    shipped: summary.tallies.shipped,
                    shelved: summary.tallies.shelved,
                    // trace:TASK-1297 | ai:claude
                    // trace:BUG-1425 | ai:codex
                    excluded_from_batch,
                    // trace:BUG-1422 | ai:codex
                    ineligible: ineligible.to_vec(),
                },
            ),
        );
        let _ = notify::passive_check(root);
    }
}

/// Real [`auto_complete::BatchDriver`] for a `nextN` drain (TASK-293). Unlike
/// [`RealBatchDriver`], the "members" are not a `batch:NAME` tag — they are
/// simply the drivable queue head, re-resolved each call. A shipped spec
/// leaves the queue, so the head advances naturally; `drain_batch`'s `--max`
/// cap (pinned to `N`) stops the drain after N specs. trace:TASK-293
pub(crate) struct RealNextNDriver<'a> {
    pub(crate) storage: &'a Storage,
    pub(crate) user_id: String,
    pub(crate) role_override: Option<String>,
    pub(crate) variant: auto_complete::AutoCompleteVariant,
    pub(crate) json: bool,
    pub(crate) permission_mode: Option<String>,
    pub(crate) no_human: Option<auto_complete::NoHumanMode>,
    /// STORY-306: advisor-escalation mode, propagated to every member.
    pub(crate) escalate_mode: auto_complete::EscalateMode,
    /// BUG-311: outer `--steal`, propagated to every member's phase 1.
    pub(crate) steal: bool,
    /// TASK-559: outer `--force-claim`, propagated to every member's phase 1.
    pub(crate) force_claim: bool,
    /// STORY-281: outer `--allow-stale-base`, propagated to every member's
    /// phase 3 pre-flight stale-base check.
    pub(crate) allow_stale_base: bool,
    /// STORY-429: outer `--no-auto-rebase`, propagated to every member.
    pub(crate) no_auto_rebase: bool,
    /// TASK-966: project root + drain-start, for the `--max-tokens` meter. `None`
    /// when no token cap is active, so the headless-log scan is skipped.
    pub(crate) token_meter: Option<(std::path::PathBuf, std::time::SystemTime)>,
    /// BUG-862: entries routed to a different role that this implementer drain
    /// skipped before launching any phase-1 session.
    pub(crate) role_skipped: Vec<(String, String)>,
    pub(crate) seen_role_skips: std::collections::HashSet<String>,
    // trace:STORY-1091 trace:ADR-28 | ai:codex
    pub(crate) pipeline_depth: usize,
    // trace:STORY-1091 trace:ADR-28 | ai:codex
    pub(crate) pipelined_children: std::collections::HashMap<usize, std::process::Child>,
    // trace:STORY-1091 trace:ADR-28 | ai:codex
    pub(crate) pipelined_result_paths: std::collections::HashMap<usize, std::path::PathBuf>,
    // trace:STORY-1091 trace:ADR-28 | ai:codex
    pub(crate) next_pipelined_handle: usize,
}

impl auto_complete::BatchDriver for RealNextNDriver<'_> {
    // trace:BUG-1980 | ai:claude
    fn stop_requested(&mut self) -> bool {
        crate::drain_cmd::stop_requested_from_env()
    }

    fn next_head(&mut self) -> Option<String> {
        let (pick, role_skipped, blocked_skipped) =
            resolve_next_n_head(self.storage, &self.user_id, self.role_override.as_deref());
        for (spec, role) in &role_skipped {
            if self.seen_role_skips.insert(spec.clone()) {
                eprintln!("skipped {spec} — routed for {role}");
                self.role_skipped
                    .push((spec.clone(), format!("routed for {role}")));
            }
        }
        // BUG-1608: a dependent whose prerequisite is not Completed is
        // skipped and reported with its blocker. The reason is refreshed when
        // the blocker's state moves (e.g. In Progress → Needs Attention after
        // a shelve) so the summary names the blocker's final state.
        // trace:BUG-1608 | ai:claude
        for (spec, reason) in &blocked_skipped {
            match self.role_skipped.iter_mut().find(|(s, _)| s == spec) {
                Some(entry) if &entry.1 != reason => {
                    eprintln!("skipped {spec} — {reason}");
                    entry.1 = reason.clone();
                }
                Some(_) => {}
                None => {
                    eprintln!("skipped {spec} — {reason}");
                    self.role_skipped.push((spec.clone(), reason.clone()));
                }
            }
        }
        let pick = pick.map(|pick| pick.spec);
        // A dependent skipped earlier whose blocker has since Completed is
        // picked now — it is no longer "skipped". trace:BUG-1608 | ai:claude
        if let Some(spec) = &pick {
            self.role_skipped.retain(|(s, _)| s != spec);
        }
        pick
    }

    fn run_spec(&mut self, spec: &str) -> auto_complete::OrchestrationResult {
        // BUG-660: retry a transient/retryable per-spec failure (locked cache,
        // GH-API blip) through an exponential backoff instead of hammering or
        // immediately shelving — an unattended drive rides out a brief blip.
        // trace:BUG-660 | ai:claude
        drive_robustness::run_with_transient_backoff(|| {
            run_auto_complete(
                self.storage,
                &self.user_id,
                spec,
                self.variant,
                self.json,
                self.permission_mode.as_deref(),
                self.no_human,
                self.escalate_mode,
                // STORY-301: a batch / nextN member does not own the drain-state
                // file — the batch orchestrator created it.
                false,
                self.steal,
                self.force_claim,
                self.allow_stale_base,
                self.no_auto_rebase,
                None,
            )
        })
    }

    // TASK-966: cumulative reported tokens across this drain's headless logs.
    fn cumulative_tokens(&mut self) -> u64 {
        match &self.token_meter {
            Some((root, since)) => sum_headless_log_tokens(root, *since),
            None => 0,
        }
    }

    // trace:BUG-1930 | ai:claude
    fn environment_halt_after(&self) -> usize {
        crate::find_project_root()
            .map(|root| crate::orchestrator::OrchestratorConfig::load(&root).environment_halt_after)
            .unwrap_or(auto_complete::DEFAULT_ENVIRONMENT_HALT_AFTER)
    }
}

impl RealNextNDriver<'_> {
    // trace:STORY-1091 trace:ADR-28 | ai:codex
    // trace:BUG-1805 | ai:claude
    pub(crate) fn child_common_args(
        &self,
        spec: &str,
        mode: auto_complete::AutoCompleteVariant,
    ) -> Vec<String> {
        pipelined_child_common_args(
            spec,
            mode,
            self.json,
            self.permission_mode.as_deref(),
            self.no_human,
            self.escalate_mode,
            self.steal,
            self.force_claim,
            self.allow_stale_base,
            self.no_auto_rebase,
        )
    }
}

impl auto_complete::PipelinedBatchDriver for RealNextNDriver<'_> {
    fn pipeline_depth(&self) -> usize {
        self.pipeline_depth
    }

    fn start_spec_through_ci(&mut self, spec: &str) -> auto_complete::PipelinedHandle {
        let handle = auto_complete::PipelinedHandle(self.next_pipelined_handle);
        self.next_pipelined_handle += 1;
        // trace:TASK-1603 | ai:codex
        // The child registers its run before bumping status. Pre-bumping here
        // creates an InProgress/no-lease window before it has ownership.
        let exe = aida_exe_path();
        let result_path = find_main_worktree_root()
            .unwrap_or_else(|_| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            })
            .join(".aida")
            .join("pipelined-results")
            .join(format!("{}-{}.json", spec, handle.0));
        let mut cmd = std::process::Command::new(exe);
        // BUG-1568: the child takes the drain lock at its top while the parent
        // batch drive still holds it, so it must BORROW. Without that it is
        // refused with "a drain is already running (pid <parent>)", the batch
        // driver reads the refusal as a phase failure of the member, and the
        // whole batch stops having shipped nothing.
        //
        // This is BUG-748's defect in a second place: that fix taught the
        // INTEGRATOR child to borrow, and the pipelined batch child never got
        // the same treatment. It only became reachable when pipeline_depth was
        // raised from 1 to 2 — at depth 1 no child overlaps a live parent lock.
        // Measured after that change: 221 consecutive drains shipping nothing
        // over 17.5 hours. trace:BUG-1568 trace:BUG-748 | ai:claude
        cmd.args(self.child_common_args(spec, auto_complete::AutoCompleteVariant::ThroughCi));
        for (k, v) in pipelined_child_env(&result_path) {
            cmd.env(k, v);
        }
        match cmd.spawn_retrying_etxtbsy() {
            Ok(child) => {
                self.pipelined_children.insert(handle.0, child);
                self.pipelined_result_paths.insert(handle.0, result_path);
                handle
            }
            Err(e) => {
                eprintln!(
                    "{} could not start pipelined implementer for {}: {}",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    spec,
                    e
                );
                handle
            }
        }
    }

    fn wait_spec_through_ci(
        &mut self,
        handle: auto_complete::PipelinedHandle,
    ) -> auto_complete::OrchestrationResult {
        let Some(mut child) = self.pipelined_children.remove(&handle.0) else {
            return auto_complete::OrchestrationResult::failed(auto_complete::Phase::Implementer);
        };
        let result_path = self.pipelined_result_paths.remove(&handle.0);
        // The sidecar is named `<spec>-<handle>.json`, and the handle is
        // numeric, so the last `-` separates them. Spec ids contain `-`
        // themselves (BUG-1462), which is why this splits from the RIGHT.
        // trace:BUG-1570 | ai:claude
        let spec_for_report = result_path
            .as_deref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .and_then(|s| s.rsplit_once('-').map(|(spec, _)| spec.to_string()))
            .unwrap_or_else(|| "the dispatched spec".to_string());
        let read_sidecar = || {
            result_path
                .as_deref()
                .and_then(read_pipelined_child_result_sidecar)
        };
        match child.wait() {
            Ok(status) => classify_pipelined_child_outcome(
                &spec_for_report,
                status.code(),
                status.success(),
                read_sidecar,
            ),
            // `wait()` itself failed, so there is no status to read at all.
            Err(_) => classify_pipelined_child_outcome(&spec_for_report, None, false, read_sidecar),
        }
    }

    fn finish_spec_after_ci(&mut self, spec: &str) -> auto_complete::OrchestrationResult {
        if i32::from(self.variant.last_phase()) <= auto_complete::Phase::Ci.index() {
            return auto_complete::OrchestrationResult::ok();
        }
        let project_root = match find_main_worktree_root() {
            Ok(root) => root,
            Err(_) => {
                return auto_complete::OrchestrationResult::failed(auto_complete::Phase::Reviewer)
            }
        };
        let lookup = detect_open_pr_for_spec_via_forge(&project_root, spec);
        let (branch, pr) = match lookup {
            PrLookup::Found(pr) => (pr.head_branch, Some(pr.number as u32)),
            _ => return auto_complete::OrchestrationResult::failed(auto_complete::Phase::Reviewer),
        };
        let resume = Some(ResumeEntry {
            // Re-probe phase 2 in this process so the reviewer carries a
            // head-bound CI conclusion, even if the pipelined child already
            // observed green. trace:BUG-1460 | ai:codex
            start_phase: auto_complete::Phase::Ci,
            branch,
            pr,
            head_sha: None,
            from_pr: true,
            ci_terminal_sha: None,
            ci_terminal_green: None,
        });
        run_auto_complete(
            self.storage,
            &self.user_id,
            spec,
            self.variant,
            self.json,
            self.permission_mode.as_deref(),
            self.no_human,
            self.escalate_mode,
            false,
            self.steal,
            self.force_claim,
            self.allow_stale_base,
            self.no_auto_rebase,
            resume,
        )
    }
}

/// Entry point for `aida queue work nextN --auto-complete` (TASK-293). Drains
/// the next `N` items from the queue head. Depth 1 keeps the historical
/// one-full-lifecycle per member loop; depth >1 starts later implementer/CI
/// children while this parent serializes reviewer/merge/pull/build for ready
/// PRs. Never returns.
// trace:TASK-293 STORY-1091 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_auto_complete_next_n(
    storage: &Storage,
    user_id: &str,
    n: usize,
    variant: auto_complete::AutoCompleteVariant,
    json: bool,
    permission_mode: Option<&str>,
    role_override: Option<&str>,
    no_human: Option<auto_complete::NoHumanMode>,
    escalate_mode: auto_complete::EscalateMode,
    // BUG-311: outer `--steal`, threaded to every drained member's phase 1.
    steal: bool,
    // TASK-559: outer `--force-claim`, threaded to every drained member's phase 1.
    force_claim: bool,
    // STORY-281: outer `--allow-stale-base`, threaded to every drained
    // member's phase 3 pre-flight stale-base check.
    allow_stale_base: bool,
    // STORY-429: outer `--no-auto-rebase`, threaded to every drained member.
    no_auto_rebase: bool,
    // EPIC-28: outer `--max-failures` cap. trace:EPIC-28 | ai:claude
    max_failures: Option<usize>,
    // TASK-966: hard budget caps for the whole drain. trace:TASK-966 | ai:claude
    caps: &drain_caps::DrainCaps,
) -> ! {
    if !json {
        eprintln!();
        eprintln!(
            "{} {} {}",
            "🚀".bold(),
            format!("auto-complete: next {n}").bold(),
            format!("({})", variant.describe()).dimmed()
        );
        // Acceptance: when N exceeds the queue length, drain all available
        // and say so up front rather than silently shipping fewer than asked.
        if let Ok(drivable) = drivable_queued_count(storage, user_id, role_override) {
            if drivable < n {
                eprintln!(
                    "  {} only {} drivable item{} queued — draining those",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    drivable,
                    if drivable == 1 { "" } else { "s" }
                );
            }
        }
    }

    // STORY-301: write the drain-state file — the drivable queue head, capped
    // at N, is the member list. Best-effort. trace:STORY-301 | ai:claude
    //
    // TASK-1490: apply the same `aida_core::pickability::pickability()`
    // verdict dispatch is gated on (BUG-1608) before capping at N. A
    // `BlockedBy` dependent is skipped here exactly as `resolve_next_n_head`
    // skips it once the live drain reaches it, so the state file never
    // predicts a member the drain will not actually run.
    let drain_root = find_main_worktree_root().ok();
    if let Some(root) = &drain_root {
        if let Ok(candidates) =
            auto_complete_head_candidates_with_blocked(storage, user_id, role_override)
        {
            let specs: Vec<String> = candidates
                .into_iter()
                .filter(|(_, status, _)| auto_complete_head_drivable(status))
                // trace:TASK-1490 | ai:claude
                .filter(|(_, _, blocked)| blocked.is_none())
                .take(n)
                .map(|(id, _, _)| id)
                .collect();
            let pipeline_depth = DrainTuning::resolve(root).pipeline_depth();
            let _ = drain_state::DrainState::new_next_n(n, &specs)
                .with_pipeline_depth(pipeline_depth)
                .write(root);
        }
    }

    // TASK-967: capture the drain origin (wall clock + base HEAD) up front so
    // the exit summary can report token spend, diff stats, and elapsed time
    // regardless of whether a token cap is active. trace:TASK-967 | ai:claude
    let drain_started = std::time::SystemTime::now();
    let drain_invocation = last_drain::DrainInvocation::capture();
    let drain_clock = std::time::Instant::now();
    let drain_base_sha = drain_root.as_deref().and_then(current_branch_head_sha);
    // TASK-966: arm the token meter only when a `--max-tokens` cap is set.
    let token_meter = caps
        .max_tokens
        .and(drain_root.clone())
        .map(|root| (root, drain_started));
    let pipeline_depth = drain_root
        .as_deref()
        .map(|root| DrainTuning::resolve(root).pipeline_depth())
        .unwrap_or_else(drain_state::default_pipeline_depth);
    let mut driver = RealNextNDriver {
        storage,
        user_id: user_id.to_string(),
        role_override: role_override.map(canonical_role_name),
        variant,
        json,
        permission_mode: permission_mode.map(|s| s.to_string()),
        no_human,
        escalate_mode,
        steal,
        force_claim,
        allow_stale_base,
        no_auto_rebase,
        token_meter,
        role_skipped: Vec::new(),
        seen_role_skips: std::collections::HashSet::new(),
        pipeline_depth,
        pipelined_children: std::collections::HashMap::new(),
        pipelined_result_paths: std::collections::HashMap::new(),
        next_pipelined_handle: 1,
    };
    // EPIC-28: apply the same default failure cap as the batch path.
    // trace:EPIC-28 | ai:claude
    let max_failures = max_failures.or(Some(DEFAULT_MAX_FAILURES));
    // TASK-966: thread the hard budget caps through the drain. A cap stop is a
    // clean `MaxReached` carrying the reason in `cap_stop`. trace:TASK-966
    let mut cap_stop = None;
    let mut result = auto_complete::drain_batch_pipelined_with_caps(
        &mut driver,
        Some(n),
        max_failures,
        caps,
        drain_clock,
        &mut cap_stop,
    );
    result.skipped.extend(driver.role_skipped);
    // An empty queue (nothing shipped, nothing punted, nothing to drain) is a
    // user error — surface it with a non-zero exit even though `drain_batch`
    // reports it as a clean `Drained`, matching the `--batch` drain. A drain
    // where every member punted is *not* empty (STORY-276).
    let exit_code = if let Some(stop) = &cap_stop {
        // TASK-966: a budget cap stopped the drain — report why, exit distinctly.
        emit_drain_cap_stop(stop, json);
        DRAIN_CAP_EXIT_CODE
    } else if matches!(result.outcome, auto_complete::BatchDrainOutcome::Drained)
        && result.shipped.is_empty()
        && result.punted.is_empty()
        && result.escalated.is_empty()
    {
        1
    } else {
        result.exit_code
    };
    // TASK-1456: resolve which of the (otherwise silently skipped) queued
    // candidates are Done-with-a-still-live-refusal, so an all-refused idle
    // wave surfaces them instead of just "nothing to drive". Best-effort —
    // a resolution failure must not change the drain's exit code.
    // trace:TASK-1456 | ai:claude
    let rework_needed =
        resolve_queue_rework_needed(storage, user_id, role_override).unwrap_or_default();
    emit_next_n_drain_summary(n, &result, exit_code, json, &rework_needed);
    // TASK-967: permanent exit summary + cost-per-drain telemetry.
    finalize_drain_summary(
        "next-n",
        format!("next {n}"),
        &result.outcome,
        cap_stop.as_ref(),
        drain_summary::DrainTallies {
            shipped: result.shipped.len(),
            shelved: result.shelved.len(),
            skipped: result.skipped.len(),
            punted: result.punted.len(),
            escalated: result.escalated.len(),
        },
        drain_root.as_deref(),
        drain_base_sha.as_deref(),
        drain_started,
        drain_clock.elapsed(),
        &drain_invocation,
        json,
        // TASK-1297: a nextN drain has no `--batch` filter, so there is
        // nothing excluded to report. trace:TASK-1297 | ai:claude
        Some(0),
        &[],
    );
    // STORY-301: clean exit removes the drain-state file; a crash leaves it.
    if let Some(root) = &drain_root {
        let _ = drain_state::DrainState::clear(root);
    }
    std::process::exit(exit_code);
}

/// TASK-966: announce a clean budget-cap stop — the one-line reason (human) or a
/// JSON event (machine). The in-flight head stays queued; the operator re-runs
/// the drain to continue once the budget refreshes.
// trace:TASK-966 | ai:claude
pub(crate) fn emit_drain_cap_stop(stop: &drain_caps::CapStop, json: bool) {
    if json {
        let obj = serde_json::json!({
            "event": "drain-cap-stop",
            "cap": stop.flag(),
            "reason": stop.reason(),
        });
        println!("{obj}");
    } else {
        eprintln!();
        eprintln!(
            "  {} {}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
            stop.reason()
        );
    }
}

/// Print the closing summary of a `nextN --auto-complete` drain (TASK-293):
/// what shipped, where it stopped, and what is left queued. The per-spec
/// failure epilogue + recovery hint are already printed by `orchestrate`;
/// this adds the drain-level framing. trace:TASK-293 | ai:claude
pub(crate) fn emit_next_n_drain_summary(
    n: usize,
    result: &auto_complete::BatchDrainResult,
    exit_code: i32,
    json: bool,
    // TASK-1456: Done specs the drain skipped because they carry a
    // still-live review refusal — named here so an all-refused wave reports
    // rework instead of going quiet. trace:TASK-1456 | ai:claude
    rework_needed: &[events::IneligibleBatchMember],
) {
    use auto_complete::BatchDrainOutcome;

    if json {
        let outcome = batch_outcome_label(&result.outcome);
        let mut obj = serde_json::Map::new();
        obj.insert(
            "event".to_string(),
            serde_json::Value::String("next-n-drain".into()),
        );
        obj.insert(
            "requested".to_string(),
            serde_json::Value::Number((n as u64).into()),
        );
        obj.insert(
            "outcome".to_string(),
            serde_json::Value::String(outcome.into()),
        );
        obj.insert(
            "shipped".to_string(),
            serde_json::Value::Array(
                result
                    .shipped
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "shipped_count".to_string(),
            serde_json::Value::Number((result.shipped.len() as u64).into()),
        );
        // STORY-276: punted members — parked in Needs Attention, not shipped.
        obj.insert(
            "punted".to_string(),
            serde_json::Value::Array(
                result
                    .punted
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "punted_count".to_string(),
            serde_json::Value::Number((result.punted.len() as u64).into()),
        );
        // STORY-306: escalated members — left for a human, not shipped.
        obj.insert(
            "escalated".to_string(),
            serde_json::Value::Array(
                result
                    .escalated
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "escalated_count".to_string(),
            serde_json::Value::Number((result.escalated.len() as u64).into()),
        );
        obj.insert(
            "stopped_at".to_string(),
            match &result.stopped_at {
                Some(s) => serde_json::Value::String(s.clone()),
                None => serde_json::Value::Null,
            },
        );
        obj.insert(
            "exit_code".to_string(),
            serde_json::Value::Number(exit_code.into()),
        );
        // trace:BUG-1930 | ai:claude
        obj.insert(
            "halt_reason".to_string(),
            batch_halt_reason(&result.outcome),
        );
        // TASK-1456: mirror the `--batch` drain's `ineligible` field so a
        // machine consumer can tell "genuinely nothing queued" apart from
        // "queued, but every candidate needs a rework round".
        // trace:TASK-1456 | ai:claude
        obj.insert(
            "rework_needed".to_string(),
            serde_json::to_value(rework_needed)
                .unwrap_or_else(|_| serde_json::Value::Array(vec![])),
        );
        println!("{}", serde_json::Value::Object(obj));
        return;
    }

    let shipped_list = if result.shipped.is_empty() {
        "(none)".to_string()
    } else {
        result.shipped.join(", ")
    };
    let count = result.shipped.len();
    let plural = if count == 1 { "" } else { "s" };
    eprintln!();
    match &result.outcome {
        // TASK-1456: an all-refused queue must not read the same as a
        // genuinely empty one — name the rework candidates and the working
        // recovery route instead of the generic "nothing to drive".
        // trace:TASK-1456 | ai:claude
        BatchDrainOutcome::Drained
            if result.shipped.is_empty()
                && result.punted.is_empty()
                && result.escalated.is_empty()
                && !rework_needed.is_empty() =>
        {
            eprintln!(
                "{} {}",
                "⏸".yellow().bold(),
                next_n_rework_idle_message(rework_needed),
            );
        }
        BatchDrainOutcome::Drained
            if result.shipped.is_empty()
                && result.punted.is_empty()
                && result.escalated.is_empty() =>
        {
            eprintln!(
                "{} no drivable items in the queue — nothing to drive",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold()
            );
        }
        BatchDrainOutcome::Drained => {
            eprintln!(
                "{} next {n} drained — queue exhausted, {count} spec{plural} shipped: {}",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                shipped_list.bold()
            );
        }
        BatchDrainOutcome::MaxReached => {
            eprintln!(
                "{} next {n} done — {count} spec{plural} shipped: {}",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                shipped_list.bold()
            );
            eprintln!(
                "  {} more items remain queued — re-run `aida queue work next{n} --auto-complete` to continue",
                "→".dimmed()
            );
        }
        BatchDrainOutcome::Failed(phase) => {
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            eprintln!(
                "{} next {n} drain stopped at {} (phase {} failed)",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                stopped.bold(),
                phase.index()
            );
            eprintln!(
                "  {} already shipped ({count}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
            eprintln!(
                "  {} the rest of the queue is untouched — fix {stopped} (hint above), then re-run",
                "→".dimmed()
            );
        }
        BatchDrainOutcome::Stalled => {
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            eprintln!(
                "{} next {n} drain stopped — {} stayed at the head after a successful run \
                 (queue did not advance)",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                stopped.bold()
            );
            eprintln!(
                "  {} already shipped ({count}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
            eprintln!(
                "  {} check {stopped}'s status (`aida show {stopped}`) — it may not have \
                 been dequeued",
                "→".dimmed()
            );
        }
        // BUG-245: phase 1 shipped a different spec than the dispatched head.
        // Credit the truth, name what stayed queued. trace:BUG-245 | ai:claude
        BatchDrainOutcome::Mismatched {
            dispatched,
            shipped,
        } => {
            eprintln!(
                "{} next {n} drain stopped — phase 1 was dispatched for {} \
                 but the PR credited {}",
                crate::glyph(crate::glyphs::Glyph::Info).cyan().bold(),
                dispatched.bold(),
                shipped.bold(),
            );
            eprintln!(
                "  {} already shipped ({count}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
            eprintln!(
                "  {} {} is still queued — pick it back up with `aida queue work {}`, \
                 or remove it with `aida queue remove {}` if {} subsumed it",
                "→".dimmed(),
                dispatched,
                dispatched,
                dispatched,
                shipped,
            );
        }
        // BUG-257: a transient GH-API outage during phase-1 PR lookup paused
        // the drain. The spec stays in its current state for retry.
        // trace:BUG-257 | ai:claude
        BatchDrainOutcome::Inconclusive => {
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            eprintln!(
                "{} next {n} drain paused at {} — phase 1 inconclusive \
                 (GH API unreachable)",
                "⏸".yellow().bold(),
                stopped.bold(),
            );
            eprintln!(
                "  {} already shipped ({count}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
            eprintln!(
                "  {} transient — retry once the API is reachable: \
                 `gh api /rate_limit` then re-run",
                "→".dimmed()
            );
        }
        // BUG-250: a member deliberately held its PR — pause for the gate.
        // trace:BUG-250 | ai:claude
        BatchDrainOutcome::Held => {
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            eprintln!(
                "{} next {n} drain paused at {} — PR deliberately held",
                "⏸".yellow().bold(),
                stopped.bold(),
            );
            eprintln!(
                "  {} already shipped ({count}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
            eprintln!(
                "  {} run your gate, open the PR (`gh pr create`), then re-run",
                "→".dimmed()
            );
        }
        // trace:BUG-1930 | ai:claude
        BatchDrainOutcome::EnvironmentBlocked { cause } => {
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            emit_environment_blocked_lines(cause, stopped, &shipped_list);
        }
        // EPIC-28 trace:EPIC-28 | ai:claude
        BatchDrainOutcome::DrainedWithShelved => {
            eprintln!(
                "{} next {n} drained with shelved members — \
                 {count} spec{plural} shipped: {}",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                shipped_list.bold()
            );
        }
    }
    // STORY-276: name the members a headless implementer punted — the drain
    // advanced past them, but they parked in Needs Attention rather than
    // shipping, so they need advisor triage.
    if !result.punted.is_empty() {
        let pn = result.punted.len();
        eprintln!(
            "  {} {pn} spec{} punted to Needs Attention: {} — triage with \
             `aida findings list`",
            "⏸".yellow(),
            if pn == 1 { "" } else { "s" },
            result.punted.join(", ")
        );
    }
    // STORY-306: name the members the drain escalated to a human.
    if !result.escalated.is_empty() {
        let en = result.escalated.len();
        eprintln!(
            "  {} {en} spec{} escalated to a human: {} — triage with \
             `aida findings list`",
            "⏸".yellow(),
            if en == 1 { "" } else { "s" },
            result.escalated.join(", ")
        );
    }
    // EPIC-28: shelved + skipped members. trace:EPIC-28 | ai:claude
    if !result.shelved.is_empty() {
        let sn = result.shelved.len();
        eprintln!(
            "  {} {sn} spec{} shelved on phase failure: {} — triage with \
             `aida findings list`",
            "⏸".yellow(),
            if sn == 1 { "" } else { "s" },
            result.shelved.join(", ")
        );
    }
    if !result.skipped.is_empty() {
        let kn = result.skipped.len();
        let render: Vec<String> = result
            .skipped
            .iter()
            .map(|(spec, reason)| format!("{spec} ({reason})"))
            .collect();
        eprintln!(
            "  {} {kn} dependent spec{} skipped: {}",
            "⤳".yellow(),
            if kn == 1 { "" } else { "s" },
            render.join(", ")
        );
    }
}

/// TASK-266: append this `--auto-complete` run to
/// `~/.aida/auto-complete.jsonl` and, on a phase failure, auto-draft a Draft
/// BUG for the failure. Honours the same opt-out as `aida usage`
/// (`AIDA_TELEMETRY=0` / `[telemetry] enabled = false`). Best-effort
/// throughout — telemetry must never break or delay the orchestrator exit.
/// trace:TASK-266 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_auto_complete_run(
    driver: &RealPhaseDriver,
    project_root: &std::path::Path,
    spec: &str,
    variant: auto_complete::AutoCompleteVariant,
    result: &auto_complete::OrchestrationResult,
    started_at: chrono::DateTime<chrono::Utc>,
    completed_at: chrono::DateTime<chrono::Utc>,
    json: bool,
    lifecycle_skip: auto_complete::LifecycleSkip,
) {
    if !usage::is_enabled(Some(project_root)) {
        return;
    }

    let phase_durations: Vec<auto_complete_telemetry::PhaseDuration> = result
        .phase_durations
        .iter()
        .map(|(phase, ms)| auto_complete_telemetry::PhaseDuration {
            phase: phase.index() as u8,
            slug: phase.slug().to_string(),
            elapsed_ms: *ms as u64,
        })
        .collect();

    let mut event = auto_complete_telemetry::AutoCompleteEvent {
        spec_id: spec.to_string(),
        started_at: started_at.to_rfc3339(),
        completed_at: completed_at.to_rfc3339(),
        outcome: if result.exit_code == 0 {
            "success".to_string()
        } else {
            "failed".to_string()
        },
        variant: variant.slug().to_string(),
        failed_phase: result.failed_phase.map(|p| p.index() as u8),
        failure_kind: result
            .failure
            .as_ref()
            .map(|f| f.kind.cause_slug().to_string()),
        failure_message: result.failure.as_ref().map(|f| f.reason.clone()),
        phase_durations,
        total_ms: result.total_ms as u64,
        drafted_bug: None,
        failure_comment: None,
        binary_sha: build_sha_short(),
        auto_rebase: driver.auto_rebase_events.clone(),
        lifecycle_skips: lifecycle_skip.active_tokens(),
    };

    // On a phase failure, attach a Draft BUG. Reuse an open BUG for the same
    // spec / phase / kind rather than spamming the backlog when a still-broken
    // `--auto-complete` is re-run. trace:BUG-864 | ai:codex
    if let (Some(phase), Some(failure)) = (result.failed_phase, result.failure.as_ref()) {
        match absorb_open_failure_bug(project_root, spec, phase, failure, &completed_at) {
            Some(existing) => {
                if !json {
                    eprintln!(
                        "  {} this failure recurs — tracked in {} (no new BUG drafted)",
                        "📋".dimmed(),
                        existing.cyan(),
                    );
                }
                event.drafted_bug = Some(existing);
            }
            // BUG-657: suppress the auto-file entirely when the failure is
            // ENVIRONMENTAL (disk full / OOM) — the auto-draft text itself tells
            // the triager to reject those, so filing one just creates a Draft to
            // immediately reject. The telemetry line still records the failure.
            None if auto_complete::is_environmental_failure(&failure.reason) => {
                if !json {
                    eprintln!(
                        "  {} environmental failure (disk/OOM) — not auto-drafting a BUG \
                         (fix the host, then re-drive)",
                        "📋".dimmed(),
                    );
                }
            }
            // BUG-908: a lease-conflict shelve already names the blocking
            // lease/worktree in the spec's FailureReason. Auto-drafting a new
            // BUG for that same typed cause recreated the spam loop this fix
            // is meant to stop.
            // trace:BUG-908 | ai:codex
            None if failure.kind == auto_complete::FailureKind::LeaseConflict => {
                if !json {
                    eprintln!(
                        "  {} lease-conflict failure — not auto-drafting a BUG \
                         (inspect the named lease/worktree, then re-drive)",
                        "📋".dimmed(),
                    );
                }
            }
            // BUG-1930: refused writes are environmental; the shelve already
            // names the denied tools. trace:BUG-1930 | ai:claude
            None if failure.kind == auto_complete::FailureKind::PermissionDenied => {
                if !json {
                    eprintln!(
                        "  {} permission-denied failure — not auto-drafting a BUG \
                         (fix the sandbox / permission mode, then re-drive)",
                        "📋".dimmed(),
                    );
                }
            }
            // TASK-1564: the narrative record is a comment on the parent spec,
            // not a new draft spec. Nothing else moves — the non-zero exit, the
            // ledger line appended below, and the seat routing are untouched.
            // trace:TASK-1564 | ai:claude
            None => {
                let hint = auto_complete::recovery_hint(
                    phase,
                    failure.kind,
                    &auto_complete::PhaseDriver::hint_context(driver),
                );
                if let Some(comment_id) = note_auto_complete_failure_on_parent(
                    project_root,
                    spec,
                    phase,
                    failure,
                    &hint,
                    &event,
                ) {
                    if !json {
                        eprintln!(
                            "  {} recorded this failure as a comment on {} \
                             (no new spec filed) — read it: `aida comment list {}`",
                            "📋".dimmed(),
                            spec.cyan(),
                            spec,
                        );
                    }
                    event.failure_comment = Some(comment_id);
                }
            }
        }
    }

    auto_complete_telemetry::append_event(&event);

    // SPIKE-67 slice 3: record any stated-rule violation this drain observed
    // (CI red on fmt/clippy/provenance, a reviewer RequestChanges on a rule, or
    // a punt citing a rule) — the real-time gate-vs-rule evidence path. Reuses
    // the failure the orchestrator already classified; observe-only, opt-in,
    // privacy-floor-preserving, best-effort. trace:SPIKE-67 | ai:claude
    let outcome = rule_violation::DrainOutcome {
        failed_phase_slug: result.failed_phase.map(|p| p.slug()),
        failure_kind: result.failure.as_ref().map(|f| f.kind.cause_slug()),
        failure_message: result.failure.as_ref().map(|f| f.reason.as_str()),
        punt_reason: result.punt_reason.as_deref(),
    };
    rule_violation::record(
        project_root,
        &outcome,
        spec,
        driver.no_human.is_some(),
        variant.slug(),
        build_sha_short(),
    );
}

// TASK-1564 moved the recurrence bump into `drain_failure_note` so the note
// path and the legacy auto-drafted-BUG absorb path share one implementation.
// trace:TASK-1564 | ai:claude
use drain_failure_note::increment_auto_failure_attempts;

/// BUG-864: absorb a recurring phase failure into the open auto-drafted BUG
/// already tracking that `(spec, phase, failure-kind)` signature. The store is
/// authoritative for the dedupe window: Draft/Approved records absorb, while a
/// triaged Rejected/Completed record ends the window and lets the next failure
/// file fresh. The older JSONL dedupe remains only as a fallback and is verified
/// against the same open-status rule before reuse.
// trace:TASK-266 trace:BUG-657 trace:BUG-864 | ai:codex
pub(crate) fn absorb_open_failure_bug(
    project_root: &std::path::Path,
    spec: &str,
    phase: auto_complete::Phase,
    failure: &auto_complete::PhaseFailure,
    completed_at: &chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    if let Some(id) = absorb_open_failure_bug_from_store(
        project_root,
        spec,
        phase.index() as u8,
        phase.slug(),
        failure.kind.cause_slug(),
        &completed_at.to_rfc3339(),
    ) {
        return Some(id);
    }

    let cutoff = chrono::Utc::now() - chrono::Duration::hours(24);
    let candidate = auto_complete_telemetry::dedup_failure_bug(
        &auto_complete_telemetry::read_events(),
        spec,
        phase.index() as u8,
        failure.kind.cause_slug(),
        cutoff,
    )?;
    open_auto_failure_bug_by_id(project_root, &candidate).map(|_| candidate)
}

pub(crate) fn absorb_open_failure_bug_from_store(
    project_root: &std::path::Path,
    spec: &str,
    phase_index: u8,
    phase_slug: &str,
    failure_kind: &str,
    latest_at: &str,
) -> Option<String> {
    let Some(store_path) = detect_distributed_store_from(project_root) else {
        return None;
    };
    let dispenser = load_dispenser(&store_path).ok()?;
    let inner = aida_core::GitBackend::new(&store_path)
        .ok()?
        .with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path).ok()?;
    let mut candidate = backend
        .list_requirements(false)
        .ok()?
        .into_iter()
        .filter(|req| auto_failure_bug_matches(req, spec, phase_index, phase_slug, failure_kind))
        .max_by_key(|req| req.modified_at)?;
    let id = candidate.display_id();
    candidate.description = increment_auto_failure_attempts(&candidate.description, latest_at);
    candidate.modified_at = chrono::Utc::now();
    backend.update_requirement(&candidate).ok()?;
    Some(id)
}

pub(crate) fn open_auto_failure_bug_by_id(
    project_root: &std::path::Path,
    spec_id: &str,
) -> Option<aida_core::Requirement> {
    let store_path = detect_distributed_store_from(project_root)?;
    let dispenser = load_dispenser(&store_path).ok()?;
    let inner = aida_core::GitBackend::new(&store_path)
        .ok()?
        .with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path).ok()?;
    let req = backend.get_requirement_by_spec_id(spec_id).ok()??;
    if is_open_auto_failure_bug(&req) {
        Some(req)
    } else {
        None
    }
}

pub(crate) fn auto_failure_bug_matches(
    req: &aida_core::Requirement,
    spec: &str,
    phase_index: u8,
    phase_slug: &str,
    failure_kind: &str,
) -> bool {
    let expected_title =
        format!("auto-complete failure: phase {phase_index} ({phase_slug}) on {spec}");
    is_open_auto_failure_bug(req)
        && req.title == expected_title
        && auto_failure_kind_matches(req, failure_kind)
}

pub(crate) fn is_open_auto_failure_bug(req: &aida_core::Requirement) -> bool {
    matches!(
        req.status,
        aida_core::RequirementStatus::Draft | aida_core::RequirementStatus::Approved
    ) && !req.archived
        && req.req_type == aida_core::RequirementType::Bug
        && req.tags.contains("auto-complete")
        && req.tags.contains("auto-drafted")
}

pub(crate) fn auto_failure_kind_matches(req: &aida_core::Requirement, failure_kind: &str) -> bool {
    req.tags.contains(&format!("failure-kind:{failure_kind}"))
        || req
            .description
            .contains(&format!("Failure kind: `{failure_kind}`"))
}

/// Record an `--auto-complete` phase failure as a comment on the parent spec.
///
/// Replaces the TASK-266 Draft-BUG auto-file: the narrative record is the same,
/// but a comment informs the next reader of the parent instead of demanding a
/// triage disposition the way a draft spec does. A recurrence of the same
/// `(phase, failure-kind)` signature bumps the existing note's counter rather
/// than appending a second one — the BUG-864 anti-spam rule, carried over.
///
/// Returns the note's comment UUID, or `None` when the store could not be
/// resolved or written (best-effort — the JSONL ledger line still records the
/// failure either way, and the drain still exits non-zero).
// trace:TASK-1564 trace:TASK-266 trace:BUG-864 | ai:claude
pub(crate) fn note_auto_complete_failure_on_parent(
    project_root: &std::path::Path,
    spec: &str,
    phase: auto_complete::Phase,
    failure: &auto_complete::PhaseFailure,
    hint: &str,
    event: &auto_complete_telemetry::AutoCompleteEvent,
) -> Option<String> {
    let phase_index = phase.index() as u8;
    let phase_slug = phase.slug();
    let failure_kind = failure.kind.cause_slug();

    let seat = match std::env::var("AIDA_SESSION_ROLE") {
        Ok(role) if !role.trim().is_empty() => format!("{phase_slug} (session role: {role})"),
        _ => phase_slug.to_string(),
    };
    let branch = aida_core::git_ops::current_branch(project_root).ok();
    let commit = aida_core::git_ops::head_sha(project_root)
        .ok()
        .map(|sha| sha.chars().take(12).collect::<String>());
    let durations = if event.phase_durations.is_empty() {
        "  (none recorded)".to_string()
    } else {
        event
            .phase_durations
            .iter()
            .map(|d| format!("  - phase {} ({}): {} ms", d.phase, d.slug, d.elapsed_ms))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let json_line = serde_json::to_string(event).unwrap_or_default();

    let body = drain_failure_note::render(&drain_failure_note::NoteContext {
        spec,
        phase_index,
        phase_slug,
        seat: &seat,
        failure_kind,
        failure_reason: &failure.reason,
        hint,
        branch: branch.as_deref(),
        commit: commit.as_deref(),
        completed_at: &event.completed_at,
        durations: &durations,
        telemetry_json: &json_line,
    });

    let store_path = detect_distributed_store_from(project_root)?;
    let mut storage = Storage::new(&store_path);
    if let Ok(dispenser) = load_dispenser(&store_path) {
        storage = storage.with_dispenser(dispenser);
    }
    let sig = drain_failure_note::signature(phase_index, phase_slug, failure_kind);
    match comment_cmd::bump_or_add_marked_comment(
        &storage,
        spec,
        &sig,
        &body,
        "auto-complete",
        &event.completed_at,
    ) {
        Ok(id) => Some(id.to_string()),
        Err(_) => None,
    }
}

#[cfg(test)]
#[path = "tests/task_266_tests.rs"]
mod task_266_tests;

#[cfg(test)]
#[path = "tests/task_1564_drain_failure_note_tests.rs"]
mod task_1564_drain_failure_note_tests;

pub(crate) fn auto_complete_queue_add_args(spec: &str) -> Vec<&str> {
    vec!["queue", "add", spec, "--for", "implementer", "--no-scope"]
}
