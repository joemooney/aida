//! `aida history` — project event timeline decoded from the orphan branch.
//!
//! Walks `git log` on the `aida-store` orphan branch via shell-out,
//! parses each commit's YAML diff with `serde_yaml::Value`, and emits one
//! logical Event per change. Output is reverse-chronological so the most
//! recent activity is at the top, matching `git log` defaults.
//!
//! trace:FR-1-037 | ai:claude

use anyhow::{Context, Result};
use colored::Colorize;
use serde::Serialize;
use serde_json::{json, Value as JsonValue};
use serde_yaml::Value;
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command as ProcessCommand;

/// Options threaded down from the CLI handler. Keeps the public function
/// signature stable when new filters get added.
#[derive(Debug, Clone)]
pub struct HistoryOpts {
    pub limit: usize,
    pub max_commits: usize,
    pub events_mode: bool,
    pub id_filter: Option<String>,
    pub type_filter: Option<String>,
    pub author_filter: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub status_changes_only: bool,
    /// TASK-507: `--shipped` — only Done→Completed status transitions (the
    /// "did my ship register?" view), vs `--all`'s recency-blind archive dump.
    /// Implies events mode. trace:TASK-507 | ai:claude
    pub shipped_only: bool,
    pub comments_only: bool,
    pub oneline: bool,
    /// Spec-IDs currently archived. The default `aida history` view hides
    /// rows whose spec_id is in this set. Empty when `--all` or `--archived`
    /// was passed (no hiding needed). trace:STORY-441 | ai:claude
    pub archived_specs: std::collections::HashSet<String>,
    /// When `Some`, only rows whose spec_id is in this set are shown.
    /// Used by `--archived` to narrow the view to the archive itself.
    /// trace:STORY-441 | ai:claude
    pub archived_only_specs: Option<std::collections::HashSet<String>>,
    /// Spec-IDs currently deferred (flag set OR `deferred:*`-tagged). The
    /// default `aida history` view hides rows whose spec_id is in this set.
    /// Empty when `--all` or `--deferred` was passed. trace:STORY-584 | ai:claude
    pub deferred_specs: std::collections::HashSet<String>,
    /// When `Some`, only rows whose spec_id is in this set are shown.
    /// Used by `--deferred` to narrow to the primed shelf. trace:STORY-584 | ai:claude
    pub deferred_only_specs: Option<std::collections::HashSet<String>>,
    /// STORY-737 (delight #4): hide stateless internal META rows (the 6
    /// AI-prompt templates `aida init` seeds) from the default activity view
    /// so a fresh project's one real spec isn't drowned — matching `aida list`
    /// and `aida status`, which already exclude them. The caller leaves this
    /// `false` when `--include-meta` or an explicit `--type meta` was passed.
    // trace:STORY-737 | ai:claude
    pub exclude_meta: bool,
}

#[derive(Debug, Clone)]
struct CommitMeta {
    sha: String,
    iso_timestamp: String,
    /// Author email from `git log %ae`. We prefer the YAML's
    /// `last_modified_by` field at event-rendering time, but the git
    /// author is the fallback when the YAML doesn't have one.
    git_author: String,
}

/// TASK-507: is this event the Done→Completed ship transition (merge-to-default
/// branch)? The `--shipped` view keeps only these. trace:TASK-507 | ai:claude
fn is_ship_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::StatusChange { from, to }
            if from.eq_ignore_ascii_case("Done") && to.eq_ignore_ascii_case("Completed")
    )
}

#[derive(Debug, Clone)]
enum EventKind {
    Added {
        title: String,
        req_type: String,
        priority: String,
    },
    Deleted {
        title: String,
    },
    StatusChange {
        from: String,
        to: String,
    },
    PriorityChange {
        from: String,
        to: String,
    },
    TitleChange {
        from: String,
        to: String,
    },
    DescriptionEdited,
    OwnerChange {
        from: String,
        to: String,
    },
    FeatureChange {
        from: String,
        to: String,
    },
    TypeChange {
        from: String,
        to: String,
    },
    TagsChange {
        added: Vec<String>,
        removed: Vec<String>,
    },
    CommentsAdded {
        count: usize,
        author: Option<String>,
    },
    RelationshipsChange {
        added: usize,
        removed: usize,
    },
}

#[derive(Debug, Clone)]
struct Event {
    sha: String,
    timestamp: String,
    /// Resolved author — YAML's `last_modified_by` if present, else git
    /// committer email's local-part.
    author: String,
    spec_id: String,
    req_type: String,
    kind: EventKind,
}

/// Structured event record for MCP and other programmatic consumers.
/// It is derived from the same orphan-branch decoder used by `aida history`
/// so CLI and MCP history views cannot drift.
/// trace:TASK-538 | ai:codex
#[derive(Debug, Clone, Serialize)]
pub struct HistoryEventRecord {
    pub sha: String,
    pub timestamp: String,
    pub author: String,
    pub spec_id: String,
    pub req_type: String,
    pub kind: String,
    pub summary: String,
    pub detail: JsonValue,
}

pub fn run(store_path: &Path, opts: &HistoryOpts) -> Result<()> {
    if !store_path.is_dir() {
        anyhow::bail!(
            "Not a git-canonical AIDA store: {}\n\
             aida history walks the orphan branch's git log; the legacy SQLite\n\
             backend has no per-edit history surface.",
            store_path.display()
        );
    }

    if !opts.events_mode {
        return run_digest(store_path, opts);
    }

    let (filtered, hidden_archived) = collect_filtered_events(store_path, opts)?;

    if filtered.is_empty() {
        eprintln!("{}", "(no events match the filter)".dimmed());
        if hidden_archived > 0 {
            eprintln!(
                "{}",
                format!(
                    "({hidden_archived} archived spec(s) hidden — pass --all to include archived events, or --archived for the archive only)"
                )
                .dimmed()
            );
        }
        return Ok(());
    }

    if opts.oneline {
        for e in &filtered {
            println!("{}", format_oneline(e));
        }
    } else {
        // Group by sha so commits with multiple events get one header.
        let mut last_sha: Option<String> = None;
        for e in &filtered {
            if Some(&e.sha) != last_sha.as_ref() {
                if last_sha.is_some() {
                    println!();
                }
                println!(
                    "{} {}  {}",
                    "commit".yellow(),
                    e.sha[..8].yellow(),
                    format!("({}, by {})", e.timestamp, e.author).dimmed()
                );
                last_sha = Some(e.sha.clone());
            }
            println!("  {}", format_event_body(e));
        }
    }

    Ok(())
}

/// Collect structured event records using the same filters as
/// `aida history --events`. Intended for MCP and other non-TTY consumers.
/// trace:TASK-538 | ai:codex
pub fn collect_event_records(
    store_path: &Path,
    opts: &HistoryOpts,
) -> Result<Vec<HistoryEventRecord>> {
    if !store_path.is_dir() {
        anyhow::bail!(
            "Not a git-canonical AIDA store: {}\n\
             aida history walks the orphan branch's git log; the legacy SQLite\n\
             backend has no per-edit history surface.",
            store_path.display()
        );
    }

    let (events, _) = collect_filtered_events(store_path, opts)?;
    Ok(events.iter().map(event_record).collect())
}

