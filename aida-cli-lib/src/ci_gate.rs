//! BUG-1180 / ADR-39: gate-aware, informational-aware refinement of a red CI
//! verdict.
//!
//! The forge's `watch_ci` collapses a PR's checks to one coarse `CiState`, so
//! two non-failures read as "CI failed":
//!
//! - the supervised **merge-hold** itself — `merge-hold-gate` (ADR-37 Layer 2)
//!   publishes pending (legacy publishers used red) while a hold exists, and
//!   `aida pr ship` releases it at merge time; aborting on it at CI-watch time
//!   made every supervised ship a two-command dance;
//! - an **informational** matrix (TASK-1205's Windows/macOS jobs) that shares
//!   the `Build (…)` job name with PR CI and differs only by workflow.
//!
//! This module re-reads the per-check rows through `gh`. A pending or red
//! hold is ignorable only with a local marker or confirmed forge label.
//! Other red checks are ignored only when NOT branch-protection-required AND
//! matched by the `[ci]` informational allow-list. Everything else red is a real
//! failure — so a repo with no branch protection and no allow-list keeps
//! today's "any red fails" semantics (a literal required-only rule would have
//! merged over a red `Build (ubuntu-latest)`, which this repo's protection does
//! not require — recorded on ADR-39).
//!
//! GitHub-only by construction (`gh pr checks --json`); other forges keep the
//! coarse verdict unchanged.
// trace:BUG-1180 | ai:claude

use std::path::Path;
use std::time::{Duration, Instant};

/// The ADR-37 Layer-2 required check whose pending (or legacy red) state IS the hold.
pub(crate) const HOLD_GATE_CHECK: &str = "merge-hold-gate";

/// One row of `gh pr checks <n> --json name,bucket,workflow,link`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckRow {
    pub(crate) name: String,
    pub(crate) workflow: String,
    /// `pass` | `fail` | `pending` | `skipping` | `cancel` (gh's bucket).
    pub(crate) bucket: String,
    /// Workflow run/pipeline that owns this check. Kept on the classified row
    /// so diagnostics cannot drift to an independently selected recent run.
    // trace:BUG-1298 | ai:codex
    pub(crate) run_id: Option<String>,
}

impl CheckRow {
    fn is_red(&self) -> bool {
        matches!(self.bucket.as_str(), "fail" | "cancel")
    }
    fn is_pending(&self) -> bool {
        self.bucket == "pending"
    }
}

/// `[ci]` config: which red checks are informational (never gate a merge).
/// Matching is case-insensitive with `*` wildcards; a check is informational
/// when its **workflow** matches `informational_workflows` or its **name**
/// matches `informational_checks`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CiGateConfig {
    pub(crate) informational_workflows: Vec<String>,
    pub(crate) informational_checks: Vec<String>,
}

impl Default for CiGateConfig {
    fn default() -> Self {
        Self {
            // The cross-platform workflow's jobs are named `Build (<os>)`
            // exactly like PR CI's Linux job, so the workflow name is the only
            // reliable discriminator. trace:BUG-1191 | ai:codex
            // ADR-72: that workflow is manual-only and never runs on PRs; the
            // default stays so a manual dispatch can never gate a merge.
            // trace:TASK-1723 | ai:claude
            informational_workflows: vec!["Cross-platform*".to_string()],
            informational_checks: Vec::new(),
        }
    }
}

/// Read `[ci]` from `.aida/config.toml` (absent section/keys → defaults).
pub(crate) fn read_ci_gate_config(project_root: &Path) -> CiGateConfig {
    let path = project_root.join(".aida").join("config.toml");
    match std::fs::read_to_string(path) {
        Ok(content) => parse_ci_gate_config(&content),
        Err(_) => CiGateConfig::default(),
    }
}

/// Pure parser for the `[ci]` section. Only the two array keys are read; an
/// explicitly empty array disables that allow-list.
pub(crate) fn parse_ci_gate_config(content: &str) -> CiGateConfig {
    let mut out = CiGateConfig::default();
    let mut in_ci = false;
    for raw in content.lines() {
        let line = crate::strip_toml_inline_comment(raw).trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            in_ci = rest.trim_start_matches('[').trim_end_matches(']').trim() == "ci";
            continue;
        }
        if !in_ci {
            continue;
        }
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "informational_workflows" => out.informational_workflows = parse_toml_string_array(val),
            "informational_checks" => out.informational_checks = parse_toml_string_array(val),
            _ => {}
        }
    }
    out
}

