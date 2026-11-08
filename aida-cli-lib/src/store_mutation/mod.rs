//! Launch-owned authorization and typed canonical-object mutation channel. No ambient
//! session environment is changed and no request supplies authority selectors.
// trace:TASK-3225 | ai:codex
use anyhow::{Context, Result};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

pub(super) const SOURCE: &str = include_str!("service.py");

// Closed host schema independently checked after the Python broker.
// trace:TASK-3223 | ai:codex
#[derive(serde::Deserialize)]
#[serde(
    tag = "op",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum HostOperation {
    CommentAdd {
        spec_id: String,
        expected_revision: String,
        content: String,
    },
    SpecEdit {
        spec_id: String,
        expected_revision: String,
        fields: aida_core::db::canonical_mutation::EditFields,
    },
    RequestStatus {
        request_id: uuid::Uuid,
    },
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct HostRequest {
    version: u32,
    launch_epoch: uuid::Uuid,
    request_id: uuid::Uuid,
    op: String,
    data: serde_json::Value,
}
fn read_operation(host: &mut UnixStream) -> Result<HostRequest> {
    let mut size = [0; 4];
    host.read_exact(&mut size)?;
    let size = u32::from_be_bytes(size) as usize;
    anyhow::ensure!(size > 0 && size <= 65536, "invalid host frame");
    let mut bytes = vec![0; size];
    host.read_exact(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}
fn host_operation(
    root: &Path,
    binding: &str,
    epoch: uuid::Uuid,
    specs: &[String],
    author: &str,
    request: &HostRequest,
    mut authorize: impl FnMut() -> Result<()>,
) -> Result<serde_json::Value> {
    use aida_core::db::{canonical_mutation::CanonicalOperation as Op, redo};
    anyhow::ensure!(
        request.version == 1 && request.launch_epoch == epoch,
        "Unauthorized"
    );
    anyhow::ensure!(request.op == "request_status", "UnsupportedOperation");
    let op: HostOperation =
        serde_json::from_value(serde_json::json!({"op":request.op,"data":request.data}))
            .map_err(|_| anyhow::anyhow!("InvalidRequest"))?;
    let op = match op {
        HostOperation::CommentAdd {
            spec_id,
            expected_revision,
            content,
        } => Op::CommentAdd {
            spec_id,
            expected_revision,
            content,
        },
        HostOperation::SpecEdit {
            spec_id,
            expected_revision,
            fields,
        } => Op::SpecEdit {
            spec_id,
            expected_revision,
            fields,
        },
        HostOperation::RequestStatus { request_id } => {
            authorize()?;
            return Ok(
                serde_json::json!({"state": if redo::status(root, binding, epoch, request_id)? {"completed"} else {"unknown"}}),
            );
        }
    };
    aida_core::db::GitBackend::new(root)?.mutate_canonical(
        binding,
        epoch,
        request.request_id,
        author,
        &op,
        || {
            authorize()?;
            let target: aida_core::Requirement =
                serde_yaml::from_slice(&redo::object_bytes(root, op.target().0)?)?;
            anyhow::ensure!(specs.contains(&target.id.to_string()), "Unauthorized");
            Ok(())
        },
    )
}

struct PinnedRoot {
    path: PathBuf,
    file: File,
}
impl PinnedRoot {
    fn open(path: &Path) -> Result<Self> {
        let path = std::fs::canonicalize(path)?;
        let file = File::open(&path)?;
        anyhow::ensure!(file.metadata()?.is_dir(), "service root is not a directory");
        Ok(Self { path, file })
    }
    fn unchanged(&self) -> bool {
        self.path
            .metadata()
            .and_then(|now| {
                self.file
                    .metadata()
                    .map(|old| now.dev() == old.dev() && now.ino() == old.ino())
            })
            .unwrap_or(false)
    }
}

/// Only finalized host Command assignments select this opt-in. The endpoint
/// and epoch are freshly generated here, never taken from the environment.
pub(super) struct Session {
    pub(super) config: serde_json::Value,
    pub(super) endpoint: String,
    pub(super) epoch: String,
    helper: UnixStream,
    stopped: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    worker: Arc<Mutex<Option<String>>>,
}
impl Session {
    pub(super) fn prepare(child: &std::process::Command, root: &Path) -> Result<Self> {
        use crate::gate::seat_authority::{self, GrantCheck};
        let explicit = |key: &str| {
            child.get_envs().find_map(|(k, v)| {
                (k == key)
                    .then(|| v.map(|v| v.to_string_lossy().into_owned()))
                    .flatten()
            })
        };
        let grant_id = explicit(seat_authority::GRANT_ENV)
            .filter(|s| !s.is_empty())
            .context("store service requires a nonempty explicit grant binding")?;
        let subject = explicit("AIDA_USER")
            .filter(|s| !s.is_empty())
            .context("store service requires explicit subject")?;
        let role = explicit("AIDA_SESSION_ROLE").context("store service requires explicit seat")?;
        let main = crate::main_worktree_root_from(root);
        let project = PinnedRoot::open(&main)?;
        let worktree = PinnedRoot::open(root)?;
        let store = PinnedRoot::open(
            &seat_authority::roster_store(&main).map_err(|e| anyhow::anyhow!("{e}"))?,
        )?;
        let GrantCheck::Valid(grant) = seat_authority::grant_check_for(&main, &grant_id, &subject)
        else {
            anyhow::bail!("store service requires a current validated grant");
        };
        anyhow::ensure!(
            grant.seat == role && !grant.binding.is_empty(),
            "store service seat/binding refused"
        );
        // Refuse ambiguous/missing ownership rather than borrowing a sibling lease.
        let mut leases = Vec::new();
        for entry in std::fs::read_dir(crate::leases::leases_dir(&main))? {
            let path = entry?.path();
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .context("invalid lease name")?;
            if !name.ends_with(".toml")
                || name.ends_with(".manifest.toml")
                || name.ends_with(".activity.toml")
            {
                continue;
            }
            // Unrelated review/claim and retired-worktree leases carry no
            // ownership evidence for this root. Refuse only matching ambiguity.
            let Ok(lease) = read_lease(&path) else {
                continue;
            };
            if lease.role.as_deref() != Some(role.as_str()) {
                continue;
            }
            let Ok(lease_root) = std::fs::canonicalize(&lease.worktree_path) else {
                continue;
            };
            if lease_root != worktree.path {
                continue;
            }
            anyhow::ensure!(
                path == crate::leases::lease_path(&main, &lease.id),
                "lease identity/path mismatch"
            );
            leases.push(lease);
        }
        anyhow::ensure!(
            leases.len() == 1,
            "store service requires one corroborated worktree lease"
        );
        let lease = &leases[0];
        let lease_path = crate::leases::lease_path(&main, &lease.id);
        let lease_id = lease.id.clone();
        let scope = lease.scope.clone();
        let started = lease.started_at;
        let graph =
            crate::load_store_for_lookup(&main).context("store service spec unavailable")?;
        let specs: Vec<_> = graph
            .requirements
            .iter()
            .filter(|r| {
                r.id.to_string() == scope
                    || r.spec_id.as_deref() == Some(&scope)
                    || r.agreed_id.as_deref() == Some(&scope)
            })
            .map(|r| r.id.to_string())
            .collect();
        anyhow::ensure!(
            specs.len() == 1,
            "store service requires an owned canonical spec"
        );
        let epoch = uuid::Uuid::new_v4().to_string();
        let endpoint = format!("/run/aida-mutation/{epoch}/service");
        let (helper, mut host) = UnixStream::pair()?;
        host.set_read_timeout(Some(Duration::from_millis(100)))?;
        host.set_write_timeout(Some(Duration::from_secs(5)))?;
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let parent = grant.parent_grant_id;
        let worker = Arc::new(Mutex::new(None::<String>));
        let bound_worker = worker.clone();
        let config = serde_json::json!({"epoch":epoch,"specs":specs,"role":role,
            "policy_version":1,"build":env!("CARGO_PKG_VERSION"),"authority_fd":helper.as_raw_fd()});
        let host_epoch = uuid::Uuid::parse_str(&epoch)?;
        let host_specs = specs.clone();
        let host_binding = format!("{}:{}:{}:{}", grant_id, subject, lease_id, started);
        let thread = std::thread::Builder::new()
            .name("store-service-authority".into())
            .spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    let mut request = [0u8; 1];
                    match host.read(&mut request) {
                        Ok(1) if request == [b'?'] || request == [b'M'] => {}
                        Err(e)
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) =>
                        {
                            continue
                        }
                        _ => break,
                    }
                    let valid = || -> Option<u8> {
                        if !project.unchanged()
                            || !worktree.unchanged()
                            || !store.unchanged()
                            || seat_authority::roster_store(&main)
                                .ok()
                                .and_then(|p| std::fs::canonicalize(p).ok())
                                .as_deref()
                                != Some(root_path(&store))
                        {
                            return None;
                        }
                        let Ok(live) = read_lease(&lease_path) else {
                            return None;
                        };
                        if live.id != lease_id
                            || live.scope != scope
                            || live.started_at != started
                            || live.role.as_deref() != Some(role.as_str())
                            || std::fs::canonicalize(&live.worktree_path).ok().as_deref()
                                != Some(root_path(&worktree))
                        {
                            return None;
                        }
                        match seat_authority::grant_check_for(&main, &grant_id, &subject) {
                            GrantCheck::Valid(g)
                                if g.seat == role
                                    && g.parent_grant_id == parent
                                    && !g.binding.is_empty()
                                    && bound_worker.lock().is_ok_and(|worker| {
                                        worker
                                            .as_ref()
                                            .is_none_or(|token| g.binding.contains(token))
                                    }) =>
                            {
                                Some(b'Y')
                            }
                            GrantCheck::Rejected { reason, .. } => Some(rejection_code(&reason)),
                            _ => None,
                        }
                    };
                    if request == [b'?'] {
                        if host.write_all(&[valid().unwrap_or(b'N')]).is_err() { break; }
                    } else {
                        // Partial or timed-out frames terminate the stream; no
                        // late byte can become a later authority response.
                        let _ = host.set_read_timeout(Some(Duration::from_secs(5)));
                        let Ok(request) = read_operation(&mut host) else { break; };
                        let result = host_operation(root_path(&store), &host_binding, host_epoch, &host_specs, &subject, &request, || {
                            match valid() { Some(b'Y') => Ok(()), Some(b'E') => anyhow::bail!("Expired"), Some(b'R') => anyhow::bail!("Revoked"), _ => anyhow::bail!("Unauthorized") }
                        });
                        let reply = match result {
                            Ok(result) => serde_json::json!({"launch_epoch":request.launch_epoch,"request_id":request.request_id,"result":result}),
                            Err(error) => {
                                let reason = error.to_string();
                                let code = if error.downcast_ref::<aida_core::db::redo::OutcomeUnknown>().is_some() { "OutcomeUnknown" }
                                    else if ["Conflict", "Expired", "Revoked", "Unauthorized", "Busy", "InvalidRequest", "UnsupportedOperation"].contains(&reason.as_str()) { &reason }
                                    else { "Unavailable" };
                                serde_json::json!({"launch_epoch":request.launch_epoch,"request_id":request.request_id,"error":code})
                            }
                        };
                        let Ok(bytes) = serde_json::to_vec(&reply) else { break; };
                        if bytes.len() > 65536 || host.write_all(&(bytes.len() as u32).to_be_bytes()).and_then(|_| host.write_all(&bytes)).is_err() { break; }
                        let _ = host.set_read_timeout(Some(Duration::from_millis(100)));
                    }
                }
            })?;
        Ok(Self {
            config,
            endpoint,
            epoch,
            helper,
            stopped,
            thread: Some(thread),
            worker,
        })
    }
    pub(super) fn bind_worker(&mut self, pid: u32, start: &str) -> Result<()> {
        *self
            .worker
            .lock()
            .map_err(|_| anyhow::anyhow!("service worker lock refused"))? =
            Some(worker_binding_token(pid, start)?);
        // Python's timeout socket shares O_NONBLOCK with this descriptor.
        // It is idle at the receipt barrier, so temporarily use blocking I/O
        // with host deadlines, then restore its timeout-compatible flags.
        self.helper.set_nonblocking(false)?;
        self.helper.set_read_timeout(Some(Duration::from_secs(5)))?;
        self.helper
            .set_write_timeout(Some(Duration::from_secs(5)))?;
        // The helper is held at the receipt barrier, so no other authority
        // request can race this host-owned check.
        let mut reply = [0u8; 1];
        if let Err(error) = self
            .helper
            .write_all(b"?")
            .and_then(|()| self.helper.read_exact(&mut reply))
        {
            // shutdown affects every duplicated endpoint, including Python's.
            // A timed-out reply must never satisfy a later authorization.
            let _ = self.helper.shutdown(std::net::Shutdown::Both);
            return Err(error.into());
        }
        self.helper.set_nonblocking(true)?;
        anyhow::ensure!(reply == [b'Y'], "service worker grant binding refused");
        Ok(())
    }
    pub(super) fn fd(&self) -> i32 {
        self.helper.as_raw_fd()
    }
}
// trace:TASK-3225 | ai:codex
fn worker_binding_token(pid: u32, receipt_start: &str) -> Result<String> {
    let identity = aida_core::proc_identity::ProcIdentity::capture(pid)
        .map_err(anyhow::Error::msg)
        .context("service worker disappeared")?;
    let start = format!("linux-starttime:{}", identity.starttime);
    anyhow::ensure!(
        start.strip_prefix("linux-starttime:") == Some(receipt_start),
        "service worker changed"
    );
    Ok(crate::gate::review_authority::author_process_token(
        "proc",
        pid,
        Some(&start),
    ))
}

