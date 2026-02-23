// trace:FR-0227 | ai:claude:high
//! AIDA gRPC Server
//!
//! A headless server for AIDA requirements management that exposes:
//! - gRPC API for native clients
//! - gRPC-Web API for browser WASM clients
//! - REST/JSON API for standard HTTP clients

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use http::{header, Method};
use tonic::transport::Server;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

use aida_core::db::create_backend;

mod admin;
mod chat;
mod convert;
mod evaluate;
mod projects;
mod rest;
mod service;
mod skill_runner;

// Include the generated protobuf code
pub mod proto {
    include!("generated/aida.rs");
}

use proto::requirements_service_server::RequirementsServiceServer;
use service::{AidaService, ServerState};

/// Kill any process using the specified port
fn kill_process_on_port(port: u16) -> Result<bool> {
    use std::process::Command;

    // Use lsof to find PID(s) using this port
    let output = Command::new("lsof")
        .args(["-t", "-i", &format!(":{}", port)])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let pids = String::from_utf8_lossy(&output.stdout);
            let mut killed_any = false;
            for pid_str in pids.lines() {
                if let Ok(pid) = pid_str.trim().parse::<i32>() {
                    // Send SIGTERM to gracefully stop the process
                    let kill_result = Command::new("kill").arg(pid.to_string()).status();

                    match kill_result {
                        Ok(status) if status.success() => {
                            info!("Killed existing process {} on port {}", pid, port);
                            killed_any = true;
                        }
                        Ok(_) => {
                            // Try SIGKILL if SIGTERM didn't work
                            let _ = Command::new("kill")
                                .args(["-9", &pid.to_string()])
                                .status();
                            info!("Force killed existing process {} on port {}", pid, port);
                            killed_any = true;
                        }
                        Err(e) => {
                            tracing::warn!("Failed to kill process {}: {}", pid, e);
                        }
                    }
                }
            }
            // Give the OS a moment to release the port
            if killed_any {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Ok(killed_any)
        }
        Ok(_) => Ok(false), // No process found on port
        Err(e) => {
            tracing::warn!("lsof command failed ({}), trying ss fallback", e);
            // Fallback: try using ss (available on most Linux systems)
            kill_process_on_port_ss(port)
        }
    }
}

/// Fallback using ss command (Linux)
fn kill_process_on_port_ss(port: u16) -> Result<bool> {
    use std::process::Command;

    let output = Command::new("ss")
        .args(["-tlnp", &format!("sport = :{}", port)])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let output_str = String::from_utf8_lossy(&output.stdout);
            let mut killed_any = false;

            // Parse ss output to find PIDs (format includes "pid=XXXX")
            for line in output_str.lines() {
                if let Some(pid_start) = line.find("pid=") {
                    let pid_part = &line[pid_start + 4..];
                    if let Some(pid_end) = pid_part.find(|c: char| !c.is_ascii_digit()) {
                        if let Ok(pid) = pid_part[..pid_end].parse::<i32>() {
                            let _ = Command::new("kill").arg(pid.to_string()).status();
                            info!("Killed existing process {} on port {}", pid, port);
                            killed_any = true;
                        }
                    }
                }
            }

            if killed_any {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Ok(killed_any)
        }
        _ => Ok(false),
    }
}

/// AIDA gRPC Server
#[derive(Parser, Debug)]
#[command(name = "aida-server")]
#[command(about = "gRPC/REST server for AIDA requirements management")]
#[command(version)]
struct Args {
    /// Port to listen on (gRPC + gRPC-Web)
    #[arg(short, long, default_value = "50051")]
    port: u16,

    /// REST API port (0 = disabled)
    #[arg(long, default_value = "8080")]
    rest_port: u16,

    /// Host/IP to bind to
    #[arg(short = 'H', long, default_value = "127.0.0.1")]
    host: String,

    /// Data directory for project databases (multi-project mode)
    #[arg(long)]
    data_dir: Option<String>,

    /// Path to a single database file (single-project/legacy mode)
    #[arg(short, long)]
    database: Option<String>,

    /// Log level (trace, debug, info, warn, error)
    #[arg(short, long, default_value = "info")]
    log_level: String,

    /// Maximum concurrent connections
    #[arg(long, default_value = "100")]
    max_connections: usize,

