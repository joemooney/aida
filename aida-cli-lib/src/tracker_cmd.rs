//! External issue-tracker bridge command handlers: `aida jira`, `aida github`,
//! `aida gitlab`. Lifted verbatim from `main.rs` (SPIKE-78 pure-movement
//! extraction). Shared forge/remote helpers (e.g. `truncate_str`) remain in
//! `main.rs` and are reached via `crate::`.

use crate::*;
use anyhow::Result;
use colored::Colorize;

// trace:ARCH-github-integration | ai:claude
/// Handle GitHub integration commands
// trace:ARCH-jira-integration | ai:claude
pub(crate) fn handle_jira_command(cmd: &JiraCommand, storage: &Storage) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;

    match cmd {
        JiraCommand::Config {
            url,
            project,
            email,
            show,
            show_mapping,
        } => {
            let mut config = aida_core::JiraConfig::load()?;

            if *show {
                println!("{}", "Jira Configuration".bold());
                println!("{}", "─".repeat(40));
                println!(
                    "Instance:  {}",
                    if config.instance_url.is_empty() {
                        "(not set)"
                    } else {
                        &config.instance_url
                    }
                );
                println!(
                    "Project:   {}",
                    if config.project_key.is_empty() {
                        "(not set)"
                    } else {
                        &config.project_key
                    }
                );
                println!(
                    "Email:     {}",
                    if config.user_email.is_empty() {
                        "(not set)"
                    } else {
                        &config.user_email
                    }
                );
                println!(
                    "Token:     {}",
                    if config.effective_token().is_ok() {
                        "configured (AIDA_JIRA_TOKEN)"
                    } else {
                        "not configured"
                    }
                );
                println!("Enabled:   {}", config.enabled);
                return Ok(());
            }

            if *show_mapping {
                println!("{}", "Field Mapping Spec".bold());
                println!("{}", "─".repeat(50));
                println!("\n{}", "Type Mapping (AIDA → Jira):".bold());
                for (aida, jira) in &config.mapping.types {
                    println!("  {:<20} → {}", aida, jira);
                }
                println!("\n{}", "Status Mapping (AIDA → Jira):".bold());
                for (aida, jira) in &config.mapping.statuses {
                    println!("  {:<20} → {}", aida, jira);
                }
                println!("\n{}", "Priority Mapping (AIDA → Jira):".bold());
                for (aida, jira) in &config.mapping.priorities {
                    println!("  {:<20} → {}", aida, jira);
                }
                println!("\n{}", "Reverse Type Mapping (Jira → AIDA):".bold());
                for (jira, aida) in &config.mapping.reverse_types {
                    println!("  {:<20} → {}", jira, aida);
                }
                println!("\n{}", "Reverse Status Mapping (Jira → AIDA):".bold());
                for (jira, aida) in &config.mapping.reverse_statuses {
                    println!("  {:<20} → {}", jira, aida);
                }
                println!(
                    "\nEdit mapping at: {}",
                    aida_core::JiraConfig::config_path()?.display()
                );
                return Ok(());
            }

            if let Some(u) = url {
                config.instance_url = u.clone();
            }
            if let Some(p) = project {
                config.project_key = p.clone();
            }
            if let Some(e) = email {
                config.user_email = e.clone();
            }

            config.save()?;
            println!(
                "{} Jira configuration saved.",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
        }
        JiraCommand::Test => {
            let config = aida_core::JiraConfig::load()?;
            let client = aida_core::JiraClient::new(config)?;
            let project = rt.block_on(client.test_connection())?;

            println!(
                "{} Connected to Jira",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
            println!("  Project: {} ({})", project.name, project.key);
        }
        JiraCommand::List { jql, limit } => {
            let config = aida_core::JiraConfig::load()?;
            let client = aida_core::JiraClient::new(config.clone())?;

            let results = if let Some(query) = jql {
                rt.block_on(client.search(query, *limit))?
            } else {
                rt.block_on(client.list_issues(*limit))?
            };

            if results.issues.is_empty() {
                println!("No issues found.");
            } else {
                println!(
                    "{:<12} {:<10} {:<12} {:<10} Summary",
                    "Key", "Type", "Status", "Priority"
                );
                println!("{}", "─".repeat(75));
                for issue in &results.issues {
                    println!(
                        "{:<12} {:<10} {:<12} {:<10} {}",
                        issue.key,
                        truncate_str(issue.issue_type_name(), 9),
                        truncate_str(issue.status_name(), 11),
                        truncate_str(issue.priority_name(), 9),
                        truncate_str(issue.summary(), 40),
                    );
                }
                println!("\n{} issues", results.issues.len());
            }
        }
        JiraCommand::Show { key } => {
            let config = aida_core::JiraConfig::load()?;
            let client = aida_core::JiraClient::new(config)?;
            let issue = rt.block_on(client.get_issue(key))?;

            println!("{}: {}", "Key".bold(), issue.key);
            println!("{}: {}", "Summary".bold(), issue.summary());
            println!("{}: {}", "Type".bold(), issue.issue_type_name());
            println!("{}: {}", "Status".bold(), issue.status_name());
            println!("{}: {}", "Priority".bold(), issue.priority_name());
            if let Some(assignee) = issue.assignee_name() {
                println!("{}: {}", "Assignee".bold(), assignee);
            }
            if !issue.labels().is_empty() {
                println!("{}: {}", "Labels".bold(), issue.labels().join(", "));
            }
            let desc = issue.description_text();
            if !desc.is_empty() {
                println!("\n{}", desc);
            }
        }
        JiraCommand::Push { id } => {
            let config = aida_core::JiraConfig::load()?;
            let client = aida_core::JiraClient::new(config.clone())?;

            let store = storage.load()?;
            let req = store
                .requirements
                .iter()
                .find(|r| r.matches_id(id))
                .ok_or_else(|| not_found::requirement_not_found(id, Some(storage.path())))?;

            let display_id = req.display_id();
            let type_name = config.map_type(&format!("{:?}", req.req_type));
            let priority_name = config.map_priority(&req.effective_priority());

            let mut labels = Vec::new();
            labels.push(format!("aida:{}", display_id));
            for tag in &req.tags {
                labels.push(format!("aida:{}", tag));
            }

            let description_text = format!(
                "{}\n\n---\nAIDA: {} | UUID: {}",
                req.description, display_id, req.id
            );

            let request = aida_core::JiraCreateIssueRequest {
                fields: aida_core::JiraCreateIssueFields {
                    project: aida_core::JiraProjectRef {
                        key: config.project_key.clone(),
                    },
                    summary: format!("[{}] {}", display_id, req.title),
                    description: Some(aida_core::text_to_adf(&description_text)),
                    issuetype: aida_core::JiraIssueTypeRef { name: type_name },
                    priority: Some(aida_core::JiraPriorityRef {
                        name: priority_name,
                    }),
                    assignee: None,
                    labels,
                },
            };

            let created = rt.block_on(client.create_issue(&request))?;
            println!(
                "{} Created Jira issue {} for {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                created.key.white().bold(),
                display_id
            );
            println!("  URL: {}/browse/{}", config.instance_url, created.key);
        }
        JiraCommand::Sync { apply } => {
            let config = aida_core::JiraConfig::load()?;
            let client = aida_core::JiraClient::new(config.clone())?;

            let store = storage.load()?;

            // Find AIDA requirements linked to Jira issues
            // Link detection: title starts with [DEV-N] or [PROJ-N], or has jira: tags
            let linked: Vec<(&Requirement, String)> = store
                .requirements
                .iter()
                .filter_map(|r| {
                    // Check [KEY-N] prefix in title
                    if r.title.starts_with('[') {
                        if let Some(end) = r.title.find(']') {
                            let key = &r.title[1..end];
                            if key.contains('-')
                                && key
                                    .split('-')
                                    .next_back()
                                    .map(|n| n.parse::<u64>().is_ok())
                                    .unwrap_or(false)
                            {
                                return Some((r, key.to_string()));
                            }
                        }
                    }
                    // Check jira: tags
                    for tag in &r.tags {
                        if let Some(key) = tag.strip_prefix("jira:key:") {
                            return Some((r, key.to_string()));
                        }
                    }
                    None
                })
                .collect();

            // Also find Jira issues with aida: labels that aren't linked from AIDA side
            // (TODO: tighten JQL to filter on aida:label_prefix once verified)
            let jira_issues = rt
                .block_on(client.search(
                    &format!("project = {} ORDER BY updated DESC", config.project_key),
                    50,
                ))
                .unwrap_or_else(|_| aida_core::JiraSearchResults {
                    issues: Vec::new(),
                    next_page_token: None,
                    is_last: Some(true),
                    total: 0,
                });

            if linked.is_empty() && jira_issues.issues.is_empty() {
                println!("No linked items found.");
                println!("Link with: aida jira push FR-001 (or aida jira pull)");
                return Ok(());
            }

            println!("{}", "Jira Sync Status".bold());
            println!("{}", "─".repeat(70));

            let mut in_sync = 0;
            let mut drifted = 0;
            let mut errors = 0;

            for (req, jira_key) in &linked {
                match rt.block_on(client.get_issue(jira_key)) {
                    Ok(issue) => {
                        let mut diffs = Vec::new();

                        // Compare title (strip [KEY] prefix for comparison)
                        let aida_title = req
                            .title
                            .strip_prefix(&format!("[{}] ", jira_key))
                            .unwrap_or(&req.title);
                        if aida_title != issue.summary() {
                            diffs.push(format!(
                                "  title: AIDA='{}' Jira='{}'",
                                truncate_str(aida_title, 25),
                                truncate_str(issue.summary(), 25)
                            ));
                        }

                        // Compare status using mapping
                        let expected_jira_status = config.map_status(&req.effective_status());
                        let actual_jira_status = issue.status_name();
                        if expected_jira_status != actual_jira_status {
                            diffs.push(format!(
                                "  status: AIDA={} (→{}) Jira={}",
                                req.effective_status(),
                                expected_jira_status,
                                actual_jira_status
                            ));
                        }

                        // Compare priority
                        let expected_priority = config.map_priority(&req.effective_priority());
                        let actual_priority = issue.priority_name();
                        if expected_priority != actual_priority {
                            diffs.push(format!(
                                "  priority: AIDA={} (→{}) Jira={}",
                                req.effective_priority(),
                                expected_priority,
                                actual_priority
                            ));
                        }

                        let spec_id = req.display_id();
                        if diffs.is_empty() {
                            in_sync += 1;
                            println!(
                                "{} {:<12} ↔ {:<10} {} — in sync",
                                crate::glyph(crate::glyphs::Glyph::Check).green(),
                                spec_id,
                                jira_key,
                                truncate_str(aida_title, 35)
                            );
                        } else {
                            drifted += 1;
                            println!(
                                "{} {:<12} ↔ {:<10} {} — DRIFTED",
                                "△".yellow(),
                                spec_id,
                                jira_key,
                                truncate_str(aida_title, 35)
                            );
                            for d in &diffs {
                                println!("    {}", d);
                            }
                        }
                    }
                    Err(e) => {
                        errors += 1;
                        println!(
                            "{} {} ↔ {} — error: {}",
                            crate::glyph(crate::glyphs::Glyph::Cross).red(),
                            req.display_id(),
                            jira_key,
                            e
                        );
                    }
                }
            }

            println!();
            println!(
                "{} in sync, {} drifted, {} errors (of {} linked)",
                in_sync,
                drifted,
                errors,
                linked.len()
            );

            if drifted > 0 && !apply {
                println!("\nUse --apply to push AIDA state to Jira.");
            }

            if *apply && drifted > 0 {
                println!("\nApplying changes...");
                for (req, jira_key) in &linked {
                    let aida_title = req
                        .title
                        .strip_prefix(&format!("[{}] ", jira_key))
                        .unwrap_or(&req.title);

                    let fields = serde_json::json!({
                        "summary": aida_title,
                        "priority": { "name": config.map_priority(&req.effective_priority()) },
                    });

                    match rt.block_on(client.update_issue(jira_key, &fields)) {
                        Ok(_) => println!(
                            "  {} Updated {}",
                            crate::glyph(crate::glyphs::Glyph::Check).green(),
                            jira_key
                        ),
                        Err(e) => eprintln!(
                            "  {} Failed {}: {}",
                            crate::glyph(crate::glyphs::Glyph::Cross).red(),
                            jira_key,
                            e
                        ),
                    }
                }
            }
        }
        JiraCommand::Pull {
            jql,
            limit,
            dry_run,
        } => {
            let config = aida_core::JiraConfig::load()?;
            let client = aida_core::JiraClient::new(config.clone())?;

            let query = jql.clone().unwrap_or_else(|| {
                format!(
                    "project = {} AND status != Done ORDER BY updated DESC",
                    config.project_key
                )
            });
            let results = rt.block_on(client.search(&query, *limit))?;

            if results.issues.is_empty() {
                println!("No issues found.");
                return Ok(());
            }

            let store = storage.load()?;
            let existing_titles: std::collections::HashSet<String> =
                store.requirements.iter().map(|r| r.title.clone()).collect();

            let mut to_import = Vec::new();
            let mut skipped = 0;

            for issue in &results.issues {
                let jira_prefix = format!("[{}]", issue.key);
                if existing_titles.contains(&issue.fields.summary)
                    || store
                        .requirements
                        .iter()
                        .any(|r| r.title.starts_with(&jira_prefix))
                {
                    skipped += 1;
                } else {
                    to_import.push(issue);
                }
            }

            if to_import.is_empty() {
                println!(
                    "All {} issues already imported ({} skipped).",
                    results.issues.len(),
                    skipped
                );
                return Ok(());
            }

            println!(
                "Found {} issues to import ({} already exist):",
                to_import.len(),
                skipped
            );
            for issue in &to_import {
                let aida_type = config.reverse_map_type(issue.issue_type_name());
                println!(
                    "  {:<12} {:<10} {}",
                    issue.key,
                    aida_type,
                    truncate_str(issue.summary(), 50)
                );
            }

            if *dry_run {
                println!("\nDry run — no requirements created.");
                return Ok(());
            }

            // Bulk-writer path (FR-1-002): one commit per pull, no full-store
            // round-trip. trace:FR-1-002 | ai:claude
            let imported = bulk_import_via_writer(
                storage,
                "feat(jira)",
                to_import.iter().map(|issue| {
                    let aida_type_str = config.reverse_map_type(issue.issue_type_name());
                    let aida_status_str = config.reverse_map_status(issue.status_name());
                    let mut req = Requirement::new(
                        format!("[{}] {}", issue.key, issue.fields.summary),
                        issue.description_text(),
                    );
                    req.req_type =
                        parse_requirement_type(&aida_type_str).unwrap_or(RequirementType::Task);
                    req.set_status_from_str(&aida_status_str);
                    for label in issue.labels() {
                        req.tags.insert(format!("jira:{}", label));
                    }
                    req
                }),
            )?;

            println!(
                "\n{} Imported {} issues as requirements.",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                imported
            );
        }
    }
    Ok(())
}

pub(crate) fn handle_github_command(cmd: &GitHubCommand, storage: &Storage) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;

    match cmd {
        GitHubCommand::Config {
            repo,
            token,
            api_url,
            show,
        } => {
            let mut config = aida_core::GitHubConfig::load()?;

            if *show {
                println!("{}", "GitHub Configuration".bold());
                println!("{}", "─".repeat(40));
                println!("API URL:  {}", config.api_url);
                println!(
                    "Repo:     {}",
                    if config.repo.is_empty() {
                        "(not set)"
                    } else {
                        &config.repo
                    }
                );
                println!(
                    "Token:    {}",
                    if config.effective_token().is_ok() {
                        "configured (AIDA_GITHUB_TOKEN)"
                    } else {
                        "not configured"
                    }
                );
                println!("Enabled:  {}", config.enabled);
                return Ok(());
            }

            if let Some(r) = repo {
                config.repo = r.clone();
            }
            if let Some(t) = token {
                std::env::set_var("AIDA_GITHUB_TOKEN", t);
                config.token = Some(t.clone());
                println!(
                    "{} Token set for this session. Set AIDA_GITHUB_TOKEN env var for persistence.",
                    "!".yellow()
                );
            }
            if let Some(u) = api_url {
                config.api_url = u.clone();
            }

            config.save()?;
            println!(
                "{} GitHub configuration saved.",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
        }
        GitHubCommand::Test => {
            let config = aida_core::GitHubConfig::load()?;
            config.validate()?;

            let client = aida_core::GitHubClient::new(config)?;
            let repo = rt.block_on(client.test_connection())?;

            println!(
                "{} Connected to GitHub",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
            println!("  Repository: {}", repo.full_name);
            println!("  URL:        {}", repo.html_url);
            println!("  Default:    {}", repo.default_branch);
            println!("  Private:    {}", repo.is_private);
        }
        GitHubCommand::List {
            state,
            labels,
            limit,
        } => {
            let config = aida_core::GitHubConfig::load()?;
            let client = aida_core::GitHubClient::new(config)?;

            let mut filter = aida_core::GitHubIssueFilter {
                state: Some(state.clone()),
                per_page: Some(*limit),
                ..Default::default()
            };
            if let Some(l) = labels {
                filter.labels = l.split(',').map(|s| s.trim().to_string()).collect();
            }

            let issues = rt.block_on(client.list_issues(Some(filter)))?;

            if issues.is_empty() {
                println!("No issues found.");
            } else {
                println!("{:<8} {:<10} {:<40} Labels", "#", "State", "Title");
                println!("{}", "─".repeat(75));
                for issue in &issues {
                    let labels_str: String = issue
                        .labels
                        .iter()
                        .map(|l| l.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    println!(
                        "{:<8} {:<10} {:<40} {}",
                        format!("#{}", issue.number),
                        issue.state,
                        truncate_str(&issue.title, 38),
                        labels_str,
                    );
                }
                println!("\n{} issues", issues.len());
            }
        }
        GitHubCommand::Show { number } => {
            let config = aida_core::GitHubConfig::load()?;
            let client = aida_core::GitHubClient::new(config)?;

            // Parse "GH-42" or "42"
            let num: u64 = number
                .trim_start_matches("GH-")
                .trim_start_matches("gh-")
                .trim_start_matches('#')
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid issue number: {}", number))?;

            let issue = rt.block_on(client.get_issue(num))?;

            println!("{}: #{}", "Number".bold(), issue.number);
            println!("{}: {}", "Title".bold(), issue.title);
            println!("{}: {}", "State".bold(), issue.state);
            println!("{}: {}", "Author".bold(), issue.user.login);
            if let Some(ref assignee) = issue.assignee {
                println!("{}: {}", "Assignee".bold(), assignee.login);
            }
            if !issue.labels.is_empty() {
                let labels: Vec<&str> = issue.labels.iter().map(|l| l.name.as_str()).collect();
                println!("{}: {}", "Labels".bold(), labels.join(", "));
            }
            println!("{}: {}", "URL".bold(), issue.html_url);
            println!("{}: {}", "Comments".bold(), issue.comments);
            if let Some(ref body) = issue.body {
                if !body.is_empty() {
                    println!("\n{}", body);
                }
            }
        }
        GitHubCommand::Push { id } => {
            let config = aida_core::GitHubConfig::load()?;
            let client = aida_core::GitHubClient::new(config.clone())?;

            let store = storage.load()?;
            let req = store
                .requirements
                .iter()
                .find(|r| r.matches_id(id))
                .ok_or_else(|| not_found::requirement_not_found(id, Some(storage.path())))?;

            // Build labels from type and priority
            let mut labels = Vec::new();
            let type_str = format!("{:?}", req.req_type);
            if let Some(label) = config.labels.types.get(&type_str) {
                labels.push(label.clone());
            }
            let priority_str = req.effective_priority();
            if let Some(label) = config.labels.priorities.get(&priority_str) {
                labels.push(label.clone());
            }
            let status_str = req.effective_status();
            if let Some(label) = config.labels.statuses.get(&status_str) {
                labels.push(label.clone());
            }

            // Build issue body with AIDA metadata
            let display_id = req.display_id();
            let body = format!(
                "{}\n\n---\n_AIDA: {} | UUID: {}_",
                req.description, display_id, req.id,
            );

            let request = aida_core::GitHubCreateIssueRequest {
                title: format!("[{}] {}", display_id, req.title),
                body: Some(body),
                labels,
                assignees: if req.owner.is_empty() {
                    Vec::new()
                } else {
                    vec![req.owner.clone()]
                },
                milestone: None,
            };

            let issue = rt.block_on(client.create_issue(&request))?;
            println!(
                "{} Created GitHub issue #{} for {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                issue.number,
                display_id
            );
            println!("  URL: {}", issue.html_url);
        }
        GitHubCommand::Sync { linked_only, apply } => {
            let config = aida_core::GitHubConfig::load()?;
            let client = aida_core::GitHubClient::new(config)?;

            let store = storage.load()?;

            // Find AIDA requirements linked to GitHub issues (by [GH-N] prefix or URL)
            let linked: Vec<(&Requirement, u64)> = store
                .requirements
                .iter()
                .filter_map(|r| {
                    // Check [GH-N] prefix
                    if r.title.starts_with("[GH-") {
                        if let Some(end) = r.title.find(']') {
                            if let Ok(n) = r.title[4..end].parse::<u64>() {
                                return Some((r, n));
                            }
                        }
                    }
                    // Check URLs
                    for url in &r.urls {
                        if url.url.contains("github.com") && url.url.contains("/issues/") {
                            if let Some(num_str) = url.url.rsplit('/').next() {
                                if let Ok(n) = num_str.parse::<u64>() {
                                    return Some((r, n));
                                }
                            }
                        }
                    }
                    None
                })
                .collect();

            if linked.is_empty() && *linked_only {
                println!("No linked GitHub issues found.");
                println!("Link with: aida github push FR-001 (or aida github pull)");
                return Ok(());
            }

            println!("{}", "GitHub Sync Status".bold());
            println!("{}", "─".repeat(65));

            let mut drift_count = 0;

            for (req, issue_number) in &linked {
                match rt.block_on(client.get_issue(*issue_number)) {
                    Ok(issue) => {
                        let mut diffs = Vec::new();

                        // Compare title (strip [GH-N] prefix for comparison)
                        let aida_title = req
                            .title
                            .strip_prefix(&format!("[GH-{}] ", issue_number))
                            .unwrap_or(&req.title);
                        if aida_title != issue.title {
                            diffs.push(format!(
                                "  title: AIDA='{}' GitHub='{}'",
                                truncate_str(aida_title, 30),
                                truncate_str(&issue.title, 30)
                            ));
                        }

                        // Compare state
                        let aida_closed = matches!(
                            req.status,
                            RequirementStatus::Completed | RequirementStatus::Rejected
                        );
                        let gh_closed = issue.state == "closed";
                        if aida_closed != gh_closed {
                            diffs.push(format!(
                                "  state: AIDA={} GitHub={}",
                                req.effective_status(),
                                issue.state
                            ));
                        }

                        if diffs.is_empty() {
                            println!(
                                "{} #{:<5} {} — in sync",
                                crate::glyph(crate::glyphs::Glyph::Check).green(),
                                issue_number,
                                truncate_str(aida_title, 45)
                            );
                        } else {
                            drift_count += 1;
                            println!(
                                "{} #{:<5} {} — DRIFTED",
                                "△".yellow(),
                                issue_number,
                                truncate_str(aida_title, 45)
                            );
                            for d in &diffs {
                                println!("    {}", d);
                            }
                        }
                    }
                    Err(e) => {
                        println!(
                            "{} #{:<5} — error: {}",
                            crate::glyph(crate::glyphs::Glyph::Cross).red(),
                            issue_number,
                            e
                        );
                    }
                }
            }

            println!();
            if drift_count == 0 {
                println!("All {} linked items in sync.", linked.len());
            } else {
                println!("{} of {} items have drifted.", drift_count, linked.len());
                if !apply {
                    println!("Use --apply to push AIDA state to GitHub.");
                }
            }

            if *apply && drift_count > 0 {
                println!();
                println!("Applying changes...");
                for (req, issue_number) in &linked {
                    let aida_title = req
                        .title
                        .strip_prefix(&format!("[GH-{}] ", issue_number))
                        .unwrap_or(&req.title);

                    let aida_closed = matches!(
                        req.status,
                        RequirementStatus::Completed | RequirementStatus::Rejected
                    );

                    let update = aida_core::GitHubUpdateIssueRequest {
                        title: Some(aida_title.to_string()),
                        body: Some(req.description.clone()),
                        state: Some(if aida_closed {
                            "closed".into()
                        } else {
                            "open".into()
                        }),
                        labels: None,
                        assignees: None,
                        milestone: None,
                    };

                    match rt.block_on(client.update_issue(*issue_number, &update)) {
                        Ok(_) => println!(
                            "  {} Updated #{}",
                            crate::glyph(crate::glyphs::Glyph::Check).green(),
                            issue_number
                        ),
                        Err(e) => eprintln!(
                            "  {} Failed #{}: {}",
                            crate::glyph(crate::glyphs::Glyph::Cross).red(),
                            issue_number,
                            e
                        ),
                    }
                }
            }
        }
        GitHubCommand::Pull {
            labels,
            open_only,
            limit,
            dry_run,
        } => {
            let config = aida_core::GitHubConfig::load()?;
            let client = aida_core::GitHubClient::new(config.clone())?;

            let mut filter = aida_core::GitHubIssueFilter {
                state: Some(if *open_only { "open" } else { "all" }.into()),
                per_page: Some(*limit),
                ..Default::default()
            };
            if let Some(l) = labels {
                filter.labels = l.split(',').map(|s| s.trim().to_string()).collect();
            }

            let issues = rt.block_on(client.list_issues(Some(filter)))?;

            if issues.is_empty() {
                println!("No issues found to import.");
                return Ok(());
            }

            // Check which issues are already imported (by matching title pattern)
            let store = storage.load()?;
            let existing_titles: std::collections::HashSet<String> =
                store.requirements.iter().map(|r| r.title.clone()).collect();

            let mut to_import: Vec<&aida_core::GitHubIssue> = Vec::new();
            let mut skipped = 0;

            for issue in &issues {
                // Skip if already imported (check for [GH-N] prefix or exact title match)
                let gh_prefix = format!("[GH-{}]", issue.number);
                let already_exists = existing_titles.contains(&issue.title)
                    || store
                        .requirements
                        .iter()
                        .any(|r| r.title.starts_with(&gh_prefix));

                if already_exists {
                    skipped += 1;
                } else {
                    to_import.push(issue);
                }
            }

            if to_import.is_empty() {
                println!(
                    "All {} issues already imported ({} skipped).",
                    issues.len(),
                    skipped
                );
                return Ok(());
            }

            println!(
                "Found {} issues to import ({} already exist):",
                to_import.len(),
                skipped
            );

            for issue in &to_import {
                // Determine type from labels
                let req_type = determine_type_from_labels(&issue.label_names(), &config.labels);
                let priority = determine_priority_from_labels(&issue.label_names(), &config.labels);

                println!(
                    "  #{:<6} {:<12} {:<8} {}",
                    issue.number,
                    format!("{:?}", req_type),
                    format!("{:?}", priority),
                    truncate_str(&issue.title, 50),
                );
            }

            if *dry_run {
                println!("\nDry run — no requirements created.");
                return Ok(());
            }

            // Import using the bulk-writer path (FR-1-002): one git commit
            // for the whole batch, no full-store load/iterate.
            // trace:FR-1-002 | ai:claude
            let imported = bulk_import_via_writer(
                storage,
                "feat(github)",
                to_import.iter().map(|issue| {
                    let req_type = determine_type_from_labels(&issue.label_names(), &config.labels);
                    let priority =
                        determine_priority_from_labels(&issue.label_names(), &config.labels);
                    let mut req = Requirement::new(
                        format!("[GH-{}] {}", issue.number, issue.title),
                        issue.body.clone().unwrap_or_default(),
                    );
                    req.req_type = req_type;
                    req.priority = priority;
                    if let Some(ref assignee) = issue.assignee {
                        req.owner = assignee.login.clone();
                    }
                    if issue.state == "closed" {
                        req.status = RequirementStatus::Completed;
                    }
                    for label in &issue.labels {
                        req.tags.insert(format!("gh:{}", label.name));
                    }
                    req.urls.push(aida_core::models::UrlLink {
                        id: Uuid::now_v7(),
                        url: issue.html_url.clone(),
                        title: format!("GitHub #{}", issue.number),
                        description: None,
                        open_mode: aida_core::models::UrlOpenMode::NewTab,
                        added_at: chrono::Utc::now(),
                        added_by: "github-import".to_string(),
                        last_verified: None,
                        last_verified_ok: None,
                    });
                    req
                }),
            )?;

            println!(
                "\n{} Imported {} issues as requirements.",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                imported
            );
        }
        GitHubCommand::Labels { create_missing } => {
            let config = aida_core::GitHubConfig::load()?;
            let client = aida_core::GitHubClient::new(config.clone())?;

            let existing = rt.block_on(client.list_labels())?;
            let existing_names: std::collections::HashSet<String> =
                existing.iter().map(|l| l.name.clone()).collect();

            println!("{}", "Repository Labels".bold());
            println!("{}", "─".repeat(40));
            for label in &existing {
                println!(
                    "  {} (#{}) {}",
                    label.name,
                    label.color,
                    label.description.as_deref().unwrap_or("")
                );
            }
            println!("\n{} labels", existing.len());

            if *create_missing {
                let all_labels: Vec<(&str, &str)> = config
                    .labels
                    .types
                    .values()
                    .map(|v| (v.as_str(), "0e8a16"))
                    .chain(
                        config
                            .labels
                            .priorities
                            .values()
                            .map(|v| (v.as_str(), "d93f0b")),
                    )
                    .chain(
                        config
                            .labels
                            .statuses
                            .values()
                            .map(|v| (v.as_str(), "1d76db")),
                    )
                    .collect();

                let mut created = 0;
                for (name, color) in all_labels {
                    if !existing_names.contains(name) {
                        match rt.block_on(client.create_label(name, color, Some("Created by AIDA")))
                        {
                            Ok(_) => {
                                println!(
                                    "  {} Created label: {}",
                                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                                    name
                                );
                                created += 1;
                            }
                            Err(e) => {
                                eprintln!(
                                    "  {} Failed to create {}: {}",
                                    crate::glyph(crate::glyphs::Glyph::Cross).red(),
                                    name,
                                    e
                                );
                            }
                        }
                    }
                }
                if created == 0 {
                    println!("\nAll AIDA labels already exist.");
                } else {
                    println!("\n{} labels created.", created);
                }
            }
        }
    }

    Ok(())
}

/// Determine AIDA requirement type from GitHub labels.
fn determine_type_from_labels(
    labels: &[&str],
    label_config: &aida_core::GitHubLabelConfig,
) -> RequirementType {
    // Check each label against the type mappings (reverse lookup)
    for label in labels {
        for (type_name, mapped_label) in &label_config.types {
            if label.eq_ignore_ascii_case(mapped_label) {
                return match type_name.as_str() {
                    "Bug" => RequirementType::Bug,
                    "Story" => RequirementType::Story,
                    "Task" => RequirementType::Task,
                    "Epic" => RequirementType::Epic,
                    "Functional" => RequirementType::Functional,
                    "NonFunctional" => RequirementType::NonFunctional,
                    _ => RequirementType::Task,
                };
            }
        }
        // Also check common GitHub labels directly
        let l = label.to_lowercase();
        if l == "bug" {
            return RequirementType::Bug;
        }
        if l == "enhancement" || l == "feature" {
            return RequirementType::Story;
        }
    }
    RequirementType::Task // default
}

/// Determine AIDA priority from GitHub labels.
fn determine_priority_from_labels(
    labels: &[&str],
    label_config: &aida_core::GitHubLabelConfig,
) -> RequirementPriority {
    for label in labels {
        for (priority_name, mapped_label) in &label_config.priorities {
            if label.eq_ignore_ascii_case(mapped_label) {
                return match priority_name.as_str() {
                    "High" => RequirementPriority::High,
                    "Low" => RequirementPriority::Low,
                    _ => RequirementPriority::Medium,
                };
            }
        }
    }
    RequirementPriority::Medium // default
}

// trace:STORY-0321 | ai:claude
/// Handle GitLab integration commands
pub(crate) fn handle_gitlab_command(cmd: &GitLabCommand, storage: &Storage) -> Result<()> {
    // Create tokio runtime for async operations
    let rt = tokio::runtime::Runtime::new()?;

    match cmd {
        GitLabCommand::Config {
            url,
            project,
            token,
            show,
        } => {
            if *show {
                // Show current configuration
                match GitLabConfig::load() {
                    Ok(Some(config)) => {
                        println!("{}", "GitLab Configuration:".bold());
                        println!("  URL:        {}", config.url);
                        println!("  Project ID: {}", config.project_id);
                        println!(
                            "  Enabled:    {}",
                            if config.enabled { "yes" } else { "no" }
                        );
                        if config.effective_token().is_some() {
                            println!("  Token:      {}", "(configured)".green());
                        } else {
                            println!("  Token:      {}", "(not set)".yellow());
                        }
                        println!("\nLabel prefix: {}", config.labels.prefix);
                        println!("Sync mode:    {:?}", config.sync.mode);
                    }
                    Ok(None) => {
                        println!("{}", "GitLab is not configured.".yellow());
                        println!("Use 'aida gitlab config --url <URL> --project <ID> --token <TOKEN>' to configure.");
                    }
                    Err(e) => {
                        println!("{}: {}", "Error loading config".red(), e);
                    }
                }
                return Ok(());
            }

            // Update configuration
            let mut config = GitLabConfig::load()?.unwrap_or_default();

            if let Some(u) = url {
                config.url = u.clone();
                println!("Set URL: {}", u);
            }
            if let Some(p) = project {
                config.project_id = *p;
                println!("Set project ID: {}", p);
            }
            if let Some(t) = token {
                // Store token in environment for this session
                // In production, would use keyring
                std::env::set_var("AIDA_GITLAB_TOKEN", t);
                config.token = Some(t.clone());
                println!("Set token: (hidden)");
            }

            // Save config (token excluded from file)
            config.save()?;
            println!("{}", "Configuration saved.".green());

            // Validate if we have enough config
            if let Err(e) = config.validate() {
                println!("{}: {}", "Warning".yellow(), e);
                println!("Run 'aida gitlab test' to verify connection.");
            }
        }

        GitLabCommand::Test => {
            // Test connection to GitLab
            let config = GitLabConfig::load()?.ok_or_else(|| {
                anyhow::anyhow!("GitLab not configured. Run 'aida gitlab config' first.")
            })?;

            println!("Testing connection to {}...", config.url);

            let client = GitLabClient::new(config)?;
            let project = rt.block_on(client.test_connection())?;

            println!("{}", "Connection successful!".green());
            println!("  Project: {}", project.name_with_namespace);
            println!("  URL:     {}", project.web_url);
            if let Some(desc) = &project.description {
                if !desc.is_empty() {
                    println!("  Desc:    {}", desc);
                }
            }
        }

        GitLabCommand::List {
            state,
            labels,
            search,
            limit,
        } => {
            let config = GitLabConfig::load()?.ok_or_else(|| {
                anyhow::anyhow!("GitLab not configured. Run 'aida gitlab config' first.")
            })?;

            let client = GitLabClient::new(config)?;

            // Build filter
            let mut filter = match state.as_str() {
                "opened" => IssueFilter::open(),
                "closed" => IssueFilter {
                    state: Some(IssueState::Closed),
                    ..Default::default()
                },
                _ => IssueFilter::default(),
            };

            if let Some(l) = labels {
                filter = filter.with_labels(l.split(',').map(|s| s.trim().to_string()).collect());
            }

            if let Some(s) = search {
                filter.search = Some(s.clone());
            }

            filter.per_page = Some(*limit);

            let issues = rt.block_on(client.list_issues(Some(filter)))?;

            if issues.is_empty() {
                println!("{}", "No issues found.".yellow());
                return Ok(());
            }

            println!("{}", format!("Found {} issues:", issues.len()).bold());
            println!();

            for issue in &issues {
                let state_indicator = if issue.is_open() {
                    "●".green()
                } else {
                    "○".bright_black()
                };

                println!(
                    "{} {} {}",
                    state_indicator,
                    format!("GL-{}", issue.iid).cyan(),
                    issue.title
                );

                if !issue.labels.is_empty() {
                    println!("    Labels: {}", issue.labels.join(", ").bright_black());
                }
            }
        }

        GitLabCommand::Show { iid } => {
            let config = GitLabConfig::load()?.ok_or_else(|| {
                anyhow::anyhow!("GitLab not configured. Run 'aida gitlab config' first.")
            })?;

            let client = GitLabClient::new(config)?;

            // Parse IID (handle "GL-123" or "123" format)
            let iid_num: u64 = iid
                .strip_prefix("GL-")
                .unwrap_or(iid)
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid issue IID: {}", iid))?;

            let issue = rt.block_on(client.get_issue(iid_num))?;

            println!("{}", format!("GL-{}: {}", issue.iid, issue.title).bold());
            println!();

            let state_str = if issue.is_open() {
                "Open".green()
            } else {
                "Closed".bright_black()
            };
            println!("State:    {}", state_str);
            println!("Author:   {}", issue.author.username);
            if let Some(assignee) = issue.assignee_username() {
                println!("Assignee: {}", assignee);
            }
            if !issue.labels.is_empty() {
                println!("Labels:   {}", issue.labels.join(", "));
            }
            if let Some(milestone) = &issue.milestone {
                println!("Milestone: {}", milestone.title);
            }
            println!("URL:      {}", issue.web_url);
            println!();

            if let Some(desc) = &issue.description {
                if !desc.is_empty() {
                    println!("{}", "Description:".bold());
                    println!("{}", desc);
                }
            }
        }

        // trace:STORY-0325 | ai:claude
        GitLabCommand::Status { id, diverged } => {
            use aida_core::{LinkOrigin, SyncStatus};

            // Check if storage is SQLite (sync state only works with SQLite)
            if !storage.is_sqlite() {
                println!(
                    "{}",
                    "GitLab sync status is only available for SQLite databases.".yellow()
                );
                return Ok(());
            }

            let store = storage.load()?;

            // Load sync states based on filter
            let sync_states = if let Some(req_id) = id {
                // Find the requirement by spec_id or UUID
                let requirement = store.requirements.iter().find(|r| {
                    r.spec_id
                        .as_deref()
                        .is_some_and(|s| s.eq_ignore_ascii_case(req_id))
                        || r.id.to_string() == *req_id
                });

                if let Some(req) = requirement {
                    storage.load_sync_states_for_requirement(req.id)?
                } else {
                    println!("{} {}", "Requirement not found:".red(), req_id);
                    return Ok(());
                }
            } else {
                storage.load_all_sync_states()?
            };

            // Filter by diverged if requested
            let sync_states: Vec<_> = if *diverged {
                sync_states
                    .into_iter()
                    .filter(|s| !matches!(s.sync_status, SyncStatus::InSync))
                    .collect()
            } else {
                sync_states
            };

            if sync_states.is_empty() {
                if *diverged {
                    println!("{}", "No diverged GitLab links found.".green());
                } else {
                    println!("{}", "No GitLab sync states found.".yellow());
                    println!("Link requirements to GitLab issues using the GUI or create issues from requirements.");
                }
                return Ok(());
            }

            // Display sync states
            println!("{}", "GitLab Sync Status".bold());
            println!("{}", "─".repeat(60));

            for state in &sync_states {
                // Find the requirement for this sync state
                let req = store
                    .requirements
                    .iter()
                    .find(|r| r.id == state.requirement_id);

                let req_display = if let Some(r) = req {
                    r.spec_id.clone().unwrap_or_else(|| r.id.to_string())
                } else {
                    state.spec_id.clone()
                };

                // Status icon and color
                let (status_icon, _status_color) = match state.sync_status {
                    SyncStatus::InSync => (crate::glyph(crate::glyphs::Glyph::Check), "green"),
                    SyncStatus::AidaModified => ("△", "yellow"),
                    SyncStatus::GitLabModified => ("▽", "cyan"),
                    SyncStatus::Conflict => (crate::glyph(crate::glyphs::Glyph::Warning), "red"),
                    SyncStatus::Error => (crate::glyph(crate::glyphs::Glyph::Cross), "red"),
                    SyncStatus::Untracked => ("?", "dimmed"),
                };

                let status_text = match state.sync_status {
                    SyncStatus::InSync => "In Sync".green(),
                    SyncStatus::AidaModified => "AIDA Modified".yellow(),
                    SyncStatus::GitLabModified => "GitLab Modified".cyan(),
                    SyncStatus::Conflict => "Conflict".red(),
                    SyncStatus::Error => "Error".red(),
                    SyncStatus::Untracked => "Untracked".dimmed(),
                };

                let origin_text = match state.link_origin {
                    LinkOrigin::CreatedFromAida => "→GL",
                    LinkOrigin::ImportedFromGitLab => "←GL",
                    LinkOrigin::ManualLink => "↔GL",
                };

                println!(
                    "{} {} {} GL-{} [{}] {}",
                    status_icon,
                    req_display.bold(),
                    origin_text.dimmed(),
                    state.gitlab_issue_iid,
                    status_text,
                    state
                        .last_sync
                        .with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M")
                        .to_string()
                        .dimmed()
                );

                if let Some(error) = &state.last_error {
                    println!("    {} {}", "Error:".red(), error);
                }
            }

            println!("{}", "─".repeat(60));
            println!(
                "Total: {} links ({} in sync, {} diverged)",
                sync_states.len(),
                sync_states
                    .iter()
                    .filter(|s| matches!(s.sync_status, SyncStatus::InSync))
                    .count(),
                sync_states
                    .iter()
                    .filter(|s| !matches!(s.sync_status, SyncStatus::InSync))
                    .count()
            );
        }

        // trace:STORY-0326 | ai:claude
        GitLabCommand::Labels {
            validate,
            create_missing,
            init,
        } => {
            // Load or create config
            let mut config = GitLabConfig::load()?.unwrap_or_default();

            // Initialize with defaults if requested
            if *init {
                config.labels = config.labels.with_defaults();
                config.save()?;
                println!("{}", "Label mappings initialized with defaults.".green());
            }

            // Show current label configuration
            println!("{}", "GitLab Label Mappings".bold());
            println!("{}", "─".repeat(50));

            if !config.labels.prefix.is_empty() {
                println!("Prefix: {}", config.labels.prefix.cyan());
            }

            println!("\n{}", "Type Mappings:".bold());
            if config.labels.types.is_empty() {
                println!(
                    "  {} (use --init to set defaults)",
                    "(none configured)".dimmed()
                );
            } else {
                for (aida_type, gitlab_label) in &config.labels.types {
                    println!("  {} → {}", aida_type, gitlab_label.cyan());
                }
            }

            println!("\n{}", "Priority Mappings:".bold());
            if config.labels.priorities.is_empty() {
                println!(
                    "  {} (use --init to set defaults)",
                    "(none configured)".dimmed()
                );
            } else {
                for (priority, gitlab_label) in &config.labels.priorities {
                    println!("  {} → {}", priority, gitlab_label.cyan());
                }
            }

            println!("\n{}", "Status Mappings:".bold());
            if config.labels.statuses.is_empty() {
                println!(
                    "  {} (use --init to set defaults)",
                    "(none configured)".dimmed()
                );
            } else {
                for (status, gitlab_label) in &config.labels.statuses {
                    println!("  {} → {}", status, gitlab_label.cyan());
                }
            }

            println!(
                "\nAuto-create labels: {}",
                if config.labels.auto_create_labels {
                    "yes".green()
                } else {
                    "no".dimmed()
                }
            );

            // Validate labels if requested
            if *validate || *create_missing {
                let Some(token) = config.effective_token() else {
                    return Err(anyhow::anyhow!("GitLab token required. Set AIDA_GITLAB_TOKEN or run 'aida gitlab config --token <TOKEN>'"));
                };

                let mut config_with_token = config.clone();
                config_with_token.token = Some(token);
                let client = GitLabClient::new(config_with_token)?;

                println!("\n{}", "Validating labels in GitLab...".dimmed());

                // Get all labels from GitLab project
                let gitlab_labels = rt.block_on(client.list_labels())?;
                let gitlab_label_names: std::collections::HashSet<_> =
                    gitlab_labels.iter().map(|l| l.name.clone()).collect();

                // Get all mapped labels
                let mapped_labels = config.labels.all_labels();
                let mut missing_labels = Vec::new();
                let mut found_labels = Vec::new();

                for label in &mapped_labels {
                    if gitlab_label_names.contains(label) {
                        found_labels.push(label.clone());
                    } else {
                        missing_labels.push(label.clone());
                    }
                }

                println!("\n{}", "Validation Results:".bold());
                println!(
                    "  {} labels found in GitLab",
                    found_labels.len().to_string().green()
                );
                if !missing_labels.is_empty() {
                    println!(
                        "  {} labels missing:",
                        missing_labels.len().to_string().yellow()
                    );
                    for label in &missing_labels {
                        println!("    - {}", label.yellow());
                    }
                }

                // Create missing labels if requested
                if *create_missing && !missing_labels.is_empty() {
                    println!("\n{}", "Creating missing labels...".dimmed());
                    for label in &missing_labels {
                        // Determine label color based on type
                        let color = if label.starts_with("type::") {
                            "#428BCA" // Blue for types
                        } else if label.starts_with("priority::") {
                            if label.contains("high") {
                                "#DC3545"
                            } else if label.contains("low") {
                                "#28A745"
                            } else {
                                "#FFC107"
                            }
                        } else if label.starts_with("status::") {
                            "#6C757D" // Gray for status
                        } else {
                            "#7950F2" // Purple default
                        };

                        match rt.block_on(client.create_label(label, color, None)) {
                            Ok(_) => println!(
                                "  {} Created: {}",
                                crate::glyph(crate::glyphs::Glyph::Check).green(),
                                label
                            ),
                            Err(e) => println!(
                                "  {} Failed to create {}: {}",
                                crate::glyph(crate::glyphs::Glyph::Cross).red(),
                                label,
                                e
                            ),
                        }
                    }
                }
            }
        }

        // trace:STORY-0327 | ai:claude
        GitLabCommand::Refresh { id, force } => {
            use aida_core::{GitLabSyncState, IssueFilter, SyncStatus};

            // Check if storage is SQLite (sync state only works with SQLite)
            if !storage.is_sqlite() {
                println!(
                    "{}",
                    "GitLab refresh is only available for SQLite databases.".yellow()
                );
                return Ok(());
            }

            // Load GitLab config
            let config = GitLabConfig::load()?.ok_or_else(|| {
                anyhow::anyhow!("GitLab not configured. Run 'aida gitlab config' first.")
            })?;

            let Some(token) = config.effective_token() else {
                return Err(anyhow::anyhow!("GitLab token required. Set AIDA_GITLAB_TOKEN or run 'aida gitlab config --token <TOKEN>'"));
            };

            let mut config_with_token = config.clone();
            config_with_token.token = Some(token);
            let client = GitLabClient::new(config_with_token)?;

            let store = storage.load()?;

            // Get sync states to refresh
            let sync_states = if let Some(req_id) = id {
                // Find the requirement by spec_id or UUID
                let requirement = store.requirements.iter().find(|r| {
                    r.spec_id
                        .as_deref()
                        .is_some_and(|s| s.eq_ignore_ascii_case(req_id))
                        || r.id.to_string() == *req_id
                });

                if let Some(req) = requirement {
                    storage.load_sync_states_for_requirement(req.id)?
                } else {
                    println!("{} {}", "Requirement not found:".red(), req_id);
                    return Ok(());
                }
            } else {
                storage.load_all_sync_states()?
            };

            if sync_states.is_empty() {
                println!("{}", "No GitLab sync states found to refresh.".yellow());
                println!("Link requirements to GitLab issues first.");
                return Ok(());
            }

            println!("{}", "Refreshing GitLab sync states...".dimmed());
            println!("{}", "─".repeat(60));

            // Collect all issue IIDs to fetch
            let iids: Vec<u64> = sync_states.iter().map(|s| s.gitlab_issue_iid).collect();

            // Fetch issues from GitLab
            let filter = IssueFilter::default().with_iids(iids);
            let issues = rt.block_on(client.list_issues(Some(filter)))?;

            // Create a map of IID -> Issue for quick lookup
            let issue_map: std::collections::HashMap<u64, _> =
                issues.into_iter().map(|i| (i.iid, i)).collect();

            let mut updated_count = 0;
            let mut error_count = 0;

            for mut state in sync_states {
                // Find the requirement
                let req = store
                    .requirements
                    .iter()
                    .find(|r| r.id == state.requirement_id);

                let req_display = if let Some(r) = req {
                    r.spec_id.clone().unwrap_or_else(|| r.id.to_string())
                } else {
                    state.spec_id.clone()
                };

                // Get the GitLab issue
                if let Some(issue) = issue_map.get(&state.gitlab_issue_iid) {
                    // Calculate current hashes
                    let current_gitlab_hash = GitLabSyncState::hash_gitlab_issue(issue);
                    let current_aida_hash = if let Some(r) = req {
                        GitLabSyncState::hash_requirement(r)
                    } else {
                        state.aida_content_hash.clone()
                    };

                    // Determine new sync status
                    let old_status = state.sync_status.clone();
                    let aida_changed = current_aida_hash != state.aida_content_hash;
                    let gitlab_changed = current_gitlab_hash != state.gitlab_content_hash;

                    let new_status = match (aida_changed, gitlab_changed) {
                        (false, false) => SyncStatus::InSync,
                        (true, false) => SyncStatus::AidaModified,
                        (false, true) => SyncStatus::GitLabModified,
                        (true, true) => SyncStatus::Conflict,
                    };

                    // Update if changed or forced
                    if *force || new_status != old_status {
                        state.sync_status = new_status.clone();
                        state.last_sync = chrono::Utc::now();
                        state.last_error = None;

                        // Update stored hashes if this is a fresh sync
                        if state.aida_content_hash.is_empty() {
                            state.aida_content_hash = current_aida_hash;
                        }
                        if state.gitlab_content_hash.is_empty() {
                            state.gitlab_content_hash = current_gitlab_hash;
                        }

                        if let Err(e) = storage.save_sync_state(&state) {
                            println!(
                                "  {} {} GL-{}: {}",
                                crate::glyph(crate::glyphs::Glyph::Cross).red(),
                                req_display,
                                state.gitlab_issue_iid,
                                e
                            );
                            error_count += 1;
                        } else {
                            let status_indicator = match new_status {
                                SyncStatus::InSync => {
                                    crate::glyph(crate::glyphs::Glyph::Check).green()
                                }
                                SyncStatus::AidaModified => "△".yellow(),
                                SyncStatus::GitLabModified => "▽".cyan(),
                                SyncStatus::Conflict => {
                                    crate::glyph(crate::glyphs::Glyph::Warning).red()
                                }
                                _ => "?".dimmed(),
                            };
                            println!(
                                "  {} {} GL-{}: {}",
                                status_indicator, req_display, state.gitlab_issue_iid, new_status
                            );
                            updated_count += 1;
                        }
                    } else {
                        println!(
                            "  {} {} GL-{}: {} (unchanged)",
                            "·".dimmed(),
                            req_display,
                            state.gitlab_issue_iid,
                            old_status
                        );
                    }
                } else {
                    println!(
                        "  {} {} GL-{}: Issue not found in GitLab",
                        "?".yellow(),
                        req_display,
                        state.gitlab_issue_iid
                    );
                    error_count += 1;
                }
            }

            println!("{}", "─".repeat(60));
            println!(
                "Refreshed: {} updated, {} errors",
                updated_count.to_string().green(),
                if error_count > 0 {
                    error_count.to_string().red()
                } else {
                    "0".dimmed()
                }
            );
        }

        // trace:STORY-0327 | ai:claude
        GitLabCommand::Poll { action, interval } => match action.to_lowercase().as_str() {
            "status" => {
                let config = GitLabConfig::load()?;
                if let Some(config) = config {
                    println!("{}", "GitLab Polling Configuration".bold());
                    println!("{}", "─".repeat(40));
                    println!(
                        "Polling enabled: {}",
                        if config.polling.enabled {
                            "yes".green()
                        } else {
                            "no".dimmed()
                        }
                    );
                    println!(
                        "Interval: {} seconds ({} minutes)",
                        config.polling.interval_seconds,
                        config.polling.interval_seconds / 60
                    );
                    println!("Batch size: {}", config.polling.batch_size);
                    println!("Max concurrent: {}", config.polling.max_concurrent);
                    println!();
                    println!(
                        "{}",
                        "Note: Background polling runs in the AIDA GUI.".dimmed()
                    );
                    println!("{}", "Use 'aida gitlab refresh' for manual sync.".dimmed());
                } else {
                    println!("{}", "GitLab not configured.".yellow());
                }
            }
            "start" => {
                println!(
                    "{}",
                    "Background polling is managed by the AIDA GUI.".yellow()
                );
                println!();
                println!("To enable polling:");
                println!("  1. Open AIDA GUI");
                println!("  2. Go to Settings > GitLab");
                println!("  3. Enable 'Background Polling'");
                println!();
                println!("For CLI-based polling, use a cron job or scheduled task:");
                println!(
                    "  {} aida gitlab refresh",
                    format!("*/{} * * * *", interval / 60).dimmed()
                );
            }
            "stop" => {
                println!(
                    "{}",
                    "Background polling is managed by the AIDA GUI.".yellow()
                );
                println!();
                println!("To disable polling:");
                println!("  1. Open AIDA GUI");
                println!("  2. Go to Settings > GitLab");
                println!("  3. Disable 'Background Polling'");
            }
            _ => {
                println!(
                    "{}: Unknown action '{}'. Use: status, start, stop",
                    "Error".red(),
                    action
                );
            }
        },
    }

    Ok(())
}
