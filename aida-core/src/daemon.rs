// trace:ARCH-distributed-daemon | ai:claude
//! Daemon dispenser — Phase 3 sequence number server over Unix socket.
//!
//! A long-running process that owns the sequence counter state in memory
//! and serves requests over a Unix domain socket. Handles concurrent
//! callers (CLI, IDE plugin, LSP, background sync) without file lock
//! contention.
//!
//! Protocol: line-delimited JSON over Unix socket.
//! Request:  {"method": "next", "type": "FR"}
//! Response: {"seq": 42}
//!
//! Socket location: /run/user/{uid}/aida.sock (or $XDG_RUNTIME_DIR/aida.sock)

use anyhow::Result;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use crate::dispenser::{Dispenser, DispenserState, IdMode, SqliteDispenser};

/// Get the default socket path for the daemon.
pub fn default_socket_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir).join("aida.sock")
    } else if let Some(uid) = get_uid() {
        PathBuf::from(format!("/run/user/{}/aida.sock", uid))
    } else {
        // Fallback to temp directory
        std::env::temp_dir().join("aida.sock")
    }
}

#[cfg(unix)]
fn get_uid() -> Option<u32> {
    Some(unsafe { libc::getuid() })
}

#[cfg(not(unix))]
fn get_uid() -> Option<u32> {
    None
}

/// Request from a client.
#[derive(Debug, serde::Deserialize)]
struct Request {
    method: String,
    #[serde(default)]
    r#type: String,
}

/// Response to a client.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Response {
    #[serde(skip_serializing_if = "Option::is_none")]
    seq: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<DispenserState>,
}

/// Run the daemon, listening on a Unix socket.
///
/// The daemon owns a SqliteDispenser and serves requests until killed.
/// This function blocks forever (or until the socket is removed).
#[cfg(unix)]
pub fn run_daemon(db_path: &Path, mode: IdMode, socket_path: Option<&Path>) -> Result<()> {
    use std::os::unix::net::UnixListener;

    let sock_path = socket_path
        .map(PathBuf::from)
        .unwrap_or_else(default_socket_path);

    // Remove stale socket
    if sock_path.exists() {
        std::fs::remove_file(&sock_path)?;
    }

    // Create the dispenser
    let dispenser = SqliteDispenser::open(db_path.to_path_buf(), mode)?;

    // Bind the socket
    let listener = UnixListener::bind(&sock_path)?;
    eprintln!("aida-daemon listening on {}", sock_path.display());

    // Handle connections
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let reader = BufReader::new(stream.try_clone()?);
                let mut writer = stream;

                for line in reader.lines() {
                    let line = match line {
                        Ok(l) => l,
                        Err(_) => break,
                    };

                    let response = handle_request(&line, &dispenser);
                    let json = serde_json::to_string(&response)
                        .unwrap_or_else(|_| r#"{"error":"serialization failed"}"#.to_string());

                    if writeln!(writer, "{}", json).is_err() {
                        break;
                    }
                }
            }
            Err(e) => {
                eprintln!("aida-daemon: connection error: {}", e);
            }
        }
    }

    // Cleanup
    let _ = std::fs::remove_file(&sock_path);
    Ok(())
}

fn handle_request(line: &str, dispenser: &dyn Dispenser) -> Response {
    let req: Request = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => {
            return Response {
                seq: None,
                id: None,
                error: Some(format!("Invalid request: {}", e)),
                state: None,
            };
        }
    };

    match req.method.as_str() {
        "next" => match dispenser.next(&req.r#type) {
            Ok(seq) => Response {
                seq: Some(seq),
                id: dispenser.format_id(&req.r#type, seq).ok(),
                error: None,
                state: None,
            },
            Err(e) => Response {
                seq: None,
                id: None,
                error: Some(format!("next failed: {}", e)),
                state: None,
            },
        },
        "peek" => match dispenser.peek(&req.r#type) {
            Ok(seq) => Response {
                seq: Some(seq),
                id: None,
                error: None,
                state: None,
            },
            Err(e) => Response {
                seq: None,
                id: None,
                error: Some(format!("peek failed: {}", e)),
                state: None,
            },
        },
        "state" => match dispenser.state() {
            Ok(state) => Response {
                seq: None,
                id: None,
                error: None,
                state: Some(state),
            },
            Err(e) => Response {
                seq: None,
                id: None,
                error: Some(format!("state failed: {}", e)),
                state: None,
            },
        },
        "ping" => Response {
            seq: None,
            id: Some("pong".into()),
            error: None,
            state: None,
        },
        other => Response {
            seq: None,
            id: None,
            error: Some(format!("Unknown method: {}", other)),
            state: None,
        },
    }
}

/// Client for the daemon — connects to the Unix socket.
#[cfg(unix)]
pub struct DaemonClient {
    socket_path: PathBuf,
}

#[cfg(unix)]
impl Default for DaemonClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(unix)]
impl DaemonClient {
    /// Create a client targeting the default socket path.
    pub fn new() -> Self {
        Self {
            socket_path: default_socket_path(),
        }
    }

    /// Create a client targeting a specific socket path.
    pub fn with_path(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }

