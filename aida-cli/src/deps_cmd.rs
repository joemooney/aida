//! `aida deps` command cluster — read-only dependency inference (STORY-447).
//!
//! Extracted verbatim from `main.rs` (SPIKE-78, pure movement; no behavior
//! change). The ranking/sweep engine (`rank_dependencies`, `sweep_all`,
//! `SpecSignals`, `Suggestion`, `Confidence`) lives in
//! `aida_core::deps_sweep`; this module is the thin CLI presentation layer
//! that assembles the trace-graph signals and renders suggestions (human
//! table + `--json`). Shared helpers (`find_project_root`,
//! `load_store_for_lookup`, `scan_trace_graph`) stay in `main.rs`, reached
//! via `crate::`.

use crate::*;

pub(crate) fn handle_deps_command(cmd: &DepsCommand) -> Result<()> {
    match cmd {
        DepsCommand::Sweep { for_spec, json } => deps_sweep(for_spec.as_deref(), *json),
    }
}

/// `aida deps sweep [--for-spec <ID>] [--json]` — list likely dependencies
/// inferred read-only from the trace graph. For each spec, the candidate
/// dependencies are other specs that share trace-link files (≥2 shared → high,
/// 1 → medium) or, weaker, share a parent. It NEVER writes the graph — confirm
/// a real edge with `aida edit <id> --blocked-by <dep>`. The `--apply`
/// interactive write-back and the scheduled-advisor variant are deliberately
/// gated until suggestion quality is observed (operator decision, 2026-06-06).
// trace:STORY-447 | ai:claude
fn deps_sweep(for_spec: Option<&str>, json: bool) -> Result<()> {
    use aida_core::deps_sweep::{rank_dependencies, sweep_all, SpecSignals};
    use std::collections::BTreeSet;

    let project_root = find_project_root()?;
    let store = load_store_for_lookup(&project_root)
        .ok_or_else(|| anyhow::anyhow!("could not load the AIDA requirements store"))?;

    // Map every UUID to its display id so parents (stored as UUIDs) and the
    // candidate ids line up on a single id space.
    let mut display_by_uuid: std::collections::HashMap<uuid::Uuid, String> =
        std::collections::HashMap::new();
    for req in &store.requirements {
        display_by_uuid.insert(req.id, req.display_id());
    }

    // Code-scanned trace comments: spec_id/agreed_id → touched files. The
    // `wanted` set is every id-string a `// trace:` comment might legitimately
    // use, so the scan buckets onto the right spec.
    let mut wanted: HashSet<String> = HashSet::new();
    for req in &store.requirements {
        if let Some(s) = &req.spec_id {
            wanted.insert(s.clone());
        }
        if let Some(a) = &req.agreed_id {
            wanted.insert(a.clone());
        }
    }
    let scanned = scan_trace_graph(&project_root, &wanted);

    // Normalize a path so a stored `./aida-cli/src/main.rs` and a scanned
    // `aida-cli/src/main.rs` collapse to the same key.
    let norm = |p: &str| -> String { p.trim_start_matches("./").to_string() };

    // Build display_id → files (stored trace_links + code-scanned comments)
    // and display_id → parent display_ids for every spec.
    let mut files_by_spec: std::collections::HashMap<String, BTreeSet<String>> =
        std::collections::HashMap::new();
    let mut parents_by_spec: std::collections::HashMap<String, BTreeSet<String>> =
        std::collections::HashMap::new();

    for req in &store.requirements {
        let did = req.display_id();
        let files = files_by_spec.entry(did.clone()).or_default();
        for link in &req.trace_links {
            files.insert(norm(&link.file_path));
        }
        // Fold in code-scanned hits keyed by either id form.
        for key in [req.spec_id.as_deref(), req.agreed_id.as_deref()]
            .into_iter()
            .flatten()
        {
            if let Some(hits) = scanned.get(key) {
                for hit in hits {
                    files.insert(norm(&hit.file));
                }
            }
        }
        let parents = parents_by_spec.entry(did).or_default();
        for rel in &req.relationships {
            if rel.rel_type == aida_core::RelationshipType::Child {
                if let Some(pd) = display_by_uuid.get(&rel.target_id) {
                    parents.insert(pd.clone());
                }
            }
        }
    }

    // Drop non-discriminating files from the overlap signal: a file touched
    // by a large share of specs (e.g. `aida-cli/src/main.rs` here — every spec
    // touches it) carries no coupling signal and would flood every spec with
    // bogus "medium" hits. `plan helpers` learned the same lesson. A file is
    // "common" if more than COMMON_FILE_FRAC of all specs touch it (with a
    // small floor so a tiny store isn't over-pruned). trace:STORY-447
    {
        let total = files_by_spec.len().max(1);
        let mut file_counts: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for files in files_by_spec.values() {
            for f in files {
                *file_counts.entry(f.as_str()).or_default() += 1;
            }
        }
        const COMMON_FILE_FRAC: f64 = 0.20;
        const COMMON_FILE_FLOOR: usize = 8;
        let threshold = ((total as f64 * COMMON_FILE_FRAC).ceil() as usize).max(COMMON_FILE_FLOOR);
        let common: HashSet<String> = file_counts
            .iter()
            .filter(|(_, &c)| c > threshold)
            .map(|(f, _)| (*f).to_string())
            .collect();
        if !common.is_empty() {
            for files in files_by_spec.values_mut() {
                files.retain(|f| !common.contains(f));
            }
        }
    }

    let signals: Vec<SpecSignals> = store
        .requirements
        .iter()
        .map(|req| {
            let did = req.display_id();
            SpecSignals {
                display_id: did.clone(),
                files: files_by_spec.get(&did).cloned().unwrap_or_default(),
                parents: parents_by_spec.get(&did).cloned().unwrap_or_default(),
            }
        })
        .collect();

    // Resolve --for-spec to a display id (accepts SPEC-ID, agreed id, or UUID).
    let results: Vec<(String, Vec<aida_core::deps_sweep::Suggestion>)> = if let Some(arg) = for_spec
    {
        let target = if let Ok(uuid) = uuid::Uuid::parse_str(arg) {
            store.requirements.iter().find(|r| r.id == uuid)
        } else {
            store
                .requirements
                .iter()
                .find(|r| r.matches_id(arg))
                .or_else(|| store.get_requirement_by_spec_id(arg))
        }
        .ok_or_else(|| anyhow::anyhow!("requirement `{arg}` not found"))?;
        let did = target.display_id();
        let target_sig = signals
            .iter()
            .find(|s| s.display_id == did)
            .cloned()
            .unwrap_or(SpecSignals {
                display_id: did.clone(),
                files: BTreeSet::new(),
                parents: BTreeSet::new(),
            });
        vec![(did, rank_dependencies(&target_sig, &signals))]
    } else {
        sweep_all(&signals)
    };

    // Cap the weak (same-parent-only) tail per source so a spec under a huge
    // parent (dozens of sibling bugs) doesn't bury its real file-overlap hits.
    // File-overlap (high/medium) suggestions are never capped. trace:STORY-447
    const WEAK_PER_SOURCE_CAP: usize = 5;
    let mut results = results;
    for (_, suggestions) in results.iter_mut() {
        let mut weak_seen = 0usize;
        suggestions.retain(|s| {
            if s.confidence == aida_core::deps_sweep::Confidence::Weak {
                weak_seen += 1;
                weak_seen <= WEAK_PER_SOURCE_CAP
            } else {
                true
            }
        });
    }

    if json {
        let arr: Vec<serde_json::Value> = results
            .iter()
            .map(|(src, suggestions)| {
                serde_json::json!({
                    "spec": src,
                    "suggestions": suggestions.iter().map(|s| serde_json::json!({
                        "candidate": s.candidate,
                        "confidence": s.confidence.label(),
                        "shared_files": s.shared_files,
                        "same_parent": s.same_parent,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
        return Ok(());
    }

    // Human output.
    if results.is_empty() || results.iter().all(|(_, s)| s.is_empty()) {
        if for_spec.is_some() {
            println!("No likely dependencies inferred for the requested spec.");
        } else {
            println!(
                "No likely dependencies inferred — no two specs share trace-link files or a parent."
            );
        }
        return Ok(());
    }

    println!(
        "{}",
        "Likely dependencies (inferred read-only — confirm before relying on them)".bold()
    );
    println!(
        "  signal: shared trace-link files (high ≥2 / medium 1) · same parent (weak)\n  confirm with: aida edit <id> --blocked-by <dep>\n"
    );

    for (src, suggestions) in &results {
        if suggestions.is_empty() {
            continue;
        }
        println!("{src}");
        for s in suggestions {
            let conf = match s.confidence {
                aida_core::deps_sweep::Confidence::High => s.confidence.label().green().to_string(),
                aida_core::deps_sweep::Confidence::Medium => {
                    s.confidence.label().yellow().to_string()
                }
                aida_core::deps_sweep::Confidence::Weak => {
                    s.confidence.label().dimmed().to_string()
                }
            };
            let mut detail = String::new();
            if !s.shared_files.is_empty() {
                detail = format!(" — shares {}", s.shared_files.join(", "));
            } else if s.same_parent {
                detail = " — same parent".to_string();
            }
            if s.same_parent && !s.shared_files.is_empty() {
                detail.push_str(" (+ same parent)");
            }
            println!(
                "    Likely depends on: {} ({}){}",
                s.candidate, conf, detail
            );
        }
        println!();
    }

    Ok(())
}
