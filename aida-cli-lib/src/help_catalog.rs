// `aida help commands` — the flat, comprehensive catalog of every command
// and subcommand, one line each. The rows are derived live from the clap
// definition (a recursive walk over `Cli::command()`), NOT hand-maintained,
// so the catalog can never drift from the real surface: a new subcommand
// appears here the moment it exists, a removed one disappears.
// trace:TASK-1098 | ai:claude

use clap::{Command, CommandFactory};
use colored::Colorize;
use rusqlite::{params, Connection, OptionalExtension};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

/// One catalog row: the full runnable invocation (`aida queue work`), the
/// first line of its clap `about`, and whether it (or an ancestor) is hidden
/// from the default `--help`.
// trace:TASK-1098 | ai:claude
pub(crate) struct CatalogRow {
    pub path: String,
    pub about: String,
    pub hidden: bool,
}

/// Collect every command and subcommand from the live clap definition,
/// depth-first, as full runnable paths, sorted so each command family's
/// subcommands read directly under it. Clap's auto-generated `help`
/// subcommands are skipped — they are navigation, not surface.
// trace:TASK-1098 | ai:claude
pub(crate) fn catalog_rows() -> Vec<CatalogRow> {
    let mut cmd = crate::cli::Cli::command();
    cmd.build();
    let mut rows = Vec::new();
    collect(&cmd, "aida", false, &mut rows);
    rows.sort_by(|a, b| a.path.cmp(&b.path));
    rows
}

// trace:TASK-1098 | ai:claude
fn collect(cmd: &clap::Command, path: &str, parent_hidden: bool, rows: &mut Vec<CatalogRow>) {
    for sub in cmd.get_subcommands() {
        // Skip clap's auto-generated `help` navigation subcommand at every
        // level; every other name is real surface.
        if sub.get_name() == "help" {
            continue;
        }
        let full = format!("{path} {}", sub.get_name());
        let about_full = sub.get_about().map(|s| s.to_string()).unwrap_or_default();
        let about = short_desc(&about_full);
        let hidden = parent_hidden || sub.is_hide_set();
        rows.push(CatalogRow {
            path: full.clone(),
            about,
            hidden,
        });
        collect(sub, &full, hidden, rows);
    }
}

/// Reduce a clap `about` paragraph to one short scannable line: first line,
/// then first sentence, then a hard cap with an ellipsis. Keeps every row of
/// the catalog one terminal line-ish so 480+ rows stay skimmable.
// trace:TASK-1098 | ai:claude
fn short_desc(about: &str) -> String {
    const MAX: usize = 88;
    let line = about.lines().next().unwrap_or("").trim();
    // Prefer the first sentence when the paragraph runs long.
    let sentence = match line.find(". ") {
        Some(idx) if idx + 1 < MAX => &line[..idx + 1],
        _ => line,
    };
    if sentence.chars().count() <= MAX {
        return sentence.to_string();
    }
    let truncated: String = sentence.chars().take(MAX - 1).collect();
    format!("{}…", truncated.trim_end())
}

