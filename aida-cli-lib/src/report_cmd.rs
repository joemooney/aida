//! `aida report` command handler.
//!
//! Extracted verbatim from `main.rs` (SPIKE-78, pure movement). Renders the
//! AI-integration report (markdown/HTML) via `aida_core::ReportGenerator`.

use anyhow::Result;
use colored::Colorize;

use aida_core::{ReportFormat, ReportGenerator, Storage};

use crate::cli::ReportCommand;

pub(crate) fn handle_report_command(
    cmd: &ReportCommand,
    storage: &Storage,
    storage_path: &str,
) -> Result<()> {
    match cmd {
        ReportCommand::AiIntegration {
            format,
            output,
            project_root,
            include_scaffold,
        } => {
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
        }
    }

    Ok(())
}