fn parse_toml_string_array(val: &str) -> Vec<String> {
    let inner = val.trim().trim_start_matches('[').trim_end_matches(']');
    inner
        .split(',')
        .map(|s| s.trim().trim_matches('"').trim_matches('\'').trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Case-insensitive `*`-wildcard match (no `?`, no classes — the allow-list
/// only ever needs prefix/suffix shapes like `Build (windows*`).
pub(crate) fn wild_match(pattern: &str, text: &str) -> bool {
    fn go(p: &[char], t: &[char]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some('*') => (0..=t.len()).any(|i| go(&p[1..], &t[i..])),
            Some(c) => t.first().is_some_and(|tc| tc == c) && go(&p[1..], &t[1..]),
        }
    }
    let p: Vec<char> = pattern.trim().to_lowercase().chars().collect();
    let t: Vec<char> = text.trim().to_lowercase().chars().collect();
    go(&p, &t)
}

impl CiGateConfig {
    fn is_informational(&self, row: &CheckRow) -> bool {
        self.informational_workflows
            .iter()
            .any(|p| wild_match(p, &row.workflow))
            || self
                .informational_checks
                .iter()
                .any(|p| wild_match(p, &row.name))
    }
}

/// What a red CI verdict really contains once each row is classified.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RedRefinement {
    /// Checks that refuse the merge: actual failures or an unconfirmed hold.
    /// These are the resolved rows, including their owning run, rather than
    /// copied names so the detail and recovery hint share one source.
    // trace:BUG-1298 | ai:codex
    pub(crate) real: Vec<CheckRow>,
    /// Red checks ignored because they are non-required AND informational.
    pub(crate) ignored_informational: Vec<String>,
    /// `merge-hold-gate` is pending or red and a local marker or confirmed forge label
    /// exists: the hold itself, released at merge — not a CI failure.
    pub(crate) hold_gate_is_the_hold: bool,
    /// `merge-hold-gate` blocks without a local marker or confirmed active
    /// label. This includes uncleared recorded holds with a removed label.
    /// Counted in `real` too; this flag adds the clearance recovery hint.
    pub(crate) hold_gate_label_lingering: bool,
    /// Relevant (non-ignored) checks still pending when classified.
    pub(crate) pending: Vec<String>,
}

impl RedRefinement {
    pub(crate) fn is_real(&self) -> bool {
        !self.real.is_empty()
    }
    pub(crate) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
    /// One line for the human / the phase log.
    pub(crate) fn describe(&self, pr: u64) -> String {
        let mut parts = Vec::new();
        if self.hold_gate_is_the_hold {
            parts.push(format!(
                "`{HOLD_GATE_CHECK}` blocks because PR-{pr} is under a supervised merge-hold — releasing at merge"
            ));
        }
        if !self.ignored_informational.is_empty() {
            parts.push(format!(
                "ignoring informational red check(s): {}",
                self.ignored_informational.join(", ")
            ));
        }
        if self.is_real() {
            parts.push(format!(
                "failing: {}",
                self.real
                    .iter()
                    .map(|row| row.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if self.hold_gate_label_lingering {
            parts.push(format!(
                "`{HOLD_GATE_CHECK}` blocks without confirmed local hold authority — inspect recorded hold labels; \
                 `aida merge-hold clear {pr}` re-syncs it"
            ));
        }
        parts.join("; ")
    }
}

/// The one check-name comparator every CI gate shares (red classification,
/// hold tolerance, train readiness): forge check names match
/// case-insensitively. trace:BUG-2401 | ai:claude
pub(crate) fn check_name_eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Direct shipping requires FULL when this workflow advertises CI tiers.
/// Match the marker to Build's run: adding ci:full on the same head leaves
/// an older fast marker visible in GitHub's per-name check rollup.
/// Untiered repositories and other forges retain their existing CI policy.
// trace:STORY-2735 | ai:codex
pub(crate) fn require_full_ship_ci(
    forge: &dyn crate::forge::Forge,
    change: &crate::forge::ChangeRef,
) -> anyhow::Result<()> {
    if forge.kind() != crate::forge::ForgeKind::GitHub {
        return Ok(());
    }
    let rows = forge.check_rows(change)?;
    let markers: Vec<_> = rows
        .iter()
        .filter(|row| row.workflow == "CI" && row.name.starts_with("ci-tier: "))
        .collect();
    if markers.is_empty() {
        return Ok(());
    }
    let build = rows
        .iter()
        .find(|row| row.workflow == "CI" && row.name == "Build (ubuntu-latest)");
    let full = build.is_some_and(|build| {
        build.bucket == "pass"
            && build
                .run_id
                .as_ref()
                .and_then(|id| id.parse::<u64>().ok())
                .is_some_and(|build_id| {
                    markers.iter().all(|marker| {
                        marker
                            .run_id
                            .as_ref()
                            .and_then(|id| id.parse::<u64>().ok())
                            .is_some_and(|marker_id| marker_id <= build_id)
                    })
                })
            && markers.iter().any(|marker| {
                marker.name == "ci-tier: full"
                    && marker.bucket == "pass"
                    && marker.run_id == build.run_id
            })
            && !markers
                .iter()
                .any(|marker| marker.name != "ci-tier: full" && marker.run_id == build.run_id)
    });
    if !full {
        return Err(crate::gate::ship::ShipRefusal::CiRed.error(format!(
            "PR-{} has FAST or unconfirmed full-tier CI; use `aida pr train` or add `ci:full` and wait for FULL CI before `aida pr ship`",
            change.id
        )));
    }
    Ok(())
}

/// Pure classification of the rows. `required` is the branch-protection
/// required-check name set (empty when the repo declares none — then nothing
/// is "non-required by policy", but the allow-list still applies because a
/// check GitHub does not require is, by definition, not required).
pub(crate) fn classify_red(
    rows: &[CheckRow],
    required: &[String],
    cfg: &CiGateConfig,
    hold_marker_present: bool,
    hold_label_present: bool,
) -> RedRefinement {
    classify_with_recorded_hold(
        rows,
        required,
        cfg,
        hold_marker_present,
        hold_label_present,
        false,
    )
}

// trace:BUG-2492 | ai:codex
fn classify_with_recorded_hold(
    rows: &[CheckRow],
    required: &[String],
    cfg: &CiGateConfig,
    hold_marker_present: bool,
    hold_label_present: bool,
    uncleared_recorded_hold: bool,
) -> RedRefinement {
    // TASK-1287: an explicit `pr ship` may confirm a label-only supervised
    // hold even when no drain-created marker exists.
    let mut out = RedRefinement::default();
    let is_required = |name: &str| required.iter().any(|r| check_name_eq(r, name));
    for row in rows {
        let informational = !is_required(&row.name) && cfg.is_informational(row);
        // trace:BUG-2492 | ai:codex — pending statuses are the supervised hold,
        // not unfinished CI. Only an uncleared recorded hold without release
        // authority refuses; an unheld inspection remains ordinary pending.
        if check_name_eq(&row.name, HOLD_GATE_CHECK)
            && (row.is_pending() || row.is_red())
            && (hold_marker_present || hold_label_present)
        {
            out.hold_gate_is_the_hold = true;
            continue;
        }
        // trace:BUG-2492 | ai:codex
        if row.is_pending() && check_name_eq(&row.name, HOLD_GATE_CHECK) && uncleared_recorded_hold
        {
            out.hold_gate_label_lingering = true;
            out.real.push(row.clone());
            continue;
        }
        if row.is_pending() {
            if !informational {
                out.pending.push(row.name.clone());
            }
            continue;
        }
        if !row.is_red() {
            continue;
        }
        if check_name_eq(&row.name, HOLD_GATE_CHECK) {
            out.hold_gate_label_lingering = true;
            out.real.push(row.clone());
            continue;
        }
        if informational {
            out.ignored_informational.push(row.name.clone());
        } else {
            out.real.push(row.clone());
        }
    }
    out
}

// trace:BUG-2492 | ai:codex
#[allow(clippy::too_many_arguments)]
fn classify_live_rows(
    project_root: &Path,
    forge: &dyn crate::forge::Forge,
    change: &crate::forge::ChangeRef,
    rows: &[CheckRow],
    required: &[String],
    cfg: &CiGateConfig,
    marker: bool,
    label: bool,
) -> anyhow::Result<RedRefinement> {
    let needs_labels = !marker
        && !label
        && rows
            .iter()
            .any(|r| r.is_pending() && check_name_eq(&r.name, HOLD_GATE_CHECK));
    let recorded = if needs_labels {
        let (active, recorded, cleared) = forge.recorded_hold_labels(project_root, change)?;
        active || (recorded && !cleared)
    } else {
        false
    };
    Ok(classify_with_recorded_hold(
        rows, required, cfg, marker, label, recorded,
    ))
}

/// Parse `gh pr checks --json name,bucket,workflow` output. `gh` exits
/// non-zero when a check failed or is pending, so callers must parse stdout
/// regardless of the exit status.
pub(crate) fn parse_check_rows(json: &str) -> Option<Vec<CheckRow>> {
    let rows: Vec<serde_json::Value> = serde_json::from_str(json).ok()?;
    Some(
        rows.iter()
            .map(|r| CheckRow {
                name: r
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                workflow: r
                    .get("workflow")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                bucket: r
                    .get("bucket")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                run_id: r
                    .get("link")
                    .and_then(|v| v.as_str())
                    .and_then(run_id_from_check_link),
            })
            .collect(),
    )
}

/// GitHub check links have the form `.../actions/runs/<run>/job/<job>`.
/// Treat any other URL as unresolved instead of guessing from branch recency.
// trace:BUG-1298 | ai:codex
fn run_id_from_check_link(link: &str) -> Option<String> {
    let suffix = link.split("/actions/runs/").nth(1)?;
    let id = suffix.split('/').next()?;
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())).then(|| id.to_string())
}

/// STORY-1166: wait until the forge has registered CI checks for `change`
/// (a freshly pushed PR whose workflows have not started yet), bounded by
/// `timeout`. `NoCi` returns immediately — nothing to wait on.
// trace:STORY-1166 | ai:claude
pub(crate) fn wait_for_checks_to_register(
    forge: &dyn crate::forge::Forge,
    change: &crate::forge::ChangeRef,
    timeout: Duration,
    poll_interval: Duration,
) -> anyhow::Result<()> {
    use crate::forge::CheckRegistration;
    let deadline = Instant::now() + timeout;
    loop {
        match forge.checks_registered(change)? {
            CheckRegistration::Registered | CheckRegistration::NoCi => return Ok(()),
            CheckRegistration::NotYet => {}
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "No CI checks registered within {}s for {}-{} — verify CI is configured for this project on {}",
                timeout.as_secs(),
                forge.kind().change_noun().to_ascii_uppercase(),
                change.id,
                forge.cli_name()
            );
        }
        std::thread::sleep(poll_interval);
    }
}

/// Classify CI for `change` (also used directly by GitHub ship): read every row the
/// forge reports, and — when nothing real is red yet but relevant checks are
/// still pending (the coarse watcher returned on the first red) — keep polling
/// until they settle or `settle_timeout` elapses. A timeout leaves the
/// still-pending names in `pending`; the caller decides (pr ship bails, the
/// drain shelves CiTimeout). While a supervised hold is present, row-read
/// failures receive two bounded retries. `Err` then means the forge exposes no
/// per-check rows (pure-git) or the read stayed unavailable; callers must not
/// turn that ambiguity into a real red failure for a supervised hold.
// trace:STORY-1166 | ai:claude (forge-routed; was gh-only under BUG-1180)
// trace:BUG-1265 | ai:codex
pub(crate) fn refine_red(
    project_root: &Path,
    forge: &dyn crate::forge::Forge,
    change: &crate::forge::ChangeRef,
    hold_marker_present: bool,
    hold_label_present: bool,
    settle_timeout: Duration,
    poll_interval: Duration,
) -> anyhow::Result<RedRefinement> {
    let cfg = read_ci_gate_config(project_root);
    let required = forge.required_check_names(change);
    let deadline = Instant::now() + settle_timeout;
    let supervised_hold = hold_marker_present || hold_label_present;
    let mut row_read_failures = 0usize;
    loop {
        let rows = match forge.check_rows(change) {
            Ok(rows) => rows,
            Err(error) if supervised_hold && row_read_failures < 2 => {
                // BUG-1265: a coarse red cannot distinguish the supervised
                // merge-hold gate from a real failure. Retry transient row
                // reads before returning an unavailable classification to the
                // caller; never let it degrade into ci-red while held.
                row_read_failures += 1;
                std::thread::sleep(poll_interval);
                continue;
            }
            Err(error) => return Err(error),
        };
        let r = classify_live_rows(
            project_root,
            forge,
            change,
            &rows,
            &required,
            &cfg,
            hold_marker_present,
            hold_label_present,
        )?;
        if r.is_real() || !r.has_pending() || Instant::now() >= deadline {
            return Ok(r);
        }
        // Keep bounded ship polling visible to headless watchdogs.
        // trace:TASK-1331 | ai:codex
        eprintln!(
            "  CI pending for {}-{}: {}",
            forge.kind().change_noun().to_ascii_uppercase(),
            change.id,
            r.pending.join(", ")
        );
        std::thread::sleep(poll_interval.min(deadline.saturating_duration_since(Instant::now())));
    }
}

/// Ship polls snapshots rather than entering a forge's unbounded watcher.
/// One deadline covers registration and settlement; absent means unbounded.
// trace:TASK-1606 | ai:codex
#[allow(clippy::too_many_arguments)]
pub(crate) fn wait_for_ship_ci(
    project_root: &Path,
    forge: &dyn crate::forge::Forge,
    change: &crate::forge::ChangeRef,
    hold_marker_present: bool,
    hold_label_present: bool,
    timeout: Option<Duration>,
    poll_interval: Duration,
) -> anyhow::Result<crate::gate::dispatch_types::CiState> {
    use crate::forge::{CheckRegistration, CiTarget, ForgeKind};
    use crate::gate::dispatch_types::CiState;
    use crate::gate::ship::ShipRefusal;
    let started = Instant::now();
    let cfg = read_ci_gate_config(project_root);
    // BUG-2184: poll at least once whatever the budget, so an exhausted or
    // zero budget still reports what CI actually says (or its verdict); the
    // first poll happens before any deadline check. trace:BUG-2184 | ai:claude
    let mut pending: String;
    let mut polled = false;
    loop {
        pending = match forge.checks_registered(change)? {
            CheckRegistration::NoCi => return Ok(CiState::None),
            CheckRegistration::NotYet => "checks have not registered".to_string(),
            CheckRegistration::Registered => {
                let rows = forge.check_rows(change);
                if rows.is_ok() || forge.kind() == ForgeKind::GitHub {
                    let rows = rows?;
                    let required = forge.required_check_names(change);
                    if polled && timeout.is_some_and(|budget| started.elapsed() >= budget) {
                        return Err(ShipRefusal::CiTimeout.error(format!(
                            "CI wait timed out for PR-{} while reading checks",
                            change.id
                        )));
                    }
                    let result = classify_live_rows(
                        project_root,
                        forge,
                        change,
                        &rows,
                        &required,
                        &cfg,
                        hold_marker_present,
                        hold_label_present,
                    )?;
                    if result.is_real() {
                        return Err(ShipRefusal::CiRed.error(format!(
                            "CI is red on PR-{}: {}",
                            change.id,
                            result.describe(change.id)
                        )));
                    }
                    if !result.has_pending() {
                        return Ok(CiState::Success);
                    }
                    result.pending.join(", ")
                } else {
                    let state = forge.ci_status(CiTarget::Change(change.clone()))?.state;
                    if polled && timeout.is_some_and(|budget| started.elapsed() >= budget) {
                        return Err(ShipRefusal::CiTimeout.error(format!(
                            "CI wait timed out for PR-{} while reading pipeline",
                            change.id
                        )));
                    }
                    match state {
                        CiState::Success => return Ok(CiState::Success),
                        CiState::Failed => {
                            return Err(ShipRefusal::CiRed
                                .error(format!("CI did not pass for PR-{}", change.id)))
                        }
                        _ => "pipeline is pending".to_string(),
                    }
                }
            }
        };
        polled = true;
        if timeout.is_some_and(|budget| started.elapsed() >= budget) {
            return Err(ShipRefusal::CiTimeout.error(format!(
                "CI wait timed out for PR-{} — still pending: {pending}",
                change.id
            )));
        }
        eprintln!(
            "  CI pending for PR-{}: {pending} (elapsed {}s)",
            change.id,
            started.elapsed().as_secs()
        );
        let remaining = timeout.map(|budget| budget.saturating_sub(started.elapsed()));
        let sleep = poll_interval.min(Duration::from_secs(60));
        std::thread::sleep(remaining.map_or(sleep, |remaining| sleep.min(remaining)));
    }
}

/// After the hold is released (label removed → the gate workflow re-runs),
/// wait for `merge-hold-gate` to report green before merging. A change with no
/// such check (no Layer 2, or a forge with no per-check rows) returns
/// immediately. `Ok(false)` = still not green at the deadline.
// trace:STORY-1166 | ai:claude (forge-routed)
pub(crate) fn wait_hold_gate_green(
    forge: &dyn crate::forge::Forge,
    change: &crate::forge::ChangeRef,
    timeout: Duration,
    poll_interval: Duration,
) -> anyhow::Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        // A nested train poll must persist intent before reading the forge.
        // Keep persistence failures outside the legacy unsupported-rows
        // fallback below: they must stop the train, never permit a merge.
        // trace:STORY-2401 | ai:codex
        crate::pr_train::before_train_forge_call("poll released hold gate")?;
        let rows = match forge.check_rows(change) {
            Ok(rows) => rows,
            Err(_) => return Ok(true), // no per-check rows on this forge — nothing to wait on
        };
        match hold_gate_state(&rows) {
            HoldGateState::Absent | HoldGateState::Green => return Ok(true),
            HoldGateState::NotGreen => {}
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(poll_interval);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum HoldGateState {
    Absent,
    Green,
    NotGreen,
}

pub(crate) fn hold_gate_state(rows: &[CheckRow]) -> HoldGateState {
    // During cutover, a legacy check run and a commit status can share this
    // name. Both must pass before the post-release wait can finish.
    // trace:BUG-2492 | ai:codex
    let mut state = HoldGateState::Absent;
    for row in rows
        .iter()
        .filter(|r| r.name.eq_ignore_ascii_case(HOLD_GATE_CHECK))
    {
        if row.bucket != "pass" {
            return HoldGateState::NotGreen;
        }
        state = HoldGateState::Green;
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, workflow: &str, bucket: &str) -> CheckRow {
        CheckRow {
            name: name.into(),
            workflow: workflow.into(),
            bucket: bucket.into(),
            run_id: None,
        }
    }
    fn req(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    // trace:TASK-1606 | ai:codex
    #[test]
    fn ship_deadline_includes_registration_and_pending_rows() {
        use crate::forge::CheckRegistration;
        for registration in [CheckRegistration::NotYet, CheckRegistration::Registered] {
            let forge = FakeForge {
                registrations: std::cell::RefCell::new(vec![registration]),
                rows: Some(vec![row("Build", "CI", "pending")]),
                required: vec![],
                row_reads: std::cell::Cell::new(0),
                registration_polls: std::cell::Cell::new(0),
                labels: (false, false, false),
            };
            let root = tempfile::tempdir().unwrap();
            let start = Instant::now();
            let error = wait_for_ship_ci(
                root.path(),
                &forge,
                &change(),
                false,
                false,
                Some(Duration::ZERO),
                Duration::from_secs(15),
            )
            .unwrap_err();
            assert_eq!(crate::exit_code_for_error(&error), 21);
            assert!(start.elapsed() < Duration::from_secs(1));
        }
    }

    // BUG-2184: an exhausted budget (the mandate scan used to eat it) still
    // polls once: green CI ships, a pending timeout names what CI really
    // reported, and a never-registered CI is reported as polled, not as the
    // placeholder. trace:BUG-2184 | ai:claude
    #[test]
    fn exhausted_ship_budget_still_polls_ci_once() {
        use crate::forge::CheckRegistration;
        let root = tempfile::tempdir().unwrap();
        let wait = |registration, rows: Vec<CheckRow>| {
            let forge = FakeForge {
                registrations: std::cell::RefCell::new(vec![registration]),
                rows: Some(rows),
                required: vec![],
                row_reads: std::cell::Cell::new(0),
                registration_polls: std::cell::Cell::new(0),
                labels: (false, false, false),
            };
            let result = wait_for_ship_ci(
                root.path(),
                &forge,
                &change(),
                false,
                false,
                Some(Duration::ZERO),
                Duration::from_secs(15),
            );
            (
                result,
                forge.row_reads.get(),
                forge.registration_polls.get(),
            )
        };
        let (green, reads, polls) = wait(
            CheckRegistration::Registered,
            vec![row("Build", "CI", "pass")],
        );
        assert!(
            matches!(green, Ok(crate::gate::dispatch_types::CiState::Success)),
            "{green:?}"
        );
        assert_eq!((reads, polls), (1, 1));

        let (pending, reads, polls) = wait(
            CheckRegistration::Registered,
            vec![row("Build", "CI", "pending")],
        );
        let message = format!("{:#}", pending.unwrap_err());
        assert!(message.contains("still pending: Build"), "{message}");
        assert_eq!((reads, polls), (1, 1));

        let (red, _, _) = wait(
            CheckRegistration::Registered,
            vec![row("Build", "CI", "fail")],
        );
        assert!(format!("{:#}", red.unwrap_err()).contains("CI is red"));

        // Checks not registered yet: one registration poll, then the timeout
        // reports what was polled, not a placeholder. No rows are read.
        let (unregistered, reads, polls) = wait(CheckRegistration::NotYet, vec![]);
        let error = unregistered.unwrap_err();
        assert_eq!(crate::exit_code_for_error(&error), 21);
        let message = format!("{error:#}");
        assert!(
            message.contains("still pending: checks have not registered"),
            "{message}"
        );
        assert_eq!((reads, polls), (0, 1));
    }

    // trace:BUG-2184 | ai:claude
    #[test]
    fn ship_ci_budget_excludes_the_mandate_scan() {
        let budget = crate::pr_cmd::ship_ci_budget;
        // 15 min wait, 16 min elapsed of which the scan took 12: 11 min left.
        assert_eq!(
            budget(900, Duration::from_secs(960), Duration::from_secs(720)),
            Duration::from_secs(660)
        );
        assert_eq!(
            budget(900, Duration::from_secs(1000), Duration::ZERO),
            Duration::ZERO
        );
        assert_eq!(
            budget(900, Duration::from_secs(5), Duration::from_secs(9)),
            Duration::from_secs(900)
        );
    }

    // TASK-2348: the ship's CI handoff forwards only what is left of one
    // two-second `--wait`, whatever ownership and preflight already used.
    // trace:TASK-2348 | ai:claude
    #[test]
    fn ship_ci_handoff_forwards_only_the_remainder() {
        let forwarded = |wait, since: u64, scan: u64| {
            crate::pr_cmd::ship_ci_handoff(
                wait,
                Duration::from_millis(since),
                Duration::from_millis(scan),
                |budget| budget,
            )
        };
        // Ownership used the whole allowance (alone, or with preflight).
        assert_eq!(forwarded(Some(2), 2_000, 0), Some(Duration::ZERO));
        assert_eq!(forwarded(Some(2), 2_600, 0), Some(Duration::ZERO));
        // Partly used: exactly the remainder, not a renewed two seconds.
        assert_eq!(
            forwarded(Some(2), 1_250, 0),
            Some(Duration::from_millis(750))
        );
        assert_eq!(
            forwarded(Some(2), 4_250, 3_000),
            Some(Duration::from_millis(750))
        );
        // The excluded scan never gives back what ownership consumed.
        assert_eq!(forwarded(Some(2), 2_000 + 900, 900), Some(Duration::ZERO));
        assert_eq!(
            forwarded(Some(2), 2_100 + 5_000, 5_000),
            Some(Duration::ZERO)
        );
        // No `--wait`: the CI wait stays unbounded.
        assert_eq!(forwarded(None, 9_000, 0), None);
    }

    // TASK-2348: an exhausted handoff reaches the real CI wait as ZERO: one
    // observation of a pending Build, a timeout naming it, no second poll.
    // trace:TASK-2348 | ai:claude
    #[test]
    fn exhausted_ship_handoff_polls_pending_ci_once_then_times_out() {
        use crate::forge::CheckRegistration;
        let root = tempfile::tempdir().unwrap();
        let forge = FakeForge {
            registrations: std::cell::RefCell::new(vec![CheckRegistration::Registered]),
            rows: Some(vec![row("Build", "CI", "pending")]),
            required: vec![],
            row_reads: std::cell::Cell::new(0),
            registration_polls: std::cell::Cell::new(0),
            labels: (false, false, false),
        };
        let mut forwarded = None;
        let result = crate::pr_cmd::ship_ci_handoff(
            Some(2),
            Duration::from_millis(2_300),
            Duration::from_millis(200),
            |budget| {
                forwarded = budget;
                wait_for_ship_ci(
                    root.path(),
                    &forge,
                    &change(),
                    false,
                    false,
                    budget,
                    Duration::from_secs(15),
                )
            },
        );
        assert_eq!(forwarded, Some(Duration::ZERO));
        let error = result.unwrap_err();
        assert_eq!(crate::exit_code_for_error(&error), 21);
        let message = format!("{error:#}");
        assert!(message.contains("still pending: Build"), "{message}");
        assert_eq!(
            (forge.row_reads.get(), forge.registration_polls.get()),
            (1, 1)
        );
    }

    // trace:BUG-2492 | ai:codex
    #[test]
    fn bug2492_pending_hold_needs_evidence_and_preserves_other_ci() {
        for (marker, label) in [(true, false), (false, true), (true, true), (false, false)] {
            for build in ["pass", "pending", "fail"] {
                let rows = [
                    row("Build", "CI", build),
                    row(HOLD_GATE_CHECK, "", "pending"),
                ];
                let r = classify_red(
                    &rows,
                    &req(&[HOLD_GATE_CHECK]),
                    &CiGateConfig::default(),
                    marker,
                    label,
                );
                assert_eq!(r.hold_gate_is_the_hold, marker || label, "{r:?}");
                assert_eq!(r.is_real(), build == "fail", "{r:?}");
                assert_eq!(
                    r.pending.contains(&HOLD_GATE_CHECK.to_string()),
                    !marker && !label,
                    "{r:?}"
                );
                assert_eq!(
                    r.pending.contains(&"Build".to_string()),
                    build == "pending",
                    "{r:?}"
                );
                assert!(!r.hold_gate_label_lingering);
            }
        }
    }

    // trace:BUG-2492 | ai:codex
    #[test]
    fn bug2492_recorded_hold_without_active_label_refuses_without_wait() {
        // BUG-1693: recorded-without-cleared publishes pending even after
        // someone removes the active label. No marker/active label grants
        // release authority; this must refuse instead of waiting forever.
        let root = tempfile::tempdir().unwrap();
        for (recorded, cleared) in [(false, false), (true, true), (true, false)] {
            let forge = FakeForge {
                registrations: std::cell::RefCell::new(vec![
                    crate::forge::CheckRegistration::Registered,
                ]),
                rows: Some(vec![row(HOLD_GATE_CHECK, "", "pending")]),
                required: req(&[HOLD_GATE_CHECK]),
                row_reads: std::cell::Cell::new(0),
                registration_polls: std::cell::Cell::new(0),
                labels: (false, recorded, cleared),
            };
            let error = wait_for_ship_ci(
                root.path(),
                &forge,
                &change(),
                false,
                false,
                Some(Duration::ZERO),
                Duration::ZERO,
            )
            .unwrap_err();
            assert_eq!(
                crate::exit_code_for_error(&error),
                if recorded && !cleared { 20 } else { 21 }
            );
            assert_eq!(forge.row_reads.get(), 1);
            if recorded && !cleared {
                assert!(format!("{error:#}").contains("recorded hold labels"));
            }
        }
    }

    // trace:BUG-2492 | ai:codex
    #[test]
    fn bug2492_release_after_pr_pending_wait_is_bounded() {
        let root = tempfile::tempdir().unwrap();
        // The production caller passes this configured ceiling, not a fixed
        // five-minute timeout that would reject ordinary queued CI.
        assert_eq!(
            crate::upgrade_release::after_pr_ci_budget(root.path()),
            Duration::from_secs(crate::ci_idle_timeout::ci_absolute_ceiling_secs(
                root.path()
            ))
        );
        std::fs::create_dir_all(root.path().join(".aida")).unwrap();
        std::fs::write(
            root.path().join(".aida/config.toml"),
            "[drain]\nci_absolute = \"2h\"\n",
        )
        .unwrap();
        assert_eq!(
            crate::upgrade_release::after_pr_ci_budget(root.path()),
            Duration::from_secs(crate::ci_idle_timeout::ci_absolute_ceiling_secs(
                root.path()
            ))
        );
        let forge = FakeForge {
            registrations: std::cell::RefCell::new(vec![
                crate::forge::CheckRegistration::Registered,
            ]),
            rows: Some(vec![row(HOLD_GATE_CHECK, "", "pending")]),
            required: req(&[HOLD_GATE_CHECK]),
            row_reads: std::cell::Cell::new(0),
            registration_polls: std::cell::Cell::new(0),
            labels: (false, false, false),
        };
        let error = crate::upgrade_release::wait_after_pr_ci(
            root.path(),
            &forge,
            &change(),
            Duration::ZERO,
        )
        .unwrap_err();
        assert_eq!(crate::exit_code_for_error(&error), 21);
        assert_eq!(forge.row_reads.get(), 1);
    }

    // trace:BUG-2492 | ai:codex
    #[test]
    fn bug2492_release_after_pr_active_only_hold_refuses_without_wait() {
        let root = tempfile::tempdir().unwrap();
        let forge = FakeForge {
            registrations: std::cell::RefCell::new(vec![
                crate::forge::CheckRegistration::Registered,
            ]),
            rows: Some(vec![row(HOLD_GATE_CHECK, "", "pending")]),
            required: req(&[HOLD_GATE_CHECK]),
            row_reads: std::cell::Cell::new(0),
            registration_polls: std::cell::Cell::new(0),
            labels: (true, false, false),
        };
        // A zero budget distinguishes an immediate hold refusal from timeout.
        let error = crate::upgrade_release::wait_after_pr_ci(
            root.path(),
            &forge,
            &change(),
            Duration::ZERO,
        )
        .unwrap_err();
        assert_eq!(crate::exit_code_for_error(&error), 20);
        assert_eq!(forge.row_reads.get(), 1);
    }

    // Exercise the ship and drain polling boundaries with no time available:
    // a confirmed pending hold must return immediately, not consume the budget.
    // trace:BUG-2492 | ai:codex
    #[test]
    fn bug2492_pending_hold_releases_ship_and_refine_waits() {
        let root = tempfile::tempdir().unwrap();
        for (marker, label) in [(true, false), (false, true)] {
            let forge = FakeForge {
                registrations: std::cell::RefCell::new(vec![
                    crate::forge::CheckRegistration::Registered,
                ]),
                rows: Some(vec![
                    row("Build", "CI", "pass"),
                    row(HOLD_GATE_CHECK, "", "pending"),
                ]),
                required: req(&[HOLD_GATE_CHECK]),
                row_reads: std::cell::Cell::new(0),
                registration_polls: std::cell::Cell::new(0),
                labels: (false, false, false),
            };
            let result = wait_for_ship_ci(
                root.path(),
                &forge,
                &change(),
                marker,
                label,
                Some(Duration::ZERO),
                Duration::ZERO,
            )
            .unwrap();
            assert!(matches!(
                result,
                crate::gate::dispatch_types::CiState::Success
            ));
            let refined = refine_red(
                root.path(),
                &forge,
                &change(),
                marker,
                label,
                Duration::ZERO,
                Duration::ZERO,
            )
            .unwrap();
            assert!(refined.hold_gate_is_the_hold);
            assert!(!refined.has_pending());
            assert!(!refined.is_real());
        }
    }

    #[test]
    fn hold_gate_red_with_marker_is_the_hold_not_a_failure() {
        // The BUG-1180 shape: PR CI green, the hold gate red because the
        // supervised marker exists. `aida pr ship` must proceed and release
        // the hold at merge instead of aborting with "CI failed".
        let rows = [
            row("Build (ubuntu-latest)", "CI", "pass"),
            row(HOLD_GATE_CHECK, "merge-hold-gate", "fail"),
        ];
        let r = classify_red(
            &rows,
            &req(&[HOLD_GATE_CHECK]),
            &CiGateConfig::default(),
            true,
            false,
        );
        assert!(!r.is_real(), "{r:?}");
        assert!(r.hold_gate_is_the_hold);
        assert!(!r.hold_gate_label_lingering);
        assert!(r.describe(1875).contains("supervised merge-hold"));
    }

    #[test]
    fn hold_gate_red_without_marker_or_confirmed_label_is_real() {
        let rows = [row(HOLD_GATE_CHECK, "merge-hold-gate", "fail")];
        let r = classify_red(
            &rows,
            &req(&[HOLD_GATE_CHECK]),
            &CiGateConfig::default(),
            false,
            false,
        );
        assert!(r.is_real());
        assert!(r.hold_gate_label_lingering);
        assert!(
            r.describe(7).contains("aida merge-hold clear 7"),
            "{}",
            r.describe(7)
        );
    }

    #[test]
    fn hold_gate_red_with_label_only_is_the_hold_not_a_failure() {
        let rows = [row(HOLD_GATE_CHECK, "merge-hold-gate", "fail")];
        let r = classify_red(
            &rows,
            &req(&[HOLD_GATE_CHECK]),
            &CiGateConfig::default(),
            false,
            true,
        );
        assert!(!r.is_real(), "{r:?}");
        assert!(r.hold_gate_is_the_hold);
        assert!(!r.hold_gate_label_lingering);
    }

    #[test]
    fn informational_matrix_red_is_ignored_only_when_not_required() {
        // TASK-1205: Windows/macOS jobs share PR CI's `Build (…)` name; the
        // workflow name is the discriminator.
        let rows = [
            row("Build (ubuntu-latest)", "CI", "pass"),
            row("Build (windows-latest)", "Cross-platform (nightly)", "fail"),
            row("Build (macos-latest)", "Cross-platform (nightly)", "pass"),
        ];
        let r = classify_red(
            &rows,
            &req(&[HOLD_GATE_CHECK]),
            &CiGateConfig::default(),
            false,
            false,
        );
        assert!(!r.is_real(), "{r:?}");
        assert_eq!(
            r.ignored_informational,
            vec!["Build (windows-latest)".to_string()]
        );
        // If branch protection REQUIRES that check, the allow-list cannot save it.
        let r = classify_red(
            &rows,
            &req(&["Build (windows-latest)"]),
            &CiGateConfig::default(),
            false,
            false,
        );
        assert_eq!(r.real[0].name, "Build (windows-latest)");
    }

    #[test]
    fn a_red_required_or_plain_check_is_always_real() {
        // This repo's protection requires ONLY merge-hold-gate; a literal
        // required-only rule would merge over a red Linux build. It must not.
        let rows = [
            row("Build (ubuntu-latest)", "CI", "fail"),
            row(HOLD_GATE_CHECK, "merge-hold-gate", "fail"),
        ];
        let r = classify_red(
            &rows,
            &req(&[HOLD_GATE_CHECK]),
            &CiGateConfig::default(),
            true,
            false,
        );
        assert_eq!(r.real[0].name, "Build (ubuntu-latest)");
        assert!(r.hold_gate_is_the_hold);
        // No branch protection at all + no allow-list hit → still real.
        let r = classify_red(&rows[..1], &[], &CiGateConfig::default(), false, false);
        assert!(r.is_real());
        // Cancelled counts as red.
        let r = classify_red(
            &[row("Build (ubuntu-latest)", "CI", "cancel")],
            &[],
            &CiGateConfig::default(),
            false,
            false,
        );
        assert!(r.is_real());
    }

    #[test]
    fn pending_relevant_rows_are_reported_but_informational_pending_is_not() {
        let rows = [
            row("Build (ubuntu-latest)", "CI", "pending"),
            row(
                "Build (windows-latest)",
                "Cross-platform (nightly)",
                "pending",
            ),
            row(HOLD_GATE_CHECK, "merge-hold-gate", "fail"),
        ];
        let r = classify_red(&rows, &[], &CiGateConfig::default(), true, false);
        assert!(!r.is_real());
        assert_eq!(r.pending, vec!["Build (ubuntu-latest)".to_string()]);
    }

    #[test]
    fn config_parses_arrays_and_empty_array_disables_default() {
        let cfg = parse_ci_gate_config(
            "[drain]\nretry_transient = 1\n\n[ci]\ninformational_workflows = [\"Cross-platform*\", 'Nightly *'] # note\ninformational_checks = [\"Build (windows*\"]\n",
        );
        assert_eq!(
            cfg.informational_workflows,
            vec!["Cross-platform*", "Nightly *"]
        );
        assert_eq!(cfg.informational_checks, vec!["Build (windows*"]);
        let cfg = parse_ci_gate_config("[ci]\ninformational_workflows = []\n");
        assert!(cfg.informational_workflows.is_empty());
        assert_eq!(parse_ci_gate_config(""), CiGateConfig::default());
        // A section that merely starts with "ci" is not [ci].
        let cfg = parse_ci_gate_config("[cicd]\ninformational_workflows = [\"x\"]\n");
        assert_eq!(cfg, CiGateConfig::default());
    }

    #[test]
    fn wild_match_is_case_insensitive_with_star() {
        assert!(wild_match("cross-platform", "Cross-platform"));
        assert!(wild_match("Build (windows*", "Build (windows-latest)"));
        assert!(wild_match("*macos*", "Build (macos-latest)"));
        assert!(!wild_match("Build (windows*", "Build (ubuntu-latest)"));
        assert!(!wild_match("CI", "CI-nightly"));
    }

    // STORY-1166: the register-wait and the red refinement are driven purely
    // through the Forge trait — a fake forge with no `gh`/`glab` proves it.
    struct FakeForge {
        registrations: std::cell::RefCell<Vec<crate::forge::CheckRegistration>>,
        rows: Option<Vec<CheckRow>>,
        required: Vec<String>,
        row_reads: std::cell::Cell<usize>,
        registration_polls: std::cell::Cell<usize>,
        labels: (bool, bool, bool),
    }
    impl crate::forge::Forge for FakeForge {
        fn recorded_hold_labels(
            &self,
            _: &Path,
            _: &crate::forge::ChangeRef,
        ) -> anyhow::Result<(bool, bool, bool)> {
            Ok(self.labels)
        }

        fn authenticated_user_login(&self) -> anyhow::Result<String> {
            Ok("fake-user".into())
        }
        fn change_metadata(
            &self,
            _: u64,
            _: &mut dyn crate::network_retry::RetrySink,
        ) -> anyhow::Result<crate::forge::ChangeMetadata> {
            unimplemented!()
        }
        fn change_commit_headlines(
            &self,
            _: u64,
            _: &mut dyn crate::network_retry::RetrySink,
        ) -> anyhow::Result<Vec<String>> {
            unimplemented!()
        }
        fn change_reviews(
            &self,
            _: u64,
            _: &mut dyn crate::network_retry::RetrySink,
        ) -> anyhow::Result<crate::forge::ChangeReviews> {
            unimplemented!()
        }
        fn merge_change(
            &self,
            _: &crate::forge::ChangeRef,
            _: &crate::forge::MergeOptions,
            _: &mut dyn crate::network_retry::RetrySink,
        ) -> anyhow::Result<crate::forge::MergeResult> {
            unimplemented!()
        }
        fn kind(&self) -> crate::forge::ForgeKind {
            crate::forge::ForgeKind::GitHub
        }
        fn open_change(
            &self,
            _: crate::forge::OpenChange,
        ) -> anyhow::Result<crate::forge::ChangeRef> {
            unimplemented!()
        }
        fn change_for_branch(&self, _: &str) -> anyhow::Result<crate::forge::ChangeLookup> {
            unimplemented!()
        }
        fn change_for_spec(&self, _: &str) -> anyhow::Result<crate::forge::ChangeLookup> {
            unimplemented!()
        }
        fn merged_change_for_branch(&self, _: &str) -> anyhow::Result<crate::forge::ChangeLookup> {
            unimplemented!()
        }
        fn change_status(
            &self,
            _: &crate::forge::ChangeRef,
        ) -> anyhow::Result<crate::forge::ChangeStatus> {
            unimplemented!()
        }
        fn diff_change(&self, _: u64) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn ci_status(&self, _: crate::forge::CiTarget) -> anyhow::Result<crate::forge::CiStatus> {
            unimplemented!()
        }
        fn ci_probe_for_branch(&self, _: &str) -> anyhow::Result<crate::forge::CiProbeResult> {
            unimplemented!()
        }
        fn watch_ci(
            &self,
            _: &crate::forge::ChangeRef,
        ) -> anyhow::Result<crate::gate::dispatch_types::CiState> {
            unimplemented!()
        }
        fn stream_ci_for_branch(
            &self,
            _: &str,
            _: bool,
        ) -> anyhow::Result<crate::forge::CiProbeResult> {
            unimplemented!()
        }
        fn comment(&self, _: &crate::forge::ChangeRef, _: &str) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn close_change(&self, _: &crate::forge::ChangeRef, _: &str) -> anyhow::Result<()> {
            unimplemented!()
        }
        // trace:BUG-1690 | ai:claude
        fn mark_change_ready(&self, _: &crate::forge::ChangeRef) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn checkout_change(&self, _: &crate::forge::ChangeRef) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn list_changes(
            &self,
            _: crate::forge::ChangeFilter,
        ) -> anyhow::Result<Vec<crate::forge::ChangeRef>> {
            unimplemented!()
        }
        fn checks_registered(
            &self,
            _: &crate::forge::ChangeRef,
        ) -> anyhow::Result<crate::forge::CheckRegistration> {
            self.registration_polls
                .set(self.registration_polls.get() + 1);
            let mut v = self.registrations.borrow_mut();
            Ok(if v.len() > 1 { v.remove(0) } else { v[0] })
        }
        fn check_rows(&self, _: &crate::forge::ChangeRef) -> anyhow::Result<Vec<CheckRow>> {
            self.row_reads.set(self.row_reads.get() + 1);
            self.rows
                .clone()
                .ok_or_else(|| anyhow::anyhow!("no rows on this forge"))
        }
        fn required_check_names(&self, _: &crate::forge::ChangeRef) -> Vec<String> {
            self.required.clone()
        }
    }
    fn change() -> crate::forge::ChangeRef {
        crate::forge::ChangeRef {
            id: 42,
            url: String::new(),
            branch: "feature".into(),
            base: String::new(),
            title: None,
        }
    }
    fn fake(regs: &[crate::forge::CheckRegistration], rows: Option<Vec<CheckRow>>) -> FakeForge {
        FakeForge {
            registrations: std::cell::RefCell::new(regs.to_vec()),
            rows,
            required: vec![HOLD_GATE_CHECK.into()],
            row_reads: std::cell::Cell::new(0),
            registration_polls: std::cell::Cell::new(0),
            labels: (false, false, false),
        }
    }

    // trace:STORY-2735 | ai:codex
    #[test]
    fn story_2735_ship_requires_full_ci_for_build_run() {
        let check = |name: &str, run: &str, bucket: &str| {
            let mut value = row(name, "CI", bucket);
            value.run_id = Some(run.into());
            value
        };
        let build = check("Build (ubuntu-latest)", "2", "pass");
        let fast = check("ci-tier: fast", "2", "pass");
        let full = check("ci-tier: full", "2", "pass");
        for rows in [
            vec![build.clone(), fast.clone()],
            vec![
                build.clone(),
                check("ci-tier: full", "1", "pass"),
                fast.clone(),
            ],
            vec![build.clone(), check("ci-tier: full", "2", "pending")],
            vec![fast.clone()],
            vec![build.clone(), full.clone(), fast.clone()],
            vec![
                build.clone(),
                full.clone(),
                check("ci-tier: fast", "3", "pass"),
            ],
            vec![
                build.clone(),
                full.clone(),
                check("ci-tier: full", "3", "pending"),
            ],
            vec![row("Build (ubuntu-latest)", "CI", "pass"), full.clone()],
        ] {
            let f = fake(&[], Some(rows));
            let err = require_full_ship_ci(&f, &change()).unwrap_err().to_string();
            assert!(
                err.contains("aida pr train") && err.contains("ci:full"),
                "{err}"
            );
        }
        for rows in [
            vec![build.clone(), full.clone()],
            vec![build.clone(), full, check("ci-tier: fast", "1", "pass")],
            vec![build], // legacy / untiered workflow
        ] {
            require_full_ship_ci(&fake(&[], Some(rows)), &change()).unwrap();
        }
        assert!(require_full_ship_ci(&fake(&[], None), &change()).is_err());
    }

    #[test]
    fn register_wait_is_forge_driven() {
        use crate::forge::CheckRegistration as R;
        let ms = Duration::from_millis(1);
        let f = fake(&[R::NotYet, R::NotYet, R::Registered], None);
        wait_for_checks_to_register(&f, &change(), Duration::from_secs(5), ms).unwrap();
        let f = fake(&[R::NoCi], None);
        wait_for_checks_to_register(&f, &change(), Duration::from_secs(5), ms).unwrap();
        let f = fake(&[R::NotYet], None);
        let err =
            wait_for_checks_to_register(&f, &change(), Duration::from_millis(5), ms).unwrap_err();
        assert!(err.to_string().contains("PR-42"), "{err}");
    }

    #[test]
    fn red_refinement_is_forge_driven_and_keeps_coarse_verdict_without_rows() {
        use crate::forge::CheckRegistration as R;
        let dir = tempfile::tempdir().unwrap();
        let ms = Duration::from_millis(1);
        let f = fake(
            &[R::Registered],
            Some(vec![
                CheckRow {
                    name: "Build (ubuntu-latest)".into(),
                    workflow: "CI".into(),
                    bucket: "pass".into(),
                    run_id: None,
                },
                CheckRow {
                    name: HOLD_GATE_CHECK.into(),
                    workflow: "merge-hold-gate".into(),
                    bucket: "fail".into(),
                    run_id: None,
                },
            ]),
        );
        let r = refine_red(
            dir.path(),
            &f,
            &change(),
            true,
            false,
            Duration::from_secs(1),
            ms,
        )
        .unwrap();
        assert!(!r.is_real() && r.hold_gate_is_the_hold);
        assert!(!wait_hold_gate_green(&f, &change(), Duration::from_millis(3), ms).unwrap());
        // No rows (the pure-git default) → Err, callers keep the coarse verdict;
        // and the hold-gate wait has nothing to wait on.
        let f = fake(&[R::Registered], None);
        assert!(refine_red(
            dir.path(),
            &f,
            &change(),
            true,
            false,
            Duration::from_secs(1),
            ms,
        )
        .is_err());
        assert_eq!(f.row_reads.get(), 3, "a supervised hold retries row reads");
        assert!(wait_hold_gate_green(&f, &change(), Duration::from_millis(3), ms).unwrap());
    }

    // trace:TASK-1331 | ai:codex
    #[test]
    fn ship_classification_waits_for_relevant_and_required_pending_rows() {
        let dir = tempfile::tempdir().unwrap();
        for workflow in ["CI", "Cross-platform nightly"] {
            let mut f = fake(&[], Some(vec![row("Build", workflow, "pending")]));
            if workflow.starts_with("Cross-platform") {
                f.required.push("Build".into());
            }
            let r = refine_red(
                dir.path(),
                &f,
                &change(),
                false,
                false,
                Duration::from_millis(3),
                Duration::from_millis(1),
            )
            .unwrap();
            assert_eq!(r.pending, vec!["Build"]);
            assert!(f.row_reads.get() > 1, "must poll relevant pending jobs");
        }
    }

    #[test]
    fn red_refinement_without_a_hold_does_not_retry_unavailable_rows() {
        use crate::forge::CheckRegistration as R;
        let dir = tempfile::tempdir().unwrap();
        let f = fake(&[R::Registered], None);
        assert!(refine_red(
            dir.path(),
            &f,
            &change(),
            false,
            false,
            Duration::from_secs(1),
            Duration::from_millis(1),
        )
        .is_err());
        assert_eq!(
            f.row_reads.get(),
            1,
            "no-hold forge behavior stays unchanged"
        );
    }

    /// Source-shape guard: this module never shells out to a forge CLI itself.
    #[test]
    fn ci_gate_has_no_direct_forge_cli_calls() {
        let src = include_str!("ci_gate.rs");
        let body = &src[..src.find("#[cfg(test)]").unwrap()];
        assert!(!body.contains(concat!("Command::new(\"g", "h\")")));
        assert!(!body.contains(concat!("Command::new(\"gl", "ab\")")));
    }

    // trace:BUG-2492 | ai:codex
    #[test]
    fn hold_gate_state_requires_all_same_named_rows_to_pass() {
        assert_eq!(hold_gate_state(&[]), HoldGateState::Absent);
        for bucket in ["pending", "fail", "cancel", "skipping", "unknown", ""] {
            let mut rows = vec![
                row("merge-hold-gate", "legacy workflow", "pass"),
                row("MERGE-HOLD-GATE", "", bucket),
                row("Build", "CI", "pass"),
            ];
            for _ in 0..2 {
                assert_eq!(hold_gate_state(&rows), HoldGateState::NotGreen, "{rows:?}");
                rows.reverse();
            }
        }
        let rows = vec![
            row("merge-hold-gate", "legacy workflow", "pass"),
            row("merge-hold-gate", "", "pass"),
            row("Build", "CI", "pending"),
        ];
        assert_eq!(hold_gate_state(&rows), HoldGateState::Green);
    }

    #[test]
    fn parses_gh_json_and_hold_gate_state() {
        let json = r#"[{"bucket":"pending","name":"Build (ubuntu-latest)","state":"IN_PROGRESS","workflow":"CI"},{"bucket":"pass","name":"merge-hold-gate","state":"SUCCESS","workflow":"merge-hold-gate"}]"#;
        let rows = parse_check_rows(json).expect("parses");
        assert_eq!(rows.len(), 2);
        assert_eq!(hold_gate_state(&rows), HoldGateState::Green);
        assert_eq!(hold_gate_state(&rows[..1]), HoldGateState::Absent);
        let rows = parse_check_rows(
            r#"[{"bucket":"fail","name":"merge-hold-gate","workflow":"merge-hold-gate"}]"#,
        )
        .unwrap();
        assert_eq!(hold_gate_state(&rows), HoldGateState::NotGreen);
        assert!(parse_check_rows("not json").is_none());
    }
}