/// Print the full catalog. Hidden commands are included (they run fine and
/// show up in usage telemetry — the whole point is a large catalog for
/// finding a forgotten command) but rendered dimmed with a marker so the
/// two tiers stay distinguishable.
// trace:TASK-1098 | ai:claude
pub(crate) fn print_command_catalog() {
    let rows = catalog_rows();
    let total = rows.len();
    let hidden_count = rows.iter().filter(|r| r.hidden).count();

    println!("{}", "📖 AIDA — complete command catalog".bold());
    println!(
        "{}",
        "Every command and subcommand, one line each — derived live from the CLI itself.".dimmed()
    );
    println!();

    // Pad the plain path first, THEN colorize — ANSI escape bytes would
    // otherwise count toward the field width and break column alignment.
    let width = rows.iter().map(|r| r.path.len()).max().unwrap_or(0);
    for row in &rows {
        let padded = format!("{:<width$}", row.path);
        if row.hidden {
            println!(
                "  {}  {} {}",
                padded.dimmed(),
                row.about.dimmed(),
                "· hidden".dimmed()
            );
        } else {
            println!("  {}  {}", padded.green(), row.about);
        }
    }

    println!();
    println!(
        "{} commands ({} hidden — still runnable, just kept out of `--help`).",
        total.to_string().bold(),
        hidden_count
    );
    println!(
        "Run {} for one command's options, or {} for the grouped view.",
        "`aida <command> --help`".bold(),
        "`aida help --all`".bold()
    );
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ConceptSurface {
    pub concept: &'static str,
    pub surface: &'static str,
    pub why: &'static str,
}

const CONCEPT_INDEX: &[ConceptSurface] = &[
    ConceptSurface {
        concept: "inbox",
        surface: "aida awaiting",
        why: "shows specs awaiting your attention",
    },
    ConceptSurface {
        concept: "inbox",
        surface: "aida mailbox inbox",
        why: "reads peer-to-peer messages routed to you",
    },
    ConceptSurface {
        concept: "inbox",
        surface: "aida brief list",
        why: "lists pickup briefs routed to this agent",
    },
    ConceptSurface {
        concept: "mail",
        surface: "aida awaiting",
        why: "shows specs awaiting your attention",
    },
    ConceptSurface {
        concept: "mail",
        surface: "aida mailbox inbox",
        why: "reads peer-to-peer messages routed to you",
    },
    ConceptSurface {
        concept: "mail",
        surface: "aida brief list",
        why: "lists pickup briefs routed to this agent",
    },
    ConceptSurface {
        concept: "busy",
        surface: "aida drain status",
        why: "shows whether an autonomous drain is active",
    },
    ConceptSurface {
        concept: "busy",
        surface: "aida ps",
        why: "lists live AIDA processes and sessions",
    },
    ConceptSurface {
        concept: "busy",
        surface: "aida tail",
        why: "follows active headless session logs",
    },
    ConceptSurface {
        concept: "busy",
        surface: "aida statusline",
        why: "prints compact project, role, queue, and cache state",
    },
    ConceptSurface {
        concept: "running",
        surface: "aida drain status",
        why: "shows whether an autonomous drain is active",
    },
    ConceptSurface {
        concept: "running",
        surface: "aida ps",
        why: "lists live AIDA processes and sessions",
    },
    ConceptSurface {
        concept: "running",
        surface: "aida tail",
        why: "follows active headless session logs",
    },
    ConceptSurface {
        concept: "running",
        surface: "aida statusline",
        why: "prints compact project, role, queue, and cache state",
    },
    ConceptSurface {
        concept: "progress",
        surface: "aida drain status",
        why: "shows whether an autonomous drain is active",
    },
    ConceptSurface {
        concept: "progress",
        surface: "aida ps",
        why: "lists live AIDA processes and sessions",
    },
    ConceptSurface {
        concept: "progress",
        surface: "aida tail",
        why: "follows active headless session logs",
    },
    ConceptSurface {
        concept: "progress",
        surface: "aida statusline",
        why: "prints compact project, role, queue, and cache state",
    },
    ConceptSurface {
        concept: "stuck",
        surface: "aida why",
        why: "explains why a spec is in its current state",
    },
    ConceptSurface {
        concept: "stuck",
        surface: "aida findings list",
        why: "lists captured risks and follow-up findings",
    },
    ConceptSurface {
        concept: "stuck",
        surface: "aida questions",
        why: "shows async decisions waiting on a person",
    },
    ConceptSurface {
        concept: "blocked",
        surface: "aida why",
        why: "explains why a spec is in its current state",
    },
    ConceptSurface {
        concept: "blocked",
        surface: "aida findings list",
        why: "lists captured risks and follow-up findings",
    },
    ConceptSurface {
        concept: "blocked",
        surface: "aida questions",
        why: "shows async decisions waiting on a person",
    },
    ConceptSurface {
        concept: "undo",
        surface: "aida unarchive",
        why: "restores an archived requirement to default views",
    },
    ConceptSurface {
        concept: "undo",
        surface: "aida undefer",
        why: "restores deferred work to the active view",
    },
    ConceptSurface {
        concept: "undo",
        surface: "aida cache rebuild",
        why: "rebuilds the disposable read cache from git",
    },
    ConceptSurface {
        concept: "restore",
        surface: "aida unarchive",
        why: "restores an archived requirement to default views",
    },
    ConceptSurface {
        concept: "restore",
        surface: "aida undefer",
        why: "restores deferred work to the active view",
    },
    ConceptSurface {
        concept: "restore",
        surface: "aida cache rebuild",
        why: "rebuilds the disposable read cache from git",
    },
    ConceptSurface {
        concept: "sync",
        surface: "aida pull",
        why: "pulls code and store updates",
    },
    ConceptSurface {
        concept: "sync",
        surface: "aida push",
        why: "pushes code and store updates",
    },
    ConceptSurface {
        concept: "sync",
        surface: "aida fetch",
        why: "refreshes remote refs without changing worktrees",
    },
    ConceptSurface {
        concept: "sync",
        surface: "aida db sync",
        why: "synchronizes the git-canonical store projection",
    },
    ConceptSurface {
        concept: "sync",
        surface: "aida remote status",
        why: "checks configured remote project state",
    },
];

pub(crate) fn concept_matches(term: &str) -> Vec<ConceptSurface> {
    let needle = normalize_help_term(term);
    CONCEPT_INDEX
        .iter()
        .copied()
        .filter(|row| row.concept == needle)
        .collect()
}

/// Validate the semantic concept table against the live clap tree and render
/// local help-search fallbacks. Exact command paths still use clap's own help
/// before the concept/FTS layers run.
// trace:STORY-837 | ai:codex
pub(crate) fn print_semantic_help_topic(term: &str, project_root: Option<&Path>) -> bool {
    let concept_rows = concept_matches(term);
    if !concept_rows.is_empty() {
        println!("{}", format!("AIDA help: {term}").bold());
        println!();
        for row in &concept_rows {
            println!("  {:<22} {}", row.surface.green(), row.why);
        }
        record_help_query(term, "concept", concept_rows.len(), project_root);
        return true;
    }

    let hits = search_help_corpus(term, project_root, 5);
    if !hits.is_empty() {
        println!("{}", format!("AIDA help: {term}").bold());
        println!();
        println!("{}", "Closest command help matches".cyan().bold());
        for hit in &hits {
            println!("  {:<24} {}", hit.path.green(), hit.snippet);
        }
        record_help_query(term, "fts", hits.len(), project_root);
        return true;
    }

    let misses = nearest_help_misses(term, 5);
    eprintln!("No help matches for {}", term.bold());
    if !misses.is_empty() {
        eprintln!();
        eprintln!("{}", "Nearest local matches:".cyan().bold());
        for hit in &misses {
            eprintln!("  {:<24} {}", hit.path.green(), hit.snippet);
        }
    }
    if std::io::stdout().is_terminal()
        && std::env::var("AIDA_HEADLESS").ok().as_deref() != Some("1")
    {
        eprintln!();
        eprintln!(
            "{}",
            "AI help escalation is available only after the ask-AI workflow is installed; no agent was started.".dimmed()
        );
    }
    record_help_query(term, "none", 0, project_root);
    true
}

pub(crate) fn print_exact_command_help(path: &str) -> bool {
    let mut cmd = crate::cli::Cli::command();
    cmd.build();
    let mut current = &mut cmd;
    for part in path.split_whitespace() {
        let Some(next) = current.find_subcommand_mut(part) else {
            return false;
        };
        current = next;
    }
    let _ = current.print_help();
    println!();
    true
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HelpHit {
    pub path: String,
    pub snippet: String,
}

fn search_help_corpus(term: &str, project_root: Option<&Path>, limit: usize) -> Vec<HelpHit> {
    if let Some(root) = project_root {
        if let Ok(hits) = search_cached_help_corpus(term, root, limit) {
            return hits;
        }
    }
    search_help_corpus_in_memory(term, limit)
}

fn search_cached_help_corpus(
    term: &str,
    project_root: &Path,
    limit: usize,
) -> anyhow::Result<Vec<HelpHit>> {
    let db_path = project_root.join(".aida").join("help-cache.db");
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(&db_path)?;
    ensure_help_cache(&conn)?;
    query_help_cache(&conn, term, limit)
}

fn ensure_help_cache(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS help_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE VIRTUAL TABLE IF NOT EXISTS help_fts USING fts5(path, about, body);",
    )?;
    let current_sha = crate::build_sha_short().unwrap_or_else(|| "unknown".to_string());
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM help_meta WHERE key = 'binary_sha'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if stored.as_deref() == Some(current_sha.as_str()) {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM help_fts", [])?;
    for doc in help_documents() {
        tx.execute(
            "INSERT INTO help_fts(path, about, body) VALUES (?1, ?2, ?3)",
            params![doc.path, doc.about, doc.body],
        )?;
    }
    tx.execute(
        "INSERT INTO help_meta(key, value) VALUES ('binary_sha', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![current_sha],
    )?;
    tx.commit()?;
    Ok(())
}

fn query_help_cache(conn: &Connection, term: &str, limit: usize) -> anyhow::Result<Vec<HelpHit>> {
    let query = escape_fts5_query(term);
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "SELECT path, snippet(help_fts, 2, '', '', '...', 12) AS snip
         FROM help_fts
         WHERE help_fts MATCH ?1
         ORDER BY bm25(help_fts)
         LIMIT ?2",
    )?;
    let hits = stmt
        .query_map(params![query, limit as i64], |row| {
            Ok(HelpHit {
                path: row.get(0)?,
                snippet: clean_snippet(row.get::<_, String>(1)?),
            })
        })?
        .filter_map(Result::ok)
        .collect();
    Ok(hits)
}

fn search_help_corpus_in_memory(term: &str, limit: usize) -> Vec<HelpHit> {
    let tokens = help_tokens(term);
    if tokens.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(usize, HelpHit)> = help_documents()
        .into_iter()
        .filter_map(|doc| {
            let haystack = format!("{} {} {}", doc.path, doc.about, doc.body).to_lowercase();
            let score = tokens.iter().filter(|tok| haystack.contains(*tok)).count();
            (score > 0).then(|| {
                (
                    score,
                    HelpHit {
                        path: doc.path,
                        snippet: short_desc(&doc.about),
                    },
                )
            })
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.path.cmp(&b.1.path)));
    scored.into_iter().take(limit).map(|(_, hit)| hit).collect()
}

fn nearest_help_misses(term: &str, limit: usize) -> Vec<HelpHit> {
    let needle = normalize_help_term(term);
    if needle.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(usize, HelpHit)> = help_documents()
        .into_iter()
        .map(|doc| {
            let distance = levenshtein(&needle, &normalize_help_term(&doc.path));
            (
                distance,
                HelpHit {
                    path: doc.path,
                    snippet: short_desc(&doc.about),
                },
            )
        })
        .collect();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.path.cmp(&b.1.path)));
    scored.into_iter().take(limit).map(|(_, hit)| hit).collect()
}

