//! Lane-side typed transport. Endpoint selection conveys no host authority.
//! No retries, subprocesses, store writes, cache setup or direct-write fallback.
// trace:TASK-3223 | ai:codex
use aida_core::db::canonical_mutation::CanonicalOperation;
use aida_core::exec_context::Environment;
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};
use uuid::Uuid;

const LIMIT: usize = 65536;
const TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct Client {
    endpoint: String,
    epoch: Uuid,
}

impl Client {
    /// Capture inputs at the entry point, before ordinary CLI/MCP setup. A
    /// missing endpoint in a selected profile is an error, never compatibility.
    pub(crate) fn from_environment(env: &Environment) -> Result<Option<Self>> {
        match env.var("AIDA_STORE_MUTATION_PROFILE") {
            Err(aida_core::exec_context::EnvLookupError::Missing) | Ok("compatibility") => Ok(None),
            Ok("mediated") => Self::new(env.var("AIDA_STORE_MUTATION_ENDPOINT")?).map(Some),
            _ => anyhow::bail!("invalid store mutation profile"),
        }
    }

    fn new(endpoint: &str) -> Result<Self> {
        let epoch = endpoint
            .strip_prefix("/run/aida-mutation/")
            .and_then(|s| s.strip_suffix("/service"))
            .context("invalid store service endpoint")?;
        let epoch = Uuid::parse_str(epoch)?;
        ensure!(
            endpoint == format!("/run/aida-mutation/{epoch}/service"),
            "invalid store service endpoint"
        );
        Ok(Self {
            endpoint: endpoint.into(),
            epoch,
        })
    }

    /// Read only the canonical preimage; never construct a cached backend. The
    /// host independently verifies identity, ownership and this exact revision.
    pub(crate) fn comment(&self, root: &std::path::Path, id: &str, content: &str) -> Result<Value> {
        let sid = aida_core::object_store::canonical_spec_id(id);
        let bytes = aida_core::db::redo::object_bytes(root, &sid)?;
        let operation = CanonicalOperation::CommentAdd {
            spec_id: sid,
            expected_revision: aida_core::db::redo::hash(&bytes),
            content: content.into(),
        };
        self.mutate(Uuid::new_v4(), &operation)
    }

    /// Resolve comment prefixes against the exact revision sent to the host.
    /// The host applies the UUID only after its revision compare succeeds.
    pub(crate) fn change_comment(
        &self,
        root: &std::path::Path,
        id: &str,
        query: &str,
        content: Option<&str>,
    ) -> Result<Value> {
        let sid = aida_core::object_store::canonical_spec_id(id);
        let bytes = aida_core::db::redo::object_bytes(root, &sid)?;
        let row = serde_yaml::from_slice(&bytes)?;
        let comment_id = crate::rel_cards::resolve_comment_uuid(&row, query)?;
        let expected_revision = aida_core::db::redo::hash(&bytes);
        let operation = match content {
            Some(content) => CanonicalOperation::CommentEdit {
                spec_id: sid,
                expected_revision,
                comment_id,
                content: content.into(),
            },
            None => CanonicalOperation::CommentDelete {
                spec_id: sid,
                expected_revision,
                comment_id,
            },
        };
        self.mutate(Uuid::new_v4(), &operation)
    }

    pub(crate) fn edit(
        &self,
        root: &std::path::Path,
        id: &str,
        fields: aida_core::db::canonical_mutation::EditFields,
    ) -> Result<Value> {
        let sid = aida_core::object_store::canonical_spec_id(id);
        let bytes = aida_core::db::redo::object_bytes(root, &sid)?;
        self.mutate(
            Uuid::new_v4(),
            &CanonicalOperation::SpecEdit {
                spec_id: sid,
                expected_revision: aida_core::db::redo::hash(&bytes),
                fields,
            },
        )
    }

