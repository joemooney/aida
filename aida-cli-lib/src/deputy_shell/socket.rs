//! Kernel-attested submit for the script deputy.
//!
//! The runner listens on `<queue>/submit.sock` (inside the 0700 queue, the
//! socket itself 0600). A client sends ONE JSON request and reads ONE JSON
//! response. For a submit the runner:
//!
//! 1. reads the peer's `(pid, uid)` from the kernel (`SO_PEERCRED`) and pins
//!    the peer process with the kernel's own pidfd (`SO_PEERPIDFD`, Linux
//!    6.5+; without it the submit is refused, never pinned by pid);
//! 2. refuses a peer of another uid;
//! 3. loads the grant the job names and walks the peer's process ancestry
//!    (`/proc/<pid>/stat`, bounded). One process on that chain must be a
//!    process binding of the grant (`proc:`/`sid:`: its launcher or session
//!    leader), compared as BUG-2419 `(pid, namespace, start)` tokens: a token
//!    bound in another pid namespace is never compared here;
//! 4. re-checks after the walk that the pinned peer is still the process it
//!    read (pidfd alive, same start identity), so a recycled pid cannot pass;
//! 5. overwrites the job's `pid`/`pid_start` with the kernel's view, records
//!    the attested [`Peer`], and files it under `accepted/`, a directory no
//!    submitter ever writes.
//!
//! No seat sandbox is given an AF_UNIX rule (on Linux the only switch would
//! open every unix socket on the machine to the seat). A sandboxed client
//! gets EPERM creating the socket, and [`request`] names the one opt-in.
// trace:STORY-2343 | ai:claude

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::attribution::Refusal;
use super::{Queue, ShellJob};
use crate::gate::seat_authority::SeatGrant;
#[cfg(all(unix, test))]
pub(crate) use unix::serve;
#[cfg(unix)]
pub(crate) use unix::{request, serve_service, Server};

/// How far up the process tree the ancestry walk goes.
pub(crate) const MAX_ANCESTRY: usize = 64;

/// The one documented way to let a sandboxed session reach the runner.
pub(crate) const SANDBOX_OPT_IN: &str = "this session's sandbox blocks unix sockets, so it cannot reach the script deputy. Submit from an unsandboxed shell, or the operator may exempt the two commands that need the socket: add `aida deputy shell submit` and `aida deputy shell cancel` to the sandbox's `excludedCommands` (docs/advanced/deputies.md, \"Letting a sandboxed advisor submit\")";

/// One request on the socket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub(crate) enum Request {
    Submit {
        job: Box<ShellJob>,
    },
    Cancel {
        id: String,
    },
    // trace:STORY-2571 | ai:codex
    ReviewWatchRebind {
        project: std::path::PathBuf,
        grant: String,
    },
}

/// The runner's one reply: accepted (with the job id) or refused.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Response {
    pub ok: bool,
    #[serde(default)]
    pub id: Option<String>,
    /// `refused:<code>` when not ok.
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub why: Option<String>,
}

impl Response {
    fn accepted(id: &str) -> Self {
        Self {
            ok: true,
            id: Some(id.to_string()),
            reason: None,
            why: None,
        }
    }
    fn refused(r: &Refusal) -> Self {
        Self {
            ok: false,
            id: None,
            reason: Some(r.reason()),
            why: Some(r.why.clone()),
        }
    }
}

fn refusal(code: &'static str, why: impl Into<String>) -> Refusal {
    Refusal {
        code,
        why: why.into(),
    }
}

/// The kernel's view of a connected peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PeerCred {
    pub pid: u32,
    pub uid: u32,
}

/// `pid`'s parent, from `/proc/<pid>/stat` (the field after the `)` that
/// closes the command name, which may itself hold spaces or parens).
pub(crate) fn parent_pid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = &stat[stat.rfind(')')? + 1..];
    after.split_whitespace().nth(1)?.parse().ok()
}

