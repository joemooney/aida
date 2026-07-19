//! `aida trace` command cluster — the code↔spec trace-comment surface.
//!
//! Extracted verbatim from `main.rs` (SPIKE-78, pure movement; no behavior
//! change). Covers `aida trace add / list / remove / scan / sweep`: filing,
//! listing, and removing typed trace links, plus the source-tree scanner
//! (`scan`) and the git-log sweeper (`sweep`) that bulk-import trace links.
//!
//! The `gate` / `coverage` / `check` handlers stay in `main.rs`: they are
//! interwoven with the shared STORY-469/STORY-498 commit-trailer guard and
//! coverage machinery (`read_commits_in_range`, `resolve_gate_range`,
//! `resolve_spec_in_store`, `walk_source_for_traces`, `read_diff_for_range`),
//! so this dispatcher reaches them via `crate::`.

use anyhow::{Context, Result};
use colored::Colorize;
use uuid::Uuid;

use aida_core::{ArtifactType, Storage, TraceLink};

use crate::cli::TraceCommand;
use crate::parse_requirement_id;

// trace:REQ-0243 | ai:claude:high
pub(crate) fn handle_trace_command(cmd: &TraceCommand, storage: &Storage) -> Result<()> {
    match cmd {
        TraceCommand::Add {
            req,
            file,
            symbol,
            line_start,
            line_end,
            r#type,
            notes,
            commit,
        } => {
            trace_add(
                storage,
                req,
                file,
                symbol.as_deref(),
                *line_start,
                *line_end,
                r#type,
                notes.as_deref(),
                commit.as_deref(),
            )?;
        }
        TraceCommand::List { req, file } => {
            trace_list(storage, req.as_deref(), file.as_deref())?;
        }
        TraceCommand::Remove { req, link_id } => {
            trace_remove(storage, req, link_id)?;
        }
        TraceCommand::Scan {
            path,
            extensions,
            update,
            verbose,
        } => {
            trace_scan(storage, path.as_deref(), extensions, *update, *verbose)?;
        }
        TraceCommand::Sweep {
            limit,
            branch,
            dry_run,
            verbose,
        } => {
            trace_sweep(storage, *limit, branch.as_deref(), *dry_run, *verbose)?;
        }
        TraceCommand::Gate { range, json } => {
            crate::handle_trace_gate(range.as_deref(), *json)?;
        }
        TraceCommand::Coverage { range, json, block } => {
            crate::handle_trace_coverage(range.as_deref(), *json, *block)?;
        }
        TraceCommand::Check { path, json, block } => {
            crate::handle_trace_check(path.as_deref(), *json, *block)?;
        }
    }
    Ok(())
}

// trace:REQ-0243 | ai:claude:high
// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
fn trace_add(
    storage: &Storage,
    req_id: &str,
    file_path: &str,
    symbol: Option<&str>,
    line_start: Option<u32>,
    line_end: Option<u32>,
    artifact_type: &str,
    notes: Option<&str>,
    commit: Option<&str>,
) -> Result<()> {
    let mut store = storage.load()?;
    let id = parse_requirement_id(req_id, &store)?;

    // Parse artifact type
    let art_type = match artifact_type.to_lowercase().as_str() {
        "source" | "sourcecode" | "src" => ArtifactType::SourceCode,
        "test" | "testcode" => ArtifactType::TestCode,
        "config" | "cfg" => ArtifactType::Config,
        "doc" | "documentation" => ArtifactType::Doc,
        _ => {
            anyhow::bail!(
                "Invalid artifact type '{}'. Use: source, test, config, doc",
                artifact_type
            )
        }
    };

    // Create trace link
    let mut trace = TraceLink::new(art_type, file_path.to_string());

    if let Some(sym) = symbol {
        trace.symbol = Some(sym.to_string());
    }

    trace.line_start = line_start;
    trace.line_end = line_end;

    if let Some(n) = notes {
        trace.notes = Some(n.to_string());
    }

    if let Some(c) = commit {
        trace.commit_hash = Some(c.to_string());
    }

    // Get current user
    trace.created_by = Some(
        std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "Unknown".to_string()),
    );

    // Find requirement and add trace link
    let req = store
        .requirements
        .iter_mut()
        .find(|r| r.id == id)
        .context("Requirement not found")?;

    let spec_id = req.spec_id.clone().unwrap_or_else(|| req.id.to_string());
    req.trace_links.push(trace.clone());

    storage.save(&store)?;

    println!(
        "{} Added trace link to {}",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        spec_id
    );
    println!("  File: {}", file_path.cyan());
    if let Some(sym) = symbol {
        println!("  Symbol: {}", sym.yellow());
    }
    if let Some(start) = line_start {
        if let Some(end) = line_end {
            println!("  Lines: {}-{}", start, end);
        } else {
            println!("  Line: {}", start);
        }
    }
    println!("  Type: {:?}", trace.artifact_type);

    Ok(())
}

