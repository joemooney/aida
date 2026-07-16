//! `aida docs` / `aida doc` command handlers (EPIC-24 living-docs).
//! Extracted verbatim from `main.rs` (SPIKE-78 pure-movement refactor).

use crate::cli::{DocCommand, DocsCommand};
use crate::*;
use aida_core::RequirementsStore;
use anyhow::Result;
use colored::Colorize;

/// Handle `aida node` subcommands. Operates on the orphan-store worktree
/// at `store_path` (typically `.aida-store/`).
/// Render the docs tree, called from the legacy (Storage facade) dispatch.
// trace:EPIC-1-052 | ai:claude
// trace:FR-1-077 | ai:claude
pub(crate) fn handle_docs_command(cmd: &DocsCommand, storage: &Storage) -> Result<()> {
    let store = storage.load()?;
    handle_docs_with_store(cmd, &store)
}

/// Dispatch `aida doc {add,list,show}`. Always operates against the
/// distributed git-canonical backend — Doc entries are just requirements
/// with `req_type == Doc` and `--about` modeled as `References`
/// relationships.
// trace:STORY-104 | ai:claude
pub(crate) fn handle_doc_command(
    cmd: &DocCommand,
    store_path: &std::path::Path,
    backend: &aida_core::CachedGitBackend,
) -> Result<()> {
    use aida_core::models::{Relationship, RelationshipType, RequirementStatus, RequirementType};
    use aida_core::DatabaseBackend;

    match cmd {
        DocCommand::Add {
            title,
            about,
            scenario,
            audience,
            description,
            description_from_file,
            description_stdin,
            tags,
        } => {
            let resolved_description =
                resolve_description(description, description_from_file, *description_stdin)?
                    .unwrap_or_default();

            // Resolve every --about id up front. Bail before writing anything
            // if any reference is missing, so we never produce an entry
            // pointing at a phantom spec.
            let mut about_targets: Vec<aida_core::models::Requirement> =
                Vec::with_capacity(about.len());
            for raw in about {
                let id = raw.trim();
                if id.is_empty() {
                    continue;
                }
                let found = backend.get_requirement_by_spec_id(id)?.ok_or_else(|| {
                    anyhow::anyhow!(
                        "--about target `{}` not found in the store. Refusing to \
                         create a doc entry that references a phantom spec.",
                        id
                    )
                })?;
                about_targets.push(found);
            }

            let mut doc = aida_core::models::Requirement::new(title.clone(), resolved_description);
            doc.req_type = RequirementType::Doc;
            // Doc entries aren't workflow items — default to Approved so they
            // show up in default `aida doc list` without needing an explicit
            // status flip. Users can still `aida edit DOC-N --status draft`
            // if they want a review gate.
            doc.status = RequirementStatus::Approved;
            if let Some(s) = scenario {
                if !s.trim().is_empty() {
                    doc.custom_fields
                        .insert("scenario".to_string(), s.trim().to_string());
                }
            }
            if !audience.is_empty() {
                let cleaned: Vec<String> = audience
                    .iter()
                    .map(|a| a.trim().to_lowercase())
                    .filter(|a| !a.is_empty())
                    .collect();
                if !cleaned.is_empty() {
                    doc.custom_fields
                        .insert("audience".to_string(), cleaned.join(","));
                }
            }
            if let Some(t) = tags {
                for tag in t.split(',') {
                    let tag = tag.trim();
                    if !tag.is_empty() {
                        doc.tags.insert(tag.to_string());
                    }
                }
            }

            // Allocate spec_id via the same path `aida add` uses — keeps
            // sharding, dispenser, and id-format policy consistent.
            let store = backend.update_atomically(|store| {
                let type_prefix = store.get_type_prefix(&doc.req_type);
                store.add_requirement_with_id(doc.clone(), None, type_prefix.as_deref());
            })?;

            let written = store.requirements.last().cloned().ok_or_else(|| {
                anyhow::anyhow!("add_requirement_with_id produced no requirement")
            })?;
            aida_core::object_store::write_object(&store_path.join("objects"), &written)?;

            // Append References edges to each --about target. Done after the
            // initial save so the doc has its uuid + spec_id.
            if !about_targets.is_empty() {
                let now = chrono::Utc::now();
                let mut doc_with_rels = written.clone();
                for target in &about_targets {
                    doc_with_rels.relationships.push(Relationship {
                        target_id: target.id,
                        rel_type: RelationshipType::References,
                        created_at: Some(now),
                        created_by: None,
                    });
                }
                backend.update_requirement(&doc_with_rels)?;
                aida_core::object_store::write_object(&store_path.join("objects"), &doc_with_rels)?;
            }

            println!(
                "Added: {} - {}",
                written.spec_id.as_deref().unwrap_or("?"),
                written.title
            );
            if !about_targets.is_empty() {
                let refs = about_targets
                    .iter()
                    .map(|r| r.spec_id.as_deref().unwrap_or("?").to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("  About: {}", refs);
            }
            if let Some(sid) = written.spec_id.as_deref() {
                record_role_activity(sid, "add");
            }
        }

        DocCommand::List {
            about,
            scenario,
            audience,
        } => {
            let store = backend.load()?;

            let about_uuid: Option<uuid::Uuid> = if let Some(id) = about {
                let target = backend.get_requirement_by_spec_id(id)?.ok_or_else(|| {
                    anyhow::anyhow!("--about target `{}` not found in the store", id)
                })?;
                Some(target.id)
            } else {
                None
            };

            let audience_filter = audience.as_deref().map(|s| s.trim().to_lowercase());

            let mut docs: Vec<&aida_core::models::Requirement> = store
                .requirements
                .iter()
                .filter(|r| r.req_type == RequirementType::Doc && !r.archived)
                .filter(|r| match &about_uuid {
                    Some(uuid) => r.relationships.iter().any(|rel| {
                        rel.rel_type == RelationshipType::References && rel.target_id == *uuid
                    }),
                    None => true,
                })
                .filter(|r| match scenario {
                    Some(s) => {
                        r.custom_fields.get("scenario").map(|x| x.as_str()) == Some(s.as_str())
                    }
                    None => true,
                })
                .filter(|r| match &audience_filter {
                    Some(needle) => r
                        .custom_fields
                        .get("audience")
                        .map(|csv| {
                            csv.split(',')
                                .any(|a| a.trim().eq_ignore_ascii_case(needle))
                        })
                        .unwrap_or(false),
                    None => true,
                })
                .collect();
            docs.sort_by_key(|a| a.display_id());

            if docs.is_empty() {
                println!("(no doc entries found)");
                return Ok(());
            }

            for d in docs {
                let scenario_str = d
                    .custom_fields
                    .get("scenario")
                    .map(|s| format!(" · {}", s))
                    .unwrap_or_default();
                let about_ids: Vec<String> = d
                    .relationships
                    .iter()
                    .filter(|rel| rel.rel_type == RelationshipType::References)
                    .filter_map(|rel| {
                        store
                            .requirements
                            .iter()
                            .find(|r| r.id == rel.target_id)
                            .and_then(|r| r.spec_id.clone())
                    })
                    .collect();
                let about_str = if about_ids.is_empty() {
                    String::new()
                } else {
                    format!(" · about: {}", about_ids.join(", "))
                };
                println!(
                    "{}{} · {}{}",
                    d.display_id().cyan(),
                    scenario_str,
                    d.title,
                    about_str
                );
            }
        }

        DocCommand::Show { id } => {
            let store = backend.load()?;

            // First try treating <id> as the doc itself.
            let direct = backend.get_requirement_by_spec_id(id)?;
            if let Some(req) = direct.as_ref() {
                if req.req_type == RequirementType::Doc {
                    print_doc_detail(req, &store);
                    return Ok(());
                }
            }

            // Otherwise treat <id> as a referenced spec and find Docs about it.
            let target = direct
                .or(backend.get_requirement_by_spec_id(id)?)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "`{}` not found — pass a Doc id (e.g., DOC-3) to show a \
                         single entry, or any other spec id to list docs about it.",
                        id
                    )
                })?;
            let target_uuid = target.id;
            let mut about_docs: Vec<&aida_core::models::Requirement> = store
                .requirements
                .iter()
                .filter(|r| r.req_type == RequirementType::Doc && !r.archived)
                .filter(|r| {
                    r.relationships.iter().any(|rel| {
                        rel.rel_type == RelationshipType::References && rel.target_id == target_uuid
                    })
                })
                .collect();
            about_docs.sort_by_key(|a| a.display_id());

            println!(
                "Docs about {} ({}):",
                target.display_id().cyan(),
                target.title
            );
            if about_docs.is_empty() {
                println!("  (none — capture one with `aida doc add --about {}`)", id);
            } else {
                for d in about_docs {
                    let scenario_str = d
                        .custom_fields
                        .get("scenario")
                        .map(|s| format!(" · {}", s))
                        .unwrap_or_default();
                    println!("  {}{} · {}", d.display_id().cyan(), scenario_str, d.title);
                }
            }
        }

        // Release-time doc-coverage gate. Warn-only.
        // trace:TASK-680 | ai:claude
        DocCommand::Coverage { since, json } => {
            let store = backend.load()?;
            let project_root =
                find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());

            // Resolve the release boundary. An explicit `--since` wins; else the
            // most recent `v*` tag; else `None` (scan all of history).
            let boundary_ref: Option<String> = match since {
                Some(s) => Some(s.clone()),
                None => git_describe_latest_tag(&project_root),
            };
            let cutoff: Option<chrono::DateTime<chrono::Utc>> = boundary_ref
                .as_deref()
                .and_then(|r| git_ref_commit_time(&project_root, r));

            let gaps = find_uncovered_completed_specs(&store.requirements, cutoff);

            if *json {
                let rows: Vec<serde_json::Value> = gaps
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "id": r.display_id(),
                            "title": r.title,
                            "type": r.req_type.to_string(),
                        })
                    })
                    .collect();
                let payload = serde_json::json!({
                    "since": boundary_ref,
                    "uncovered_count": gaps.len(),
                    "uncovered": rows,
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
                return Ok(());
            }

            let window_label = match &boundary_ref {
                Some(r) => format!("since {}", r),
                None => "across all history".to_string(),
            };

            if gaps.is_empty() {
                println!(
                    "{} Doc coverage OK — every spec completed {} has a doc entry.",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    window_label
                );
                return Ok(());
            }

            println!(
                "{} {} spec(s) completed {} have no doc entry:",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                gaps.len(),
                window_label
            );
            for r in &gaps {
                println!("  {} · {}", r.display_id().cyan(), r.title);
            }
            println!();
            println!("Capture each with: aida doc add --title \"…\" --about <ID>");
            println!("(warn-only — this gate does not block the release)");
        }

        // Diff-driven doc nudge at PR-open. Warn-only. trace:TASK-939 | ai:claude
        DocCommand::Suggest { range, json } => {
            return handle_doc_suggest(range.as_deref(), *json);
        }
    }

    Ok(())
}