fn read_lease(path: &Path) -> Result<crate::leases::SessionLease> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    let md = file.metadata()?;
    anyhow::ensure!(
        md.is_file()
            && md.nlink() == 1
            && md.uid() == unsafe { libc::getuid() }
            && md.len() <= 65536,
        "store service lease evidence refused"
    );
    let mut body = String::new();
    file.take(65537).read_to_string(&mut body)?;
    anyhow::ensure!(
        body.len() <= 65536,
        "store service lease evidence oversized"
    );
    Ok(toml::from_str(&body)?)
}

fn rejection_code(reason: &crate::gate::seat_authority::GrantRejection) -> u8 {
    use crate::gate::seat_authority::GrantRejection;
    match reason {
        GrantRejection::Expired { .. } => b'E',
        GrantRejection::Revoked { .. } => b'R',
        GrantRejection::Parent(parent) => rejection_code(parent),
        _ => b'N',
    }
}
fn root_path(root: &PinnedRoot) -> &Path {
    &root.path
}
impl Drop for Session {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        let _ = self.helper.shutdown(std::net::Shutdown::Both);
        if let Some(thread) = self.thread.take() {
            // No request can use the shut-down channel. Existing authority
            // readers may block on external storage; never make teardown wait
            // indefinitely for one. There is just one guardian per launch.
            let end = std::time::Instant::now() + Duration::from_millis(200);
            while !thread.is_finished() && std::time::Instant::now() < end {
                std::thread::sleep(Duration::from_millis(5));
            }
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // trace:TASK-3225 | ai:codex
    #[test]
    fn task_3225_worker_binding_uses_canonical_start_identity() {
        let pid = std::process::id();
        let start = aida_core::liveness::process_start_identity(pid).unwrap();
        let raw = start.strip_prefix("linux-starttime:").unwrap();
        assert_eq!(
            worker_binding_token(pid, raw).unwrap(),
            crate::gate::review_authority::author_process_token("proc", pid, Some(&start))
        );
        assert!(worker_binding_token(pid, "changed").is_err());
    }
    // A worker is a child, not a cooperative holder captured by the host itself.
    // trace:TASK-3225 | ai:codex
    #[test]
    fn task_3225_worker_binding_captures_child_and_refuses_exited_process() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id();
        let start = aida_core::liveness::process_start_identity(pid).unwrap();
        let raw = start.strip_prefix("linux-starttime:").unwrap();
        let binding = worker_binding_token(pid, raw);
        let changed = worker_binding_token(pid, "changed");
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(
            binding.unwrap(),
            crate::gate::review_authority::author_process_token("proc", pid, Some(&start))
        );
        assert!(changed.is_err());
        assert!(worker_binding_token(pid, raw).is_err());
    }
    // trace:TASK-3225 | ai:codex
    #[test]
    fn task_3225_authority_errors_preserve_parent_expiry_and_revocation() {
        use crate::gate::seat_authority::GrantRejection;
        let at = chrono::Utc::now();
        assert_eq!(rejection_code(&GrantRejection::Expired { at }), b'E');
        assert_eq!(
            rejection_code(&GrantRejection::Parent(Box::new(GrantRejection::Revoked {
                at
            }))),
            b'R'
        );
        assert_eq!(rejection_code(&GrantRejection::Unreadable), b'N');
    }
}

