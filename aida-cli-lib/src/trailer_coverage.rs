//! BUG-HOME-048: the parsing and display side of the trailer coverage check.
//!
//! The decision — which specs a PR covers, and what ship, `review record`
//! and the gates refuse — lives on the protected floor in
//! [`crate::gate::trailer_coverage`]. This module reads the completion
//! trailers out of commit messages the way the pull-side auto-bump does,
//! rewrites squash-body trailer lines, reads a branch's commit range and the
//! leases on a branch, and formats the refusal phrases.
// trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude

use aida_core::Requirement;

/// `` `id` resolves to X ("title"), which this PR does not cover (it claims
/// Y) `` for trailer `id` resolving to the uncovered `req`.
// trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
pub(crate) fn uncovered_phrase(id: &str, req: &Requirement, claimed: &[String]) -> String {
    // spec_id first: a legacy spec's agreed id IS the bare trailer, so only
    // its spec id tells the reader it is a different spec.
    let resolved = req.spec_id.as_deref().unwrap_or(id);
    format!(
        "`{id}` resolves to {resolved} (\"{}\"), which this PR does not cover (it claims {})",
        short_title(&req.title),
        claimed.join(", "),
    )
}

/// The spec ids the pull-side auto-bump completes from `message` once it is
/// a commit on the default branch: the subject's trailers, and — on a
/// `(#N)` squash/merge subject — the body's trailer lines, minus `(Refs …)`
/// references. A plan, non-completing or slice commit completes nothing.
/// Mirrors the scan in `db_reconcile` so ship refuses exactly what the pull
/// would complete.
// trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
pub(crate) fn commit_completion_ids(message: &str) -> Vec<String> {
    let subject = message.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    if crate::zen_drive_root::is_plan_commit_subject(subject)
        || crate::db_reconcile::commit_declares_non_completing(message)
        || crate::db_reconcile::slice_subject_withholds_completion(subject)
    {
        return Vec::new();
    }
    let is_ref = crate::db_reconcile::refs_completion_filter(subject, message);
    let mut out: Vec<String> = crate::zen_drive_root::extract_spec_ids_from_commit(subject)
        .into_iter()
        .filter(|id| !is_ref.subject(id))
        .collect();
    if crate::pr_lookup::extract_pr_number_from_commit_subject(subject).is_some() {
        for id in crate::pr_lookup::extract_referenced_spec_ids_from_commit(message) {
            if !is_ref.body(&id) && !out.iter().any(|o| o.eq_ignore_ascii_case(&id)) {
                out.push(id);
            }
        }
    }
    out
}

/// One trailing paren group of a body line, as the pull-side scan walks it.
enum TrailerGroup {
    /// `(#N)`, kept verbatim.
    Pr(String),
    /// A spec-id group: its inner text and the ids the scan harvests from it.
    Ids(String, Vec<String>),
}

/// `body` with each trailer line rewritten so the ids `uncovered` answers
/// `Some` for move into a leading `(Refs …)` group: the pull-side scan stops
/// at a group that does not start with a spec id, and its Refs filter never
/// completes a referenced id. The other trailers on the line keep their
/// groups (and their completion). Code-like lines, which the scan skips,
/// are left as they are.
// trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
pub(crate) fn rewrite_body_trailers(
    body: &str,
    mut uncovered: impl FnMut(&str) -> Option<()>,
) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in body.lines() {
        if crate::pr_lookup::body_line_is_code_like(line) {
            out.push(line.to_string());
            continue;
        }
        out.push(rewrite_trailer_line(line, &mut uncovered).unwrap_or_else(|| line.to_string()));
    }
    let mut joined = out.join("\n");
    if body.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

/// `line` rewritten (see [`rewrite_body_trailers`]), or `None` when none of
/// its trailers is uncovered.
fn rewrite_trailer_line(
    line: &str,
    uncovered: &mut impl FnMut(&str) -> Option<()>,
) -> Option<String> {
    let mut tail = line.trim_end();
    let mut groups: Vec<TrailerGroup> = Vec::new();
    while tail.ends_with(')') {
        let Some(open) = tail.rfind('(') else {
            break;
        };
        let inner = &tail[open + 1..tail.len() - 1];
        if inner.len() > 1
            && inner.starts_with('#')
            && inner[1..].chars().all(|c| c.is_ascii_digit())
        {
            groups.push(TrailerGroup::Pr(format!("({inner})")));
        } else {
            let mut ids = Vec::new();
            crate::pr_lookup::push_paren_spec_ids_from_line(&format!("({inner})"), &mut ids);
            if ids.is_empty() {
                break;
            }
            groups.push(TrailerGroup::Ids(inner.to_string(), ids));
        }
        tail = tail[..open].trim_end();
    }
    let mut refs: Vec<String> = Vec::new();
    let mut kept: Vec<String> = Vec::new();
    for group in groups.into_iter().rev() {
        match group {
            TrailerGroup::Pr(text) => kept.push(text),
            TrailerGroup::Ids(inner, ids) => {
                let (bad, good): (Vec<String>, Vec<String>) =
                    ids.into_iter().partition(|id| uncovered(id).is_some());
                if bad.is_empty() {
                    kept.push(format!("({inner})"));
                    continue;
                }
                refs.extend(bad);
                if !good.is_empty() {
                    kept.push(format!("({})", good.join(", ")));
                }
            }
        }
    }
    if refs.is_empty() {
        return None;
    }
    let mut rewritten = tail.to_string();
    if !rewritten.trim().is_empty() {
        rewritten.push(' ');
    }
    rewritten.push_str(&format!("(Refs {})", refs.join(", ")));
    for k in kept {
        rewritten.push(' ');
        rewritten.push_str(&k);
    }
    Some(rewritten)
}

/// The full messages of the commits in `base..head`, oldest first. An
/// unreadable range is an error: the trailer check fails closed on it.
// trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
pub(crate) fn branch_commit_messages(
    project_root: &std::path::Path,
    base: &str,
    head: &str,
) -> anyhow::Result<Vec<String>> {
    let range = format!("{base}..{head}");
    let out = aida_core::git_ops::host_git_command(project_root)
        .args(["log", "--reverse", "-z", "--format=%B", &range, "--"])
        .output()
        .map_err(|e| anyhow::anyhow!("could not run git log {range}: {e}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "git log {range} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .collect())
}

/// The `--owns` scopes of leases on `branch` in `project_root`'s main
/// worktree.
// trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
pub(crate) fn lease_scopes_for_branch(project_root: &std::path::Path, branch: &str) -> Vec<String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Vec::new();
    }
    let root = crate::worktree_gc::main_worktree_root_from(project_root);
    crate::leases::list_leases(&root)
        .into_iter()
        .filter(|l| l.branch == branch)
        .map(|l| l.scope)
        .collect()
}

pub(crate) fn short_title(title: &str) -> String {
    const MAX: usize = 60;
    if title.chars().count() <= MAX {
        title.to_string()
    } else {
        let cut: String = title.chars().take(MAX).collect();
        format!("{cut}…")
    }
}