#[derive(Debug, Clone)]
struct HelpDocument {
    path: String,
    about: String,
    body: String,
}

fn help_documents() -> Vec<HelpDocument> {
    let mut cmd = crate::cli::Cli::command();
    cmd.build();
    let mut docs = Vec::new();
    collect_help_documents(&cmd, "aida", false, &mut docs);
    docs.extend(discipline_heading_documents());
    docs.sort_by(|a, b| a.path.cmp(&b.path));
    docs
}

fn collect_help_documents(
    cmd: &Command,
    path: &str,
    parent_hidden: bool,
    docs: &mut Vec<HelpDocument>,
) {
    for sub in cmd.get_subcommands() {
        if sub.get_name() == "help" {
            continue;
        }
        let full = format!("{path} {}", sub.get_name());
        let hidden = parent_hidden || sub.is_hide_set();
        let about_full = sub.get_about().map(|s| s.to_string()).unwrap_or_default();
        let mut cloned = sub.clone();
        let body = cloned.render_long_help().to_string();
        docs.push(HelpDocument {
            path: full.clone(),
            about: short_desc(&about_full),
            body: if hidden {
                format!("{body}\nhidden")
            } else {
                body
            },
        });
        collect_help_documents(sub, &full, hidden, docs);
    }
}

fn discipline_heading_documents() -> Vec<HelpDocument> {
    let Some(root) = crate::find_project_root().ok() else {
        return Vec::new();
    };
    let dir = root.join("docs").join("aida").join("discipline");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
        .flat_map(discipline_heading_docs_from_file)
        .collect()
}

