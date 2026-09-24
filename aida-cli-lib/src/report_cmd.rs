//! `aida report` command handler.

use anyhow::{Context, Result};
use colored::Colorize;
use std::collections::HashSet;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};

use aida_core::{
    models::{Requirement, RequirementStatus, RequirementType},
    ReportFormat, ReportGenerator, Storage,
};

use crate::cli::ReportCommand;

const UPSTREAM_AIDA_TAG: &str = "upstream:aida";
const OBSERVED_VERSION_PREFIX: &str = "observed-version:";
const REPORT_KIND_PREFIX: &str = "report-kind:";
const NOTICE_MARKER: &str = ".aida/upstream-report-notice-version";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpstreamReportKind {
    Bug,
    Idea,
}

impl UpstreamReportKind {
    fn as_str(self) -> &'static str {
        match self {
            UpstreamReportKind::Bug => "bug",
            UpstreamReportKind::Idea => "idea",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UpstreamReportContext {
    version: String,
    build_sha: String,
    project_name: String,
    agent_vendor: String,
    enabled_agents: String,
    os: String,
}

pub(crate) fn handle_report_command(
    recheck: bool,
    cmd: Option<&ReportCommand>,
    storage: &Storage,
    storage_path: &str,
) -> Result<()> {
    if recheck {
        if cmd.is_some() {
            anyhow::bail!("pass either `aida report --recheck` or a report subcommand, not both");
        }
        return handle_report_recheck(storage);
    }

    match cmd {
        Some(ReportCommand::Bug {
            title,
            description,
            copy,
            no_file,
        }) => handle_upstream_report(
            storage,
            UpstreamReportKind::Bug,
            title,
            description.as_deref(),
            *copy,
            *no_file,
        ),
        Some(ReportCommand::Idea {
            title,
            description,
            copy,
            no_file,
        }) => handle_upstream_report(
            storage,
            UpstreamReportKind::Idea,
            title,
            description.as_deref(),
            *copy,
            *no_file,
        ),
        Some(ReportCommand::AiIntegration {
            format,
            output,
            project_root,
            include_scaffold,
        }) => {
            let store = storage.load()?;

            // Parse format
            let report_format = match format.to_lowercase().as_str() {
                "markdown" | "md" => ReportFormat::Markdown,
                "html" | "htm" => ReportFormat::Html,
                _ => {
                    return Err(anyhow::anyhow!(
                        "Unknown format '{}'. Use 'markdown' or 'html'.",
                        format
                    ))
                }
            };

            // Create report generator
            let mut generator = ReportGenerator::new(store, storage_path.to_string());

            // Set project root if provided or use current directory for scaffold status
            let root = if let Some(ref root) = project_root {
                root.clone()
            } else if *include_scaffold {
                std::env::current_dir()?
            } else {
                // No root needed if not checking scaffold
                std::path::PathBuf::new()
            };

            if (*include_scaffold || project_root.is_some()) && root.exists() {
                generator = generator.with_project_root(root.clone());
            }

            // Generate report
            let report = generator.generate();

            // Render based on format
            let content = match report_format {
                ReportFormat::Markdown => generator.render_markdown(&report),
                ReportFormat::Html => generator.render_html(&report),
            };

            // Output
            if let Some(ref output_path) = output {
                std::fs::write(output_path, &content)?;
                println!(
                    "{} Report generated: {}",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    output_path.display()
                );
            } else {
                println!("{}", content);
            }
            Ok(())
        }
        None => anyhow::bail!(
            "pass `bug` or `idea` to compose an upstream AIDA report, or `--recheck` to review filed reports"
        ),
    }
}

fn handle_upstream_report(
    storage: &Storage,
    kind: UpstreamReportKind,
    title: &str,
    description: Option<&str>,
    copy: bool,
    no_file: bool,
) -> Result<()> {
    let title = title.trim();
    if title.is_empty() {
        anyhow::bail!("--title must not be empty");
    }
    let description = resolve_report_description(description)?;
    let store = storage.load()?;
    let project_root = project_root_for_storage(storage);
    let context = upstream_report_context(&store, &project_root);
    let body = compose_upstream_report(kind, title, &description, &context);

    print!("{body}");
    if copy {
        if crate::copy_to_clipboard(&body) {
            eprintln!(
                "{} copied report to clipboard",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
        } else {
            eprintln!(
                "{} no clipboard tool found (wl-copy/xclip/xsel/pbcopy/clip)",
                "Warning:".yellow()
            );
        }
    }
    if !no_file {
        let filed = file_upstream_report(storage, kind, title, &body, &context.version)?;
        eprintln!(
            "Filed local upstream AIDA report {} — {}",
            filed.spec_id.as_deref().unwrap_or("?"),
            filed.title
        );
    }
    Ok(())
}

fn resolve_report_description(description: Option<&str>) -> Result<String> {
    if let Some(description) = description {
        return Ok(description.trim_end().to_string());
    }
    if !std::io::stdin().is_terminal() {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("reading report description from stdin")?;
        return Ok(buf.trim_end().to_string());
    }
    let edited = crate::edit_buffer::edit_spec_in_editor(
        "AIDA-REPORT",
        "Report description",
        "Describe what happened, what you expected, and any reproduction steps.",
    )?;
    Ok(edited.map(|(_, body)| body).unwrap_or_default())
}

// trace:STORY-1004 | ai:codex
fn compose_upstream_report(
    kind: UpstreamReportKind,
    title: &str,
    description: &str,
    context: &UpstreamReportContext,
) -> String {
    format!(
        "Subject: aida: {title}\n\n\
         Title: {title}\n\
         Kind: {kind}\n\n\
         Description:\n{description}\n\n\
         Context:\n\
         - AIDA version: {version}\n\
         - Build SHA: {build_sha}\n\
         - Project: {project_name}\n\
         - Agent vendor: {agent_vendor}\n\
         - Agent enabled: {enabled_agents}\n\
         - OS: {os}\n",
        kind = kind.as_str(),
        version = context.version,
        build_sha = context.build_sha,
        project_name = context.project_name,
        agent_vendor = context.agent_vendor,
        enabled_agents = context.enabled_agents,
        os = context.os,
    )
}

fn upstream_report_context(
    store: &aida_core::models::RequirementsStore,
    project_root: &Path,
) -> UpstreamReportContext {
    let version = env!("CARGO_PKG_VERSION").to_string();
    let build_sha = crate::build_sha_short().unwrap_or_else(|| "unknown".to_string());
    let project_name = if !store.name.trim().is_empty() {
        store.name.clone()
    } else if !store.title.trim().is_empty() {
        store.title.clone()
    } else {
        project_root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string()
    };
    let agent_vendor = aida_core::agents_config::resolve_default_vendor(project_root)
        .unwrap_or_else(|| "claude".to_string());
    let enabled_agents = crate::init_cmd::read_enabled_agent_selection(project_root)
        .map(|s| {
            let mut names = Vec::new();
            if s.claude {
                names.push("claude");
            }
            if s.codex {
                names.push("codex");
            }
            if s.antigravity {
                names.push("antigravity");
            }
            if names.is_empty() {
                "none".to_string()
            } else {
                names.join(",")
            }
        })
        .unwrap_or_else(|| "claude,codex,antigravity".to_string());
    let os = format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH);
    UpstreamReportContext {
        version,
        build_sha,
        project_name,
        agent_vendor,
        enabled_agents,
        os,
    }
}

fn project_root_for_storage(storage: &Storage) -> PathBuf {
    let path = storage.path();
    if path.file_name().and_then(|s| s.to_str()) == Some(".aida-store") {
        return path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    }
    path.parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

fn file_upstream_report(
    storage: &Storage,
    kind: UpstreamReportKind,
    title: &str,
    body: &str,
    version: &str,
) -> Result<Requirement> {
    let mut store = storage.load()?;
    let mut req = Requirement::new(format!("AIDA: {title}"), body.to_string());
    req.req_type = RequirementType::Bug;
    req.status = RequirementStatus::Draft;
    req.owner = crate::get_default_author();
    req.tags.insert(UPSTREAM_AIDA_TAG.to_string());
    req.tags
        .insert(format!("{OBSERVED_VERSION_PREFIX}{version}"));
    req.tags
        .insert(format!("{}{}", REPORT_KIND_PREFIX, kind.as_str()));

    let type_prefix = store.get_type_prefix(&req.req_type);
    store.add_requirement_with_id(req, None, type_prefix.as_deref());
    let written = store
        .requirements
        .last()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("report filing produced no requirement"))?;
    storage.save(&store)?;
    Ok(written)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UpstreamRecheckRow {
    spec_id: String,
    title: String,
    observed_version: String,
    current_version: String,
    older: bool,
}

fn open_upstream_report_rows(
    store: &aida_core::models::RequirementsStore,
    current_version: &str,
) -> Vec<UpstreamRecheckRow> {
    let mut rows: Vec<_> = store
        .requirements
        .iter()
        .filter(|req| is_open_upstream_aida_report(req))
        .map(|req| {
            let observed = observed_version(&req.tags).unwrap_or_else(|| "unknown".to_string());
            UpstreamRecheckRow {
                spec_id: req.spec_id.clone().unwrap_or_else(|| req.id.to_string()),
                title: req.title.clone(),
                older: version_is_older(&observed, current_version),
                observed_version: observed,
                current_version: current_version.to_string(),
            }
        })
        .collect();
    rows.sort_by(|a, b| a.spec_id.cmp(&b.spec_id));
    rows
}

fn is_open_upstream_aida_report(req: &Requirement) -> bool {
    if matches!(
        req.status,
        RequirementStatus::Done | RequirementStatus::Completed | RequirementStatus::Rejected
    ) || req.archived
    {
        return false;
    }
    req.title.starts_with("AIDA:") || req.tags.contains(UPSTREAM_AIDA_TAG)
}

/// BUG-1606: the same open-upstream-report rows as
/// [`open_upstream_report_rows`], computed from the cache's summary rows
/// instead of a full store load.
///
/// The cache's status column holds `custom_status` when one is set, not the
/// base status. So a row whose status string is not a canonical
/// [`RequirementStatus`] name has its base status resolved through
/// `base_status` (one targeted read, and only for report rows). If that
/// lookup fails the report counts as open, which errs toward showing the
/// advisory notice.
// trace:BUG-1606 | ai:claude
fn open_upstream_report_rows_from_summaries(
    summaries: &[aida_core::RequirementSummary],
    current_version: &str,
    base_status: &dyn Fn(&aida_core::RequirementSummary) -> Option<RequirementStatus>,
) -> Vec<UpstreamRecheckRow> {
    let is_closed = |s: &aida_core::RequirementSummary| {
        let status = serde_yaml::from_str::<RequirementStatus>(&s.status)
            .ok()
            .or_else(|| base_status(s));
        matches!(
            status,
            Some(
                RequirementStatus::Done
                    | RequirementStatus::Completed
                    | RequirementStatus::Rejected
            )
        )
    };
    let mut rows: Vec<_> = summaries
        .iter()
        .filter(|s| {
            !s.archived
                && (s.title.starts_with("AIDA:") || s.tags.iter().any(|t| t == UPSTREAM_AIDA_TAG))
                && !is_closed(s)
        })
        .map(|s| {
            let tags: HashSet<String> = s.tags.iter().cloned().collect();
            let observed = observed_version(&tags).unwrap_or_else(|| "unknown".to_string());
            UpstreamRecheckRow {
                spec_id: s.spec_id.clone().unwrap_or_else(|| s.id.to_string()),
                title: s.title.clone(),
                older: version_is_older(&observed, current_version),
                observed_version: observed,
                current_version: current_version.to_string(),
            }
        })
        .collect();
    rows.sort_by(|a, b| a.spec_id.cmp(&b.spec_id));
    rows
}

fn observed_version(tags: &HashSet<String>) -> Option<String> {
    tags.iter()
        .find_map(|t| t.strip_prefix(OBSERVED_VERSION_PREFIX).map(str::to_string))
}

fn version_is_older(observed: &str, current: &str) -> bool {
    let Some(observed) = parse_version_triplet(observed) else {
        return false;
    };
    let Some(current) = parse_version_triplet(current) else {
        return false;
    };
    observed < current
}

fn parse_version_triplet(raw: &str) -> Option<(u64, u64, u64)> {
    let raw = raw.trim().trim_start_matches('v');
    let mut parts = raw.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch_raw = parts.next().unwrap_or("0");
    let patch_digits = patch_raw
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>();
    let patch = patch_digits.parse().ok()?;
    Some((major, minor, patch))
}

fn handle_report_recheck(storage: &Storage) -> Result<()> {
    let store = storage.load()?;
    let rows = open_upstream_report_rows(&store, env!("CARGO_PKG_VERSION"));
    render_recheck_rows(&rows);
    Ok(())
}

fn render_recheck_rows(rows: &[UpstreamRecheckRow]) {
    if rows.is_empty() {
        println!("No open upstream AIDA reports.");
        return;
    }
    println!("Upstream AIDA reports:");
    for row in rows {
        let marker = if row.older { "re-test" } else { "current" };
        println!(
            "{}  observed:{}  current:{}  {}  {}",
            row.spec_id, row.observed_version, row.current_version, marker, row.title
        );
    }
}

pub(crate) fn maybe_print_upstream_recheck_notice(
    storage: &Storage,
    backend: &aida_core::CachedGitBackend,
) {
    let project_root = project_root_for_storage(storage);
    let marker = project_root.join(NOTICE_MARKER);
    let current = env!("CARGO_PKG_VERSION");
    // BUG-1594: one cheap `rev-parse` of the store worktree's HEAD. The
    // report scan below runs only when the binary version OR the store HEAD
    // changed since the last check.
    let git_head = store_head_sha(storage.path());
    let head = git_head
        .clone()
        .or_else(|| store_objects_fingerprint(storage.path()));
    if upstream_notice_is_current(&marker, current, head.as_deref()) {
        return;
    }
    // BUG-1606: a store that is not a git worktree has no HEAD the cache can
    // be checked against, so it keeps the authoritative load (these are small
    // `--file <dir>` stores, not the busy git-canonical store).
    // trace:BUG-1606 | ai:claude
    let Some(git_head) = git_head else {
        let Ok(store) = storage.load() else {
            return;
        };
        print_stale_upstream_notice(&open_upstream_report_rows(&store, current));
        record_upstream_notice_checked(&marker, current, head.as_deref());
        return;
    };
    // BUG-1606: read the report rows from the cache, never a full store load.
    // In a busy store HEAD moves every few minutes, so the first command after
    // each commit (an `aida show`, say) used to parse all ~4,400 objects here;
    // on a loaded spinning disk with a cold page cache that alone took minutes.
    // The notice is advisory: a cache error skips it and leaves the marker
    // unwritten, so the next command checks again.
    // trace:BUG-1606 | ai:claude
    let Ok(summaries) = backend.list_summaries(&aida_core::ListFilter {
        archive: aida_core::ArchiveFilter::NonArchivedOnly,
        defer: aida_core::DeferFilter::Both,
        ..Default::default()
    }) else {
        return;
    };
    let objects_root = storage.path().join("objects");
    let base_status = |s: &aida_core::RequirementSummary| {
        let spec_id = s.spec_id.as_deref()?;
        let req = aida_core::object_store::read_object(&objects_root, spec_id).ok()?;
        (req.id == s.id).then_some(req.status)
    };
    print_stale_upstream_notice(&open_upstream_report_rows_from_summaries(
        &summaries,
        current,
        &base_status,
    ));
    // BUG-1594: record the (version, store HEAD) pair this check covered,
    // whether or not anything was stale. This notice runs before EVERY
    // git-backend command; the marker used to be written only when a stale
    // report was found, so in the common no-stale case every `aida show`,
    // `aida list`, … paid a full store load. Keying on the store HEAD as well
    // as the version means a report that arrives later (filed locally or
    // brought in by `aida pull`) moves the HEAD and re-arms the check.
    //
    // BUG-1606: the rows came from the cache, and a read serves the last
    // committed snapshot WITHOUT catching up when another live process holds
    // the cache write lock (BUG-664). The marker claims "checked at
    // `git_head`", so it is written only when the snapshot we read is stamped
    // at exactly that HEAD. Otherwise the rows above are still printed and
    // the marker stays unwritten, so the next command checks again.
    // trace:BUG-1594 trace:BUG-1606 | ai:claude
    let snapshot_head = backend.cache().source_head_sha().ok().flatten();
    if snapshot_head.as_deref() == Some(git_head.as_str()) {
        record_upstream_notice_checked(&marker, current, Some(&git_head));
    }
}

/// Print the one-line notice when any open upstream report predates this
/// binary.
// trace:BUG-1606 | ai:claude
fn print_stale_upstream_notice(rows: &[UpstreamRecheckRow]) {
    let stale = rows.iter().filter(|row| row.older).count();
    if stale > 0 {
        eprintln!("{stale} upstream aida report(s) predate this binary - aida report --recheck");
    }
}

/// Read the requirements store's HEAD commit, or `None` when it is not a
/// readable git worktree.
// trace:BUG-1594 | ai:claude
fn store_head_sha(store_path: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(store_path)
        .args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

/// The marker's content for a check that covered `version` at store `head`.
// trace:BUG-1594 | ai:claude
fn upstream_notice_marker_contents(version: &str, head: Option<&str>) -> String {
    match head {
        Some(sha) => format!("{version}\n{sha}\n"),
        None => format!("{version}\n"),
    }
}

/// True only when the marker records BOTH this binary version and a store
/// HEAD equal to `head`. An unreadable HEAD (`None`) is never current.
// trace:BUG-1594 | ai:claude
fn upstream_notice_is_current(marker: &Path, version: &str, head: Option<&str>) -> bool {
    let Some(head) = head else {
        return false;
    };
    let Ok(contents) = std::fs::read_to_string(marker) else {
        return false;
    };
    let mut lines = contents.lines().map(str::trim);
    lines.next() == Some(version) && lines.next() == Some(head)
}

/// A store that is not a git worktree (a plain `--file <dir>` store) has no
/// HEAD to key on. Fingerprint its object files instead — count plus newest
/// mtime, from file metadata only (no YAML parsing) — so a changed store
/// still re-arms the check without paying the full load on every command.
// trace:BUG-1594 | ai:claude
fn store_objects_fingerprint(store_path: &Path) -> Option<String> {
    fn walk(dir: &Path, count: &mut u64, newest: &mut u128) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                walk(&entry.path(), count, newest);
            } else {
                *count += 1;
                if let Some(ns) = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos())
                {
                    *newest = (*newest).max(ns);
                }
            }
        }
    }
    let objects = store_path.join("objects");
    if !objects.is_dir() {
        return None;
    }
    let (mut count, mut newest) = (0u64, 0u128);
    walk(&objects, &mut count, &mut newest);
    Some(format!("files:{count}:{newest}"))
}

