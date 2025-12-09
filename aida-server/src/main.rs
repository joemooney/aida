// trace:FR-0227 | ai:claude:high
//! AIDA gRPC Server
//!
//! A headless server for AIDA requirements management that exposes
//! a gRPC API for all requirement CRUD operations.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use tonic::transport::Server;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

use aida_core::Storage;

mod convert;
mod service;

// Include the generated protobuf code
pub mod proto {
    include!("generated/aida.rs");
}

use proto::requirements_service_server::RequirementsServiceServer;
use service::{AidaService, ServerState};

/// AIDA gRPC Server
#[derive(Parser, Debug)]
#[command(name = "aida-server")]
#[command(about = "gRPC server for AIDA requirements management")]
#[command(version)]
struct Args {
    /// Port to listen on
    #[arg(short, long, default_value = "50051")]
    port: u16,

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
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

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

    // Build the server address
    let addr: SocketAddr = format!("{}:{}", args.host, args.port).parse()?;

    info!("Starting AIDA gRPC server on {}", addr);
    info!("Server version: {}", env!("CARGO_PKG_VERSION"));

    // Create the service
    let service = AidaService::new(state.clone());

    // Create and run the server
    Server::builder()
        .concurrency_limit_per_connection(args.max_connections)
        .add_service(RequirementsServiceServer::new(service))
        .serve_with_shutdown(addr, async move {
            // Wait for shutdown signal
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to install CTRL+C signal handler");
            info!("Received shutdown signal, stopping server...");
        })
        .await?;

    info!("Server stopped");
    Ok(())
}