/// `pid` and its ancestors, nearest first, at most [`MAX_ANCESTRY`] long;
/// stops at pid 1 or a process it cannot read.
pub(crate) fn ancestry(pid: u32) -> Vec<u32> {
    let mut chain = vec![pid];
    let mut cur = pid;
    while chain.len() < MAX_ANCESTRY {
        match parent_pid(cur) {
            Some(p) if p > 1 && !chain.contains(&p) => {
                chain.push(p);
                cur = p;
            }
            _ => break,
        }
    }
    chain
}

/// PURE over its probes: the grant binding token the peer runs under, or
/// the refusal. `chain` is the peer's ancestry (peer first); `alive(pid,
/// start)` says whether `pid` is still the process that started at `start`.
/// Only process bindings taken in the runner's own pid namespace count
/// (BUG-2419): a pid from another namespace names some other process here.
pub(crate) fn attest(
    cred: PeerCred,
    runner_uid: u32,
    grant: &SeatGrant,
    chain: &[u32],
    alive: &dyn Fn(u32, &str) -> bool,
) -> Result<String, Refusal> {
    if cred.uid != runner_uid {
        return Err(refusal(
            "foreign-uid",
            format!(
                "the submitting process runs as uid {}, not as the runner's uid {runner_uid}",
                cred.uid
            ),
        ));
    }
    let own_ns = crate::gate::review_authority::origin_pid_ns();
    for token in &grant.binding {
        let Some((pid, ns, start)) = crate::gate::seat_authority::binding_process(token) else {
            continue;
        };
        if ns.is_some_and(|ns| own_ns.as_deref() != Some(ns)) {
            continue;
        }
        if chain.contains(&pid) && alive(pid, start) {
            return Ok(token.clone());
        }
    }
    Err(refusal(
        "not-under-grant",
        format!(
            "the submitting process (pid {}) does not run under any process the grant {} is bound to",
            cred.pid,
            grant.id.chars().take(8).collect::<String>()
        ),
    ))
}

#[cfg(not(unix))]
pub(crate) fn request(_q: &Queue, _req: &Request) -> Result<Response> {
    bail!("the script deputy needs a Unix machine")
}

#[cfg(unix)]
mod unix {
    use anyhow::{bail, Context, Result};
    use std::io::{Read, Write};
    #[cfg(target_os = "linux")]
    use std::os::fd::FromRawFd;
    use std::os::fd::{AsRawFd, OwnedFd, RawFd};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::Duration;

    use super::{ancestry, attest, refusal, PeerCred, Request, Response, SANDBOX_OPT_IN};
    use crate::deputy_shell::attribution::{Probes, Refusal};
    use crate::deputy_shell::{Clock, Peer, Queue};
    use crate::gate::seat_authority::GrantCheck;

    /// The largest request the runner reads (a script plus its record).
    pub(crate) const MAX_REQUEST: u64 = 8 << 20;
    /// `sun_path` holds 108 bytes including the terminating NUL.
    const MAX_SOCKET_PATH: usize = 107;
    /// How long the client waits on the runner's reply.
    const IO_TIMEOUT: Duration = Duration::from_secs(30);
    /// How long the runner gives a client to send its WHOLE request (a
    /// deadline, not a per-read timeout, so a slow drip cannot extend it).
    const REQUEST_DEADLINE: Duration = Duration::from_secs(5);
    /// Requests the runner handles at once; more are refused as busy.
    const MAX_CONNECTIONS: usize = 16;
    /// How long the accept loop spends draining a request it refuses as busy.
    const BUSY_DRAIN: Duration = Duration::from_millis(250);

