//! Checked completion evidence for `aida db reconcile-status`.
//!
//! The replay used to collapse every open-change lookup failure into a bare
//! `None` and report the resulting empty flip list as "nothing matched", so
//! an operator could not tell a spec with no referencing commit from one
//! whose required forge check never ran. This module keeps that distinction:
//! a checked lookup that separates an authoritative clear/open answer from
//! unavailable evidence, a per-candidate outcome recorded before any filter
//! drops the candidate, and a pure renderer for both.
// trace:TASK-1335 | ai:claude

use crate::forge::ForgeKind;
use crate::process_retry::RetryEtxtbsy;
use aida_core::RequirementStatus;
use std::collections::BTreeMap;
use std::path::Path;

/// Why a required open-change check produced no authoritative answer. Each
/// variant names the failure actually observed; none is inferred (a nonzero
/// `gh` exit is reported with its own stderr, not guessed to be auth).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EvidenceUnavailable {
    /// The forge could not be classified (unreadable config, unrecognized
    /// provider, failed `origin` lookup).
    ForgeDiscovery(String),
    /// A configured forge this check has no query for.
    UnsupportedForge(ForgeKind),
    /// The forge CLI executable could not be found.
    MissingExecutable(&'static str),
    /// The forge CLI could not be started.
    Spawn(String),
    /// The forge CLI exited unsuccessfully.
    Exit { code: Option<i32>, stderr: String },
    /// The output was not JSON.
    Undecodable,
    /// The output was JSON but not an array of rows.
    NotArray,
    /// A row lacked a positive change number.
    BadRow,
    /// A diagnostic row lacked the string `title` and `body` it was asked for.
    BadDiagnosticRow,
}

impl std::fmt::Display for EvidenceUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ForgeDiscovery(why) => write!(f, "could not determine the forge ({why})"),
            Self::UnsupportedForge(kind) => write!(
                f,
                "the configured forge `{}` has no supported open-change query",
                kind.config_token()
            ),
            Self::MissingExecutable(cli) => write!(f, "`{cli}` executable not found"),
            Self::Spawn(e) => write!(f, "could not run `gh`: {e}"),
            Self::Exit { code, stderr } => {
                match code {
                    Some(code) => write!(f, "`gh pr list` exited with status {code}")?,
                    None => write!(f, "`gh pr list` was terminated by a signal")?,
                }
                if !stderr.is_empty() {
                    write!(f, ": {stderr}")?;
                }
                Ok(())
            }
            Self::Undecodable => write!(f, "`gh pr list` output was not JSON"),
            Self::NotArray => write!(f, "`gh pr list` returned JSON that is not an array"),
            Self::BadRow => write!(
                f,
                "`gh pr list` returned a row without a positive PR number"
            ),
            Self::BadDiagnosticRow => write!(
                f,
                "`gh pr list` returned a row without string `title` and `body` fields"
            ),
        }
    }
}

/// One candidate's open-change answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum OpenChangeCheck {
    /// Checked: no open change references the spec.
    Clear,
    /// Checked: this open change references the spec.
    Open(u64),
    /// No authoritative answer.
    Unavailable(EvidenceUnavailable),
}