#[cfg(test)]
mod owner_tests;

#[cfg(test)]
#[test]
fn staged_writes_cannot_reach_host_mutation_or_authorization() {
    for op in [
        "comment_add",
        "comment_edit",
        "comment_delete",
        "spec_edit",
        "own_claim",
        "queue_done",
    ] {
        let request = HostRequest {
            version: 1,
            launch_epoch: uuid::Uuid::nil(),
            request_id: uuid::Uuid::new_v4(),
            op: op.into(),
            data: serde_json::json!({}),
        };
        let error = host_operation(
            Path::new("/nonexistent-store"),
            "binding",
            uuid::Uuid::nil(),
            &[],
            "author",
            &request,
            || panic!("staged writes must not reach authority or store access"),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "UnsupportedOperation");
    }
}

// Exercise the actual Python authority RPC against the Rust host, including a
// held writer lock: read-only status must neither wait on it nor create redo.
// trace:TASK-3223 | ai:codex
#[cfg(test)]
#[test]
fn request_status_real_authority_is_read_only_and_preserves_local_results() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    aida_core::db::with_store_write_lock(root, || {
        let (mut host, helper) = UnixStream::pair()?;
        host.set_read_timeout(Some(Duration::from_secs(5)))?;
        let root = root.to_path_buf();
        let worker = std::thread::spawn(move || {
            let mut operations = 0;
            loop {
                let mut byte = [0];
                match host.read_exact(&mut byte) {
                    Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                    other => other.unwrap(),
                }
                if byte == [b'?'] {
                    host.write_all(b"Y").unwrap();
                } else {
                    assert_eq!(byte, [b'M']);
                    let request = read_operation(&mut host).unwrap();
                    let result = host_operation(&root, "fixed", request.launch_epoch, &[], "author", &request, || Ok(())).unwrap();
                    assert_eq!(result, serde_json::json!({"state":"unknown"}));
                    let wire = serde_json::to_vec(&serde_json::json!({"launch_epoch":request.launch_epoch,"request_id":request.request_id,"result":result})).unwrap();
                    host.write_all(&(wire.len() as u32).to_be_bytes()).unwrap();
                    host.write_all(&wire).unwrap();
                    operations += 1;
                }
            }
            assert_eq!(operations, 1, "local completed IDs must not fall through to host");
        });
        let script = r#"
import json, os, socket, uuid
from tests.test_store_mutation import ns, Service, Identity
channel = socket.socket(fileno=0)
authority = ns['MutationAuthority'](channel)
epoch = str(uuid.uuid4())
service = Service(dict(epoch=epoch, specs=[], role='implementer'), Identity(), authority)
def dispatch(op, data):
    request = dict(version=1, launch_epoch=epoch, request_id=str(uuid.uuid4()), op=op, data=data)
    reply = json.loads(service.dispatch(json.dumps(request).encode(), (os.getpid(), os.getuid(), os.getgid())))
    return request['request_id'], reply['result']
probe, _ = dispatch('session_probe', dict(challenge='read-only'))
status, result = dispatch('request_status', dict(request_id=probe))
assert result == dict(state='completed'), result
_, result = dispatch('request_status', dict(request_id=status))
assert result == dict(state='completed'), result
_, result = dispatch('request_status', dict(request_id=str(uuid.uuid4())))
assert result == dict(state='unknown'), result
service.close()
channel.close()
"#;
        let file = File::from(std::os::fd::OwnedFd::from(helper));
        let output = std::process::Command::new("python3")
            .args(["-c", script])
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap())
            .stdin(std::process::Stdio::from(file))
            .output()?;
        worker.join().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(!tmp.path().join(".aida/redo").exists());
        Ok(())
    }).unwrap();
}
