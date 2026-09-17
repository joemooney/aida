//! BUG-1180 / ADR-39: gate-aware, informational-aware refinement of a red CI
//! verdict.
//!
//! The forge's `watch_ci` collapses a PR's checks to one coarse `CiState`, so
//! two non-failures read as "CI failed":
//!
//! - the supervised **merge-hold** itself — `merge-hold-gate` (ADR-37 Layer 2)
//!   is *designed* to be red while `.aida/merge-holds/PR-<n>` exists, and
//!   `aida pr ship` releases it at merge time; aborting on it at CI-watch time
//!   made every supervised ship a two-command dance;
//! - an **informational** matrix (TASK-1205's Windows/macOS jobs) that shares
//!   the `Build (…)` job name with PR CI and differs only by workflow.
//!
//! This module re-reads the per-check rows through `gh` and classifies each red
//! row. A red check is ignorable only when it is (a) `merge-hold-gate` while the
//! local hold marker exists, or (b) NOT a branch-protection-required check AND
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

/// The ADR-37 Layer-2 required check whose red state IS the supervised hold.
pub(crate) const HOLD_GATE_CHECK: &str = "merge-hold-gate";

/// One row of `gh pr checks <n> --json name,bucket,workflow`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckRow {
    pub(crate) name: String,
    pub(crate) workflow: String,
    /// `pass` | `fail` | `pending` | `skipping` | `cancel` (gh's bucket).
    pub(crate) bucket: String,
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
            // TASK-1205's path-filtered Windows/macOS matrix. Its jobs are named
            // `Build (<os>)` exactly like PR CI's Linux job, so the workflow
            // name is the only reliable discriminator.
            // trace:BUG-1191 | ai:codex
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
    /// Red checks that gate the merge — any entry means CI really failed.
    pub(crate) real: Vec<String>,
    /// Red checks ignored because they are non-required AND informational.
    pub(crate) ignored_informational: Vec<String>,
    /// `merge-hold-gate` is red and the local hold marker exists: the hold
    /// itself, released at merge — not a CI failure.
    pub(crate) hold_gate_is_the_hold: bool,
    /// `merge-hold-gate` is red but NO local marker exists — a lingering label
    /// (the marker was cleared without the label re-sync). Counted in `real`
    /// as well; this flag lets the caller add the `aida merge-hold clear` hint.
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
                "`{HOLD_GATE_CHECK}` is red because PR-{pr} is under a supervised merge-hold — releasing at merge"
            ));
        }
        if !self.ignored_informational.is_empty() {
            parts.push(format!(
                "ignoring informational red check(s): {}",
                self.ignored_informational.join(", ")
            ));
        }
        if self.is_real() {
            parts.push(format!("failing: {}", self.real.join(", ")));
        }
        if self.hold_gate_label_lingering {
            parts.push(format!(
                "`{HOLD_GATE_CHECK}` is red but no local hold marker exists — the label is lingering; \
                 `aida merge-hold clear {pr}` re-syncs it"
            ));
        }
        parts.join("; ")
    }
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
) -> RedRefinement {
    let mut out = RedRefinement::default();
    let is_required = |name: &str| required.iter().any(|r| r.eq_ignore_ascii_case(name));
    for row in rows {
        let informational = !is_required(&row.name) && cfg.is_informational(row);
        if row.is_pending() {
            if !informational {
                out.pending.push(row.name.clone());
            }
            continue;
        }
        if !row.is_red() {
            continue;
        }
        if row.name.eq_ignore_ascii_case(HOLD_GATE_CHECK) {
            if hold_marker_present {
                out.hold_gate_is_the_hold = true;
            } else {
                out.hold_gate_label_lingering = true;
                out.real.push(row.name.clone());
            }
            continue;
        }
        if informational {
            out.ignored_informational.push(row.name.clone());
        } else {
            out.real.push(row.name.clone());
        }
    }
    out
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
            })
            .collect(),
    )
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