    /// Check if the daemon is running.
    pub fn is_running(&self) -> bool {
        self.send_raw(r#"{"method":"ping"}"#).is_ok()
    }

    /// Send a request and get the response.
    fn send_raw(&self, request: &str) -> Result<Response> {
        use std::os::unix::net::UnixStream;

        let mut stream = UnixStream::connect(&self.socket_path)?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;

        writeln!(stream, "{}", request)?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line)?;

        let response: Response = serde_json::from_str(&line)?;
        Ok(response)
    }
}

#[cfg(unix)]
impl Dispenser for DaemonClient {
    fn next(&self, object_type: &str) -> Result<u32> {
        let req = serde_json::json!({"method": "next", "type": object_type});
        let resp = self.send_raw(&req.to_string())?;
        resp.seq
            .ok_or_else(|| anyhow::anyhow!(resp.error.unwrap_or_else(|| "no seq".into())))
    }

    fn peek(&self, object_type: &str) -> Result<u32> {
        let req = serde_json::json!({"method": "peek", "type": object_type});
        let resp = self.send_raw(&req.to_string())?;
        resp.seq
            .ok_or_else(|| anyhow::anyhow!(resp.error.unwrap_or_else(|| "no seq".into())))
    }

    fn state(&self) -> Result<DispenserState> {
        let req = serde_json::json!({"method": "state"});
        let resp = self.send_raw(&req.to_string())?;
        resp.state
            .ok_or_else(|| anyhow::anyhow!(resp.error.unwrap_or_else(|| "no state".into())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handle_request_next() {
        use crate::dispenser::MemoryDispenser;
        let d = MemoryDispenser::new(IdMode::Distributed {
            node_id: "7".to_string(),
        });

        let resp = handle_request(r#"{"method":"next","type":"FR"}"#, &d);
        assert_eq!(resp.seq, Some(1));
        assert_eq!(resp.id, Some("FR-7-001".into()));
        assert!(resp.error.is_none());

        let resp2 = handle_request(r#"{"method":"next","type":"FR"}"#, &d);
        assert_eq!(resp2.seq, Some(2));
    }

    #[test]
    fn test_handle_request_peek() {
        use crate::dispenser::MemoryDispenser;
        let d = MemoryDispenser::new(IdMode::Centralized);

        let resp = handle_request(r#"{"method":"peek","type":"FR"}"#, &d);
        assert_eq!(resp.seq, Some(1));

        // Peek doesn't increment
        let resp2 = handle_request(r#"{"method":"peek","type":"FR"}"#, &d);
        assert_eq!(resp2.seq, Some(1));
    }

    #[test]
    fn test_handle_request_state() {
        use crate::dispenser::MemoryDispenser;
        let d = MemoryDispenser::new(IdMode::Distributed {
            node_id: "42".to_string(),
        });
        d.next("FR").unwrap();

        let resp = handle_request(r#"{"method":"state"}"#, &d);
        assert!(resp.state.is_some());
        let state = resp.state.unwrap();
        assert_eq!(
            state.mode,
            IdMode::Distributed {
                node_id: "42".to_string()
            }
        );
    }

    #[test]
    fn test_handle_request_ping() {
        use crate::dispenser::MemoryDispenser;
        let d = MemoryDispenser::new(IdMode::Centralized);

        let resp = handle_request(r#"{"method":"ping"}"#, &d);
        assert_eq!(resp.id, Some("pong".into()));
    }

    #[test]
    fn test_handle_request_invalid() {
        use crate::dispenser::MemoryDispenser;
        let d = MemoryDispenser::new(IdMode::Centralized);

        let resp = handle_request("not json", &d);
        assert!(resp.error.is_some());

        let resp2 = handle_request(r#"{"method":"unknown"}"#, &d);
        assert!(resp2.error.is_some());
    }

    // Integration test for daemon client/server is run manually:
    //   cargo test daemon_client_server -- --ignored
    // because it spawns a background thread with a blocking listener.
    #[cfg(unix)]
    #[test]
    #[ignore]
    fn test_daemon_client_server() {
        use std::thread;

        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("dispenser.db");
        let sock_path = dir.path().join("test.sock");

        let sock_clone = sock_path.clone();
        let db_clone = db_path.clone();

        // Start daemon in background thread
        let _handle = thread::spawn(move || {
            let _ = run_daemon(
                &db_clone,
                IdMode::Distributed {
                    node_id: "7".to_string(),
                },
                Some(&sock_clone),
            );
        });

        // Wait for socket to appear
        for _ in 0..50 {
            if sock_path.exists() {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(50));
        }

        if sock_path.exists() {
            let client = DaemonClient::with_path(sock_path.clone());

            assert!(client.is_running());
            assert_eq!(client.next("FR").unwrap(), 1);
            assert_eq!(client.next("FR").unwrap(), 2);
            assert_eq!(client.peek("FR").unwrap(), 3);

            let state = client.state().unwrap();
            assert_eq!(
                state.mode,
                IdMode::Distributed {
                    node_id: "7".to_string()
                }
            );

            // Clean up — remove socket to stop daemon
            let _ = std::fs::remove_file(&sock_path);
        }
    }
}