    /// Closed MCP translation: unknown fields and unsupported semantics refuse
    /// before opening the channel or reading the canonical preimage.
    pub(crate) fn mcp(&self, root: &std::path::Path, name: &str, args: &Value) -> Result<Value> {
        let object = args.as_object().context("InvalidRequest")?;
        let allowed: &[&str] = match name {
            "add_comment" => &["id", "text"],
            "update_requirement" => &["id", "title", "description"],
            _ => anyhow::bail!("UnsupportedOperation"),
        };
        ensure!(
            object.keys().all(|key| allowed.contains(&key.as_str())),
            "UnsupportedOperation"
        );
        let id = args["id"].as_str().context("InvalidRequest")?;
        match name {
            "add_comment" => {
                self.comment(root, id, args["text"].as_str().context("InvalidRequest")?)
            }
            _ => {
                let mut fields = object.clone();
                fields.remove("id");
                let fields = serde_json::from_value(Value::Object(fields))
                    .map_err(|_| anyhow::anyhow!("InvalidRequest"))?;
                self.edit(root, id, fields)
            }
        }
    }

    /// A caller retains this UUID after uncertain delivery. Retrying uses the
    /// same serialized operation and UUID; it must never generate a fresh one.
    pub(crate) fn mutate(&self, request_id: Uuid, operation: &CanonicalOperation) -> Result<Value> {
        operation.validate()?;
        let op = serde_json::to_value(operation)?;
        self.request(request_id, op["op"].as_str().unwrap(), op["data"].clone())
    }

    pub(crate) fn status(&self, request_id: Uuid) -> Result<Value> {
        self.request(
            Uuid::new_v4(),
            "request_status",
            json!({"request_id":request_id}),
        )
    }

    fn request(&self, request_id: Uuid, op: &str, data: Value) -> Result<Value> {
        let request = json!({
            "version":1,"launch_epoch":self.epoch,"request_id":request_id,"op":op,"data":data
        });
        let raw = serde_json::to_vec(&request)?;
        ensure!(raw.len() <= LIMIT, "InvalidRequest");
        let result: Result<Value> = (|| {
            let channel = connect(&self.endpoint)?;
            let result = exchange(&channel, &raw, self.epoch, request_id, TIMEOUT)?;
            validate_result(op, &request["data"], &result)?;
            Ok(result)
        })();
        result.with_context(|| {
            format!("store service request {request_id}; do not repeat with a new request ID")
        })
    }
}

fn validate_result(op: &str, data: &Value, result: &Value) -> Result<()> {
    let object = result.as_object().context("OutcomeUnknown")?;
    if op == "request_status" {
        ensure!(
            object.len() == 1 && matches!(result["state"].as_str(), Some("completed" | "unknown")),
            "OutcomeUnknown"
        );
    } else {
        ensure!(
            object.len() == 3
                && object.contains_key("comment_id")
                && result["spec_id"] == data["spec_id"],
            "OutcomeUnknown"
        );
        let revision = result["revision"].as_str().context("OutcomeUnknown")?;
        ensure!(
            revision.len() == 64
                && revision
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "OutcomeUnknown"
        );
        if matches!(op, "comment_add" | "comment_edit" | "comment_delete") {
            ensure!(
                result["comment_id"]
                    .as_str()
                    .and_then(|id| Uuid::parse_str(id).ok())
                    .is_some(),
                "OutcomeUnknown"
            );
            if op != "comment_add" {
                ensure!(result["comment_id"] == data["comment_id"], "OutcomeUnknown");
            }
        } else {
            ensure!(
                op == "spec_edit" && result["comment_id"].is_null(),
                "OutcomeUnknown"
            );
        }
    }
    Ok(())
}

fn connect(endpoint: &str) -> Result<OwnedFd> {
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    ensure!(fd >= 0, "Unavailable");
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    ensure!(endpoint.len() < addr.sun_path.len(), "invalid endpoint");
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (target, byte) in addr.sun_path.iter_mut().zip(endpoint.bytes()) {
        *target = byte as libc::c_char;
    }
    let result = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&addr as *const libc::sockaddr_un).cast(),
            std::mem::size_of_val(&addr) as libc::socklen_t,
        )
    };
    // A local nonblocking SEQPACKET connect either completes or refuses. A full
    // listener backlog is Unavailable, with no request sent and no fallback.
    ensure!(result == 0, "Unavailable");
    Ok(fd)
}

