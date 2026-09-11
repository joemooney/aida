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

pub(crate) fn maybe_print_upstream_recheck_notice(storage: &Storage) {
    let project_root = project_root_for_storage(storage);
    let marker = project_root.join(NOTICE_MARKER);
    let current = env!("CARGO_PKG_VERSION");
    if std::fs::read_to_string(&marker)
        .map(|s| s.trim() == current)
        .unwrap_or(false)
    {
        return;
    }
    let Ok(store) = storage.load() else {
        return;
    };
    let stale = open_upstream_report_rows(&store, current)
        .into_iter()
        .filter(|row| row.older)
        .count();
    if stale == 0 {
        return;
    }
    eprintln!("{stale} upstream aida report(s) predate this binary - aida report --recheck");
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(marker, current);
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
}