    /// Allowed CORS origins (comma-separated, or '*' for all)
    #[arg(long, default_value = "*")]
    cors_origins: String,

    /// Kill any existing process using the specified ports before starting
    #[arg(short, long)]
    force: bool,

    /// Directory containing static web files (React dashboard build output)
    #[arg(long)]
    static_dir: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok(); // Load .env if present, silently ignore if missing

    let args = Args::parse();

    // Kill existing processes on ports if --force is specified
    if args.force {
        kill_process_on_port(args.port)?;
        if args.rest_port > 0 {
            kill_process_on_port(args.rest_port)?;
        }
    }

    // Initialize logging
    let log_level = match args.log_level.to_lowercase().as_str() {
        "trace" => Level::TRACE,
        "debug" => Level::DEBUG,
        "info" => Level::INFO,
        "warn" => Level::WARN,
        "error" => Level::ERROR,
        _ => Level::INFO,
    };

    let subscriber = FmtSubscriber::builder()
        .with_max_level(log_level)
        .with_target(true)
        .with_thread_ids(true)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    // Create admin state (dev-mode gated)
    let dev_mode = std::env::var("AIDA_DEV_MODE")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);
    let admin_state = Arc::new(admin::AdminState::new(dev_mode));
    if dev_mode {
        info!("Dev mode enabled (AIDA_DEV_MODE)");
    }

    // Build CORS layer - allows gRPC-Web requests from browser clients
    let cors = CorsLayer::new()
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::CONTENT_TYPE,
            header::AUTHORIZATION,
            header::ACCEPT,
            header::HeaderName::from_static("x-grpc-web"),
            header::HeaderName::from_static("x-user-agent"),
            header::HeaderName::from_static("x-project"),
            header::HeaderName::from_static("grpc-timeout"),
            header::HeaderName::from_static("grpc-accept-encoding"),
            header::HeaderName::from_static("grpc-encoding"),
        ])
        .allow_origin(Any)
        .expose_headers([
            header::HeaderName::from_static("grpc-status"),
            header::HeaderName::from_static("grpc-message"),
            header::HeaderName::from_static("grpc-encoding"),
            header::HeaderName::from_static("grpc-accept-encoding"),
        ]);

    // Build the gRPC server address
    let grpc_addr: SocketAddr = format!("{}:{}", args.host, args.port).parse()?;

    info!("Starting AIDA server v{}", env!("CARGO_PKG_VERSION"));
    if let Some(ref dir) = args.static_dir {
        info!("Static files: {}", dir);
    }
    info!("gRPC/gRPC-Web: http://{}", grpc_addr);

    // Create a shutdown signal that can be shared between gRPC and REST servers
    #[allow(unused_variables)]
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(());

    // Determine mode: multi-project (--data-dir) or single-project (--database)
    let use_multi_project = args.data_dir.is_some() ||
        (args.database.is_none() && std::env::var("AIDA_DATABASE_URL").is_err());

    if use_multi_project {
        // Multi-project mode
        let data_dir = args.data_dir
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                // Default: /data in Docker, ~/.aida locally
                if std::path::Path::new("/data").exists() {
                    std::path::PathBuf::from("/data")
                } else {
                    dirs::home_dir()
                        .unwrap_or_else(|| std::path::PathBuf::from("."))
                        .join(".aida")
                }
            });

        info!("Multi-project mode: data_dir={:?}", data_dir);

        let project_manager = Arc::new(projects::ProjectManager::new(data_dir)?);

        // Migrate legacy database if exists
        if project_manager.migrate_legacy_database().await? {
            info!("Migrated legacy database to 'default' project");
        }

        // Start REST server if enabled
        let rest_handle = if args.rest_port > 0 {
            let rest_addr: SocketAddr = format!("{}:{}", args.host, args.rest_port).parse()?;
            info!("REST API: http://{}/api", rest_addr);
            info!("Projects API: http://{}/api/projects", rest_addr);

            let rest_router = rest::create_rest_router(project_manager.clone())
                .merge(admin::create_admin_router(admin_state.clone()))
                .merge(chat::create_chat_router_stub())
                .layer(cors.clone());

            let rest_router = if let Some(ref dir) = args.static_dir {
                let index = format!("{}/index.html", dir);
                rest_router.fallback_service(
                    ServeDir::new(dir).not_found_service(ServeFile::new(index)),
                )
            } else {
                rest_router
            };

            let rest_listener = tokio::net::TcpListener::bind(rest_addr).await?;
            let mut rest_shutdown_rx = shutdown_rx.clone();
            Some(tokio::spawn(async move {
                axum::serve(rest_listener, rest_router)
                    .with_graceful_shutdown(async move {
                        let _ = rest_shutdown_rx.changed().await;
                    })
                    .await
            }))
        } else {
            info!("REST API: disabled");
            None
        };

        // Create the gRPC service with multi-project support
        let grpc_service = service::AidaServiceMultiProject::new(project_manager);
        let grpc_service = RequirementsServiceServer::new(grpc_service);
        let grpc_web_service = tonic_web::enable(grpc_service);

        // Run gRPC server
        Server::builder()
            .accept_http1(true)
            .layer(cors)
            .concurrency_limit_per_connection(args.max_connections)
            .add_service(grpc_web_service)
            .serve_with_shutdown(grpc_addr, async move {
                tokio::signal::ctrl_c()
                    .await
                    .expect("Failed to install CTRL+C signal handler");
                info!("Received shutdown signal, stopping server...");
                let _ = shutdown_tx.send(());
            })
            .await?;

        if let Some(handle) = rest_handle {
            let _ = handle.await;
        }
    } else {
        // Single-project/legacy mode
        let db_path = if let Some(path) = args.database {
            path
        } else if let Ok(url) = std::env::var("AIDA_DATABASE_URL") {
            url
        } else {
            aida_core::determine_requirements_path(None)?
                .to_string_lossy()
                .to_string()
        };

        info!("Single-project mode: database={}", db_path);

        let db_path_clone = db_path.clone();
        let backend = tokio::task::spawn_blocking(move || {
            create_backend(std::path::Path::new(&db_path_clone), None)
        })
        .await??;
        info!("Backend type: {}", backend.backend_type());

        let state = Arc::new(ServerState::new(backend)?);

        // Start REST server if enabled (legacy mode)
        let rest_handle = if args.rest_port > 0 {
            let rest_addr: SocketAddr = format!("{}:{}", args.host, args.rest_port).parse()?;
            info!("REST API: http://{}/api", rest_addr);

            let rest_router = rest::create_rest_router_legacy(state.clone())
                .merge(admin::create_admin_router(admin_state.clone()))
                .merge(chat::create_chat_router(state.clone(), admin_state.clone()))
                .merge(evaluate::create_evaluate_router(state.clone(), admin_state.clone()))
                .merge(skill_runner::create_skill_runner_router(state.clone(), admin_state.clone()))
                .layer(cors.clone());

            let rest_router = if let Some(ref dir) = args.static_dir {
                let index = format!("{}/index.html", dir);
                rest_router.fallback_service(
                    ServeDir::new(dir).not_found_service(ServeFile::new(index)),
                )
            } else {
                rest_router
            };

            let rest_listener = tokio::net::TcpListener::bind(rest_addr).await?;
            let mut rest_shutdown_rx = shutdown_rx.clone();
            Some(tokio::spawn(async move {
                axum::serve(rest_listener, rest_router)
                    .with_graceful_shutdown(async move {
                        let _ = rest_shutdown_rx.changed().await;
                    })
                    .await
            }))
        } else {
            info!("REST API: disabled");
            None
        };

        // Create the gRPC service (legacy single-project)
        let grpc_service = AidaService::new(state.clone());
        let grpc_service = RequirementsServiceServer::new(grpc_service);
        let grpc_web_service = tonic_web::enable(grpc_service);

        // Run gRPC server
        Server::builder()
            .accept_http1(true)
            .layer(cors)
            .concurrency_limit_per_connection(args.max_connections)
            .add_service(grpc_web_service)
            .serve_with_shutdown(grpc_addr, async move {
                tokio::signal::ctrl_c()
                    .await
                    .expect("Failed to install CTRL+C signal handler");
                info!("Received shutdown signal, stopping server...");
                let _ = shutdown_tx.send(());
            })
            .await?;

        if let Some(handle) = rest_handle {
            let _ = handle.await;
        }
    }

    info!("Server stopped");
    Ok(())
}