/// Resolve the commit time of a git ref/tag as a UTC timestamp. Best-effort —
/// `None` on any git failure or unparseable output.
// trace:TASK-680 | ai:claude
fn git_ref_commit_time(
    root: &std::path::Path,
    git_ref: &str,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["log", "-1", "--format=%cI", git_ref])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(&s)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

/// Print the full detail view for a single Doc entry.
// trace:STORY-104 | ai:claude
fn print_doc_detail(req: &aida_core::models::Requirement, store: &aida_core::RequirementsStore) {
    use aida_core::models::RelationshipType;

    println!("{}: {}", "ID".blue(), req.display_id());
    println!("{}: {}", "Title".blue(), req.title);
    if let Some(s) = req.custom_fields.get("scenario") {
        println!("{}: {}", "Scenario".blue(), s);
    }
    if let Some(a) = req.custom_fields.get("audience") {
        println!("{}: {}", "Audience".blue(), a);
    }
    let about_ids: Vec<String> = req
        .relationships
        .iter()
        .filter(|rel| rel.rel_type == RelationshipType::References)
        .filter_map(|rel| {
            store
                .requirements
                .iter()
                .find(|r| r.id == rel.target_id)
                .map(|r| {
                    let id = r.spec_id.as_deref().unwrap_or("?");
                    format!("{} ({})", id, r.title)
                })
        })
        .collect();
    if !about_ids.is_empty() {
        println!("{}: {}", "About".blue(), about_ids.join(", "));
    }
    if !req.tags.is_empty() {
        let mut t: Vec<&String> = req.tags.iter().collect();
        t.sort();
        let csv = t.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
        println!("{}: {}", "Tags".blue(), csv);
    }
    println!("{}: {}", "Status".blue(), req.status);
    println!("{}: {}", "Created".blue(), req.created_at);
    println!("{}: {}", "Modified".blue(), req.modified_at);
    if !req.description.is_empty() {
        println!();
        println!("{}", req.description);
    }
}

