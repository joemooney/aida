// trace:FR-0278 | ai:claude:high
//! Embedded server wrapper for native desktop builds
//!
//! This module provides functionality to start an embedded AIDA server
//! as a subprocess for local storage access via gRPC.

use anyhow::{Context, Result};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

/// Embedded server that runs as a subprocess
///
/// When dropped, the server process is terminated.
pub struct EmbeddedServer {
    process: Child,
    port: u16,
    db_path: PathBuf,
}

impl EmbeddedServer {
    /// Start an embedded server for the given database path
    ///
    /// This spawns the aida-server binary as a subprocess, binding to
    /// a random available port on localhost.
    pub fn start(db_path: PathBuf) -> Result<Self> {
        // Find an available port
        let port = find_available_port()
            .context("Failed to find available port for embedded server")?;

        // Find the server binary
        let server_binary = find_server_binary()
            .context("Failed to find aida-server binary")?;

        // Start the server process
        let process = Command::new(&server_binary)
            .arg("--port")
            .arg(port.to_string())
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--rest-port")
            .arg("0") // Disable REST API for embedded mode
            .arg("--database")
            .arg(&db_path)
            .arg("--log-level")
            .arg("warn") // Quieter logging for embedded mode
            .spawn()
            .with_context(|| format!("Failed to start embedded server: {}", server_binary.display()))?;

        let embedded = EmbeddedServer {
            process,
            port,
            db_path,
        };

        // Wait for server to be ready
        embedded.wait_for_ready(Duration::from_secs(10))?;

        Ok(embedded)
    }

    /// Get the server address (host:port)
    pub fn address(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    /// Get the database path
    #[allow(dead_code)]
    pub fn db_path(&self) -> &PathBuf {
        &self.db_path
    }

    /// Get the port number
    #[allow(dead_code)]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Wait for the server to be ready to accept connections
    fn wait_for_ready(&self, timeout: Duration) -> Result<()> {
        let start = std::time::Instant::now();
        let addr = format!("127.0.0.1:{}", self.port);

        loop {
            // Try to connect
            if std::net::TcpStream::connect(&addr).is_ok() {
                return Ok(());
            }

            // Check if we've timed out
            if start.elapsed() > timeout {
                anyhow::bail!(
                    "Embedded server failed to start within {:?} on port {}",
                    timeout,
                    self.port
                );
            }

            // Wait a bit before retrying
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for EmbeddedServer {
    fn drop(&mut self) {
        // Attempt graceful shutdown first
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Send SIGTERM
            unsafe {
                libc::kill(self.process.id() as i32, libc::SIGTERM);
            }
            // Give it a moment to shut down gracefully
            std::thread::sleep(Duration::from_millis(500));
        }

        // Force kill if still running
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

/// Find an available port for the embedded server
fn find_available_port() -> Result<u16> {
    // Bind to port 0 to get a random available port
    let listener = TcpListener::bind("127.0.0.1:0")
        .context("Failed to bind to ephemeral port")?;
    let port = listener.local_addr()
        .context("Failed to get local address")?
        .port();
    // Listener is dropped here, freeing the port
    Ok(port)
}

/// Find the aida-server binary
fn find_server_binary() -> Result<PathBuf> {
    // Try several locations
    let candidates = [
        // Same directory as the current executable
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("aida-server"))),
        // Current directory
        Some(PathBuf::from("./aida-server")),
        // Common development locations
        Some(PathBuf::from("./target/debug/aida-server")),
        Some(PathBuf::from("./target/release/aida-server")),
        // System PATH
        which_aida_server(),
    ];

    for candidate in candidates.into_iter().flatten() {
        if candidate.exists() {
            return Ok(candidate);
        }
        // Also try with .exe extension on Windows
        #[cfg(windows)]
        {
            let with_exe = candidate.with_extension("exe");
            if with_exe.exists() {
                return Ok(with_exe);
            }
        }
    }

    anyhow::bail!(
        "Could not find aida-server binary. Ensure it is built and either:\n\
         - In the same directory as aida-gui\n\
         - In ./target/debug/ or ./target/release/\n\
         - In your system PATH"
    )
}

/// Try to find aida-server in PATH
fn which_aida_server() -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .filter_map(|dir| {
                let candidate = dir.join("aida-server");
                if candidate.exists() {
                    Some(candidate)
                } else {
                    #[cfg(windows)]
                    {
                        let with_exe = candidate.with_extension("exe");
                        if with_exe.exists() {
                            return Some(with_exe);
                        }
                    }
                    None
                }
            })
            .next()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_available_port() {
        let port = find_available_port().expect("Should find an available port");
        assert!(port > 0);
    }
}