fn collect_filtered_events(store_path: &Path, opts: &HistoryOpts) -> Result<(Vec<Event>, usize)> {
    // Build a `git log` command bounded by --since / --until / --max_commits.
    let mut log_args: Vec<String> = vec![
        "log".into(),
        "--pretty=format:%H%x09%aI%x09%ae".into(),
        format!("-n{}", opts.max_commits),
    ];
    if let Some(s) = &opts.since {
        log_args.push(format!("--since={}", s));
    }
    if let Some(u) = &opts.until {
        log_args.push(format!("--until={}", u));
    }

    // Path-scope the log walk to the one spec when `--id` is set. Without
    // this, git walks the entire orphan-branch history (~15.7k commits on the
    // AIDA store) and the spec filter happens in-memory below; the pathspec
    // makes git emit only the handful of commits that touched this spec's
    // YAML, so a targeted `--id` query is ~as fast as the spec's edit count
    // rather than the whole-store walk. The in-memory id filter below still
    // runs (it also resolves UUIDs/agreed-ids to the canonical spec_id), so
    // correctness is unchanged — this just shrinks the candidate set.
    // trace:TASK-1055
    let id_pathspec: Option<String> = opts
        .id_filter
        .as_deref()
        .and_then(|id| aida_core::object_store::relative_object_path(id).ok());
    if let Some(ref path) = id_pathspec {
        log_args.push("--".into());
        log_args.push(path.clone());
    }

    let log_output = run_git(store_path, &log_args)?;
    let commits: Vec<CommitMeta> = log_output.lines().filter_map(parse_log_line).collect();

    let mut events: Vec<Event> = Vec::new();

    for commit in &commits {
        // Only commits that actually touch object YAML files contribute
        // events. The auto-commit "chore: update requirements store" still
        // gets walked because each individual file change inside it is
        // its own event.
        let mut show_args: Vec<String> = vec![
            "show".into(),
            "--name-status".into(),
            "--format=".into(),
            commit.sha.clone(),
        ];
        // When path-scoped (`--id`), restrict the file listing to the spec's
        // YAML so a bulk "chore: update requirements store" commit that
        // happens to touch this spec doesn't also decode every other file it
        // changed. trace:TASK-1055
        if let Some(ref path) = id_pathspec {
            show_args.push("--".into());
            show_args.push(path.clone());
        }
        let changed = run_git(store_path, &show_args)?;
        for line in changed.lines() {
            // Lines look like:  M\tobjects/FR/000/FR-1-011.yaml
            //                   A\tobjects/TASK/000/TASK-1-021.yaml
            //                   D\tobjects/EPIC/000/EPIC-9999.yaml
            let mut parts = line.splitn(2, '\t');
            let status = parts.next().unwrap_or("").trim();
            let path = parts.next().unwrap_or("").trim();
            if path.is_empty() || !path.starts_with("objects/") || !path.ends_with(".yaml") {
                continue;
            }

            // Pull before/after content via `git show <sha>^:path` and
            // `git show <sha>:path`. The `^` form fails on the first commit
            // — that's fine; we treat absence as "added".
            let after = git_show_blob(store_path, &commit.sha, path).ok();
            let before = git_show_blob(store_path, &format!("{}^", commit.sha), path).ok();

            decode_into_events(
                commit,
                status,
                path,
                before.as_deref(),
                after.as_deref(),
                &mut events,
            );
        }
    }

    // Apply filters not handled by `git log` itself.
    // STORY-441: archive filtering replaces the older terminal-status hide.
    // `archived_specs` (non-empty when default `--non-archived`) hides those
    // spec events; `archived_only_specs` (Some(...) when `--archived`)
    // narrows to only those. trace:STORY-441 | ai:claude
    let filtered: Vec<Event> = events
        .into_iter()
        .filter(|e| match &opts.id_filter {
            Some(id) => e.spec_id.eq_ignore_ascii_case(id),
            None => true,
        })
        .filter(|e| match &opts.type_filter {
            Some(t) => e.req_type.eq_ignore_ascii_case(t),
            None => true,
        })
        // STORY-737 (delight #4): drop stateless META prompt-template rows from
        // the default view. trace:STORY-737 | ai:claude
        .filter(|e| !opts.exclude_meta || !e.req_type.eq_ignore_ascii_case("meta"))
        .filter(|e| match &opts.author_filter {
            Some(a) => e.author.contains(a),
            None => true,
        })
        .filter(|e| {
            if !opts.status_changes_only {
                return true;
            }
            matches!(e.kind, EventKind::StatusChange { .. })
        })
        .filter(|e| {
            // TASK-507: `--shipped` keeps only the Done→Completed transition —
            // the merge-to-default ship event. trace:TASK-507 | ai:claude
            if !opts.shipped_only {
                return true;
            }
            is_ship_event(&e.kind)
        })
        .filter(|e| {
            if !opts.comments_only {
                return true;
            }
            matches!(e.kind, EventKind::CommentsAdded { .. })
        })
        .filter(|e| !opts.archived_specs.contains(&e.spec_id))
        .filter(|e| match &opts.archived_only_specs {
            Some(only) => only.contains(&e.spec_id),
            None => true,
        })
        // STORY-584: same shape on the defer axis. trace:STORY-584 | ai:claude
        .filter(|e| !opts.deferred_specs.contains(&e.spec_id))
        .filter(|e| match &opts.deferred_only_specs {
            Some(only) => only.contains(&e.spec_id),
            None => true,
        })
        .take(opts.limit)
        .collect();

    Ok((filtered, opts.archived_specs.len()))
}