/// Shared implementation — both the legacy Storage path and the git-canonical
/// path call this with a loaded store.
// trace:FR-1-077 | ai:claude
pub(crate) fn handle_docs_with_store(cmd: &DocsCommand, store: &RequirementsStore) -> Result<()> {
    match cmd {
        DocsCommand::Build { output, dry_run } => {
            let out = output
                .clone()
                .unwrap_or_else(|| std::path::PathBuf::from("docs/aida"));
            let report = docs::build(store, &out, *dry_run)?;

            let total_planned =
                report.written.len() + report.unchanged.len() + report.drifted.len();
            println!(
                "{} {} layer file{} ({} written, {} updated, {} unchanged)",
                if *dry_run {
                    "→ dry-run:".cyan()
                } else {
                    crate::glyph(crate::glyphs::Glyph::Check).green()
                },
                total_planned,
                if total_planned == 1 { "" } else { "s" },
                report.written.len(),
                report.drifted.len(),
                report.unchanged.len(),
            );
            for p in &report.written {
                println!("    + {}", p.display().to_string().green());
            }
            for p in &report.drifted {
                println!(
                    "    {} {}",
                    crate::glyph(crate::glyphs::Glyph::FlowQueued),
                    p.display().to_string().yellow()
                );
            }
            for p in &report.deleted {
                println!(
                    "    {} stale (decision deleted from graph): {}",
                    "?".yellow(),
                    p.display()
                );
            }
            // BUG-41: signpost the README so the user has an entry point
            // into the projected layers without browsing the directory.
            // trace:BUG-41 | ai:claude
            if !*dry_run {
                let readme = out.join("README.md");
                if readme.exists() {
                    println!(
                        "\n  → Open {} to navigate the layers.",
                        readme.display().to_string().cyan()
                    );
                }
            }
        }
        DocsCommand::Check { output } => {
            let out = output
                .clone()
                .unwrap_or_else(|| std::path::PathBuf::from("docs/aida"));
            let report = docs::build(store, &out, /* dry_run */ true)?;
            if report.has_drift() || !report.written.is_empty() {
                eprintln!(
                    "{} docs tree differs from graph projection:",
                    "drift:".yellow().bold()
                );
                for p in &report.written {
                    eprintln!("    missing: {}", p.display());
                }
                for p in &report.drifted {
                    eprintln!("    drifted: {}", p.display());
                }
                eprintln!("\n    Run `aida docs build` to regenerate.");
                std::process::exit(1);
            }
            println!(
                "{} docs tree matches graph projection.",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
        }
        // TASK-589: surface the embedded discipline glossary from the CLI.
        // trace:TASK-589 | ai:claude
        DocsCommand::Glossary {
            machinery,
            lifecycle,
        } => {
            print!("{}", render_discipline_glossary(*machinery, *lifecycle)?);
        }
    }
    Ok(())
}
