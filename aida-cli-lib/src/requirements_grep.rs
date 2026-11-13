// trace:TASK-3428 | ai:codex
use crate::requirements_cli::{print_grep_match, search_field, search_multiline_field, GrepMatch};
use crate::Storage;
use anyhow::{Context, Result};
use colored::Colorize;
use std::collections::HashSet;

/// Search requirements for a pattern
#[allow(clippy::too_many_arguments)]
pub(crate) fn grep_requirements(
    storage: &Storage,
    pattern: &str,
    ignore_case: bool,
    extended_regex: bool,
    after_context: usize,
    before_context: usize,
    context: Option<usize>,
    field_filter: Option<&str>,
    status_filter: Option<&str>,
    type_filter: Option<&str>,
    feature_filter: Option<&str>,
    files_with_matches: bool,
    count_only: bool,
    invert_match: bool,
) -> Result<()> {
    use regex::RegexBuilder;

    let store = storage.load_for_read()?;

    // Build the regex pattern
    let regex = if extended_regex {
        RegexBuilder::new(pattern)
            .case_insensitive(ignore_case)
            .build()
            .context("Invalid regex pattern")?
    } else {
        // Escape special regex characters for literal search
        let escaped = regex::escape(pattern);
        RegexBuilder::new(&escaped)
            .case_insensitive(ignore_case)
            .build()
            .context("Invalid pattern")?
    };

    // Parse field filter
    let fields: HashSet<&str> = if let Some(f) = field_filter {
        f.split(',').map(|s| s.trim()).collect()
    } else {
        [
            "title",
            "description",
            "comments",
            "tags",
            "owner",
            "feature",
            "spec_id",
        ]
        .iter()
        .copied()
        .collect()
    };

    // Context lines (C overrides A and B)
    let ctx_before = context.unwrap_or(before_context);
    let ctx_after = context.unwrap_or(after_context);

    let mut total_matches = 0;
    let mut matching_reqs = 0;

    for req in &store.requirements {
        // Apply filters
        if let Some(status_str) = status_filter {
            let req_status = req.status.to_string().to_lowercase();
            if !req_status.contains(&status_str.to_lowercase()) {
                continue;
            }
        }

        if let Some(type_str) = type_filter {
            let req_type = req.req_type.to_string().to_lowercase();
            if !req_type.contains(&type_str.to_lowercase()) {
                continue;
            }
        }

        if let Some(feature_str) = feature_filter {
            if !req
                .feature
                .to_lowercase()
                .contains(&feature_str.to_lowercase())
            {
                continue;
            }
        }

        // Collect matches from all fields
        let mut matches: Vec<GrepMatch> = Vec::new();

        // Search title
        if fields.contains("title") {
            if let Some(m) = search_field(&regex, "title", &req.title, ctx_before, ctx_after) {
                matches.push(m);
            }
        }

        // Search description
        if fields.contains("description") {
            for m in search_multiline_field(
                &regex,
                "description",
                &req.description,
                ctx_before,
                ctx_after,
            ) {
                matches.push(m);
            }
        }

        // Search spec_id
        if fields.contains("spec_id") {
            if let Some(spec_id) = &req.spec_id {
                if let Some(m) = search_field(&regex, "spec_id", spec_id, ctx_before, ctx_after) {
                    matches.push(m);
                }
            }
        }

        // Search owner
        if fields.contains("owner") {
            if let Some(m) = search_field(&regex, "owner", &req.owner, ctx_before, ctx_after) {
                matches.push(m);
            }
        }

        // Search feature
        if fields.contains("feature") {
            if let Some(m) = search_field(&regex, "feature", &req.feature, ctx_before, ctx_after) {
                matches.push(m);
            }
        }

        // Search tags
        if fields.contains("tags") {
            for tag in &req.tags {
                if let Some(m) = search_field(&regex, "tags", tag, ctx_before, ctx_after) {
                    matches.push(m);
                }
            }
        }

        // Search comments
        if fields.contains("comments") {
            for comment in &req.comments {
                for m in search_multiline_field(
                    &regex,
                    &format!("comment:{}", comment.author),
                    &comment.content,
                    ctx_before,
                    ctx_after,
                ) {
                    matches.push(m);
                }
            }
        }

        let has_matches = !matches.is_empty();
        let should_show = if invert_match {
            !has_matches
        } else {
            has_matches
        };

        if should_show {
            matching_reqs += 1;
            let match_count = matches.len();
            total_matches += match_count;

            let id_string = req.id.to_string();
            let spec_id = req.spec_id.as_deref().unwrap_or(&id_string);

            if files_with_matches {
                // Just print the SPEC-ID
                println!("{}", spec_id.cyan());
            } else if count_only {
                // Print count for this requirement
                println!("{}: {}", spec_id.cyan(), match_count);
            } else if invert_match {
                // For invert match, just show SPEC-ID and title
                println!("{}: {}", spec_id.cyan(), req.title);
            } else {
                // Full output with matches
                println!("{}: {}", spec_id.cyan().bold(), req.title);
                for m in matches {
                    print_grep_match(&m);
                }
                println!();
            }
        }
    }

    // Summary
    if !files_with_matches && !count_only {
        if matching_reqs == 0 {
            if invert_match {
                println!("{}", "All requirements matched the pattern.".yellow());
            } else {
                println!("{}", "No matches found.".yellow());
            }
        } else {
            println!(
                "{} match(es) in {} requirement(s)",
                total_matches.to_string().green(),
                matching_reqs.to_string().green()
            );
        }
    }

    Ok(())
}
