//! Positive review credit shared by dispatch and review views.
//! Decisions stay on the protected floor; recorder/grant evidence is read
//! through the existing durable authority providers.
// mandate-gate
// trace:BUG-2398 | ai:codex

/// Read-only positive review authority for dispatch and review views. Reuse
/// shipping's durable grant/receipt, identity, independence and time checks.
/// Unlike shipping, a view never fetches a missing commit or grants a solo
/// self-review exception. Missing local evidence stays unverified.
// trace:BUG-2398 | ai:codex
pub(crate) fn independent_approval_authorized(
    project_root: &std::path::Path,
    spec_ids: &[String],
    branch: Option<&str>,
    verdict: &crate::gate::review_verdict::RecordedVerdict,
) -> bool {
    if !verdict.kind.approves() || verdict.is_closed() {
        return false;
    }
    let candidates = std::slice::from_ref(verdict);
    let ids: Vec<String> = crate::gate::kernel_deps::normalized_spec_ids(spec_ids)
        .into_iter()
        .filter(|id| !id.starts_with("PR-") && !id.starts_with("MR-"))
        .collect();
    let author_sessions = if ids.is_empty() {
        crate::gate::review_authority::all_author_sessions(project_root, branch)
    } else {
        crate::gate::review_authority::author_sessions_at(project_root, &ids, branch)
    };
    let policy = crate::gate::ship::ShipApprovalPolicy {
        author_sessions,
        verified_authority: crate::gate::review_authority::verified_authority_keys(
            project_root,
            candidates,
        ),
        reviewed_committed_at: verdict
            .reviewed_sha
            .as_deref()
            .and_then(|sha| crate::gate::kernel_deps::commit_committed_at(project_root, sha)),
        shipper: Vec::new(),
        solo_human: false,
        verified_solo: Default::default(),
    };
    crate::gate::ship::approval_authority_refusal(candidates, &policy).is_none()
}

// trace:BUG-2398 | ai:codex
pub(crate) fn human_approval_authorized(
    root: &std::path::Path,
    req: &aida_core::Requirement,
    spec: &str,
    head: Option<&str>,
    branch: Option<&str>,
    verdict: &crate::gate::review_verdict::RecordedVerdict,
) -> bool {
    let ids: Vec<String> = [Some(spec), req.spec_id.as_deref(), req.agreed_id.as_deref()]
        .into_iter()
        .flatten()
        .map(str::to_string)
        .collect();
    matches!((verdict.reviewed_sha.as_deref(), head), (Some(a), Some(b))
        if crate::gate::review_verdict::same_reviewed_sha(a, b))
        && independent_approval_authorized(root, &ids, branch, verdict)
}