fn discipline_heading_docs_from_file(path: PathBuf) -> Vec<HelpDocument> {
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let file = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("discipline")
        .to_string();
    body.lines()
        .filter_map(|line| {
            let heading = line
                .trim_start()
                .strip_prefix('#')?
                .trim_start_matches('#')
                .trim();
            (!heading.is_empty()).then(|| HelpDocument {
                path: format!("docs/aida/discipline/{file}"),
                about: heading.to_string(),
                body: body.clone(),
            })
        })
        .collect()
}

fn record_help_query(term: &str, layer: &str, hit_count: usize, project_root: Option<&Path>) {
    if !crate::usage::is_enabled(project_root) {
        return;
    }
    let safe_term = if is_safe_help_query_term(term) {
        term
    } else {
        "other"
    };
    let value = serde_json::json!({
        "event": "help_query",
        "ts": chrono::Utc::now().to_rfc3339(),
        "term": safe_term,
        "matched_layer": layer,
        "hit_count": hit_count,
        "binary_sha": crate::build_sha_short(),
    });
    crate::usage::append_value(&value);
}

fn is_safe_help_query_term(term: &str) -> bool {
    let len = term.len();
    (1..=24).contains(&len) && term.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
}

fn normalize_help_term(term: &str) -> String {
    term.trim().to_ascii_lowercase().replace('_', "-")
}

fn help_tokens(term: &str) -> Vec<String> {
    normalize_help_term(term)
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .filter(|tok| !tok.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn escape_fts5_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|tok| format!("\"{}\"", tok.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn clean_snippet(snippet: String) -> String {
    let compact = snippet.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        "(match in command help)".to_string()
    } else {
        compact
    }
}

fn levenshtein(a: &str, b: &str) -> usize {
    let mut costs: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.bytes().enumerate() {
        let mut last = i;
        costs[0] = i + 1;
        for (j, cb) in b.bytes().enumerate() {
            let old = costs[j + 1];
            costs[j + 1] = if ca == cb {
                last
            } else {
                1 + last.min(costs[j]).min(old)
            };
            last = old;
        }
    }
    costs[b.len()]
}

#[cfg(test)]
#[path = "tests/help_catalog_tests.rs"]
mod help_catalog_tests;
