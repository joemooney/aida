// trace:TASK-3428 | ai:codex
use crate::{advance_backend, detect_distributed_store_from, is_terminal_status_str, worker};
use anyhow::Result;

/// `aida worker gc`: prune `drain <SPEC-ID>` directives whose target spec is
/// archived or terminal (Completed / Rejected) from `.aida/worker.cmd`.
// trace:BUG-723 trace:BUG-1670 | ai:claude
pub(crate) fn run_worker_gc(project_root: &std::path::Path, dry_run: bool) -> Result<()> {
    let path = worker::worker_cmd_path(project_root);
    let body = std::fs::read_to_string(&path).unwrap_or_default();
    if worker::parse_directives_from_str(&body).is_empty() {
        println!("No pending directives.");
        return Ok(());
    }
    let store_path = detect_distributed_store_from(project_root).ok_or_else(|| {
        anyhow::anyhow!("no requirement store found — cannot resolve directive target specs")
    })?;
    let backend = advance_backend(&store_path)?;
    // Both view axes wide open: an archived (or deferred) target must
    // still resolve so its directive is classified correctly. Strict: the
    // tolerant read serves an old snapshot while another process writes the
    // cache, and this pass deletes directives. trace:BUG-1670 | ai:claude
    let summaries = backend.list_summaries_strict(&aida_core::ListFilter {
        archive: aida_core::ArchiveFilter::Both,
        defer: aida_core::DeferFilter::Both,
        ..Default::default()
    })?;
    // Dead = the spec still exists AND is archived or terminal
    // (Completed / Rejected) — same predicate as the queue's GC. A
    // directive targeting an unknown spec is LEFT alone (fail-safe:
    // no store row means no evidence the work shipped).
    let mut dead_ids = std::collections::HashSet::new();
    for s in &summaries {
        if s.archived || is_terminal_status_str(&s.status) {
            for id in [s.spec_id.as_deref(), s.agreed_id.as_deref()]
                .into_iter()
                .flatten()
            {
                dead_ids.insert(id.to_ascii_uppercase());
            }
        }
    }
    // Every prune candidate is re-read from its stored object before its
    // directive is dropped; a read error or a missing object keeps it.
    // trace:BUG-1670 | ai:claude
    let is_dead = |spec: &str| {
        dead_ids.contains(&spec.to_ascii_uppercase()) && worker_gc_target_still_dead(&backend, spec)
    };
    let outcome = worker::gc_directives_body(&body, &is_dead);
    if outcome.pruned.is_empty() {
        println!("No stale directives to prune.");
        return Ok(());
    }
    println!(
        "{} stale directive{} (target spec archived / Completed / Rejected):",
        outcome.pruned.len(),
        if outcome.pruned.len() == 1 { "" } else { "s" }
    );
    for d in &outcome.pruned {
        println!("  - {}", d.raw);
    }
    if dry_run {
        println!("Dry run — file unchanged.");
        return Ok(());
    }
    std::fs::write(&path, &outcome.kept_body)?;
    let remaining = worker::parse_directives_from_str(&outcome.kept_body).len();
    println!(
        "Pruned {} directive{}; {} remain{}.",
        outcome.pruned.len(),
        if outcome.pruned.len() == 1 { "" } else { "s" },
        remaining,
        if remaining == 1 { "s" } else { "" }
    );
    Ok(())
}

/// Whether the stored object for a worker directive's target is still
/// archived or terminal. `get_requirement_by_spec_id` reads the spec's YAML,
/// not the cache row, so a spec reopened since any cache snapshot reads as
/// live. An unreadable or missing object is not proof the work shipped.
// trace:BUG-1670 | ai:claude
pub(crate) fn worker_gc_target_still_dead(
    backend: &aida_core::CachedGitBackend,
    spec: &str,
) -> bool {
    use aida_core::db::DatabaseBackend;
    match backend.get_requirement_by_spec_id(spec) {
        Ok(Some(req)) => req.archived || is_terminal_status_str(&format!("{:?}", req.status)),
        _ => false,
    }
}
