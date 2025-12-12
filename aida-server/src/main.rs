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
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

use aida_core::Storage;

mod convert;
mod rest;
mod service;

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

    /// Path to the requirements database file
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
}

#[tokio::main]
async fn main() -> Result<()> {
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

    // Determine database path
    let db_path = if let Some(path) = args.database {
        std::path::PathBuf::from(path)
    } else {
        // Use default path from aida-core
        aida_core::determine_requirements_path(None)?
    };

    info!("Using database: {}", db_path.display());

    // Initialize storage and load data
    let storage = Storage::new(&db_path);
    let state = Arc::new(ServerState::new(storage)?);

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
    info!("gRPC/gRPC-Web: http://{}", grpc_addr);

    // Create the gRPC service with gRPC-Web support
    let grpc_service = AidaService::new(state.clone());
    let grpc_service = RequirementsServiceServer::new(grpc_service);
    let grpc_web_service = tonic_web::enable(grpc_service);

    // Create a shutdown signal that can be shared between gRPC and REST servers
    // Note: _shutdown_rx is unused when REST is disabled (rest_port=0)
    #[allow(unused_variables)]
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(());

    // Start REST server if enabled
    let rest_handle = if args.rest_port > 0 {
        let rest_addr: SocketAddr = format!("{}:{}", args.host, args.rest_port).parse()?;
        info!("REST API: http://{}/api", rest_addr);

        let rest_router = rest::create_rest_router(state.clone()).layer(cors.clone());

        let rest_listener = tokio::net::TcpListener::bind(rest_addr).await?;
        let mut rest_shutdown_rx = shutdown_rx.clone();
        Some(tokio::spawn(async move {
            axum::serve(rest_listener, rest_router)
                .with_graceful_shutdown(async move {
                    // Wait for the shutdown signal
                    let _ = rest_shutdown_rx.changed().await;
                })
                .await
        }))
    } else {
        info!("REST API: disabled");
        None
    };

    // Create and run the gRPC server with gRPC-Web support
    // Using tonic_web::enable() on the service wraps it with gRPC-Web support
    // CORS layer handles browser cross-origin requests
    Server::builder()
        .accept_http1(true) // Required for gRPC-Web
        .layer(cors)
        .concurrency_limit_per_connection(args.max_connections)
        .add_service(grpc_web_service)
        .serve_with_shutdown(grpc_addr, async move {
            // Wait for shutdown signal
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to install CTRL+C signal handler");
            info!("Received shutdown signal, stopping server...");
            // Signal the REST server to shutdown too
            let _ = shutdown_tx.send(());
        })
        .await?;

    // Wait for REST server to finish if running
    if let Some(handle) = rest_handle {
        let _ = handle.await;
    }

    info!("Server stopped");
    Ok(())
}