/// Look up open changes for each spec id against an already-classified
/// forge. `forge` comes from [`crate::forge::checked_forge_kind`] (read-only)
/// or, for compatibility callers, `resolve_forge_kind`.
///
/// "Not GitHub" is two situations with opposite answers. A pure-git project
/// has no change requests, so "none is open" is a measured truth — every
/// candidate is `Clear`. A configured GitLab project has merge requests that
/// `gh` cannot read, so its state is unknown and every candidate is
/// `Unavailable`; unknown must never read as clear.
///
/// With `stop_at_unavailable`, the first unavailable answer ends the scan
/// (the compatibility wrapper needs only "all clear or not"); otherwise each
/// candidate is queried so a candidate whose own check succeeded is reported
/// as checked rather than as unavailable.
// trace:TASK-1335 | ai:claude
pub(crate) fn check_open_changes(
    project_root: &Path,
    forge: &Result<ForgeKind, String>,
    spec_ids: impl IntoIterator<Item = String>,
    stop_at_unavailable: bool,
) -> Vec<(String, OpenChangeCheck)> {
    let ids: Vec<String> = spec_ids.into_iter().collect();
    if ids.is_empty() {
        return Vec::new();
    }
    let all = |check: OpenChangeCheck| {
        ids.iter()
            .map(|id| (id.clone(), check.clone()))
            .collect::<Vec<_>>()
    };
    match forge {
        Err(why) => {
            return all(OpenChangeCheck::Unavailable(
                EvidenceUnavailable::ForgeDiscovery(why.clone()),
            ))
        }
        Ok(ForgeKind::None) => return all(OpenChangeCheck::Clear),
        Ok(ForgeKind::GitLab) => {
            return all(OpenChangeCheck::Unavailable(
                EvidenceUnavailable::UnsupportedForge(ForgeKind::GitLab),
            ))
        }
        Ok(ForgeKind::GitHub) => {}
    }
    let Some(gh) = crate::resolve_gh_binary() else {
        return all(OpenChangeCheck::Unavailable(
            EvidenceUnavailable::MissingExecutable("gh"),
        ));
    };
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let check = query_open_change(&gh, project_root, &id);
        let stop = stop_at_unavailable && matches!(check, OpenChangeCheck::Unavailable(_));
        out.push((id, check));
        if stop {
            break;
        }
    }
    out
}

/// The compatibility `Option` contract: `None` unless every lookup answered.
// trace:TASK-1335 | ai:claude
pub(crate) fn open_changes_or_none(
    checks: Vec<(String, OpenChangeCheck)>,
) -> Option<BTreeMap<String, u64>> {
    let mut open = BTreeMap::new();
    for (id, check) in checks {
        match check {
            OpenChangeCheck::Clear => {}
            OpenChangeCheck::Open(number) => {
                open.insert(id, number);
            }
            OpenChangeCheck::Unavailable(_) => return None,
        }
    }
    Some(open)
}

fn query_open_change(gh: &Path, project_root: &Path, spec_id: &str) -> OpenChangeCheck {
    let out = match std::process::Command::new(gh)
        .current_dir(project_root)
        // BUG-2134: GitHub search is token-fuzzy and also matches review
        // comments, so the first hit is not necessarily a PR that names the
        // spec. Fetch a page of hits with the fields the match is decided on
        // and let `parse_open_change_rows` keep only a genuine reference.
        // trace:BUG-2134 | ai:claude
        .args([
            "pr",
            "list",
            "--state",
            "open",
            "--search",
            spec_id,
            "--limit",
            "30",
            "--json",
            // BUG-2163: `state` is read live, unlike the `--state open`
            // filter, which goes through GitHub's lagging search index.
            // trace:BUG-2163 | ai:claude
            "number,title,body,headRefName,state",
        ])
        .output_retrying_etxtbsy()
    {
        Ok(out) => out,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return OpenChangeCheck::Unavailable(EvidenceUnavailable::MissingExecutable("gh"))
        }
        Err(e) => return OpenChangeCheck::Unavailable(EvidenceUnavailable::Spawn(e.to_string())),
    };
    if !out.status.success() {
        return OpenChangeCheck::Unavailable(exit_failure(&out));
    }
    parse_open_change_rows(&out.stdout, spec_id)
}

fn exit_failure(out: &std::process::Output) -> EvidenceUnavailable {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let first = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    EvidenceUnavailable::Exit {
        code: out.status.code(),
        stderr: first.trim().chars().take(200).collect(),
    }
}