    /// Read one request (to the client's half-close) within `deadline`,
    /// at most `MAX_REQUEST + 1` bytes.
    fn read_request(stream: &UnixStream, deadline: Duration) -> std::io::Result<Vec<u8>> {
        let until = std::time::Instant::now() + deadline;
        let mut bytes = Vec::new();
        let mut buf = [0u8; 64 * 1024];
        loop {
            let left = until.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "the request was not complete in time",
                ));
            }
            stream.set_read_timeout(Some(left))?;
            match (&*stream).read(&mut buf) {
                Ok(0) => return Ok(bytes),
                Ok(n) => {
                    bytes.extend_from_slice(&buf[..n]);
                    if bytes.len() as u64 > MAX_REQUEST {
                        return Ok(bytes);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn peer_cred(fd: RawFd) -> std::io::Result<PeerCred> {
        let mut cred = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: getsockopt fills a ucred we own; len is its size.
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut cred as *mut libc::ucred).cast(),
                &mut len,
            )
        };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(PeerCred {
            pid: cred.pid as u32,
            uid: cred.uid,
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn peer_cred(_fd: RawFd) -> std::io::Result<PeerCred> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "kernel-attested submit needs Linux SO_PEERCRED",
        ))
    }

    /// The kernel's own pidfd for the peer (`SO_PEERPIDFD`, Linux 6.5+): it
    /// names the process that connected even if that pid has since been
    /// reused. There is deliberately no `pidfd_open(pid)` fallback: a peer
    /// that wrote its request and exited could have its pid recycled before
    /// the fallback ran, and the recycled process would be pinned and walked
    /// in its place. `None` (an older kernel) refuses the submit.
    // trace:STORY-2343 | ai:claude
    #[cfg(target_os = "linux")]
    fn peer_pidfd(fd: RawFd) -> Option<OwnedFd> {
        const SO_PEERPIDFD: libc::c_int = 77;
        let mut pidfd: libc::c_int = -1;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: getsockopt fills an int we own; len is its size.
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                SO_PEERPIDFD,
                (&mut pidfd as *mut libc::c_int).cast(),
                &mut len,
            )
        };
        // SAFETY: on success the kernel just handed us this descriptor.
        (rc == 0 && pidfd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(pidfd) })
    }

    #[cfg(not(target_os = "linux"))]
    fn peer_pidfd(_fd: RawFd) -> Option<OwnedFd> {
        None
    }

    /// Whether the process a pidfd pins still runs (signal 0 sends nothing).
    #[cfg(target_os = "linux")]
    fn pidfd_running(fd: &OwnedFd) -> bool {
        // SAFETY: a valid pidfd, signal 0, a null siginfo and no flags.
        let rc = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                fd.as_raw_fd(),
                0,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        rc == 0
    }

    #[cfg(not(target_os = "linux"))]
    fn pidfd_running(_fd: &OwnedFd) -> bool {
        false
    }

    /// The live socket server; dropping it stops the thread and removes the
    /// socket.
    pub(crate) struct Server {
        path: PathBuf,
        done: Arc<AtomicBool>,
        handle: Option<JoinHandle<()>>,
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.done.store(true, Ordering::SeqCst);
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn check_path_len(path: &Path) -> Result<()> {
        use std::os::unix::ffi::OsStrExt;
        let n = path.as_os_str().as_bytes().len();
        if n > MAX_SOCKET_PATH {
            bail!(
            "the script deputy's socket path {} is {n} bytes, over the {MAX_SOCKET_PATH}-byte unix socket limit; use a shorter AIDA home or --name",
            path.display()
        );
        }
        Ok(())
    }

    /// Bind `<queue>/submit.sock` and serve it on a thread. Call under the
    /// runner lock: a leftover socket from a dead runner is removed first (any
    /// other kind of file there is refused, never removed).
    #[cfg(test)]
    pub(crate) fn serve(q: &Queue, probes: Probes, clock: Clock) -> Result<Server> {
        serve_service(q, probes, clock, None)
    }

    // Only the operator-started runner supplies this trusted project/authority context.
    // trace:STORY-2571 | ai:codex
    pub(crate) fn serve_service(
        q: &Queue,
        probes: Probes,
        clock: Clock,
        service: Option<crate::review_watch::service::HostAuthority>,
    ) -> Result<Server> {
        use std::os::unix::fs::{FileTypeExt, PermissionsExt};
        let path = q.socket_path();
        check_path_len(&path)?;
        match std::fs::symlink_metadata(&path) {
            Ok(md) if md.file_type().is_socket() => {
                std::fs::remove_file(&path).context("remove the stale submit socket")?
            }
            Ok(_) => bail!(
                "{} exists and is not a socket; remove it by hand",
                path.display()
            ),
            Err(_) => {}
        }
        let listener =
            UnixListener::bind(&path).with_context(|| format!("listen on {}", path.display()))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .context("make the submit socket 0600")?;
        listener
            .set_nonblocking(true)
            .context("make the submit socket non-blocking")?;
        let done = Arc::new(AtomicBool::new(false));
        let q2 = q.clone();
        let done2 = Arc::clone(&done);
        let handle = std::thread::spawn(move || {
            // One thread per connection, so a client that connects and never
            // finishes its request stalls only itself (and only until
            // REQUEST_DEADLINE), never every other submit and cancel.
            // trace:STORY-2343 | ai:claude
            let mut live: Vec<JoinHandle<()>> = Vec::new();
            while !done2.load(Ordering::SeqCst) {
                live.retain(|h| !h.is_finished());
                match listener.accept() {
                    Ok((stream, _)) if live.len() >= MAX_CONNECTIONS => {
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_write_timeout(Some(BUSY_DRAIN));
                        // Drain first (briefly): closing a unix socket with
                        // unread bytes resets the peer, which would lose the
                        // reply.
                        let _ = read_request(&stream, BUSY_DRAIN);
                        reply(
                            stream,
                            &Response::refused(&refusal(
                                "busy",
                                format!(
                                    "the script deputy is already handling {MAX_CONNECTIONS} requests; try again"
                                ),
                            )),
                        );
                    }
                    Ok((stream, _)) => {
                        let (q3, probes3, clock3) = (q2.clone(), probes.clone(), clock.clone());
                        let service = service.clone();
                        live.push(std::thread::spawn(move || {
                            let resp = serve_one(&q3, &probes3, &clock3, &stream, service.as_ref());
                            reply(stream, &resp);
                        }));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    Err(e) => {
                        eprintln!("deputy shell {}: accept failed: {e}", q2.name);
                        std::thread::sleep(Duration::from_millis(250));
                    }
                }
            }
            // Nothing is filed after the runner stops: in-flight requests
            // finish (each bounded by REQUEST_DEADLINE) before the socket goes.
            for h in live {
                let _ = h.join();
            }
        });
        Ok(Server {
            path,
            done,
            handle: Some(handle),
        })
    }

    fn reply(mut stream: UnixStream, resp: &Response) {
        if let Ok(mut bytes) = serde_json::to_vec(resp) {
            bytes.push(b'\n');
            let _ = stream.write_all(&bytes);
        }
    }

    /// Handle one connection: who is it (kernel), what does it ask, and does
    /// its process ancestry run under the grant it names.
    fn serve_one(
        q: &Queue,
        probes: &Probes,
        clock: &Clock,
        stream: &UnixStream,
        service: Option<&crate::review_watch::service::HostAuthority>,
    ) -> Response {
        let now = clock();
        let refuse = |r: Refusal, actor: &str, job: Option<&str>| {
            q.audit_event(now, "submit-refused", job, actor, |row| {
                row.reason = Some(format!("{}: {}", r.reason(), r.why))
            });
            Response::refused(&r)
        };
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_write_timeout(Some(REQUEST_DEADLINE));
        let cred = match peer_cred(stream.as_raw_fd()) {
            Ok(c) => c,
            Err(e) => {
                return refuse(
                    refusal("unattested", format!("no peer credentials: {e}")),
                    "unknown",
                    None,
                )
            }
        };
        // Pin the peer before anything is read from /proc about it.
        let pidfd = peer_pidfd(stream.as_raw_fd());
        let start = aida_core::liveness::process_start_identity(cred.pid);
        // Service admission uses namespace-qualified evidence.
        // trace:STORY-2571 | ai:codex
        let service_peer = aida_core::proc_identity::ProcIdentity::capture(cred.pid).ok();
        let bytes = match read_request(stream, REQUEST_DEADLINE) {
            Ok(b) => b,
            Err(e) => {
                return refuse(
                    refusal("schema", format!("could not read the request: {e}")),
                    "unknown",
                    None,
                )
            }
        };
        if bytes.len() as u64 > MAX_REQUEST {
            return refuse(
                refusal("schema", "the request is over the size limit"),
                "unknown",
                None,
            );
        }
        let req: Request = match serde_json::from_slice(&bytes) {
            Ok(r) => r,
            Err(e) => {
                return refuse(
                    refusal("schema", format!("malformed request ({e})")),
                    "unknown",
                    None,
                )
            }
        };
        let runner_uid = crate::deputy::current_uid();
        match req {
            // This closed operation shares the existing pinned peer transport;
            // it accepts neither scripts nor argv/environment overrides.
            // trace:STORY-2571 | ai:codex
            Request::ReviewWatchRebind { project, grant } => {
                let result = (|| -> anyhow::Result<()> {
                    let host = service.context("this deputy has no service authority")?;
                    anyhow::ensure!(
                        project == host.project,
                        "service request names another project"
                    );
                    host.validate()?;
                    let named = crate::gate::seat_authority::read_grant(&grant)?;
                    let grant = crate::gate::seat_authority::validated_grant(
                        &project,
                        &grant,
                        &named.subject,
                        now,
                    )
                    .context("advisor grant does not validate")?;
                    anyhow::ensure!(
                        pidfd.as_ref().is_some_and(pidfd_running) && service_peer.is_some(),
                        "service peer could not be pinned"
                    );
                    attest(
                        cred,
                        runner_uid,
                        &grant,
                        &ancestry(cred.pid),
                        &|pid, start| {
                            aida_core::proc_identity::ProcIdentity::capture(pid).is_ok_and(
                                |identity| {
                                    format!("linux-starttime:{}", identity.starttime) == start
                                        && matches!(
                                            aida_core::proc_identity::observe(&identity, true),
                                            aida_core::proc_identity::Verdict::Alive
                                        )
                                },
                            )
                        },
                    )
                    .map_err(|r| anyhow::anyhow!(r.why))?;
                    anyhow::ensure!(
                        pidfd.as_ref().is_some_and(pidfd_running)
                            && service_peer.as_ref().is_some_and(|identity| matches!(
                                aida_core::proc_identity::observe(identity, true),
                                aida_core::proc_identity::Verdict::Alive
                            )),
                        "service peer changed during attribution"
                    );
                    crate::review_watch::service::rebind(&project, &q.name, &grant)
                })();
                match result {
                    Ok(()) => Response::accepted("review-watch"),
                    Err(error) => refuse(refusal("service", format!("{error:#}")), "advisor", None),
                }
            }
            Request::Cancel { id } => {
                if cred.uid != runner_uid {
                    return refuse(
                        refusal("foreign-uid", "a cancel from another uid"),
                        "unknown",
                        Some(&id),
                    );
                }
                match q.locate(&id) {
                    Ok(Some(_)) => {}
                    _ => {
                        return refuse(
                            refusal("schema", format!("no script job {id}")),
                            "unknown",
                            None,
                        )
                    }
                }
                match q.request_cancel(&id) {
                    Ok(()) => Response::accepted(&id),
                    Err(e) => refuse(refusal("schema", format!("{e:#}")), "unknown", Some(&id)),
                }
            }
            Request::Submit { job } => {
                let mut job = *job;
                let actor = job.submitter.actor();
                if crate::deputy::validate_id(&job.id).is_err() || job.deputy != q.name {
                    return refuse(
                        refusal("schema", "its id is malformed or names another deputy"),
                        &actor,
                        None,
                    );
                }
                let Some(grant_id) = job.submitter.grant_id.clone() else {
                    return refuse(
                        refusal("unattributed", "the job names no grant"),
                        &actor,
                        Some(&job.id),
                    );
                };
                let grant = match (probes.grant)(&job.submitter.project_root, &grant_id) {
                    GrantCheck::Valid(g) => g,
                    other => {
                        return refuse(
                            refusal(
                                "grant",
                                other
                                    .rejection_sentence()
                                    .unwrap_or_else(|| "the named grant does not validate".into()),
                            ),
                            &actor,
                            Some(&job.id),
                        )
                    }
                };
                if pidfd.is_none() {
                    return refuse(
                        refusal(
                            "unattested",
                            "the kernel gave no pidfd for the submitting process (SO_PEERPIDFD needs Linux 6.5 or later), so it cannot be pinned against pid reuse",
                        ),
                        &actor,
                        Some(&job.id),
                    );
                }
                let chain = ancestry(cred.pid);
                let alive = |pid: u32, s: &str| {
                    aida_core::liveness::process_identity_matches_strict(pid, s)
                };
                let matched = match attest(cred, runner_uid, &grant, &chain, &alive) {
                    Ok(m) => m,
                    Err(r) => return refuse(r, &actor, Some(&job.id)),
                };
                // The pin: the peer we walked is still the process that
                // connected (a recycled pid would fail one of these).
                let still = pidfd.as_ref().is_some_and(pidfd_running)
                    && start.is_some()
                    && aida_core::liveness::process_start_identity(cred.pid) == start;
                if !still {
                    return refuse(
                        refusal(
                            "unattested",
                            "the submitting process could not be pinned for the whole check",
                        ),
                        &actor,
                        Some(&job.id),
                    );
                }
                job.submitter.pid = cred.pid;
                job.submitter.pid_start = start.clone();
                job.submitter.peer = Some(Peer {
                    pid: cred.pid,
                    uid: cred.uid,
                    start,
                    matched,
                    attested: true,
                });
                match q.accept(&job) {
                    Ok(_) => Response::accepted(&job.id),
                    Err(e) => refuse(refusal("schema", format!("{e:#}")), &actor, Some(&job.id)),
                }
            }
        }
    }

    /// CLIENT: send `req` to deputy `q`'s runner and read its reply. Errors name
    /// what to do: start the runner, or (sandboxed) the `excludedCommands`
    /// opt-in.
    pub(crate) fn request(q: &Queue, req: &Request) -> Result<Response> {
        let path = q.socket_path();
        let not_running = || {
            anyhow::anyhow!(
            "no script deputy {:?} is running; the operator starts it with `aida deputy shell run --name {}` in their terminal first",
            q.name,
            q.name
        )
        };
        let mut stream = match UnixStream::connect(&path) {
            Ok(s) => s,
            Err(e) => {
                return Err(match e.raw_os_error() {
                    Some(libc::EPERM) | Some(libc::EACCES) | Some(libc::EAFNOSUPPORT) => {
                        if matches!(req, Request::ReviewWatchRebind { .. }) {
                            anyhow::anyhow!("the sandbox blocks the service socket; retry `aida advisor loops rebind` from the host, or ask the operator for a narrow exemption for that command ({e})")
                        } else {
                            anyhow::anyhow!("{SANDBOX_OPT_IN} ({e})")
                        }
                    }
                    Some(libc::ENOENT) | Some(libc::ECONNREFUSED) => not_running(),
                    _ => anyhow::anyhow!(
                        "cannot reach script deputy {:?} at {}: {e}",
                        q.name,
                        path.display()
                    ),
                })
            }
        };
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        stream
            .write_all(&serde_json::to_vec(req)?)
            .context("send the request to the script deputy")?;
        stream
            .shutdown(std::net::Shutdown::Write)
            .context("finish the request")?;
        let mut bytes = Vec::new();
        stream
            .take(1 << 20)
            .read_to_end(&mut bytes)
            .context("read the script deputy's reply")?;
        serde_json::from_slice(&bytes).context("the script deputy's reply is malformed")
    }
}

/// CLIENT: `request`, with a refusal turned into an error.
pub(crate) fn expect_accepted(q: &Queue, req: &Request) -> Result<String> {
    let resp = request(q, req)?;
    if resp.ok {
        return resp.id.context("the script deputy accepted without an id");
    }
    bail!(
        "script deputy {:?} refused it: {}: {}",
        q.name,
        resp.reason.as_deref().unwrap_or("refused"),
        resp.why.as_deref().unwrap_or("-")
    )
}