// trace:REQ-0244 | ai:claude:high
fn trace_list(storage: &Storage, req_id: Option<&str>, file_path: Option<&str>) -> Result<()> {
    let store = storage.load()?;

    if req_id.is_none() && file_path.is_none() {
        // List all trace links across all requirements
        println!("{}", "All Trace Links".cyan().bold());
        println!("{}", "=".repeat(80));

        let mut total = 0;
        for req in &store.requirements {
            if !req.trace_links.is_empty() {
                let spec_id = req.spec_id.as_deref().unwrap_or("N/A");
                println!("\n{} - {}", spec_id.yellow(), req.title);

                for trace in &req.trace_links {
                    print_trace_link(trace, "  ");
                    total += 1;
                }
            }
        }

        if total == 0 {
            println!("{}", "No trace links found.".yellow());
        } else {
            println!("\n{} trace links total", total);
        }
        return Ok(());
    }

    if let Some(req_id_str) = req_id {
        // List trace links for a specific requirement
        let id = parse_requirement_id(req_id_str, &store)?;
        let req = store
            .get_requirement_by_id(&id)
            .context("Requirement not found")?;

        let spec_id = req.spec_id.as_deref().unwrap_or("N/A");
        println!("{}: {} - {}", "Requirement".blue(), spec_id, req.title);
        println!();

        if req.trace_links.is_empty() {
            println!("{}", "No trace links found.".yellow());
        } else {
            println!("{}:", "Trace Links".green());
            for trace in &req.trace_links {
                print_trace_link(trace, "  ");
            }
            println!("\n{} trace links total", req.trace_links.len());
        }
    }

    if let Some(file) = file_path {
        // List trace links for a specific file
        println!("{}: {}", "File".blue(), file);
        println!();

        let mut found = Vec::new();
        for req in &store.requirements {
            for trace in &req.trace_links {
                if trace.file_path.contains(file) {
                    found.push((req, trace));
                }
            }
        }

        if found.is_empty() {
            println!("{}", "No trace links found for this file.".yellow());
        } else {
            println!("{}:", "Trace Links".green());
            for (req, trace) in &found {
                let spec_id = req.spec_id.as_deref().unwrap_or("N/A");
                println!("  {} -> {}", spec_id.yellow(), trace.file_path.cyan());
                if let Some(sym) = &trace.symbol {
                    println!("    Symbol: {}", sym);
                }
                if let Some(start) = trace.line_start {
                    if let Some(end) = trace.line_end {
                        println!("    Lines: {}-{}", start, end);
                    } else {
                        println!("    Line: {}", start);
                    }
                }
            }
            println!("\n{} trace links total", found.len());
        }
    }

    Ok(())
}

fn print_trace_link(trace: &TraceLink, indent: &str) {
    println!("{}ID: {}", indent, trace.id.to_string().dimmed());
    println!("{}File: {}", indent, trace.file_path.cyan());
    if let Some(sym) = &trace.symbol {
        println!("{}Symbol: {}", indent, sym.yellow());
    }
    if let Some(start) = trace.line_start {
        if let Some(end) = trace.line_end {
            println!("{}Lines: {}-{}", indent, start, end);
        } else {
            println!("{}Line: {}", indent, start);
        }
    }
    println!("{}Type: {:?}", indent, trace.artifact_type);
    if let Some(notes) = &trace.notes {
        println!("{}Notes: {}", indent, notes);
    }
    if let Some(commit) = &trace.commit_hash {
        println!("{}Commit: {}", indent, commit);
    }
    if let Some(created_by) = &trace.created_by {
        println!(
            "{}Created: {} by {}",
            indent,
            trace
                .created_at
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M"),
            created_by
        );
    }
    println!();
}