/// Validate the complete `gh pr list --json number,title,body,headRefName,state`
/// response. Only a JSON array whose every row carries a positive integer
/// `number` and string `title` / `body` is an answer; `[]` is the
/// authoritative "no open change".
///
/// BUG-2134: a row counts as an open change for `spec_id` only when its title
/// or body names the id as a whole token, or its head branch is named for it
/// (`bug-1994`, `bug-1994-work`, `fix/bug-1994`). `gh pr list --search` also
/// returns PRs that mention the id only in a review comment, or that merely
/// contain its tokens (`BUG`, `1994`) apart. Before this filter such a PR held
/// a merged spec open: BUG-1994 stayed Needs Attention after #2597 and #2618
/// merged because unrelated #2606 quoted it in a review comment.
// trace:TASK-1335 trace:BUG-2134 | ai:claude
pub(crate) fn parse_open_change_rows(stdout: &[u8], spec_id: &str) -> OpenChangeCheck {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(stdout) else {
        return OpenChangeCheck::Unavailable(EvidenceUnavailable::Undecodable);
    };
    let Some(rows) = value.as_array() else {
        return OpenChangeCheck::Unavailable(EvidenceUnavailable::NotArray);
    };
    let ids = [spec_id.to_string()];
    let mut first = None;
    for row in rows {
        let Some(number) = row
            .get("number")
            .and_then(|n| n.as_u64())
            .filter(|n| *n > 0)
        else {
            return OpenChangeCheck::Unavailable(EvidenceUnavailable::BadRow);
        };
        let field = |name: &str| row.get(name).and_then(|v| v.as_str());
        let (Some(title), Some(body)) = (field("title"), field("body")) else {
            return OpenChangeCheck::Unavailable(EvidenceUnavailable::BadDiagnosticRow);
        };
        // BUG-2163: `--state open` filters on GitHub's search index, which
        // lags a merge by seconds to minutes. `aida pr ship` runs its pull
        // about 20s after merging, so the PR it just merged came back as
        // "open", its own `(SPEC-ID)` title named the spec, and the guard
        // withheld the very completion that merge earned (TASK-1730..1733).
        // The row's `state` is read live, so a row that says it is not OPEN
        // is not an open change. A row without `state` (older fixtures)
        // keeps the search filter's answer. trace:BUG-2163 | ai:claude
        if field("state").is_some_and(|state| !state.eq_ignore_ascii_case("OPEN")) {
            continue;
        }
        let branch = field("headRefName").unwrap_or("");
        let names_spec = crate::count_ids_mentioned(&format!("{title}\n{body}"), &ids) > 0
            || branch_names_spec(branch, spec_id);
        if names_spec {
            first.get_or_insert(number);
        }
    }
    first.map_or(OpenChangeCheck::Clear, OpenChangeCheck::Open)
}

/// BUG-2134: is `branch` named for `spec_id`? A path segment of the branch
/// must equal the id or start with it followed by `-` (`bug-1994-work`), so
/// `bug-19945` does not match `BUG-1994`.
// trace:BUG-2134 | ai:claude
fn branch_names_spec(branch: &str, spec_id: &str) -> bool {
    let id = spec_id.to_ascii_lowercase();
    if id.is_empty() {
        return false;
    }
    branch.to_ascii_lowercase().split('/').any(|segment| {
        segment
            .strip_prefix(id.as_str())
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'))
    })
}

/// The already-Completed sweep diagnostic: how many of `candidate_ids` an
/// open PR's title or body still names, from ONE `gh pr list` call. A
/// pure-git project answers 0; every other failure is reported, never 0.
/// One call regardless of candidate count, because this direction can hold
/// every already-Completed spec a wide scan touches.
// trace:TASK-1446 | ai:claude
// trace:TASK-1335 | ai:claude
pub(crate) fn count_completed_with_open_changes(
    project_root: &Path,
    forge: &Result<ForgeKind, String>,
    candidate_ids: &[String],
) -> Result<usize, EvidenceUnavailable> {
    match forge {
        Err(why) => return Err(EvidenceUnavailable::ForgeDiscovery(why.clone())),
        Ok(ForgeKind::None) => return Ok(0),
        Ok(ForgeKind::GitLab) => {
            return Err(EvidenceUnavailable::UnsupportedForge(ForgeKind::GitLab))
        }
        Ok(ForgeKind::GitHub) => {}
    }
    let gh = crate::resolve_gh_binary().ok_or(EvidenceUnavailable::MissingExecutable("gh"))?;
    let out = std::process::Command::new(&gh)
        .current_dir(project_root)
        .args([
            "pr",
            "list",
            "--state",
            "open",
            "--limit",
            "200",
            "--json",
            "title,body",
        ])
        .output_retrying_etxtbsy()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                EvidenceUnavailable::MissingExecutable("gh")
            } else {
                EvidenceUnavailable::Spawn(e.to_string())
            }
        })?;
    if !out.status.success() {
        return Err(exit_failure(&out));
    }
    count_diagnostic_rows(&out.stdout, candidate_ids)
}

