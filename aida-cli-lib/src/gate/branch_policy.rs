//! Branch-policy merge refusals (STORY-1889): a forge refusal that a
//! supervised hold or branch protection caused must never be retried as
//! transient or reach the drain's auto-rebase path.
// mandate-gate
// trace:STORY-1889 | ai:claude

/// BUG-1447: GitHub prefixes both policy refusals and genuine conflicts with
/// "not mergeable". Recognize the stable policy detail before consulting the
/// broader conflict vocabulary so a supervised hold can neither spend the
/// transient retry budget nor reach the destructive auto-rebase path.
// trace:BUG-1447 | ai:codex
pub(crate) fn is_branch_policy_merge_refusal(reason: &str) -> bool {
    aida_core::external_tool_output::contains_any_case_insensitive(
        reason,
        aida_core::external_tool_output::GH_BRANCH_POLICY_MERGE_REFUSAL,
    )
}