/// Digest mode (default): one row per recently-touched requirement, sorted
/// by last-touch time. Aimed at "what was I up to last session?" — answers
/// in a glance without decoding every individual diff.
///
/// Source of truth for "when did this actually change" is each YAML's own
/// `modified_at` field, NOT the git commit timestamp. Reason: the legacy
/// Storage façade still calls `GitBackend::save(&store)` for some paths,
/// which rewrites every YAML and produces "chore: update requirements
/// store" bulk-commits that touch dozens of files at once but don't bump
/// modified_at on any of them. Sorting by git ts would cluster every spec
/// into the most recent bulk commit's timestamp; sorting by modified_at
/// reflects real edits.
///
/// "C" column counts only commits whose subject explicitly mentions this
/// spec_id (`update X`, `add X`, `delete X`) so chore bulk-saves don't
/// inflate it.
///
/// Speed: one `git log --name-status` call + one filesystem read per
/// distinct requirement that surfaces. Sub-second on the AIDA store.
/// trace:FR-1-037 | ai:claude
fn run_digest(store_path: &Path, opts: &HistoryOpts) -> Result<()> {
    let mut log_args: Vec<String> = vec![
        "log".into(),
        "--name-status".into(),
        "--pretty=format:%H%x09%aI%x09%ae%x09%s".into(),
        format!("-n{}", opts.max_commits),
    ];
    if let Some(s) = &opts.since {
        log_args.push(format!("--since={}", s));
    }
    if let Some(u) = &opts.until {
        log_args.push(format!("--until={}", u));
    }

    let log_output = run_git(store_path, &log_args)?;

    // Walk the streamed output line-by-line. Commit-metadata lines have
    // exactly three tabs (sha\tts\tauthor\tsubject); --name-status lines
    // have one (M\tpath / A\tpath / D\tpath). Blank lines separate commits.
    use std::collections::BTreeMap;
    let mut summaries: BTreeMap<String, DigestEntry> = BTreeMap::new();
    let mut current: Option<CommitInfo> = None;

    for line in log_output.lines() {
        if line.is_empty() {
            continue;
        }
        let tabs = line.bytes().filter(|b| *b == b'\t').count();
        if tabs >= 3 {
            // commit metadata
            let mut parts = line.splitn(4, '\t');
            let sha = parts.next().unwrap_or("").to_string();
            let ts = parts.next().unwrap_or("").to_string();
            let author = parts.next().unwrap_or("").to_string();
            let subject = parts.next().unwrap_or("").to_string();
            current = Some(CommitInfo {
                sha,
                ts,
                author,
                subject_spec: targeted_spec_id_from_subject(&subject),
            });
            continue;
        }

        // Otherwise treat as a name-status line. Skip non-object paths (the
        // orphan branch carries oplog.yaml and a few control files we don't
        // want surfacing here).
        let mut parts = line.split('\t');
        let status_letter = parts.next().unwrap_or("").to_string();
        let path = parts.next().unwrap_or("").to_string();
        if !path.starts_with("objects/") || !path.ends_with(".yaml") {
            continue;
        }
        let Some(commit) = current.as_ref() else {
            continue;
        };

        let spec_id = spec_id_from_path(&path);
        let req_type = req_type_from_path(&path);

        let entry = summaries
            .entry(spec_id.clone())
            .or_insert_with(|| DigestEntry {
                spec_id: spec_id.clone(),
                req_type,
                last_git_ts: commit.ts.clone(),
                last_author_email: commit.author.clone(),
                had_add: false,
                had_delete: false,
            });
        // Track whether this YAML was added or deleted somewhere in the
        // window so we can show "+ / − / ·" markers. Status letter is the
        // one from `git log --name-status`.
        match status_letter.chars().next() {
            Some('A') => entry.had_add = true,
            Some('D') => entry.had_delete = true,
            _ => {}
        }
    }

    // Read the current state for each surfaced spec_id from the worktree.
    // YAML's `modified_at` is the canonical timestamp — git commit ts is
    // only the fallback for deleted requirements (no YAML to read).
    let mut entries: Vec<DigestRow> = summaries
        .into_values()
        .map(|e| {
            let yaml_path = store_path
                .join("objects")
                .join(&e.req_type)
                .join("000")
                .join(format!("{}.yaml", e.spec_id));
            let (status, title, modified_at) = read_current(&yaml_path);
            // Prefer the YAML's modified_at (canonical), fall back to git ts.
            let last_ts_iso = modified_at.unwrap_or_else(|| e.last_git_ts.clone());
            DigestRow {
                spec_id: e.spec_id,
                req_type: e.req_type,
                status,
                title,
                last_ts_iso,
                last_author: pick_author_email(&e.last_author_email),
                had_add: e.had_add,
                had_delete: e.had_delete,
            }
        })
        .collect();

    // Apply filters that depend on the resolved row.
    let id_filter = opts.id_filter.clone();
    let type_filter = opts.type_filter.clone();
    let author_filter = opts.author_filter.clone();
    entries.retain(|e| {
        if let Some(ref id) = id_filter {
            if !e.spec_id.eq_ignore_ascii_case(id) {
                return false;
            }
        }
        if let Some(ref t) = type_filter {
            // Allow either the path-prefix form ("FR") or the human form
            // ("functional"). Path prefix is the canonical hit.
            let want = t.to_uppercase();
            let path_prefix = e.req_type.to_uppercase();
            let display = display_type_name(&e.req_type).to_uppercase();
            if path_prefix != want && display != want {
                return false;
            }
        }
        if let Some(ref a) = author_filter {
            if !e.last_author.contains(a) {
                return false;
            }
        }
        // STORY-737 (delight #4): hide stateless META prompt-template rows from
        // the default digest. `e.req_type` is the path-prefix form ("META"), so
        // a case-insensitive compare catches it. trace:STORY-737 | ai:claude
        if opts.exclude_meta && e.req_type.eq_ignore_ascii_case("meta") {
            return false;
        }
        true
    });

    // STORY-441: hide archived rows in the default view; count drops so we
    // can print a "(N archived hidden — pass --all …)" hint that mirrors
    // `aida list`. With `--archived`, narrow to the archive itself.
    // trace:STORY-441 | ai:claude
    let archived_hidden = {
        let before = entries.len();
        entries.retain(|e| !opts.archived_specs.contains(&e.spec_id));
        before - entries.len()
    };
    if let Some(only) = &opts.archived_only_specs {
        entries.retain(|e| only.contains(&e.spec_id));
    }

    // STORY-584: same shape on the defer axis. trace:STORY-584 | ai:claude
    let deferred_hidden = {
        let before = entries.len();
        entries.retain(|e| !opts.deferred_specs.contains(&e.spec_id));
        before - entries.len()
    };
    if let Some(only) = &opts.deferred_only_specs {
        entries.retain(|e| only.contains(&e.spec_id));
    }

    // Sort newest-first by ISO timestamp (string compare works for ISO 8601).
    entries.sort_by(|a, b| b.last_ts_iso.cmp(&a.last_ts_iso));
    entries.truncate(opts.limit);

    if entries.is_empty() {
        eprintln!("{}", "(no recent activity)".dimmed());
        if archived_hidden > 0 {
            eprintln!(
                "{}",
                format!(
                    "({archived_hidden} archived hidden — pass --all or --archived to see them)"
                )
                .dimmed()
            );
        }
        if deferred_hidden > 0 {
            eprintln!(
                "{}",
                format!(
                    "({deferred_hidden} deferred hidden — pass --all or --deferred to see them)"
                )
                .dimmed()
            );
        }
        return Ok(());
    }

    // Width-align spec_id column so the rest reads as a table.
    let id_w = entries.iter().map(|e| e.spec_id.len()).max().unwrap_or(8);
    let status_w = entries.iter().map(|e| e.status.len()).max().unwrap_or(10);
    // Time column needs to fit the widest rendered value (today's HH:MM
    // collapses to 5 chars; older "MM-DD HH:MM" is 11). Compute once.
    let time_w = entries
        .iter()
        .map(|e| short_clock(&e.last_ts_iso).len())
        .max()
        .unwrap_or(5);

    // Header: same column widths as rows. dimmed() applies after padding
    // so the color codes don't break alignment of the row data below.
    let header = format!(
        "{:<2} {:<id_w$}  {:<8}  {:<status_w$}  {:<time_w$}  {}",
        "",
        "ID",
        "TYPE",
        "STATUS",
        "WHEN",
        "TITLE",
        id_w = id_w,
        status_w = status_w,
        time_w = time_w,
    );
    println!("{}", header.dimmed());

    for e in &entries {
        // Inline marker per row: "+" for added in window, "−" for deleted,
        // "·" for edited. Keeps the C-column noise out and makes the
        // "what's new" answer immediately visible.
        let marker = if e.had_delete {
            "−".red().to_string()
        } else if e.had_add {
            "+".green().bold().to_string()
        } else {
            "·".dimmed().to_string()
        };

        let time = short_clock(&e.last_ts_iso);
        let title = shorten(&e.title, 70);

        // Pad PLAIN text first, THEN apply color — Rust's `{:<width$}`
        // counts bytes (including ANSI escape codes) so colorizing before
        // padding produces visibly misaligned columns.
        let id_padded = format!("{:<id_w$}", e.spec_id, id_w = id_w);
        let type_padded = format!("{:<8}", display_type_name(&e.req_type));
        let status_padded = format!("{:<status_w$}", e.status, status_w = status_w);
        let time_padded = format!("{:<time_w$}", time, time_w = time_w);

        println!(
            "{:<2} {}  {}  {}  {}  {}",
            marker,
            id_padded.bold(),
            type_padded,
            colorize_status(&status_padded),
            time_padded,
            title.dimmed(),
        );
    }

    if archived_hidden > 0 {
        println!(
            "{}",
            format!("  ({archived_hidden} archived hidden — pass --all or --archived to see them)")
                .dimmed()
        );
    }
    if deferred_hidden > 0 {
        println!(
            "{}",
            format!("  ({deferred_hidden} deferred hidden — pass --all or --deferred to see them)")
                .dimmed()
        );
    }

    Ok(())
}

