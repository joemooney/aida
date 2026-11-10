//! Disabled process-observation service, rollout slice 1.
//!
//! There is deliberately no CLI registration, launcher, client adapter, receipt
//! persistence or observation dispatch. The staged transport is exercised only
//! by protocol tests. Enabling it requires the pending manual rollout gate and
//! separately reviewed scope/grant/handle authorization. A kernel-authenticated
//! sender is not a role or holder. No sandbox socket route is installed.
// trace:TASK-3368 | ai:codex

#[cfg(test)]
mod tests;
mod transport;

use aida_core::proc_identity::ProcIdentity;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

const MAX_FRAME: usize = 64 * 1024;
const MAX_NONCES: usize = 128;
const DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// Unconditional refusal, independent of configuration or environment. Nothing
/// calls this from a consumer or launch path in slice 1.
pub(crate) fn run() -> Result<()> {
    bail!("process identity broker disabled: manual rollout gate pending")
}

// Keep the wire tuple closed without changing the shared legacy schema.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    boot_id: Uuid,
    pidns_inode: u64,
    pid: u32,
    starttime: u64,
}
impl Identity {
    fn checked(&self) -> Result<ProcIdentity> {
        if self.boot_id.is_nil()
            || self.pidns_inode == 0
            || self.pid == 0
            || self.pid > i32::MAX as u32
            || self.starttime == 0
        {
            bail!("invalid identity");
        }
        Ok(ProcIdentity {
            boot_id: self.boot_id.hyphenated().to_string(),
            pidns_inode: self.pidns_inode,
            pid: self.pid,
            starttime: self.starttime,
        })
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u8,
    nonce: Uuid,
    project: String,
    store: String,
    operation: Operation,
    subject: Identity,
    holder_generation: Uuid,
    grant: Option<Uuid>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    ObserveSelf,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Refusal {
    Disabled,
    Schema,
    Scope,
    Replay,
    SessionFull,
    NamespaceMismatch,
    IdentityMismatch,
}

/// A refusal is not a receipt: no Alive/Dead verdict, role, handle or lease can
/// be derived from this response. Future observation responses need a separate
/// reviewed type with the complete incarnation/node/boot/sequence bindings.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    version: u8,
    nonce: Option<Uuid>,
    refusal: Refusal,
}

struct Session {
    nonces: HashSet<Uuid>,
    closed: bool,
}
impl Session {
    fn new() -> Self {
        Self {
            nonces: HashSet::new(),
            closed: false,
        }
    }

    // Scope is supplied by the host service, never inferred from a request or
    // environment. This staged protocol never opens the named project/store.
    fn dispatch(
        &mut self,
        bytes: &[u8],
        sender: &ProcIdentity,
        project: &str,
        store: &str,
    ) -> Response {
        let reply = |nonce, refusal| Response {
            version: 1,
            nonce,
            refusal,
        };
        if self.closed || self.nonces.len() >= MAX_NONCES {
            self.closed = true;
            return reply(None, Refusal::SessionFull);
        }
        if bytes.len() > MAX_FRAME {
            return reply(None, Refusal::Schema);
        }
        let request: Request = match serde_json::from_slice(bytes) {
            Ok(r) => r,
            _ => return reply(None, Refusal::Schema),
        };
        let nonce = Some(request.nonce);
        let subject = match request.subject.checked() {
            Ok(id)
                if request.version == 1
                    && !request.nonce.is_nil()
                    && !request.holder_generation.is_nil() =>
            {
                id
            }
            _ => return reply(nonce, Refusal::Schema),
        };
        if !self.nonces.insert(request.nonce) {
            return reply(nonce, Refusal::Replay);
        }
        if request.project != project || request.store != store {
            return reply(nonce, Refusal::Scope);
        }
        if subject.pidns_inode != sender.pidns_inode {
            return reply(nonce, Refusal::NamespaceMismatch);
        }
        if &subject != sender {
            return reply(nonce, Refusal::IdentityMismatch);
        }
        // Even a correct self request remains disabled. In particular a grant
        // reference is NOT validated or treated as authorization in this slice.
        reply(nonce, Refusal::Disabled)
    }
}