/// Validate the complete `gh pr list --json title,body` response and count
/// the candidates its rows name.
// trace:TASK-1335 | ai:claude
pub(crate) fn count_diagnostic_rows(
    stdout: &[u8],
    candidate_ids: &[String],
) -> Result<usize, EvidenceUnavailable> {
    let value = serde_json::from_slice::<serde_json::Value>(stdout)
        .map_err(|_| EvidenceUnavailable::Undecodable)?;
    let rows = value.as_array().ok_or(EvidenceUnavailable::NotArray)?;
    let mut haystack = String::new();
    // Every row must carry BOTH requested fields as strings (an empty body is
    // `""`); anything else is not a measurement, so it cannot count as 0.
    for row in rows {
        for field in ["title", "body"] {
            let text = row
                .get(field)
                .and_then(|v| v.as_str())
                .ok_or(EvidenceUnavailable::BadDiagnosticRow)?;
            haystack.push_str(text);
            haystack.push('\n');
        }
    }
    Ok(crate::count_ids_mentioned(&haystack, candidate_ids))
}

/// Where a candidate's completion evidence came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EvidenceSource {
    SubjectTrailer,
    SquashBodyTrailer,
    ReviewStoryMerge,
    CoversChain,
}

impl EvidenceSource {
    fn label(self) -> &'static str {
        match self {
            Self::SubjectTrailer => "subject trailer",
            Self::SquashBodyTrailer => "squash-body trailer",
            Self::ReviewStoryMerge => "review-story PR merge",
            Self::CoversChain => "covers chain",
        }
    }
}

/// The requirement a candidate id resolved to at scan time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CandidateIdentity {
    pub(crate) uuid: uuid::Uuid,
    pub(crate) display: String,
    pub(crate) status: RequirementStatus,
}

impl CandidateIdentity {
    pub(crate) fn of(req: &aida_core::Requirement) -> Self {
        Self {
            uuid: req.id,
            display: req
                .agreed_id
                .clone()
                .or_else(|| req.spec_id.clone())
                .unwrap_or_else(|| req.id.to_string()),
            status: req.status.clone(),
        }
    }
}

/// Where a candidate ended up, at the stage that decided it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CandidateReason {
    UnknownSpec,
    AmbiguousSpec,
    AlreadyTerminal,
    Ineligible,
    StaleAfterReopen,
    OpenChange(u64),
    RequiredCheckUnavailable(EvidenceUnavailable),
    /// Its own check was clear, but another candidate's required check was
    /// unavailable and the batch completes all or nothing.
    BatchDeferred,
    ClosureHeld,
    WouldComplete,
    Applied,
    /// The live store no longer supported completion at the write seam.
    ChangedDuringApply(String),
    /// A closure holder keeps this review story from completing (no landing).
    Held(String),
    /// Another accepted id of the same requirement was applied instead.
    SameRequirementAs(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CandidateOutcome {
    /// The id as the evidence named it.
    pub(crate) spec_id: String,
    pub(crate) identity: Option<CandidateIdentity>,
    pub(crate) sha: String,
    pub(crate) source: EvidenceSource,
    pub(crate) reason: CandidateReason,
}

/// Per-candidate outcomes for one reconcile run, in first-seen order. A
/// later stage replaces the reason an earlier stage recorded.
#[derive(Debug, Default)]
pub(crate) struct ReconcileOutcomes {
    entries: Vec<CandidateOutcome>,
}

impl ReconcileOutcomes {
    pub(crate) fn record(&mut self, outcome: CandidateOutcome) {
        match self
            .entries
            .iter_mut()
            .find(|o| o.spec_id.eq_ignore_ascii_case(&outcome.spec_id))
        {
            Some(existing) => *existing = outcome,
            None => self.entries.push(outcome),
        }
    }

    pub(crate) fn set_reason(&mut self, spec_id: &str, reason: CandidateReason) {
        if let Some(o) = self
            .entries
            .iter_mut()
            .find(|o| o.spec_id.eq_ignore_ascii_case(spec_id))
        {
            o.reason = reason;
        }
    }

    pub(crate) fn get(&self, spec_id: &str) -> Option<&CandidateOutcome> {
        self.entries
            .iter()
            .find(|o| o.spec_id.eq_ignore_ascii_case(spec_id))
    }

    pub(crate) fn entries(&self) -> &[CandidateOutcome] {
        &self.entries
    }

    pub(crate) fn required_unavailable(&self) -> Vec<&CandidateOutcome> {
        self.entries
            .iter()
            .filter(|o| matches!(o.reason, CandidateReason::RequiredCheckUnavailable(_)))
            .collect()
    }
}

/// The pinned default-branch snapshot a run scanned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScanRange {
    pub(crate) branch: String,
    pub(crate) oid: String,
    pub(crate) since: Option<String>,
    pub(crate) max_count: usize,
}

