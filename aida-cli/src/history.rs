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
    pub id_filter: Option<String>,
    pub type_filter: Option<String>,
    pub author_filter: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub status_changes_only: bool,
    pub comments_only: bool,
    pub oneline: bool,
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

pub fn run(store_path: &Path, opts: &HistoryOpts) -> Result<()> {
    if !store_path.is_dir() {
        anyhow::bail!(
            "Not a git-canonical AIDA store: {}\n\
             aida history walks the orphan branch's git log; the legacy SQLite\n\
             backend has no per-edit history surface.",
            store_path.display()
        );
    }

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

    let log_output = run_git(store_path, &log_args)?;
    let commits: Vec<CommitMeta> = log_output
        .lines()
        .filter_map(parse_log_line)
        .collect();

    let mut events: Vec<Event> = Vec::new();

    for commit in &commits {
        // Only commits that actually touch object YAML files contribute
        // events. The auto-commit "chore: update requirements store" still
        // gets walked because each individual file change inside it is
        // its own event.
        let changed = run_git(
            store_path,
            &[
                "show".into(),
                "--name-status".into(),
                "--format=".into(),
                commit.sha.clone(),
            ],
        )?;
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

            decode_into_events(commit, status, path, before.as_deref(), after.as_deref(), &mut events);
        }
    }

    // Apply filters not handled by `git log` itself.
    let filtered: Vec<&Event> = events
        .iter()
        .filter(|e| match &opts.id_filter {
            Some(id) => e.spec_id.eq_ignore_ascii_case(id),
            None => true,
        })
        .filter(|e| match &opts.type_filter {
            Some(t) => e.req_type.eq_ignore_ascii_case(t),
            None => true,
        })
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
            if !opts.comments_only {
                return true;
            }
            matches!(e.kind, EventKind::CommentsAdded { .. })
        })
        .take(opts.limit)
        .collect();

    if filtered.is_empty() {
        eprintln!("{}", "(no events match the filter)".dimmed());
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
                    priority: effective(v, "priority", "custom_priority")
                        .unwrap_or_default(),
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

fn pick_author(
    before: &Option<Value>,
    after: &Option<Value>,
    commit: &CommitMeta,
) -> String {
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

/// Trim the ISO timestamp to "YYYY-MM-DD HH:MM" — full RFC3339 is too noisy
/// for a feed view, but date+time without timezone is enough to scan.
fn human_timestamp(iso: &str) -> String {
    iso.split_once('T')
        .map(|(d, t)| {
            let t = t.split_once('.').map(|(a, _)| a).unwrap_or(t);
            let t = t.split_once('+').map(|(a, _)| a).unwrap_or(t);
            let t = t.split_once('-').map(|(a, _)| a).unwrap_or(t);
            let t = t.strip_suffix('Z').unwrap_or(t);
            let t = &t[..t.len().min(5)];
            format!("{} {}", d, t)
        })
        .unwrap_or_else(|| iso.to_string())
}

fn format_oneline(e: &Event) -> String {
    let head = format!("{} {} {}", e.timestamp.dimmed(), e.author.cyan(), e.spec_id.bold());
    format!("{}  {}", head, format_event_body(e))
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
        EventKind::Deleted { title } => format!(
            "{} {}",
            "deleted".red(),
            shorten(title, 80).dimmed()
        ),
        EventKind::StatusChange { from, to } => format!(
            "{}: {} → {}",
            "status".bold(),
            from.yellow(),
            to.green()
        ),
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
            format!("{}: {} → {}", "owner".bold(), maybe_dash(from), maybe_dash(to))
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

fn shorten(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max.saturating_sub(1)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_id_from_path_extracts_stem() {
        assert_eq!(spec_id_from_path("objects/FR/000/FR-1-011.yaml"), "FR-1-011");
        assert_eq!(spec_id_from_path("objects/EPIC/000/EPIC-1-005.yaml"), "EPIC-1-005");
    }

    #[test]
    fn req_type_from_path_pulls_first_segment() {
        assert_eq!(req_type_from_path("objects/FR/000/FR-1-011.yaml"), "FR");
        assert_eq!(req_type_from_path("objects/TASK/000/TASK-1.yaml"), "TASK");
    }

    #[test]
    fn human_timestamp_trims_tz_and_seconds() {
        assert_eq!(
            human_timestamp("2026-05-04T18:02:38-07:00"),
            "2026-05-04 18:02"
        );
        assert_eq!(
            human_timestamp("2026-05-04T18:02:38.123456Z"),
            "2026-05-04 18:02"
        );
    }

    #[test]
    fn effective_falls_back_to_custom() {
        let v: Value = serde_yaml::from_str("status: Draft\ncustom_status: Awaiting Review").unwrap();
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
