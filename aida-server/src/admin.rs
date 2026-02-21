// trace:TASK-0373 | ai:claude
//! Admin endpoints for dev-mode server management.
//!
//! Provides server status and a rebuild+restart SSE endpoint,
//! gated behind the AIDA_DEV_MODE=1 environment variable.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{
        sse::{Event, Sse},
        IntoResponse, Json,
    },
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncBufReadExt;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{info, warn};

// ============================================================================
// State & types
// ============================================================================

pub struct AdminState {
    pub dev_mode: bool,
    pub version: String,
    pub start_time: Instant,
    pub building: AtomicBool,
}

impl AdminState {
    pub fn new(dev_mode: bool) -> Self {
        Self {
            dev_mode,
            version: env!("CARGO_PKG_VERSION").to_string(),
            start_time: Instant::now(),
            building: AtomicBool::new(false),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminStatusResponse {
    dev_mode: bool,
    version: String,
    uptime_seconds: u64,
    building: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SseStatusData {
    phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
}

#[derive(Serialize)]
struct SseLogData {
    line: String,
    stream: String,
}

#[derive(Deserialize)]
struct RebuildQuery {
    #[serde(default)]
    restart: Option<bool>,
}

// ============================================================================
// Router
// ============================================================================

pub fn create_admin_router(admin_state: Arc<AdminState>) -> Router {
    Router::new()
        .route("/api/v2/admin/status", get(admin_status))
        .route("/api/v2/admin/rebuild", get(admin_rebuild_sse))
        .with_state(admin_state)
}

// ============================================================================
// Handlers
// ============================================================================

async fn admin_status(
    State(state): State<Arc<AdminState>>,
) -> Json<AdminStatusResponse> {
    Json(AdminStatusResponse {
        dev_mode: state.dev_mode,
        version: state.version.clone(),
        uptime_seconds: state.start_time.elapsed().as_secs(),
        building: state.building.load(Ordering::Relaxed),
    })
}

async fn admin_rebuild_sse(
    State(state): State<Arc<AdminState>>,
    Query(query): Query<RebuildQuery>,
) -> Result<Sse<ReceiverStream<Result<Event, std::convert::Infallible>>>, impl IntoResponse> {
    // Gate behind dev mode
    if !state.dev_mode {
        return Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "Dev mode is not enabled. Set AIDA_DEV_MODE=1" })),
        ));
    }

    // Prevent concurrent builds
    if state
        .building
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err((
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "A build is already in progress" })),
        ));
    }

    let restart = query.restart.unwrap_or(false);
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(256);

    // Spawn the build task
    let build_state = state.clone();
    tokio::spawn(async move {
        let result = run_build(&tx, restart).await;
        build_state.building.store(false, Ordering::SeqCst);
        if let Err(e) = result {
            warn!("Build task error: {}", e);
            let _ = send_status(&tx, "failed", None, None).await;
        }
    });

    Ok(Sse::new(ReceiverStream::new(rx)))
}

// ============================================================================
// Build logic
// ============================================================================

async fn send_event(
    tx: &tokio::sync::mpsc::Sender<Result<Event, std::convert::Infallible>>,
    event_type: &str,
    data: &str,
) -> Result<(), anyhow::Error> {
    tx.send(Ok(Event::default().event(event_type).data(data)))
        .await
        .map_err(|_| anyhow::anyhow!("SSE channel closed"))
}

async fn send_status(
    tx: &tokio::sync::mpsc::Sender<Result<Event, std::convert::Infallible>>,
    phase: &str,
    duration_ms: Option<u64>,
    exit_code: Option<i32>,
) -> Result<(), anyhow::Error> {
    let data = SseStatusData {
        phase: phase.to_string(),
        duration_ms,
        exit_code,
    };
    send_event(tx, "status", &serde_json::to_string(&data)?).await
}

async fn send_log(
    tx: &tokio::sync::mpsc::Sender<Result<Event, std::convert::Infallible>>,
    line: &str,
    stream: &str,
) -> Result<(), anyhow::Error> {
    let data = SseLogData {
        line: line.to_string(),
        stream: stream.to_string(),
    };
    send_event(tx, "log", &serde_json::to_string(&data)?).await
}