fn trace_remove(storage: &Storage, req_id: &str, link_id: &str) -> Result<()> {
    let mut store = storage.load()?;
    let id = parse_requirement_id(req_id, &store)?;
    let link_uuid = Uuid::parse_str(link_id).context("Invalid trace link ID")?;

    let req = store
        .requirements
        .iter_mut()
        .find(|r| r.id == id)
        .context("Requirement not found")?;

    let initial_len = req.trace_links.len();
    req.trace_links.retain(|t| t.id != link_uuid);

    if req.trace_links.len() == initial_len {
        anyhow::bail!("Trace link not found");
    }

    storage.save(&store)?;
    println!(
        "{} Trace link removed",
        crate::glyph(crate::glyphs::Glyph::Check).green()
    );
    Ok(())
}

/// One scanned trace hit: `(req_id, file_path, line_content, line_num, tool,
/// confidence, title, impl_date, by_user)`.
type TraceScanHit = (
    String,
    String,
    String,
    u32,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

// trace:REQ-0245 | ai:claude:high
fn trace_scan(
    storage: &Storage,
    path: Option<&str>,
    extensions: &str,
    update: bool,
    verbose: bool,
) -> Result<()> {
    use std::fs;
    use std::io::BufRead;

    let scan_path = path.unwrap_or(".");
    let ext_list: Vec<&str> = extensions.split(',').map(|s| s.trim()).collect();

    println!(
        "Scanning {} for trace comments (extensions: {})...",
        scan_path.cyan(),
        extensions
    );

    // Regex pattern for trace comments (supports both old and new formats):
    // Old: // trace:REQ-ID | ai:tool:confidence
    // New: // trace:REQ-ID - Title | ai:tool:confidence | impl:date | by:user
    let trace_pattern = regex::Regex::new(
        r"//\s*trace:([A-Z]+-\d+)(?:\s*-\s*([^|]+))?(?:\s*\|\s*ai:(\w+):(\w+))?(?:\s*\|\s*impl:(\S+))?(?:\s*\|\s*by:(\S+))?"
    ).unwrap();

    // (req_id, file_path, line_content, line_num, tool, confidence, title, impl_date, by_user)
    let mut found_traces: Vec<TraceScanHit> = Vec::new();

    // Walk through files
    fn scan_dir(
        dir: &std::path::Path,
        ext_list: &[&str],
        pattern: &regex::Regex,
        found: &mut Vec<TraceScanHit>,
        verbose: bool,
    ) -> Result<()> {
        if dir.is_file() {
            if let Some(ext) = dir.extension() {
                if ext_list.contains(&ext.to_str().unwrap_or("")) {
                    scan_file(dir, pattern, found, verbose)?;
                }
            }
            return Ok(());
        }

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            // Skip hidden directories and common non-source directories
            if let Some(name) = path.file_name() {
                let name_str = name.to_string_lossy();
                if name_str.starts_with('.')
                    || name_str == "target"
                    || name_str == "node_modules"
                    || name_str == "vendor"
                {
                    continue;
                }
            }

            if path.is_dir() {
                scan_dir(&path, ext_list, pattern, found, verbose)?;
            } else if let Some(ext) = path.extension() {
                if ext_list.contains(&ext.to_str().unwrap_or("")) {
                    scan_file(&path, pattern, found, verbose)?;
                }
            }
        }
        Ok(())
    }

    fn scan_file(
        path: &std::path::Path,
        pattern: &regex::Regex,
        found: &mut Vec<TraceScanHit>,
        verbose: bool,
    ) -> Result<()> {
        let file = fs::File::open(path)?;
        let reader = std::io::BufReader::new(file);

        for (line_num, line) in reader.lines().enumerate() {
            let line = line?;
            if let Some(caps) = pattern.captures(&line) {
                let req_id = caps.get(1).map(|m| m.as_str().to_string()).unwrap();
                let title = caps.get(2).map(|m| m.as_str().trim().to_string());
                let tool = caps.get(3).map(|m| m.as_str().to_string());
                let confidence = caps.get(4).map(|m| m.as_str().to_string());
                let impl_date = caps.get(5).map(|m| m.as_str().to_string());
                let by_user = caps.get(6).map(|m| m.as_str().to_string());

                if verbose {
                    let title_str = title.as_deref().unwrap_or("");
                    println!(
                        "  Found: {}{} in {}:{}",
                        req_id.yellow(),
                        if title_str.is_empty() {
                            "".to_string()
                        } else {
                            format!(" - {}", title_str)
                        },
                        path.display(),
                        line_num + 1
                    );
                }

                found.push((
                    req_id,
                    path.to_string_lossy().to_string(),
                    line.clone(),
                    (line_num + 1) as u32,
                    tool,
                    confidence,
                    title,
                    impl_date,
                    by_user,
                ));
            }
        }
        Ok(())
    }

    scan_dir(
        std::path::Path::new(scan_path),
        &ext_list,
        &trace_pattern,
        &mut found_traces,
        verbose,
    )?;

    println!("\nFound {} trace comments:", found_traces.len());

    // Group by requirement
    let mut by_req: std::collections::HashMap<String, Vec<_>> = std::collections::HashMap::new();
    for trace in &found_traces {
        by_req
            .entry(trace.0.clone())
            .or_default()
            .push(trace.clone());
    }

    for (req_id, traces) in &by_req {
        println!("\n  {} ({} links):", req_id.yellow(), traces.len());
        for (_, file, _, line, tool, conf, title, impl_date, by_user) in traces {
            let title_info = title
                .as_ref()
                .map(|t| format!(" - {}", t))
                .unwrap_or_default();
            let ai_info = match (tool, conf) {
                (Some(t), Some(c)) => format!(" [ai:{t}:{c}]").dimmed().to_string(),
                _ => String::new(),
            };
            let impl_info = impl_date
                .as_ref()
                .map(|d| format!(" impl:{}", d))
                .unwrap_or_default();
            let by_info = by_user
                .as_ref()
                .map(|u| format!(" by:{}", u))
                .unwrap_or_default();
            println!(
                "    {}:{}{}{}{}{}",
                file.cyan(),
                line,
                title_info,
                ai_info,
                impl_info.dimmed(),
                by_info.dimmed()
            );
        }
    }

    if update {
        println!("\n{} Updating requirements database...", "→".blue());
        let mut store = storage.load()?;
        let mut added = 0;

        for (req_id, file_path, _, line_num, tool, confidence, _title, impl_date, by_user) in
            found_traces
        {
            // Find requirement by spec_id (case-insensitive — trace comments may be lowercase)
            if let Some(req) = store.requirements.iter_mut().find(|r| {
                r.spec_id
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(&req_id))
            }) {
                // Check if trace link already exists for this file and line
                let exists = req
                    .trace_links
                    .iter()
                    .any(|t| t.file_path == file_path && t.line_start == Some(line_num));

                if !exists {
                    let mut trace = TraceLink::new(ArtifactType::SourceCode, file_path.clone());
                    trace.line_start = Some(line_num);

                    // Use by_user from comment if present, otherwise default to "scan"
                    trace.created_by = by_user.or_else(|| Some("scan".to_string()));

                    // Build notes from available info
                    let mut notes_parts = Vec::new();
                    if let Some(t) = &tool {
                        let conf_str = confidence
                            .as_ref()
                            .map(|c| format!(":{}", c))
                            .unwrap_or_default();
                        notes_parts.push(format!("AI tool: {}{}", t, conf_str));
                    }
                    if let Some(date) = &impl_date {
                        notes_parts.push(format!("Implemented: {}", date));
                    }
                    if !notes_parts.is_empty() {
                        trace.notes = Some(notes_parts.join(", "));
                    }

                    req.trace_links.push(trace);
                    added += 1;
                }
            } else {
                println!(
                    "  {} Requirement {} not found in database",
                    "!".yellow(),
                    req_id
                );
            }
        }

        storage.save(&store)?;
        println!(
            "{} Added {} new trace links",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            added
        );
    }

    Ok(())
}

