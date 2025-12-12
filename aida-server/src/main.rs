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

    // Build CORS layer
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
            header::HeaderName::from_static("grpc-timeout"),
        ])
        .allow_origin(Any)
        .expose_headers([
            header::HeaderName::from_static("grpc-status"),
            header::HeaderName::from_static("grpc-message"),
        ]);

    // Build the gRPC server address
    let grpc_addr: SocketAddr = format!("{}:{}", args.host, args.port).parse()?;

    info!("Starting AIDA server v{}", env!("CARGO_PKG_VERSION"));
    info!("gRPC/gRPC-Web: http://{}", grpc_addr);

    // Create the gRPC service with gRPC-Web support
    let grpc_service = AidaService::new(state.clone());
    let grpc_service = RequirementsServiceServer::new(grpc_service);
    let grpc_web_service = tonic_web::enable(grpc_service);

    // Start REST server if enabled
    let rest_handle = if args.rest_port > 0 {
        let rest_addr: SocketAddr = format!("{}:{}", args.host, args.rest_port).parse()?;
        info!("REST API: http://{}/api", rest_addr);

        let rest_router = rest::create_rest_router(state.clone()).layer(cors.clone());

        let rest_listener = tokio::net::TcpListener::bind(rest_addr).await?;
        Some(tokio::spawn(async move {
            axum::serve(rest_listener, rest_router).await
        }))
    } else {
        info!("REST API: disabled");
        None
    };

    // Create and run the gRPC server with gRPC-Web support
    Server::builder()
        .accept_http1(true) // Required for gRPC-Web
        .layer(cors)
        .layer(tonic_web::GrpcWebLayer::new())
        .concurrency_limit_per_connection(args.max_connections)
        .add_service(grpc_web_service)
        .serve_with_shutdown(grpc_addr, async move {
            // Wait for shutdown signal
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to install CTRL+C signal handler");
            info!("Received shutdown signal, stopping server...");
        })
        .await?;

    // Wait for REST server to finish if running
    if let Some(handle) = rest_handle {
        let _ = handle.await;
    }

    info!("Server stopped");
    Ok(())
}