/// In-flight commit metadata while parsing `git log --name-status`.
#[derive(Debug)]
struct CommitInfo {
    #[allow(dead_code)] // kept for symmetry / future per-row "last sha" column
    sha: String,
    ts: String,
    author: String,
    /// SPEC-ID explicitly named in the commit subject (e.g. "update FR-1-037").
    /// Currently unused after the C-column simplification but kept for a
    /// future "targeted edits only" filter — the parsing is essentially
    /// free and removing/restoring it churns the format string.
    #[allow(dead_code)]
    subject_spec: Option<String>,
}

#[derive(Debug)]
struct DigestEntry {
    spec_id: String,
    req_type: String,
    /// Newest git commit ts that touched this YAML (chore commits included).
    /// Used only as a fallback when the YAML's `modified_at` can't be read
    /// (e.g. the file was deleted).
    last_git_ts: String,
    last_author_email: String,
    had_add: bool,
    had_delete: bool,
}

#[derive(Debug)]
struct DigestRow {
    spec_id: String,
    req_type: String,
    status: String,
    title: String,
    last_ts_iso: String,
    last_author: String,
    had_add: bool,
    had_delete: bool,
}

/// Returns (status, title, modified_at). `modified_at` is None for
/// deleted requirements or when the field is missing — the caller then
/// falls back to the git commit timestamp.
fn read_current(yaml_path: &Path) -> (String, String, Option<String>) {
    let Ok(text) = std::fs::read_to_string(yaml_path) else {
        return ("(deleted)".to_string(), String::new(), None);
    };
    let v: Value = match serde_yaml::from_str(&text) {
        Ok(v) => v,
        Err(_) => return ("(parse-error)".to_string(), String::new(), None),
    };
    let status = effective(&v, "status", "custom_status").unwrap_or_default();
    let title = yaml_string(&v, "title").unwrap_or_default();
    let modified_at = yaml_string(&v, "modified_at");
    (status, title, modified_at)
}

/// Extract the SPEC-ID a commit's subject line explicitly targets, if any.
/// Subjects emitted by GitBackend look like "update FR-1-037" / "add
/// FR-1-037 — title" / "delete FR-1-037". The bulk-save path emits
/// "chore: update requirements store" — None for those, so they don't
/// inflate the per-spec commit count.
/// trace:FR-1-037 | ai:claude
fn targeted_spec_id_from_subject(subject: &str) -> Option<String> {
    let s = subject.trim();
    for prefix in ["update ", "add ", "delete "] {
        if let Some(rest) = s.strip_prefix(prefix) {
            // Take the first whitespace- or em-dash-separated token.
            let token: String = rest
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != '—')
                .collect();
            if !token.is_empty() && looks_like_spec_id(&token) {
                return Some(token);
            }
        }
    }
    None
}

fn looks_like_spec_id(s: &str) -> bool {
    // SPEC-IDs are letters then dash-and-digits, optionally with a node
    // segment (`FR-1-037`). Reject pure numbers and chore-style words.
    let mut chars = s.chars();
    if !chars
        .next()
        .map(|c| c.is_ascii_alphabetic())
        .unwrap_or(false)
    {
        return false;
    }
    s.contains('-') && s.chars().any(|c| c.is_ascii_digit())
}

fn pick_author_email(email: &str) -> String {
    email.split('@').next().unwrap_or(email).to_string()
}

/// HH:MM if today (in the user's local tz), else "MM-DD HH:MM". Always
/// converts the input ISO timestamp from UTC (or whatever offset it
/// carries) into the user's local time first — YAML modified_at fields
/// are stored as UTC (`Z` suffix), so showing the raw HH:MM made
/// timestamps look up to 12 hours in the future on west-of-UTC
/// machines.
/// trace:FR-1-037 | ai:claude
fn short_clock(iso: &str) -> String {
    use chrono::{DateTime, FixedOffset, Local};
    let Ok(dt_offset) = iso.parse::<DateTime<FixedOffset>>() else {
        return iso.to_string();
    };
    let dt_local = dt_offset.with_timezone(&Local);
    let today = Local::now().date_naive();
    if dt_local.date_naive() == today {
        dt_local.format("%H:%M").to_string()
    } else {
        dt_local.format("%m-%d %H:%M").to_string()
    }
}

fn display_type_name(s: &str) -> String {
    // Path prefixes are uppercase "FR", "EPIC", "TASK", "BUG", "STORY",
    // etc. Map to short display tokens that fit an 8-char column.
    match s {
        "FR" => "Func".into(),
        "NFR" => "NonFn".into(),
        "BUG" => "Bug".into(),
        "EPIC" => "Epic".into(),
        "STORY" => "Story".into(),
        "TASK" => "Task".into(),
        "SPIKE" => "Spike".into(),
        "SPRINT" => "Sprint".into(),
        "FOLDER" => "Folder".into(),
        "META" => "Meta".into(),
        "UR" => "User".into(),
        "SR" => "System".into(),
        "CR" => "ChgReq".into(),
        other => other.to_string(),
    }
}

/// Colourise an already-padded status cell for the `aida history` table.
/// TASK-269 unified the per-command palettes — this delegates to the shared
/// `status_display` module. `status` arrives column-padded; `paint_status`
/// normalises the trailing spaces away when picking the colour. No glyph is
/// added here: a 2-char glyph prefix would break the fixed-width column.
/// trace:TASK-269 | ai:claude
fn colorize_status(status: &str) -> String {
    crate::status_display::paint_status(status, status).to_string()
}