fn ready(fd: &OwnedFd, events: libc::c_short, until: Instant) -> Result<()> {
    loop {
        let remaining = until.saturating_duration_since(Instant::now());
        ensure!(!remaining.is_zero(), "OutcomeUnknown");
        let mut pfd = libc::pollfd {
            fd: fd.as_raw_fd(),
            events,
            revents: 0,
        };
        let result = unsafe {
            libc::poll(
                &mut pfd,
                1,
                remaining.as_millis().max(1).min(i32::MAX as u128) as i32,
            )
        };
        if result < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        ensure!(result > 0 && pfd.revents & events != 0, "OutcomeUnknown");
        return Ok(());
    }
}

fn exchange(fd: &OwnedFd, raw: &[u8], epoch: Uuid, id: Uuid, timeout: Duration) -> Result<Value> {
    let until = Instant::now() + timeout;
    ready(fd, libc::POLLOUT, until)?;
    // write(2) preserves the sender's kernel credentials in the broker. Do not
    // replace this with sendmsg: that goes through the generic syscall deputy.
    let sent = unsafe { libc::write(fd.as_raw_fd(), raw.as_ptr().cast(), raw.len()) };
    ensure!(sent == raw.len() as isize, "OutcomeUnknown");
    ready(fd, libc::POLLIN, until)?;
    let mut bytes = vec![0u8; LIMIT];
    let mut control = [0usize; 64];
    let mut iov = libc::iovec {
        iov_base: bytes.as_mut_ptr().cast(),
        iov_len: bytes.len(),
    };
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = std::mem::size_of_val(&control);
    let received = unsafe {
        libc::recvmsg(
            fd.as_raw_fd(),
            &mut message,
            libc::MSG_CMSG_CLOEXEC | libc::MSG_DONTWAIT,
        )
    };
    // Reject all ancillary data, closing delivered rights even on a malformed
    // or truncated reply. Never retain a descriptor supplied by a peer.
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&message);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let size = (*cmsg).cmsg_len.saturating_sub(libc::CMSG_LEN(0) as usize);
                for i in 0..size / std::mem::size_of::<libc::c_int>() {
                    libc::close(std::ptr::read_unaligned(
                        libc::CMSG_DATA(cmsg).cast::<libc::c_int>().add(i),
                    ));
                }
            }
            cmsg = libc::CMSG_NXTHDR(&message, cmsg);
        }
    }
    ensure!(
        received > 0
            && message.msg_controllen == 0
            && message.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) == 0,
        "OutcomeUnknown"
    );
    decode_reply(&bytes[..received as usize], epoch, id)
}

fn decode_reply(raw: &[u8], epoch: Uuid, id: Uuid) -> Result<Value> {
    #[derive(serde::Deserialize)]
    #[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
    enum Reply {
        Success {
            version: u32,
            launch_epoch: Uuid,
            request_id: Uuid,
            result: Value,
        },
        Error {
            version: u32,
            launch_epoch: Uuid,
            request_id: Uuid,
            error: String,
        },
    }
    let reply: Reply =
        serde_json::from_slice(raw).map_err(|_| anyhow::anyhow!("OutcomeUnknown"))?;
    let (version, actual_epoch, actual_id) = match &reply {
        Reply::Success {
            version,
            launch_epoch,
            request_id,
            ..
        }
        | Reply::Error {
            version,
            launch_epoch,
            request_id,
            ..
        } => (*version, *launch_epoch, *request_id),
    };
    ensure!(
        version == 1 && actual_epoch == epoch && actual_id == id,
        "OutcomeUnknown"
    );
    match reply {
        Reply::Success { result, .. } => Ok(result),
        Reply::Error { error, .. } => {
            ensure!(
                [
                    "InvalidRequest",
                    "UnsupportedVersion",
                    "UnsupportedOperation",
                    "Unauthorized",
                    "Expired",
                    "Revoked",
                    "Conflict",
                    "Busy",
                    "Unavailable",
                    "OutcomeUnknown"
                ]
                .contains(&error.as_str()),
                "OutcomeUnknown"
            );
            anyhow::bail!("{error}")
        }
    }
}

#[cfg(test)]
#[path = "store_mutation_client_tests.rs"]
mod tests;