/// Record the check. Never CREATES the marker's `.aida/` directory: that
/// directory's presence is how the cache path is resolved
/// (`CachedGitBackend::default_cache_path` walks up looking for `.aida/`), so
/// conjuring it here would move the cache between the first command and the
/// next, stranding every row written before it (a first `aida add` to a fresh
/// `--file` store then vanished from `aida list`). Without the directory the
/// check simply runs again next time.
// trace:BUG-1594 | ai:claude
fn record_upstream_notice_checked(marker: &Path, version: &str, head: Option<&str>) {
    if !marker.parent().is_some_and(Path::is_dir) {
        return;
    }
    let _ = std::fs::write(marker, upstream_notice_marker_contents(version, head));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command};
    use aida_core::models::RequirementsStore;
    use clap::Parser;
    use tempfile::TempDir;

    fn context() -> UpstreamReportContext {
        UpstreamReportContext {
            version: "1.2.3".to_string(),
            build_sha: "abc123".to_string(),
            project_name: "demo".to_string(),
            agent_vendor: "codex".to_string(),
            enabled_agents: "codex".to_string(),
            os: "linux/x86_64".to_string(),
        }
    }

    #[test]
    fn upstream_report_composer_prints_subject_and_context() {
        let text = compose_upstream_report(
            UpstreamReportKind::Bug,
            "picker lists claude",
            "Steps\nExpected codex only.",
            &context(),
        );

        assert!(text.starts_with("Subject: aida: picker lists claude\n\n"));
        assert!(text.contains("Title: picker lists claude"));
        assert!(text.contains("Kind: bug"));
        assert!(text.contains("- AIDA version: 1.2.3"));
        assert!(text.contains("- Build SHA: abc123"));
        assert!(text.contains("- Agent vendor: codex"));
        assert!(text.contains("- Agent enabled: codex"));
    }

    #[test]
    fn report_cli_parses_bug_idea_and_recheck_forms() {
        let cli = Cli::try_parse_from([
            "aida",
            "report",
            "bug",
            "--title",
            "bad picker",
            "--description",
            "body",
            "--copy",
        ])
        .unwrap();
        let Command::Report {
            recheck: false,
            command:
                Some(ReportCommand::Bug {
                    title,
                    description,
                    copy: true,
                    no_file: false,
                }),
        } = cli.command
        else {
            panic!("expected report bug");
        };
        assert_eq!(title, "bad picker");
        assert_eq!(description.as_deref(), Some("body"));

        let cli =
            Cli::try_parse_from(["aida", "report", "idea", "--title", "better flow"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Report {
                recheck: false,
                command: Some(ReportCommand::Idea { .. })
            }
        ));

        let cli = Cli::try_parse_from(["aida", "report", "--recheck"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Report {
                recheck: true,
                command: None
            }
        ));
    }

    #[test]
    fn filing_report_creates_local_upstream_bug_with_tags() {
        let dir = TempDir::new().unwrap();
        let storage = Storage::new(dir.path().join("requirements.yaml"));
        storage.save(&RequirementsStore::new()).unwrap();

        let written = file_upstream_report(
            &storage,
            UpstreamReportKind::Idea,
            "better reports",
            "Subject: aida: better reports\n\nbody\n",
            "1.2.3",
        )
        .unwrap();

        assert_eq!(written.title, "AIDA: better reports");
        assert_eq!(written.req_type, RequirementType::Bug);
        assert!(written.tags.contains(UPSTREAM_AIDA_TAG));
        assert!(written.tags.contains("observed-version:1.2.3"));
        assert!(written.tags.contains("report-kind:idea"));
    }

    #[test]
    fn recheck_lists_open_reports_and_ignores_closed() {
        let mut store = RequirementsStore::new();
        let mut old = Requirement::new("AIDA: old".into(), "body".into());
        old.spec_id = Some("BUG-1".to_string());
        old.tags.insert(UPSTREAM_AIDA_TAG.to_string());
        old.tags.insert("observed-version:1.0.0".to_string());
        let mut current = Requirement::new("AIDA: current".into(), "body".into());
        current.spec_id = Some("BUG-2".to_string());
        current.tags.insert(UPSTREAM_AIDA_TAG.to_string());
        current.tags.insert("observed-version:2.0.0".to_string());
        let mut closed = Requirement::new("AIDA: closed".into(), "body".into());
        closed.spec_id = Some("BUG-3".to_string());
        closed.status = RequirementStatus::Completed;
        closed.tags.insert(UPSTREAM_AIDA_TAG.to_string());
        closed.tags.insert("observed-version:1.0.0".to_string());
        store.requirements = vec![old, current, closed];

        let rows = open_upstream_report_rows(&store, "2.0.0");

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].spec_id, "BUG-1");
        assert!(rows[0].older);
        assert_eq!(rows[1].spec_id, "BUG-2");
        assert!(!rows[1].older);
    }

    // BUG-1606: the per-command notice now reads cache summaries instead of
    // loading every object. The cache-derived rows must match the load-derived
    // rows: open reports in, closed and archived ones out, the same `older`
    // verdicts. trace:BUG-1606 | ai:claude
    #[test]
    fn bug_1606_notice_rows_from_cache_match_full_load_rows() {
        use aida_core::DatabaseBackend;

        let tmp = TempDir::new().unwrap();
        let store_root = tmp.path().join(".aida-store");
        std::fs::create_dir_all(&store_root).unwrap();
        aida_core::git_ops::init(&store_root).unwrap();
        aida_core::git_ops::configure_user(&store_root, "Test", "test@example.com").unwrap();
        let cache_path = tmp.path().join(".aida").join("cache.db");
        let backend = aida_core::CachedGitBackend::open(&store_root, &cache_path).unwrap();

        let report = |spec: &str, title: &str, version: &str| {
            let mut r = Requirement::new(title.into(), "body".into());
            r.spec_id = Some(spec.to_string());
            r.tags.insert(UPSTREAM_AIDA_TAG.to_string());
            r.tags.insert(format!("observed-version:{version}"));
            r
        };
        backend
            .add_requirement(report("BUG-1", "old report", "1.0.0"))
            .unwrap();
        // Title-only report (no upstream tag), still open.
        let mut titled = Requirement::new("AIDA: titled".into(), "body".into());
        titled.spec_id = Some("BUG-2".to_string());
        backend.add_requirement(titled).unwrap();
        let mut closed = report("BUG-3", "closed report", "1.0.0");
        closed.status = RequirementStatus::Completed;
        backend.add_requirement(closed).unwrap();
        let mut archived = report("BUG-4", "archived report", "1.0.0");
        archived.archived = true;
        backend.add_requirement(archived).unwrap();
        let mut not_a_report = Requirement::new("unrelated".into(), "body".into());
        not_a_report.spec_id = Some("BUG-5".to_string());
        backend.add_requirement(not_a_report).unwrap();

        let from_load = open_upstream_report_rows(&backend.load().unwrap(), "2.0.0");
        let summaries = backend
            .list_summaries(&aida_core::ListFilter {
                archive: aida_core::ArchiveFilter::NonArchivedOnly,
                defer: aida_core::DeferFilter::Both,
                ..Default::default()
            })
            .unwrap();
        let from_cache = open_upstream_report_rows_from_summaries(&summaries, "2.0.0", &|_| None);

        assert_eq!(from_cache, from_load);
        let ids: Vec<&str> = from_cache.iter().map(|r| r.spec_id.as_str()).collect();
        assert_eq!(ids, vec!["BUG-1", "BUG-2"]);
        assert!(from_cache[0].older);
        assert!(
            !from_cache[1].older,
            "an unknown observed version is not older"
        );
    }

    // BUG-1606: the cache's status column carries `custom_status` when set,
    // so a closed report with a custom label must be resolved to its base
    // status, not counted as open. An unresolvable one errs toward noticing.
    // trace:BUG-1606 | ai:claude
    #[test]
    fn bug_1606_custom_status_report_uses_base_status() {
        use aida_core::DatabaseBackend;

        let tmp = TempDir::new().unwrap();
        let store_root = tmp.path().join(".aida-store");
        std::fs::create_dir_all(&store_root).unwrap();
        aida_core::git_ops::init(&store_root).unwrap();
        aida_core::git_ops::configure_user(&store_root, "Test", "test@example.com").unwrap();
        let cache_path = tmp.path().join(".aida").join("cache.db");
        let backend = aida_core::CachedGitBackend::open(&store_root, &cache_path).unwrap();
        let mut shipped = Requirement::new("AIDA: shipped".into(), "body".into());
        shipped.spec_id = Some("BUG-1".to_string());
        shipped.status = RequirementStatus::Completed;
        shipped.custom_status = Some("Shipped".to_string());
        shipped.tags.insert("observed-version:1.0.0".to_string());
        backend.add_requirement(shipped).unwrap();

        let summaries = backend
            .list_summaries(&aida_core::ListFilter::default())
            .unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(
            summaries[0].status, "Shipped",
            "fixture: the cache column carries the custom status"
        );

        let closed = open_upstream_report_rows_from_summaries(&summaries, "2.0.0", &|_| {
            Some(RequirementStatus::Completed)
        });
        assert!(
            closed.is_empty(),
            "base status Completed is closed: {closed:?}"
        );

        let unknown = open_upstream_report_rows_from_summaries(&summaries, "2.0.0", &|_| None);
        assert_eq!(
            unknown.len(),
            1,
            "an unresolvable custom status counts as open"
        );
    }

    // BUG-1606: the per-command notice reads the cache, and a read serves the
    // last committed snapshot without catching up while another live process
    // holds the cache write lock (BUG-664). Then the rows are older than the
    // store HEAD, so the "checked at HEAD" marker must NOT be written. Once
    // the cache can catch up, the marker is written for that HEAD.
    // trace:BUG-1606 | ai:claude
    #[test]
    fn bug_1606_notice_marker_not_written_from_a_stale_cache_snapshot() {
        use aida_core::DatabaseBackend;

        let tmp = TempDir::new().unwrap();
        let store_root = tmp.path().join(".aida-store");
        std::fs::create_dir_all(&store_root).unwrap();
        aida_core::git_ops::init(&store_root).unwrap();
        aida_core::git_ops::configure_user(&store_root, "Test", "test@example.com").unwrap();
        let cache_path = tmp.path().join(".aida").join("cache.db");
        let backend = aida_core::CachedGitBackend::open(&store_root, &cache_path).unwrap();
        let mut first = Requirement::new("AIDA: first".into(), "body".into());
        first.spec_id = Some("BUG-1".to_string());
        backend.add_requirement(first).unwrap();

        // Another agent commits to the store: the cache is now behind HEAD.
        {
            let external = aida_core::GitBackend::new(&store_root).unwrap();
            let mut second = Requirement::new("AIDA: second".into(), "body".into());
            second.spec_id = Some("BUG-2".to_string());
            second.tags.insert(UPSTREAM_AIDA_TAG.to_string());
            second.tags.insert("observed-version:0.0.1".to_string());
            external.add_requirement(second).unwrap();
        }
        let head = store_head_sha(&store_root).unwrap();

        // A live foreign writer (pid 1) holds the cache write lock, so the
        // read path serves the old snapshot instead of catching up.
        let lock_info = std::path::PathBuf::from(format!("{}.lock-info", cache_path.display()));
        let info = aida_core::CacheLockInfo {
            pid: 1,
            command: "test-foreign-writer".to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            user: "test".to_string(),
            ..Default::default()
        };
        std::fs::write(&lock_info, serde_json::to_string(&info).unwrap()).unwrap();

        let storage = Storage::new(&store_root);
        let marker = tmp.path().join(NOTICE_MARKER);
        maybe_print_upstream_recheck_notice(&storage, &backend);
        assert_ne!(
            backend.cache().source_head_sha().unwrap().as_deref(),
            Some(head.as_str()),
            "fixture: the catch-up must have been skipped"
        );
        assert!(
            !marker.exists(),
            "a stale snapshot must not record the marker for the new HEAD"
        );

        // The foreign writer is gone: the read catches up, and the marker is
        // recorded for exactly this HEAD.
        std::fs::remove_file(&lock_info).unwrap();
        maybe_print_upstream_recheck_notice(&storage, &backend);
        assert_eq!(
            backend.cache().source_head_sha().unwrap().as_deref(),
            Some(head.as_str())
        );
        assert!(upstream_notice_is_current(
            &marker,
            env!("CARGO_PKG_VERSION"),
            Some(&head)
        ));
    }

    // BUG-1594: the marker re-arms the check when the store HEAD moves (a
    // report pulled in later), stays quiet on an unchanged store, and an
    // unreadable HEAD always fails toward checking. trace:BUG-1594 | ai:claude
    #[test]
    fn bug_1594_notice_marker_rearms_when_store_head_moves() {
        let tmp = TempDir::new().unwrap();
        let store = tmp.path().join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(&store)
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q", "-b", "aida-store"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "T"]);
        git(&["commit", "-q", "--allow-empty", "-m", "add TASK-1"]);
        let marker = tmp
            .path()
            .join(".aida")
            .join("upstream-report-notice-version");
        // No `.aida/` yet: recording must not create it (it steers the cache
        // path), so nothing is written and the check stays armed.
        record_upstream_notice_checked(&marker, "1.0.0", Some("abc"));
        assert!(!tmp.path().join(".aida").exists());
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();

        let head1 = store_head_sha(&store).expect("store head");
        assert!(!upstream_notice_is_current(&marker, "1.0.0", Some(&head1)));
        record_upstream_notice_checked(&marker, "1.0.0", Some(&head1));
        assert!(upstream_notice_is_current(&marker, "1.0.0", Some(&head1)));
        // A new binary version re-arms the check.
        assert!(!upstream_notice_is_current(&marker, "1.0.1", Some(&head1)));

        // A new store commit (e.g. a report arriving via `aida pull`).
        git(&["commit", "-q", "--allow-empty", "-m", "add BUG-9"]);
        let head2 = store_head_sha(&store).expect("store head");
        assert_ne!(head1, head2);
        assert!(!upstream_notice_is_current(&marker, "1.0.0", Some(&head2)));

        // Unreadable HEAD: never current, and recording without a SHA does
        // not make it current either.
        assert_eq!(store_head_sha(&tmp.path().join("missing")), None);
        record_upstream_notice_checked(&marker, "1.0.0", None);
        assert!(!upstream_notice_is_current(&marker, "1.0.0", None));
        assert!(!upstream_notice_is_current(&marker, "1.0.0", Some(&head2)));
    }

    // BUG-1594: a non-git store still gets a cheap, change-sensitive key.
    // trace:BUG-1594 | ai:claude
    #[test]
    fn bug_1594_non_git_store_fingerprint_moves_when_objects_change() {
        let tmp = TempDir::new().unwrap();
        let store = tmp.path().join("store");
        assert_eq!(store_objects_fingerprint(&store), None);
        let dir = store.join("objects").join("FR").join("000");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("FR-1.yaml"), "a").unwrap();
        let one = store_objects_fingerprint(&store).unwrap();
        assert_eq!(one, store_objects_fingerprint(&store).unwrap());
        std::fs::write(dir.join("FR-2.yaml"), "b").unwrap();
        assert_ne!(one, store_objects_fingerprint(&store).unwrap());
    }
}