// trace:REQ-0258 | ai:claude:high
fn trace_sweep(
    storage: &Storage,
    limit: Option<u32>,
    branch: Option<&str>,
    dry_run: bool,
    verbose: bool,
) -> Result<()> {
    use std::process::Command;

    println!(
        "Sweeping git commits for requirement references{}...",
        if dry_run { " (dry run)" } else { "" }
    );

    // Get git log
    let mut cmd = Command::new("git");
    cmd.arg("log")
        .arg("--pretty=format:%H|%s|%b")
        .arg("--no-merges");

    if let Some(b) = branch {
        cmd.arg(b);
    }

    if let Some(n) = limit {
        cmd.arg(format!("-{}", n));
    }

    let output = cmd.output().context("Failed to run git log")?;

    if !output.status.success() {
        anyhow::bail!(
            "git log failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let log_output = String::from_utf8_lossy(&output.stdout);

    // Pattern to find requirement references in commit messages
    let req_pattern = regex::Regex::new(r"\b([A-Z]+-\d+)\b").unwrap();

    let mut found_refs: Vec<(String, String, String)> = Vec::new(); // (commit_hash, req_id, subject)

    for entry in log_output.split('\n') {
        if entry.is_empty() {
            continue;
        }

        let parts: Vec<&str> = entry.splitn(3, '|').collect();
        if parts.len() < 2 {
            continue;
        }

        let commit_hash = parts[0];
        let subject = parts[1];
        let body = parts.get(2).unwrap_or(&"");

        // Search for requirement IDs in subject and body
        let full_text = format!("{} {}", subject, body);
        for caps in req_pattern.captures_iter(&full_text) {
            let req_id = caps.get(1).map(|m| m.as_str().to_string()).unwrap();

            if verbose {
                println!(
                    "  Found: {} in commit {} - {}",
                    req_id.yellow(),
                    &commit_hash[..8].cyan(),
                    subject
                );
            }

            found_refs.push((commit_hash.to_string(), req_id, subject.to_string()));
        }
    }

    println!("\nFound {} requirement references:", found_refs.len());

    // Group by requirement
    let mut by_req: std::collections::HashMap<String, Vec<(String, String)>> =
        std::collections::HashMap::new();
    for (hash, req_id, subject) in &found_refs {
        by_req
            .entry(req_id.clone())
            .or_default()
            .push((hash.clone(), subject.clone()));
    }

    for (req_id, commits) in &by_req {
        println!("\n  {} ({} commits):", req_id.yellow(), commits.len());
        for (hash, subject) in commits {
            println!("    {} - {}", hash[..8].cyan(), subject);
        }
    }

    if !dry_run {
        println!("\n{} Updating requirements database...", "→".blue());
        let mut store = storage.load()?;
        let mut updated = 0;

        for (commit_hash, req_id, subject) in found_refs {
            // Find requirement by spec_id (case-insensitive — commit refs may be lowercase)
            if let Some(req) = store.requirements.iter_mut().find(|r| {
                r.spec_id
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(&req_id))
            }) {
                // Check if this commit is already linked
                let exists = req
                    .trace_links
                    .iter()
                    .any(|t| t.commit_hash.as_deref() == Some(&commit_hash));

                if !exists {
                    let mut trace = TraceLink::new(ArtifactType::SourceCode, "".to_string());
                    trace.commit_hash = Some(commit_hash.clone());
                    trace.notes = Some(format!("Git commit: {}", subject));
                    trace.created_by = Some("sweep".to_string());
                    req.trace_links.push(trace);
                    updated += 1;
                }
            }
        }

        storage.save(&store)?;
        println!(
            "{} Added {} new commit trace links",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            updated
        );
    }

    Ok(())
}
