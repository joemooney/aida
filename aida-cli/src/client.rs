// trace:FR-0227 | ai:claude:high
//! gRPC client for AIDA remote server
//!
//! This module provides a client for connecting to an AIDA gRPC server.
//! It's only available when built with the "remote" feature.

#[cfg(feature = "remote")]
pub mod proto {
    include!("generated/aida.rs");
}

#[cfg(feature = "remote")]
use proto::requirements_service_client::RequirementsServiceClient;

#[cfg(feature = "remote")]
use tonic::transport::Channel;

#[cfg(feature = "remote")]
use anyhow::{Context, Result};

#[cfg(feature = "remote")]
use colored::Colorize;

/// Connect to an AIDA gRPC server
#[cfg(feature = "remote")]
pub async fn connect(server_addr: &str) -> Result<RequirementsServiceClient<Channel>> {
    // Parse the server address
    let addr = if server_addr.starts_with("http://") || server_addr.starts_with("https://") {
        server_addr.to_string()
    } else if server_addr.starts_with("grpc://") {
        server_addr.replace("grpc://", "http://")
    } else {
        format!("http://{}", server_addr)
    };

    let client = RequirementsServiceClient::connect(addr)
        .await
        .context("Failed to connect to AIDA server")?;

    Ok(client)
}

/// Check server status
#[cfg(feature = "remote")]
pub async fn get_server_status(server_addr: &str) -> Result<()> {
    let mut client = connect(server_addr).await?;

    let request = tonic::Request::new(proto::GetServerStatusRequest {});
    let response = client.get_server_status(request).await?;
    let status = response.into_inner();

    println!("{}", "Server Status".cyan().bold());
    println!("{}", "=".repeat(40));
    println!("{}: {}", "Version".bold(), status.version);
    println!("{}: {}", "Status".bold(), status.status.green());
    println!("{}: {}s", "Uptime".bold(), status.uptime_seconds);
    println!("{}: {}", "Connections".bold(), status.active_connections);
    println!("{}: {}", "Backend".bold(), status.storage_backend);
    println!("{}: {}", "Storage Path".bold(), status.storage_path);

    Ok(())
}

/// List requirements from server
#[cfg(feature = "remote")]
pub async fn list_requirements(
    server_addr: &str,
    status_filter: Option<&str>,
    feature_filter: Option<&str>,
    limit: i32,
) -> Result<()> {
    let mut client = connect(server_addr).await?;

    let request = tonic::Request::new(proto::ListRequirementsRequest {
        status_filter: status_filter.unwrap_or_default().to_string(),
        priority_filter: String::new(),
        type_filter: String::new(),
        feature_filter: feature_filter.unwrap_or_default().to_string(),
        owner_filter: String::new(),
        include_archived: false,
        limit,
        offset: 0,
    });

    let response = client.list_requirements(request).await?;
    let list = response.into_inner();

    if list.requirements.is_empty() {
        println!("{}", "No requirements found.".yellow());
        return Ok(());
    }

    println!(
        "{:<10} | {:<36} | {:<30} | {:<10} | {:<15}",
        "SPEC-ID", "UUID", "Title", "Status", "Feature"
    );
    println!("{}", "-".repeat(110));

    for req in list.requirements {
        let status = proto::RequirementStatus::try_from(req.status)
            .unwrap_or(proto::RequirementStatus::Unspecified);
        let status_str = match status {
            proto::RequirementStatus::Draft => "Draft".yellow(),
            proto::RequirementStatus::Approved => "Approved".blue(),
            proto::RequirementStatus::Planned => "Planned".cyan(),
            proto::RequirementStatus::InProgress => "In Progress".magenta(),
            proto::RequirementStatus::Completed => "Completed".green(),
            proto::RequirementStatus::Rejected => "Rejected".red(),
            _ => "Unknown".normal(),
        };

        let spec_id = if req.spec_id.is_empty() {
            "-".to_string()
        } else {
            req.spec_id
        };
        let title = if req.title.len() > 28 {
            format!("{}...", &req.title[..28])
        } else {
            req.title
        };

        println!(
            "{:<10} | {:<36} | {:<30} | {:<10} | {:<15}",
            spec_id, req.id, title, status_str, req.feature
        );
    }

    println!("\n{} requirement(s) total", list.total_count);

    Ok(())
}