/// Refine a coarse "CI failed" verdict for `change`: classify every row the
/// forge reports, and — when nothing real is red yet but relevant checks are
/// still pending (the coarse watcher returned on the first red) — keep polling
/// until they settle or `settle_timeout` elapses. A timeout leaves the
/// still-pending names in `pending`; the caller decides (pr ship bails, the
/// drain shelves CiTimeout). `Err` means the forge exposes no per-check rows
/// (pure-git) or the read failed — callers keep the coarse verdict.
// trace:STORY-1166 | ai:claude (forge-routed; was gh-only under BUG-1180)
pub(crate) fn refine_red(
    project_root: &Path,
    forge: &dyn crate::forge::Forge,
    change: &crate::forge::ChangeRef,
    hold_marker_present: bool,
    settle_timeout: Duration,
    poll_interval: Duration,
) -> anyhow::Result<RedRefinement> {
    let cfg = read_ci_gate_config(project_root);
    let required = forge.required_check_names(change);
    let deadline = Instant::now() + settle_timeout;
    loop {
        let rows = forge.check_rows(change)?;
        let r = classify_red(&rows, &required, &cfg, hold_marker_present);
        if r.is_real() || !r.has_pending() || Instant::now() >= deadline {
            return Ok(r);
        }
        std::thread::sleep(poll_interval);
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
    let Some(row) = rows
        .iter()
        .find(|r| r.name.eq_ignore_ascii_case(HOLD_GATE_CHECK))
    else {
        return HoldGateState::Absent;
    };
    if row.bucket == "pass" {
        HoldGateState::Green
    } else {
        HoldGateState::NotGreen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, workflow: &str, bucket: &str) -> CheckRow {
        CheckRow {
            name: name.into(),
            workflow: workflow.into(),
            bucket: bucket.into(),
        }
    }
    fn req(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
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
        );
        assert!(!r.is_real(), "{r:?}");
        assert!(r.hold_gate_is_the_hold);
        assert!(!r.hold_gate_label_lingering);
        assert!(r.describe(1875).contains("supervised merge-hold"));
    }

    #[test]
    fn hold_gate_red_without_marker_is_a_lingering_label_and_real() {
        let rows = [row(HOLD_GATE_CHECK, "merge-hold-gate", "fail")];
        let r = classify_red(
            &rows,
            &req(&[HOLD_GATE_CHECK]),
            &CiGateConfig::default(),
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
        );
        assert_eq!(r.real, vec!["Build (windows-latest)".to_string()]);
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
        );
        assert_eq!(r.real, vec!["Build (ubuntu-latest)".to_string()]);
        assert!(r.hold_gate_is_the_hold);
        // No branch protection at all + no allow-list hit → still real.
        let r = classify_red(&rows[..1], &[], &CiGateConfig::default(), false);
        assert!(r.is_real());
        // Cancelled counts as red.
        let r = classify_red(
            &[row("Build (ubuntu-latest)", "CI", "cancel")],
            &[],
            &CiGateConfig::default(),
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
        let r = classify_red(&rows, &[], &CiGateConfig::default(), true);
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
    }
    impl crate::forge::Forge for FakeForge {
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
        fn watch_ci(&self, _: &crate::forge::ChangeRef) -> anyhow::Result<crate::forge::CiState> {
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
            let mut v = self.registrations.borrow_mut();
            Ok(if v.len() > 1 { v.remove(0) } else { v[0] })
        }
        fn check_rows(&self, _: &crate::forge::ChangeRef) -> anyhow::Result<Vec<CheckRow>> {
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
        }
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
                },
                CheckRow {
                    name: HOLD_GATE_CHECK.into(),
                    workflow: "merge-hold-gate".into(),
                    bucket: "fail".into(),
                },
            ]),
        );
        let r = refine_red(dir.path(), &f, &change(), true, Duration::from_secs(1), ms).unwrap();
        assert!(!r.is_real() && r.hold_gate_is_the_hold);
        assert!(!wait_hold_gate_green(&f, &change(), Duration::from_millis(3), ms).unwrap());
        // No rows (the pure-git default) → Err, callers keep the coarse verdict;
        // and the hold-gate wait has nothing to wait on.
        let f = fake(&[R::Registered], None);
        assert!(refine_red(dir.path(), &f, &change(), true, Duration::from_secs(1), ms).is_err());
        assert!(wait_hold_gate_green(&f, &change(), Duration::from_millis(3), ms).unwrap());
    }

    /// Source-shape guard: this module never shells out to a forge CLI itself.
    #[test]
    fn ci_gate_has_no_direct_forge_cli_calls() {
        let src = include_str!("ci_gate.rs");
        let body = &src[..src.find("#[cfg(test)]").unwrap()];
        assert!(!body.contains(concat!("Command::new(\"g", "h\")")));
        assert!(!body.contains(concat!("Command::new(\"gl", "ab\")")));
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