impl std::fmt::Display for ScanRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.branch, short_sha(&self.oid))?;
        match &self.since {
            Some(since) => write!(f, " (commits after `{since}`)"),
            None => write!(f, " (last {} commits)", self.max_count),
        }
    }
}

fn short_sha(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

/// One human-readable line for `outcome`, or `None` when another report
/// (the dry-run/applied summaries, closure-hold lines, the no-flip message)
/// already says it. A sweep shows only what needs the operator; a targeted
/// `--spec` run explains every outcome.
// trace:TASK-1335 | ai:claude
pub(crate) fn render_outcome(outcome: &CandidateOutcome, targeted: bool) -> Option<String> {
    let name = match &outcome.identity {
        Some(id) if !id.display.eq_ignore_ascii_case(&outcome.spec_id) => {
            format!("{} (named as {})", id.display, outcome.spec_id)
        }
        _ => outcome.spec_id.clone(),
    };
    let evidence = if outcome.sha.is_empty() {
        format!("{} evidence", outcome.source.label())
    } else {
        format!(
            "commit {} ({})",
            short_sha(&outcome.sha),
            outcome.source.label()
        )
    };
    let status = outcome
        .identity
        .as_ref()
        .map(|id| id.status.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let line = match &outcome.reason {
        CandidateReason::OpenChange(pr) => {
            format!(
                "↷ {} stays Done — open PR #{} still references it",
                outcome.spec_id, pr
            )
        }
        CandidateReason::RequiredCheckUnavailable(why) => format!(
            "✗ {name}: {evidence} matched; completion deferred — required open-PR check \
             unavailable: {why}. Status unchanged ({status}); retry when forge access is restored."
        ),
        CandidateReason::BatchDeferred => format!(
            "↷ {name}: {evidence} matched and its open-PR check was clear; completion deferred \
             because another candidate's required check was unavailable (the batch completes all \
             or nothing). Status unchanged ({status})."
        ),
        CandidateReason::ChangedDuringApply(why) => format!(
            "↷ {name}: {evidence} matched, but the spec changed after the scan ({why}); not \
             completed — rerun to re-evaluate."
        ),
        CandidateReason::Held(why) => {
            format!(
                "↷ {name}: {evidence} matched, but completion is held — {why}; status unchanged."
            )
        }
        _ if !targeted => return None,
        CandidateReason::SameRequirementAs(other) => format!(
            "· {name}: {evidence} names the same requirement as {other}, which was completed \
             once from its own evidence."
        ),
        CandidateReason::UnknownSpec => {
            format!("· {name}: {evidence} names it, but no requirement has that ID.")
        }
        CandidateReason::AmbiguousSpec => format!(
            "· {name}: {evidence} names an ID shared by more than one requirement; refused."
        ),
        CandidateReason::Ineligible => format!(
            "· {name}: {evidence} matched, but its status ({status}) is not one the replay \
             completes."
        ),
        CandidateReason::StaleAfterReopen => format!(
            "· {name}: {evidence} matched, but the spec was reopened after that evidence; a \
             later qualifying commit is needed."
        ),
        CandidateReason::AlreadyTerminal
        | CandidateReason::ClosureHeld
        | CandidateReason::WouldComplete
        | CandidateReason::Applied => return None,
    };
    Some(line)
}

/// What the run changed through paths other than the normal completion
/// batch, for an honest final error.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct OtherEffects {
    pub(crate) drafts_landed: usize,
    pub(crate) review_stories_completed: usize,
}

/// The ordinary error a run returns when an actual candidate lost a required
/// evidence check. No numeric exit family: callers surface it as any error.
// trace:TASK-1335 | ai:claude
pub(crate) fn required_unavailable_error(
    unavailable: &[&CandidateOutcome],
    scan: &ScanRange,
    dry_run: bool,
    other: OtherEffects,
) -> String {
    let ids: Vec<&str> = unavailable
        .iter()
        .map(|o| {
            o.identity
                .as_ref()
                .map(|id| id.display.as_str())
                .unwrap_or(o.spec_id.as_str())
        })
        .collect();
    let mut msg = format!(
        "reconcile-status could not verify required completion evidence for {} candidate{} ({}) \
         scanned on {scan}; no spec was completed from that batch",
        ids.len(),
        if ids.len() == 1 { "" } else { "s" },
        ids.join(", ")
    );
    if dry_run {
        msg.push_str(" (dry run: nothing was written)");
    } else if other == OtherEffects::default() {
        msg.push_str(" and nothing else changed");
    } else {
        msg.push_str(&format!(
            "; other independent changes WERE applied: {} Draft spec{} landed at Done, {} review \
             stor{} completed",
            other.drafts_landed,
            if other.drafts_landed == 1 { "" } else { "s" },
            other.review_stories_completed,
            if other.review_stories_completed == 1 {
                "y"
            } else {
                "ies"
            },
        ));
    }
    msg.push_str(". Retry when forge access is restored.");
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_change_rows_require_a_complete_valid_array() {
        assert_eq!(
            parse_open_change_rows(b"[]\n", "TASK-11"),
            OpenChangeCheck::Clear
        );
        assert_eq!(
            parse_open_change_rows(
                br#"[{"number":2436,"title":"wip (TASK-11)","body":""}]"#,
                "TASK-11"
            ),
            OpenChangeCheck::Open(2436)
        );
        for (bytes, want) in [
            (&b"not json"[..], EvidenceUnavailable::Undecodable),
            (&b""[..], EvidenceUnavailable::Undecodable),
            (&br#"[{"number":1}"#[..], EvidenceUnavailable::Undecodable),
            (&br#"{"number":1}"#[..], EvidenceUnavailable::NotArray),
            (&b"null"[..], EvidenceUnavailable::NotArray),
            (&br#"[{"title":"x"}]"#[..], EvidenceUnavailable::BadRow),
            (&br#"[{"number":"7"}]"#[..], EvidenceUnavailable::BadRow),
            (
                &br#"[{"number":7}]"#[..],
                EvidenceUnavailable::BadDiagnosticRow,
            ),
            (
                &br#"[{"number":7,"title":"x","body":null}]"#[..],
                EvidenceUnavailable::BadDiagnosticRow,
            ),
            (&br#"[{"number":0}]"#[..], EvidenceUnavailable::BadRow),
            (&br#"[{"number":-3}]"#[..], EvidenceUnavailable::BadRow),
            (
                &br#"[{"number":5,"title":"x","body":""},7]"#[..],
                EvidenceUnavailable::BadRow,
            ),
        ] {
            assert_eq!(
                parse_open_change_rows(bytes, "TASK-11"),
                OpenChangeCheck::Unavailable(want),
                "{}",
                String::from_utf8_lossy(bytes)
            );
        }
    }

    /// BUG-2134: only a PR whose title, body or head branch names the spec
    /// holds it open. A search hit that matched on a review comment or on the
    /// id's tokens apart (the #2606 / BUG-1994 case) is not an open change.
    // trace:BUG-2134 | ai:claude
    #[test]
    fn open_change_rows_keep_only_prs_that_name_the_spec() {
        let check = |bytes: &[u8]| parse_open_change_rows(bytes, "BUG-1994");
        // Comment-only / fuzzy hit: nothing in title, body or branch.
        assert_eq!(
            check(br#"[{"number":2606,"title":"fix(pr-ship): never None (BUG-1997)","body":"see 1994 bugs","headRefName":"bug-1997"}]"#),
            OpenChangeCheck::Clear
        );
        // A longer id is not the spec.
        assert_eq!(
            check(br#"[{"number":9,"title":"x (BUG-19945)","body":"","headRefName":"bug-19945"}]"#),
            OpenChangeCheck::Clear
        );
        // Title trailer, body prose, and branch name each count.
        assert_eq!(
            check(br#"[{"number":2606,"title":"x","body":"","headRefName":"y"},{"number":2597,"title":"fix (BUG-1994)","body":"","headRefName":"z"}]"#),
            OpenChangeCheck::Open(2597)
        );
        assert_eq!(
            check(br#"[{"number":5,"title":"x","body":"part 2 of BUG-1994","headRefName":"z"}]"#),
            OpenChangeCheck::Open(5)
        );
        assert_eq!(
            check(br#"[{"number":6,"title":"x","body":"","headRefName":"bug-1994-work"}]"#),
            OpenChangeCheck::Open(6)
        );
        assert_eq!(
            check(br#"[{"number":7,"title":"x","body":""}]"#),
            OpenChangeCheck::Clear
        );
    }

    /// BUG-2163: the search index still lists a just-merged PR as open, but
    /// the row's live `state` says MERGED. That PR must not hold the spec its
    /// own merge completed; a genuinely OPEN row still does, and a row with no
    /// `state` keeps the old behaviour.
    // trace:BUG-2163 | ai:claude
    #[test]
    fn open_change_rows_ignore_rows_whose_live_state_is_not_open() {
        let check = |bytes: &[u8]| parse_open_change_rows(bytes, "TASK-1732");
        assert_eq!(
            check(br#"[{"number":2666,"title":"[AI:codex] feat(regen): score environment seam adapters (TASK-1732)","body":"","headRefName":"codex/task-1732","state":"MERGED"}]"#),
            OpenChangeCheck::Clear
        );
        assert_eq!(
            check(br#"[{"number":2666,"title":"x (TASK-1732)","body":"","headRefName":"y","state":"CLOSED"},{"number":2670,"title":"part two (TASK-1732)","body":"","headRefName":"z","state":"OPEN"}]"#),
            OpenChangeCheck::Open(2670)
        );
        assert_eq!(
            check(
                br#"[{"number":2670,"title":"part two (TASK-1732)","body":"","headRefName":"z"}]"#
            ),
            OpenChangeCheck::Open(2670)
        );
    }

    #[test]
    fn diagnostic_rows_require_string_title_and_body() {
        let ids = vec!["TASK-11".to_string(), "TASK-12".to_string()];
        assert_eq!(count_diagnostic_rows(b"[]", &ids), Ok(0));
        assert_eq!(
            count_diagnostic_rows(br#"[{"title":"wip (TASK-11)","body":""}]"#, &ids),
            Ok(1)
        );
        assert_eq!(
            count_diagnostic_rows(br#"[{"title":"other","body":"TASK-110"}]"#, &ids),
            Ok(0)
        );
        for (bytes, want) in [
            (&b"[{}]"[..], EvidenceUnavailable::BadDiagnosticRow),
            (
                &br#"[{"title":42,"body":false}]"#[..],
                EvidenceUnavailable::BadDiagnosticRow,
            ),
            (
                &br#"[{"title":"x"}]"#[..],
                EvidenceUnavailable::BadDiagnosticRow,
            ),
            (
                &br#"[{"title":"x","body":null}]"#[..],
                EvidenceUnavailable::BadDiagnosticRow,
            ),
            (
                &br#"[{"title":"TASK-11","body":""},{"body":""}]"#[..],
                EvidenceUnavailable::BadDiagnosticRow,
            ),
            (
                &br#"["TASK-11"]"#[..],
                EvidenceUnavailable::BadDiagnosticRow,
            ),
            (
                &br#"[{"title":"x","body":""}"#[..],
                EvidenceUnavailable::Undecodable,
            ),
            (
                &br#"{"title":"x","body":""}"#[..],
                EvidenceUnavailable::NotArray,
            ),
        ] {
            assert_eq!(
                count_diagnostic_rows(bytes, &ids),
                Err(want),
                "{}",
                String::from_utf8_lossy(bytes)
            );
        }
    }

    #[test]
    fn compatibility_wrapper_is_none_on_any_unavailable_answer() {
        let checks = vec![
            ("A-1".to_string(), OpenChangeCheck::Open(4)),
            ("A-2".to_string(), OpenChangeCheck::Clear),
        ];
        assert_eq!(
            open_changes_or_none(checks),
            Some(BTreeMap::from([("A-1".to_string(), 4)]))
        );
        let checks = vec![
            ("A-1".to_string(), OpenChangeCheck::Clear),
            (
                "A-2".to_string(),
                OpenChangeCheck::Unavailable(EvidenceUnavailable::BadRow),
            ),
        ];
        assert_eq!(open_changes_or_none(checks), None);
    }

    #[test]
    fn unclassified_and_unsupported_forges_are_never_clear() {
        let root = Path::new("/nonexistent-task-1335");
        let ids = || vec!["A-1".to_string(), "A-2".to_string()];
        let failed: Result<ForgeKind, String> = Err("bad config".into());
        assert!(check_open_changes(root, &failed, ids(), false)
            .iter()
            .all(|(_, c)| matches!(
                c,
                OpenChangeCheck::Unavailable(EvidenceUnavailable::ForgeDiscovery(_))
            )));
        assert!(
            check_open_changes(root, &Ok(ForgeKind::GitLab), ids(), false)
                .iter()
                .all(|(_, c)| matches!(
                    c,
                    OpenChangeCheck::Unavailable(EvidenceUnavailable::UnsupportedForge(_))
                ))
        );
        assert!(check_open_changes(root, &Ok(ForgeKind::None), ids(), false)
            .iter()
            .all(|(_, c)| *c == OpenChangeCheck::Clear));
        assert_eq!(
            count_completed_with_open_changes(root, &Ok(ForgeKind::None), &ids()),
            Ok(0)
        );
        assert_eq!(
            count_completed_with_open_changes(root, &Ok(ForgeKind::GitLab), &ids()),
            Err(EvidenceUnavailable::UnsupportedForge(ForgeKind::GitLab))
        );
    }

    #[test]
    fn render_names_the_matched_commit_and_required_failure() {
        let outcome = CandidateOutcome {
            spec_id: "TASK-1-226".into(),
            identity: Some(CandidateIdentity {
                uuid: uuid::Uuid::nil(),
                display: "TASK-1588".into(),
                status: RequirementStatus::Done,
            }),
            sha: "978c4974292a59513f53bab40b53b10be8122ca7".into(),
            source: EvidenceSource::SubjectTrailer,
            reason: CandidateReason::RequiredCheckUnavailable(
                EvidenceUnavailable::MissingExecutable("gh"),
            ),
        };
        let line = render_outcome(&outcome, false).unwrap();
        assert!(line.contains("TASK-1588 (named as TASK-1-226)"), "{line}");
        assert!(
            line.contains("commit 978c497 (subject trailer) matched"),
            "{line}"
        );
        assert!(line.contains("`gh` executable not found"), "{line}");
        assert!(line.contains("Status unchanged (Done)"), "{line}");
        assert!(!line.contains("no commit"), "{line}");

        let stale = CandidateOutcome {
            reason: CandidateReason::StaleAfterReopen,
            ..outcome
        };
        assert_eq!(render_outcome(&stale, false), None);
        assert!(render_outcome(&stale, true).unwrap().contains("reopened"));
    }

    #[test]
    fn final_error_is_truthful_about_partial_effects() {
        let outcome = CandidateOutcome {
            spec_id: "B-2".into(),
            identity: None,
            sha: "abcdef0123".into(),
            source: EvidenceSource::SubjectTrailer,
            reason: CandidateReason::RequiredCheckUnavailable(EvidenceUnavailable::Undecodable),
        };
        let scan = ScanRange {
            branch: "main".into(),
            oid: "0123456789abcdef".into(),
            since: None,
            max_count: 200,
        };
        let dry = required_unavailable_error(&[&outcome], &scan, true, OtherEffects::default());
        assert!(dry.contains("main@0123456 (last 200 commits)"), "{dry}");
        assert!(dry.contains("dry run: nothing was written"), "{dry}");
        let quiet = required_unavailable_error(&[&outcome], &scan, false, OtherEffects::default());
        assert!(quiet.contains("nothing else changed"), "{quiet}");
        let partial = required_unavailable_error(
            &[&outcome],
            &scan,
            false,
            OtherEffects {
                drafts_landed: 1,
                review_stories_completed: 0,
            },
        );
        assert!(
            partial.contains("WERE applied: 1 Draft spec landed"),
            "{partial}"
        );
        assert!(!partial.contains("nothing else changed"), "{partial}");
    }
}