/// Get a single requirement from server
#[cfg(feature = "remote")]
pub async fn get_requirement(server_addr: &str, id: &str) -> Result<()> {
    let mut client = connect(server_addr).await?;

    let request = tonic::Request::new(proto::GetRequirementRequest { id: id.to_string() });

    let response = client.get_requirement(request).await?;
    // STORY-729: the legacy gRPC not-found used to be a bare "Requirement not
    // found" with no id, no store context, no next step — the exact gap the
    // engineered `not_found.rs` errors were built to close. Bring this holdout
    // up to that bar: name the id, say where we looked, name the command.
    // trace:STORY-729
    let req = response.into_inner().requirement.ok_or_else(|| {
        anyhow::anyhow!(
            "Requirement not found: {id}\n  \
             Searched in: the AIDA server at {server_addr}\n  \
             Hint: check the spec ID (try `aida list` or `aida search <terms>`), \
             or confirm the server points at the right store."
        )
    })?;

    println!("{}: {}", "ID".blue(), req.id);
    if !req.spec_id.is_empty() {
        println!("{}: {}", "SPEC-ID".blue(), req.spec_id);
    }
    println!("{}: {}", "Title".blue(), req.title);
    println!("{}: {}", "Description".blue(), req.description);

    let status = proto::RequirementStatus::try_from(req.status)
        .unwrap_or(proto::RequirementStatus::Unspecified);
    let status_str = match status {
        proto::RequirementStatus::Draft => "Draft".yellow(),
        proto::RequirementStatus::Approved => "Approved".blue(),
        proto::RequirementStatus::Planned => "Planned".cyan(),
        proto::RequirementStatus::InProgress => "In Progress".magenta(),
        proto::RequirementStatus::Completed => "Completed".green(),
        proto::RequirementStatus::Rejected => "Rejected".red(),
        _ => "Unknown".normal(),
    };
    println!("{}: {}", "Status".blue(), status_str);

    let priority = proto::RequirementPriority::try_from(req.priority)
        .unwrap_or(proto::RequirementPriority::Unspecified);
    let priority_str = match priority {
        proto::RequirementPriority::High => "High".red(),
        proto::RequirementPriority::Medium => "Medium".yellow(),
        proto::RequirementPriority::Low => "Low".green(),
        _ => "Unknown".normal(),
    };
    println!("{}: {}", "Priority".blue(), priority_str);

    println!("{}: {}", "Owner".blue(), req.owner);
    println!("{}: {}", "Feature".blue(), req.feature);

    if !req.tags.is_empty() {
        println!("{}: {}", "Tags".blue(), req.tags.join(", "));
    }

    if !req.comments.is_empty() {
        println!("\n{}:", "Comments".green());
        for comment in &req.comments {
            println!("  {}: {}", comment.author.cyan(), comment.content);
        }
    }

    Ok(())
}

/// Ping server to check connectivity
#[cfg(feature = "remote")]
pub async fn ping_server(server_addr: &str) -> Result<()> {
    use std::time::Instant;

    let start = Instant::now();
    let mut client = connect(server_addr).await?;

    let request = tonic::Request::new(proto::GetServerStatusRequest {});
    let response = client.get_server_status(request).await?;
    let elapsed = start.elapsed();

    let status = response.into_inner();
    // trace:TASK-840 | ai:claude — route the success check through the registry.
    let check = crate::glyphs::get(
        crate::glyphs::Glyph::Check,
        crate::find_project_root().ok().as_deref(),
    );
    println!(
        "{} Server at {} is {} ({}ms, v{})",
        check.green(),
        server_addr,
        status.status.green(),
        elapsed.as_millis(),
        status.version
    );

    Ok(())
}

// Stub implementations when remote feature is not enabled
#[cfg(not(feature = "remote"))]
pub fn remote_not_available() -> anyhow::Result<()> {
    anyhow::bail!(
        "Remote server support is not enabled. \
        Build with: cargo build -p aida-cli --features remote"
    )
}
