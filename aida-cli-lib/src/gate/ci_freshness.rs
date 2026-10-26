//! Stale-definition and behind-base ship gates.
// trace:STORY-1889 | ai:claude
// mandate-gate

use crate::pr_cmd::{fetch_ship_refs, pr_ship_target_branch};
use crate::*;

/// The stale-definition gate's own predicate: PR `pr_number`'s base branch
/// and the staleness warning its ship branch carries against it. A warning
/// with `definition_files` is exactly what [`ship_behind_base_gate`]
/// refuses, so `aida pr ship-batch` reads this, never a re-implementation.
// trace:STORY-1887 | ai:claude
pub(crate) fn ship_stale_definition_warning(
    project_root: &std::path::Path,
    ship_branch: &str,
    pr_number: u64,
) -> (String, Option<StaleCheckWarning>) {
    let base_branch = pr_ship_target_branch(pr_number);
    let base_ref = format!("origin/{base_branch}");
    let warning = pr_stale_check_warning(project_root, ship_branch, &base_ref);
    (base_branch, warning)
}

/// What the stale gate does with PR `pr_number`'s staleness against
/// `base_branch`: `Ok` ships, `Err` is the refusal. TASK-1606 refused only a
/// green that predates a CI-definition change. STORY-1690: a head missing
/// ANY commit on the base refuses, and no flag overrides it — two PRs each
/// green on its own base can break main together (#2599/#2597), so a behind
/// head lands through `aida pr train N`, which tests it on the current base.
// trace:TASK-1606 | ai:codex
// trace:STORY-1690 | ai:claude
pub(crate) fn stale_gate_decision(
    pr_number: u64,
    base_branch: &str,
    warning: Option<&StaleCheckWarning>,
    override_stale_check: bool,
) -> std::result::Result<(), String> {
    let Some(warning) = warning else {
        return Ok(());
    };
    let definition = if warning.definition_files.is_empty() {
        String::new()
    } else {
        format!(
            " Its CI also ran against an OLDER definition of the check ({} changed since).",
            warning.definition_files.join(", ")
        )
    };
    let flag = if override_stale_check {
        " `--override-stale-check` does not ship a head that is behind its base."
    } else {
        ""
    };
    Err(format!(
        "PR-{pr_number}'s head is {} commit(s) behind {base_branch}, so its green was never \
         tested together with them; two PRs each green on its own base can still turn \
         {base_branch} red together.{definition} Land it with `aida pr train {pr_number}` \
         (it tests the PR on top of the current {base_branch} before merging), or merge \
         {base_branch} into the branch, wait for green, and ship again.{flag}",
        warning.behind_commits,
    ))
}

/// The behind-base gate's outcome for PR `pr_number`: `staleness` is the
/// fail-closed [`crate::pr_stale_check`] result (or the fetch error). A head
/// behind its base refuses as `BehindBase`, or `StaleDefinition` when a CI
/// definition also changed; staleness that cannot be computed refuses too.
// trace:STORY-1690 | ai:claude
pub(crate) fn behind_base_refusal(
    pr_number: u64,
    base_branch: &str,
    staleness: std::result::Result<Option<StaleCheckWarning>, String>,
    override_stale_check: bool,
) -> std::result::Result<(), (crate::gate::ship::ShipRefusal, String)> {
    use crate::gate::ship::ShipRefusal;
    let warning = staleness.map_err(|e| {
        (
            ShipRefusal::BehindBase,
            format!(
                "PR-{pr_number}: whether its head is behind {base_branch} could not be \
                 computed ({e}), so its green may be stale; refusing rather than shipping it. \
                 Fix the fetch, or land it with `aida pr train {pr_number}`."
            ),
        )
    })?;
    let kind = if warning
        .as_ref()
        .is_some_and(|w| !w.definition_files.is_empty())
    {
        ShipRefusal::StaleDefinition
    } else {
        ShipRefusal::BehindBase
    };
    stale_gate_decision(
        pr_number,
        base_branch,
        warning.as_ref(),
        override_stale_check,
    )
    .map_err(|refusal| (kind, refusal))
}

/// The behind-base gate, live: fetch the base and the head, then refuse a
/// head behind its base. `aida pr ship` (both preflight passes) and the
/// drain's merge phase call it.
// trace:TASK-1606 | ai:codex
// trace:STORY-1690 | ai:claude
pub(crate) fn ship_behind_base_gate(
    project_root: &std::path::Path,
    ship_branch: &str,
    pr_number: u64,
    override_stale_check: bool,
) -> std::result::Result<(), (crate::gate::ship::ShipRefusal, String)> {
    behind_base_gate_on(
        project_root,
        ship_branch,
        pr_number,
        &pr_ship_target_branch(pr_number),
        override_stale_check,
    )
}

/// [`ship_behind_base_gate`] against a known base branch.
// trace:STORY-1690 | ai:claude
pub(crate) fn behind_base_gate_on(
    project_root: &std::path::Path,
    ship_branch: &str,
    pr_number: u64,
    base_branch: &str,
    override_stale_check: bool,
) -> std::result::Result<(), (crate::gate::ship::ShipRefusal, String)> {
    let staleness = fetch_ship_refs(project_root, base_branch, ship_branch).and_then(|()| {
        crate::pr_stale_check(project_root, ship_branch, &format!("origin/{base_branch}"))
    });
    behind_base_refusal(pr_number, base_branch, staleness, override_stale_check)
}