/// Find the workspace root by walking up from cwd looking for a Cargo.toml
/// that has an aida-server/ sibling directory.
fn find_workspace_root() -> Option<std::path::PathBuf> {
    // Check env var first
    if let Ok(root) = std::env::var("AIDA_WORKSPACE_ROOT") {
        let p = std::path::PathBuf::from(root);
        if p.join("Cargo.toml").exists() {
            return Some(p);
        }
    }

    // Walk up from cwd
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join("Cargo.toml").exists() && dir.join("aida-server").is_dir() {
            return Some(dir);
        }
        if !dir.pop() {
            break;
        }
    }

    // Walk up from the current binary location
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent()?.to_path_buf();
        loop {
            if dir.join("Cargo.toml").exists() && dir.join("aida-server").is_dir() {
                return Some(dir);
            }
            if !dir.pop() {
                break;
            }
        }
    }

    None
}

async fn run_build(
    tx: &tokio::sync::mpsc::Sender<Result<Event, std::convert::Infallible>>,
    restart: bool,
) -> Result<(), anyhow::Error> {
    let workspace_root = find_workspace_root().ok_or_else(|| {
        anyhow::anyhow!("Cannot find workspace root. Set AIDA_WORKSPACE_ROOT env var.")
    })?;

    info!("Starting cargo build in {:?}", workspace_root);
    send_status(tx, "building", None, None).await?;

    let build_start = Instant::now();

    let mut child = tokio::process::Command::new("cargo")
        .args(["build", "-p", "aida-server"])
        .current_dir(&workspace_root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    // Stream stdout
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let tx_out = tx.clone();
    let stdout_handle = tokio::spawn(async move {
        if let Some(stdout) = stdout {
            let reader = tokio::io::BufReader::new(stdout);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if send_log(&tx_out, &line, "stdout").await.is_err() {
                    break;
                }
            }
        }
    });

    let tx_err = tx.clone();
    let stderr_handle = tokio::spawn(async move {
        if let Some(stderr) = stderr {
            let reader = tokio::io::BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if send_log(&tx_err, &line, "stderr").await.is_err() {
                    break;
                }
            }
        }
    });

    // Wait for streams to finish
    let _ = tokio::join!(stdout_handle, stderr_handle);

    let status = child.wait().await?;
    let duration_ms = build_start.elapsed().as_millis() as u64;

    if status.success() {
        info!("Build succeeded in {}ms", duration_ms);
        send_status(tx, "success", Some(duration_ms), Some(0)).await?;

        if restart {
            send_status(tx, "restarting", Some(duration_ms), None).await?;

            // Determine the new binary path
            let binary_path = workspace_root
                .join("target")
                .join("debug")
                .join("aida-server");

            // Reconstruct the command-line args from the current process
            let args: Vec<String> = std::env::args().skip(1).collect();

            info!(
                "Restarting server: {} {}",
                binary_path.display(),
                args.join(" ")
            );

            // Build the replacement command with --force to reclaim ports
            let binary_str = binary_path.to_string_lossy().to_string();
            let mut all_args = args.clone();
            if !all_args.iter().any(|a| a == "--force" || a == "-f") {
                all_args.push("--force".to_string());
            }
            let args_str = all_args
                .iter()
                .map(|a| shell_escape(a))
                .collect::<Vec<_>>()
                .join(" ");

            // Use setsid to start a new session so the child survives parent exit.
            // sleep gives time for the current process to exit and release ports.
            let shell_cmd = format!(
                "sleep 2 && exec {} {}",
                shell_escape(&binary_str),
                args_str
            );

            std::process::Command::new("setsid")
                .args(["sh", "-c", &shell_cmd])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()?;

            // Give the SSE event time to flush
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;

            // Exit current process
            std::process::exit(0);
        }
    } else {
        let code = status.code().unwrap_or(-1);
        warn!("Build failed with exit code {} in {}ms", code, duration_ms);
        send_status(tx, "failed", Some(duration_ms), Some(code)).await?;
    }

    Ok(())
}

/// Simple shell escaping for arguments
fn shell_escape(s: &str) -> String {
    if s.contains(' ') || s.contains('\'') || s.contains('"') || s.contains('\\') {
        format!("'{}'", s.replace('\'', "'\\''"))
    } else {
        s.to_string()
    }
}