fn parse_log_line(line: &str) -> Option<CommitMeta> {
    let mut parts = line.split('\t');
    let sha = parts.next()?.to_string();
    let iso_timestamp = parts.next()?.to_string();
    let git_author = parts.next()?.to_string();
    Some(CommitMeta {
        sha,
        iso_timestamp,
        git_author,
    })
}

fn run_git(cwd: &Path, args: &[String]) -> Result<String> {
    let out = ProcessCommand::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git in {}", cwd.display()))?;
    if !out.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn git_show_blob(cwd: &Path, rev: &str, path: &str) -> Result<String> {
    run_git(cwd, &["show".into(), format!("{}:{}", rev, path)])
}

/// Decode one (commit, path) tuple into zero or more events and append
/// them to `out`. `status` is the git `--name-status` letter (A/M/D/...).
fn decode_into_events(
    commit: &CommitMeta,
    status: &str,
    path: &str,
    before: Option<&str>,
    after: Option<&str>,
    out: &mut Vec<Event>,
) {
    let req_type = req_type_from_path(path);

    let after_yaml = after.and_then(|s| serde_yaml::from_str::<Value>(s).ok());
    let before_yaml = before.and_then(|s| serde_yaml::from_str::<Value>(s).ok());

    let spec_id = after_yaml
        .as_ref()
        .or(before_yaml.as_ref())
        .and_then(|v| v.get("spec_id").and_then(Value::as_str).map(String::from))
        .unwrap_or_else(|| spec_id_from_path(path));

    let author = pick_author(&before_yaml, &after_yaml, commit);
    let mk = |kind: EventKind| Event {
        sha: commit.sha.clone(),
        timestamp: human_timestamp(&commit.iso_timestamp),
        author: author.clone(),
        spec_id: spec_id.clone(),
        req_type: req_type.clone(),
        kind,
    };

    match status.chars().next() {
        Some('A') => {
            if let Some(v) = &after_yaml {
                out.push(mk(EventKind::Added {
                    title: yaml_string(v, "title").unwrap_or_default(),
                    req_type: yaml_string(v, "req_type")
                        .or_else(|| yaml_string(v, "type"))
                        .unwrap_or_else(|| req_type.clone()),
                    priority: effective(v, "priority", "custom_priority").unwrap_or_default(),
                }));
            }
        }
        Some('D') => {
            if let Some(v) = &before_yaml {
                out.push(mk(EventKind::Deleted {
                    title: yaml_string(v, "title").unwrap_or_default(),
                }));
            }
        }
        _ => {
            // Modified — diff before vs after.
            if let (Some(a), Some(b)) = (&before_yaml, &after_yaml) {
                diff_modified(a, b, &mk, out);
            }
        }
    }
}

/// Compare scalar fields and emit a tagged event for each that changed.
/// Uses the `mk` closure so every emitted event inherits the commit
/// metadata without us re-cloning it inline.
fn diff_modified(
    before: &Value,
    after: &Value,
    mk: &dyn Fn(EventKind) -> Event,
    out: &mut Vec<Event>,
) {
    // Status — read effective (custom_status overrides) so we report the
    // user-visible value, not the underlying enum that might lag behind.
    let s_before = effective(before, "status", "custom_status");
    let s_after = effective(after, "status", "custom_status");
    if s_before != s_after {
        out.push(mk(EventKind::StatusChange {
            from: s_before.clone().unwrap_or_else(|| "—".into()),
            to: s_after.clone().unwrap_or_else(|| "—".into()),
        }));
    }

    let p_before = effective(before, "priority", "custom_priority");
    let p_after = effective(after, "priority", "custom_priority");
    if p_before != p_after {
        out.push(mk(EventKind::PriorityChange {
            from: p_before.unwrap_or_else(|| "—".into()),
            to: p_after.unwrap_or_else(|| "—".into()),
        }));
    }

    let t_before = yaml_string(before, "title");
    let t_after = yaml_string(after, "title");
    if t_before != t_after {
        out.push(mk(EventKind::TitleChange {
            from: t_before.unwrap_or_default(),
            to: t_after.unwrap_or_default(),
        }));
    }

    if yaml_string(before, "description") != yaml_string(after, "description") {
        out.push(mk(EventKind::DescriptionEdited));
    }

    if yaml_string(before, "owner") != yaml_string(after, "owner") {
        out.push(mk(EventKind::OwnerChange {
            from: yaml_string(before, "owner").unwrap_or_default(),
            to: yaml_string(after, "owner").unwrap_or_default(),
        }));
    }
    if yaml_string(before, "feature") != yaml_string(after, "feature") {
        out.push(mk(EventKind::FeatureChange {
            from: yaml_string(before, "feature").unwrap_or_default(),
            to: yaml_string(after, "feature").unwrap_or_default(),
        }));
    }

    let rt_before = yaml_string(before, "req_type").or_else(|| yaml_string(before, "type"));
    let rt_after = yaml_string(after, "req_type").or_else(|| yaml_string(after, "type"));
    if rt_before != rt_after {
        out.push(mk(EventKind::TypeChange {
            from: rt_before.unwrap_or_default(),
            to: rt_after.unwrap_or_default(),
        }));
    }

    // Tags — set diff so {a,b} → {b,c} reports +c, -a.
    let tags_before = yaml_string_set(before, "tags");
    let tags_after = yaml_string_set(after, "tags");
    if tags_before != tags_after {
        let added: Vec<String> = tags_after.difference(&tags_before).cloned().collect();
        let removed: Vec<String> = tags_before.difference(&tags_after).cloned().collect();
        out.push(mk(EventKind::TagsChange { added, removed }));
    }

    // Comments — count delta on the array. If the array grew, the new
    // tail entry's `author` field is surfaced.
    let cb = yaml_array_len(before, "comments");
    let ca = yaml_array_len(after, "comments");
    if ca > cb {
        let last_author = after
            .get("comments")
            .and_then(Value::as_sequence)
            .and_then(|s| s.last())
            .and_then(|v| v.get("author"))
            .and_then(Value::as_str)
            .map(String::from);
        out.push(mk(EventKind::CommentsAdded {
            count: ca - cb,
            author: last_author,
        }));
    }

    let rb = yaml_array_len(before, "relationships");
    let ra = yaml_array_len(after, "relationships");
    if ra != rb {
        let added = ra.saturating_sub(rb);
        let removed = rb.saturating_sub(ra);
        out.push(mk(EventKind::RelationshipsChange { added, removed }));
    }
}

fn pick_author(before: &Option<Value>, after: &Option<Value>, commit: &CommitMeta) -> String {
    // Prefer the YAML's last_modified_by if the field exists and is set.
    // Falls back to the git committer's local-part.
    let from_yaml = after
        .as_ref()
        .or(before.as_ref())
        .and_then(|v| yaml_string(v, "last_modified_by"));
    if let Some(name) = from_yaml {
        if !name.is_empty() {
            return name;
        }
    }
    commit
        .git_author
        .split('@')
        .next()
        .unwrap_or(&commit.git_author)
        .to_string()
}

/// "objects/FR/000/FR-1-011.yaml" → "FR-1-011"
fn spec_id_from_path(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("?")
        .to_string()
}

/// "objects/FR/000/FR-1-011.yaml" → "FR"
fn req_type_from_path(path: &str) -> String {
    path.strip_prefix("objects/")
        .and_then(|s| s.split('/').next())
        .unwrap_or("?")
        .to_string()
}

fn yaml_string(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(String::from)
}

/// Read `key` first; if absent or empty, fall back to `custom_key`. This
/// mirrors `effective_status()` / `effective_priority()` semantics on the
/// model side.
fn effective(v: &Value, key: &str, custom_key: &str) -> Option<String> {
    if let Some(custom) = yaml_string(v, custom_key) {
        if !custom.is_empty() {
            return Some(custom);
        }
    }
    yaml_string(v, key)
}

fn yaml_string_set(v: &Value, key: &str) -> BTreeSet<String> {
    v.get(key)
        .and_then(Value::as_sequence)
        .map(|seq| {
            seq.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn yaml_array_len(v: &Value, key: &str) -> usize {
    v.get(key)
        .and_then(Value::as_sequence)
        .map(|s| s.len())
        .unwrap_or(0)
}

/// "YYYY-MM-DD HH:MM" in the user's LOCAL timezone. Full RFC3339 is too
/// noisy for a feed view, but date+time without timezone is enough to
/// scan. Always converts to local — input ISO strings are usually UTC
/// (`Z` suffix on YAML modified_at) and showing them raw made timestamps
/// look up to 12 hours in the future on west-of-UTC machines.
/// trace:FR-1-037 | ai:claude
fn human_timestamp(iso: &str) -> String {
    use chrono::{DateTime, FixedOffset, Local};
    let Ok(dt_offset) = iso.parse::<DateTime<FixedOffset>>() else {
        return iso.to_string();
    };
    dt_offset
        .with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

fn format_oneline(e: &Event) -> String {
    let head = format!(
        "{} {} {}",
        e.timestamp.dimmed(),
        e.author.cyan(),
        e.spec_id.bold()
    );
    format!("{}  {}", head, format_event_body(e))
}

fn event_record(e: &Event) -> HistoryEventRecord {
    let (kind, summary, detail) = event_kind_record(&e.kind);
    HistoryEventRecord {
        sha: e.sha.clone(),
        timestamp: e.timestamp.clone(),
        author: e.author.clone(),
        spec_id: e.spec_id.clone(),
        req_type: e.req_type.clone(),
        kind,
        summary,
        detail,
    }
}

fn event_kind_record(kind: &EventKind) -> (String, String, JsonValue) {
    match kind {
        EventKind::Added {
            title,
            req_type,
            priority,
        } => (
            "added".to_string(),
            format!("added ({req_type}, {priority}) {}", shorten(title, 80)),
            json!({
                "title": title,
                "req_type": req_type,
                "priority": priority,
            }),
        ),
        EventKind::Deleted { title } => (
            "deleted".to_string(),
            format!("deleted {}", shorten(title, 80)),
            json!({ "title": title }),
        ),
        EventKind::StatusChange { from, to } => (
            "status_change".to_string(),
            format!("status: {from} -> {to}"),
            json!({ "from": from, "to": to }),
        ),
        EventKind::PriorityChange { from, to } => (
            "priority_change".to_string(),
            format!("priority: {from} -> {to}"),
            json!({ "from": from, "to": to }),
        ),
        EventKind::TitleChange { from, to } => (
            "title_change".to_string(),
            format!("title: {} -> {}", shorten(from, 40), shorten(to, 40)),
            json!({ "from": from, "to": to }),
        ),
        EventKind::DescriptionEdited => (
            "description_edited".to_string(),
            "description edited".to_string(),
            json!({}),
        ),
        EventKind::OwnerChange { from, to } => (
            "owner_change".to_string(),
            format!("owner: {} -> {}", maybe_dash(from), maybe_dash(to)),
            json!({ "from": from, "to": to }),
        ),
        EventKind::FeatureChange { from, to } => (
            "feature_change".to_string(),
            format!("feature: {} -> {}", maybe_dash(from), maybe_dash(to)),
            json!({ "from": from, "to": to }),
        ),
        EventKind::TypeChange { from, to } => (
            "type_change".to_string(),
            format!("type: {from} -> {to}"),
            json!({ "from": from, "to": to }),
        ),
        EventKind::TagsChange { added, removed } => {
            let mut parts: Vec<String> = added.iter().map(|t| format!("+{t}")).collect();
            parts.extend(removed.iter().map(|t| format!("-{t}")));
            (
                "tags_change".to_string(),
                format!("tags: {}", parts.join(" ")),
                json!({ "added": added, "removed": removed }),
            )
        }
        EventKind::CommentsAdded { count, author } => {
            let noun = if *count == 1 {
                "comment added"
            } else {
                "comments added"
            };
            let summary = match author {
                Some(a) => format!("{noun} {count} by {a}"),
                None => format!("{noun} {count}"),
            };
            (
                "comments_added".to_string(),
                summary,
                json!({ "count": count, "author": author }),
            )
        }
        EventKind::RelationshipsChange { added, removed } => (
            "relationships_change".to_string(),
            format!("relationships: +{added} added, -{removed} removed"),
            json!({ "added": added, "removed": removed }),
        ),
    }
}

fn format_event_body(e: &Event) -> String {
    match &e.kind {
        EventKind::Added {
            title,
            req_type,
            priority,
        } => format!(
            "{} ({}, {}) {}",
            "added".green(),
            req_type,
            priority,
            shorten(title, 80).dimmed()
        ),
        EventKind::Deleted { title } => {
            format!("{} {}", "deleted".red(), shorten(title, 80).dimmed())
        }
        EventKind::StatusChange { from, to } => {
            format!("{}: {} → {}", "status".bold(), from.yellow(), to.green())
        }
        EventKind::PriorityChange { from, to } => {
            format!("{}: {} → {}", "priority".bold(), from, to)
        }
        EventKind::TitleChange { from, to } => format!(
            "{}: {} → {}",
            "title".bold(),
            shorten(from, 40).dimmed(),
            shorten(to, 40)
        ),
        EventKind::DescriptionEdited => format!("{}", "description edited".bold()),
        EventKind::OwnerChange { from, to } => {
            format!(
                "{}: {} → {}",
                "owner".bold(),
                maybe_dash(from),
                maybe_dash(to)
            )
        }
        EventKind::FeatureChange { from, to } => format!(
            "{}: {} → {}",
            "feature".bold(),
            maybe_dash(from),
            maybe_dash(to)
        ),
        EventKind::TypeChange { from, to } => {
            format!("{}: {} → {}", "type".bold(), from, to)
        }
        EventKind::TagsChange { added, removed } => {
            let mut parts: Vec<String> = Vec::new();
            for t in added {
                parts.push(format!("+{}", t).green().to_string());
            }
            for t in removed {
                parts.push(format!("-{}", t).red().to_string());
            }
            format!("{}: {}", "tags".bold(), parts.join(" "))
        }
        EventKind::CommentsAdded { count, author } => {
            let by = match author {
                Some(a) => format!(" by {}", a.cyan()),
                None => String::new(),
            };
            if *count == 1 {
                format!("{}{}", "comment added".bold(), by)
            } else {
                format!("{} {}{}", "comments added".bold(), count, by)
            }
        }
        EventKind::RelationshipsChange { added, removed } => {
            let mut parts = Vec::new();
            if *added > 0 {
                parts.push(format!("+{} added", added).green().to_string());
            }
            if *removed > 0 {
                parts.push(format!("-{} removed", removed).red().to_string());
            }
            format!("{}: {}", "relationships".bold(), parts.join(", "))
        }
    }
}

fn maybe_dash(s: &str) -> String {
    if s.is_empty() {
        "—".into()
    } else {
        s.to_string()
    }
}

// Char-boundary-safe truncation: titles carry emoji/unicode, so slicing by raw
// byte index panics mid-codepoint (BUG-424). Truncate by chars. trace:BUG-424
fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TASK-1055: a path-scoped `--id` walk only visits the spec's own commits,
    /// not the whole orphan-branch history. Builds a tiny store with three
    /// commits — one touching only TASK-1, one touching only STORY-1, and a
    /// bulk commit touching BOTH — then asserts (a) git's pathspec sees just
    /// the two commits that touched TASK-1's YAML (not all three), and (b) the
    /// filtered events come back as TASK-1's only.
    // trace:TASK-1055
    #[test]
    fn history_id_filter_path_scopes_log_walk() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let git = |args: &[&str]| {
            let out = ProcessCommand::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);

        let write = |rel: &str, body: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, body).unwrap();
        };
        let task_path = "objects/TASK/000/TASK-1.yaml";
        let story_path = "objects/STORY/000/STORY-1.yaml";

        // Commit 1: TASK-1 only.
        write(task_path, "spec_id: TASK-1\ntitle: t\nstatus: Draft\n");
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "add TASK-1"]);

        // Commit 2: STORY-1 only (a different spec — must NOT be walked).
        write(story_path, "spec_id: STORY-1\ntitle: s\nstatus: Draft\n");
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "add STORY-1"]);

        // Commit 3: a bulk commit touching BOTH specs.
        write(task_path, "spec_id: TASK-1\ntitle: t\nstatus: Approved\n");
        write(story_path, "spec_id: STORY-1\ntitle: s\nstatus: Approved\n");
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "chore: update requirements store"]);

        // (a) git's pathspec sees only the two TASK-1 commits, not all three.
        let scoped = run_git(
            root,
            &[
                "log".into(),
                "--oneline".into(),
                "--".into(),
                task_path.into(),
            ],
        )
        .unwrap();
        let full = run_git(root, &["log".into(), "--oneline".into()]).unwrap();
        assert_eq!(
            scoped.lines().count(),
            2,
            "pathspec must scope the walk to TASK-1's 2 commits"
        );
        assert_eq!(
            full.lines().count(),
            3,
            "the full history has all 3 commits — proving the pathspec narrows it"
        );

        // (b) the filtered events are TASK-1's only.
        let opts = HistoryOpts {
            id_filter: Some("TASK-1".to_string()),
            ..base_opts()
        };
        let (events, _) = collect_filtered_events(root, &opts).unwrap();
        assert!(!events.is_empty(), "expected at least one TASK-1 event");
        assert!(
            events.iter().all(|e| e.spec_id == "TASK-1"),
            "every event must be for the path-scoped spec, got: {:?}",
            events.iter().map(|e| &e.spec_id).collect::<Vec<_>>()
        );
    }

    /// STORY-737 (delight #4): the default `aida history` view hides the
    /// stateless internal META prompt-template rows (`exclude_meta`), but they
    /// stay reachable. This drives the real orphan-store git log and asserts the
    /// META spec's events drop out by default and return when `exclude_meta` is
    /// off (the `--include-meta` / `--type meta` path).
    // trace:STORY-737
    #[test]
    fn history_excludes_meta_rows_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let git = |args: &[&str]| {
            let out = ProcessCommand::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);

        let write = |rel: &str, body: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, body).unwrap();
        };
        let meta_path = "objects/META/000/META-1.yaml";
        let task_path = "objects/TASK/000/TASK-1.yaml";

        // Commit 1: seed both specs as Draft.
        write(meta_path, "spec_id: META-1\ntitle: m\nstatus: Draft\n");
        write(task_path, "spec_id: TASK-1\ntitle: t\nstatus: Draft\n");
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "seed"]);

        // Commit 2: flip both to Approved — one status-change event each.
        write(meta_path, "spec_id: META-1\ntitle: m\nstatus: Approved\n");
        write(task_path, "spec_id: TASK-1\ntitle: t\nstatus: Approved\n");
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "approve both"]);

        // Default view (exclude_meta = true): META-1 is hidden, TASK-1 shows.
        let hidden = HistoryOpts {
            exclude_meta: true,
            ..base_opts()
        };
        let (events, _) = collect_filtered_events(root, &hidden).unwrap();
        assert!(
            events.iter().any(|e| e.spec_id == "TASK-1"),
            "the real spec's events must still show, got: {:?}",
            events.iter().map(|e| &e.spec_id).collect::<Vec<_>>()
        );
        assert!(
            !events.iter().any(|e| e.spec_id == "META-1"),
            "META rows must be hidden by default, got: {:?}",
            events.iter().map(|e| &e.spec_id).collect::<Vec<_>>()
        );

        // `--include-meta` / `--type meta` (exclude_meta = false): META returns.
        let shown = HistoryOpts {
            exclude_meta: false,
            ..base_opts()
        };
        let (events, _) = collect_filtered_events(root, &shown).unwrap();
        assert!(
            events.iter().any(|e| e.spec_id == "META-1"),
            "META rows must be visible when not excluded, got: {:?}",
            events.iter().map(|e| &e.spec_id).collect::<Vec<_>>()
        );
    }

    /// A `HistoryOpts` with every filter off — tests override the one field
    /// they exercise.
    // trace:TASK-1055
    fn base_opts() -> HistoryOpts {
        HistoryOpts {
            limit: 1000,
            max_commits: 1000,
            events_mode: true,
            id_filter: None,
            type_filter: None,
            author_filter: None,
            since: None,
            until: None,
            status_changes_only: false,
            shipped_only: false,
            comments_only: false,
            oneline: false,
            archived_specs: std::collections::HashSet::new(),
            archived_only_specs: None,
            deferred_specs: std::collections::HashSet::new(),
            deferred_only_specs: None,
            exclude_meta: false,
        }
    }

    /// BUG-424: a multibyte char straddling the truncation point must not panic
    /// (raw byte-slicing did). Titles carry emoji/unicode glyphs.
    #[test]
    fn shorten_is_char_boundary_safe_on_emoji_title() {
        let s = "Review PR-75: apply ⇒/⏸ glyph set to skill templates (BUG-116)";
        // byte index ~59 lands inside the ⏸ codepoint — the old panic point.
        for max in [40usize, 58, 59, 60, 1] {
            let out = shorten(s, max); // must not panic at any boundary
            assert!(out.chars().count() <= max.max(1));
        }
        assert!(shorten(s, 30).ends_with('…'));
        // No truncation when it fits (by char count).
        assert_eq!(shorten("short ⏸ title", 100), "short ⏸ title");
    }

    #[test]
    fn spec_id_from_path_extracts_stem() {
        assert_eq!(
            spec_id_from_path("objects/FR/000/FR-1-011.yaml"),
            "FR-1-011"
        );
        assert_eq!(
            spec_id_from_path("objects/EPIC/000/EPIC-1-005.yaml"),
            "EPIC-1-005"
        );
    }

    #[test]
    fn req_type_from_path_pulls_first_segment() {
        assert_eq!(req_type_from_path("objects/FR/000/FR-1-011.yaml"), "FR");
        assert_eq!(req_type_from_path("objects/TASK/000/TASK-1.yaml"), "TASK");
    }

    #[test]
    fn human_timestamp_converts_utc_to_local() {
        // The input is UTC; the output is in the user's local zone. We
        // can't assert a specific value here without controlling TZ, but
        // we CAN assert the parse path round-trips: parsing then
        // formatting produces the same instant regardless of zone.
        use chrono::{DateTime, FixedOffset, Local};
        let iso = "2026-05-04T18:02:38.123456Z";
        let formatted = human_timestamp(iso);
        // Re-parse our formatted local string + assert it matches the
        // expected local representation of the original UTC instant.
        let utc: DateTime<FixedOffset> = iso.parse().unwrap();
        let expected = utc
            .with_timezone(&Local)
            .format("%Y-%m-%d %H:%M")
            .to_string();
        assert_eq!(formatted, expected);
        // Sanity: the bug we're guarding against is "shows raw UTC HH:MM
        // regardless of local zone" — so on west-of-UTC machines the
        // output should NOT contain "18:02" verbatim.
        if Local::now().offset().local_minus_utc() < 0 {
            assert!(
                !formatted.ends_with("18:02"),
                "output is still in UTC: {}",
                formatted
            );
        }
    }

    #[test]
    fn effective_falls_back_to_custom() {
        let v: Value =
            serde_yaml::from_str("status: Draft\ncustom_status: Awaiting Review").unwrap();
        assert_eq!(
            effective(&v, "status", "custom_status").as_deref(),
            Some("Awaiting Review")
        );
    }

    #[test]
    fn diff_status_emits_one_event() {
        let before: Value = serde_yaml::from_str("status: Draft\nspec_id: FR-1\ntitle: t").unwrap();
        let after: Value =
            serde_yaml::from_str("status: Approved\nspec_id: FR-1\ntitle: t").unwrap();
        let commit = CommitMeta {
            sha: "abc".into(),
            iso_timestamp: "2026-05-04T12:00:00Z".into(),
            git_author: "joe@example.com".into(),
        };
        let mut out = Vec::new();
        let mk = |kind: EventKind| Event {
            sha: commit.sha.clone(),
            timestamp: human_timestamp(&commit.iso_timestamp),
            author: "joe".into(),
            spec_id: "FR-1".into(),
            req_type: "FR".into(),
            kind,
        };
        diff_modified(&before, &after, &mk, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0].kind, EventKind::StatusChange { .. }));
    }

    /// TASK-507: `--shipped` keeps only Done→Completed, not other status flips.
    #[test]
    fn is_ship_event_only_done_to_completed() {
        let ship = EventKind::StatusChange {
            from: "Done".into(),
            to: "Completed".into(),
        };
        assert!(is_ship_event(&ship));
        // case-insensitive
        assert!(is_ship_event(&EventKind::StatusChange {
            from: "done".into(),
            to: "completed".into(),
        }));
        // other transitions are not ships
        assert!(!is_ship_event(&EventKind::StatusChange {
            from: "Approved".into(),
            to: "Done".into(),
        }));
        assert!(!is_ship_event(&EventKind::StatusChange {
            from: "InProgress".into(),
            to: "Done".into(),
        }));
        assert!(!is_ship_event(&EventKind::PriorityChange {
            from: "Low".into(),
            to: "High".into(),
        }));
    }

    #[test]
    fn event_record_exposes_structured_status_change() {
        let event = Event {
            sha: "abcdef123456".into(),
            timestamp: "2026-05-24 10:00".into(),
            author: "codex".into(),
            spec_id: "TASK-538".into(),
            req_type: "TASK".into(),
            kind: EventKind::StatusChange {
                from: "Approved".into(),
                to: "Completed".into(),
            },
        };

        let record = event_record(&event);
        assert_eq!(record.kind, "status_change");
        assert_eq!(record.spec_id, "TASK-538");
        assert_eq!(record.detail["from"], "Approved");
        assert_eq!(record.detail["to"], "Completed");
        assert!(record.summary.contains("Approved -> Completed"));
    }

    #[test]
    fn diff_tags_set_semantics() {
        let before: Value = serde_yaml::from_str("tags: [a, b]").unwrap();
        let after: Value = serde_yaml::from_str("tags: [b, c]").unwrap();
        let mut out = Vec::new();
        let commit = CommitMeta {
            sha: "abc".into(),
            iso_timestamp: "2026-05-04T12:00:00Z".into(),
            git_author: "joe@example.com".into(),
        };
        let mk = |kind: EventKind| Event {
            sha: commit.sha.clone(),
            timestamp: "x".into(),
            author: "joe".into(),
            spec_id: "FR-1".into(),
            req_type: "FR".into(),
            kind,
        };
        diff_modified(&before, &after, &mk, &mut out);
        assert_eq!(out.len(), 1);
        match &out[0].kind {
            EventKind::TagsChange { added, removed } => {
                assert_eq!(added, &vec!["c".to_string()]);
                assert_eq!(removed, &vec!["a".to_string()]);
            }
            _ => panic!("expected TagsChange"),
        }
    }

    #[test]
    fn comment_added_uses_last_author() {
        let before: Value = serde_yaml::from_str("comments: []").unwrap();
        let after: Value =
            serde_yaml::from_str("comments:\n  - author: alice\n    content: hi").unwrap();
        let mut out = Vec::new();
        let commit = CommitMeta {
            sha: "abc".into(),
            iso_timestamp: "2026-05-04T12:00:00Z".into(),
            git_author: "joe@example.com".into(),
        };
        let mk = |kind: EventKind| Event {
            sha: commit.sha.clone(),
            timestamp: "x".into(),
            author: "joe".into(),
            spec_id: "FR-1".into(),
            req_type: "FR".into(),
            kind,
        };
        diff_modified(&before, &after, &mk, &mut out);
        assert_eq!(out.len(), 1);
        match &out[0].kind {
            EventKind::CommentsAdded { count, author } => {
                assert_eq!(*count, 1);
                assert_eq!(author.as_deref(), Some("alice"));
            }
            _ => panic!("expected CommentsAdded"),
        }
    }
}
