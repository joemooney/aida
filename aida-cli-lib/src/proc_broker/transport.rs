//! Kernel message authentication. Connection pidfds are feature probes ONLY.
// trace:TASK-3368 | ai:codex
use super::{Session, DEADLINE, MAX_FRAME};
use aida_core::proc_identity::ProcIdentity;
use anyhow::{bail, ensure, Context, Result};
use std::ffi::CString;
use std::fs::File;
use std::mem::{size_of, size_of_val};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

const SO_PEERPIDFD: i32 = 77;
const SO_PASSPIDFD: i32 = 76;
const SCM_PIDFD: i32 = 4;

fn owned(fd: i32) -> Result<OwnedFd> {
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: caller supplies a newly returned descriptor, owned exactly once.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}
fn option(fd: RawFd, name: i32) -> Result<()> {
    let one: i32 = 1;
    // SAFETY: the option points at an initialized int of the advertised size.
    let rc = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            name,
            (&one as *const i32).cast(),
            size_of::<i32>() as _,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error())
            .context(format!("required socket option {name}"));
    }
    Ok(())
}
fn enable(fd: RawFd) -> Result<()> {
    option(fd, libc::SO_PASSCRED)?;
    option(fd, SO_PASSPIDFD)
}
fn socket() -> Result<OwnedFd> {
    // SAFETY: socket returns a fresh descriptor or -1.
    owned(unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    })
}

pub(super) fn pair() -> Result<(OwnedFd, OwnedFd)> {
    let mut fds = [-1; 2];
    // SAFETY: fds has space for the two returned descriptors.
    ensure!(
        unsafe {
            libc::socketpair(
                libc::AF_UNIX,
                libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
                0,
                fds.as_mut_ptr(),
            )
        } == 0,
        "socketpair: {}",
        std::io::Error::last_os_error()
    );
    let a = owned(fds[0])?;
    let b = owned(fds[1])?;
    enable(a.as_raw_fd())?;
    enable(b.as_raw_fd())?;
    Ok((a, b))
}

fn peer_pidfd(fd: RawFd) -> Result<OwnedFd> {
    let mut raw: i32 = -1;
    let mut len = size_of::<i32>() as libc::socklen_t;
    // SAFETY: getsockopt writes an int into the supplied storage.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            SO_PEERPIDFD,
            (&mut raw as *mut i32).cast(),
            &mut len,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error()).context("SO_PEERPIDFD unavailable");
    }
    let fd = owned(raw)?;
    ensure!(
        len as usize == size_of::<i32>(),
        "invalid SO_PEERPIDFD size"
    );
    Ok(fd)
}

/// Exercise both options AND an actual kernel-attached message before binding.
/// There is no pidfd_open syscall or numeric-PID authentication fallback.
pub(super) fn probe() -> Result<()> {
    let (a, b) = pair().context("SO_PASSPIDFD/SCM_PIDFD unavailable")?;
    let _connection_probe = peer_pidfd(a.as_raw_fd())?;
    let until = Instant::now() + DEADLINE;
    send(b.as_raw_fd(), b"probe", until)?;
    let message = receive(a.as_raw_fd(), until)?;
    message.authenticate()?;
    Ok(())
}

fn ready(fd: RawFd, events: i16, until: Instant) -> Result<()> {
    loop {
        let left = until.saturating_duration_since(Instant::now());
        ensure!(!left.is_zero(), "request deadline exceeded (unavailable)");
        let mut pfd = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        // SAFETY: poll gets one initialized pollfd.
        let rc = unsafe {
            libc::poll(
                &mut pfd,
                1,
                left.as_millis().clamp(1, i32::MAX as u128) as i32,
            )
        };
        if rc < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        ensure!(
            rc > 0 && pfd.revents & events != 0,
            "socket unavailable or timed out"
        );
        return Ok(());
    }
}
pub(super) fn send(fd: RawFd, bytes: &[u8], until: Instant) -> Result<()> {
    ensure!(bytes.len() <= MAX_FRAME, "oversize response");
    ready(fd, libc::POLLOUT, until)?;
    // SAFETY: bytes is readable; MSG_NOSIGNAL prevents process termination.
    let n = unsafe { libc::send(fd, bytes.as_ptr().cast(), bytes.len(), libc::MSG_NOSIGNAL) };
    ensure!(n == bytes.len() as isize, "packet send failed");
    Ok(())
}

pub(super) struct Message {
    pub bytes: Vec<u8>,
    cred: libc::ucred,
    pidfd: OwnedFd,
}
impl Message {
    /// Read the PID from the *kernel pidfd's* fdinfo, cross-check credentials,
    /// then capture in proven procfs coordinates, checking the pin on both sides.
    /// NSpid is only a refusal check, never a coordinate translation.
    pub fn authenticate(&self) -> Result<ProcIdentity> {
        // SAFETY: geteuid has no pointer arguments or side effects.
        ensure!(self.cred.uid == unsafe { libc::geteuid() }, "foreign uid");
        self.live()?;
        let info =
            std::fs::read_to_string(format!("/proc/self/fdinfo/{}", self.pidfd.as_raw_fd()))?;
        let pid: i32 = info
            .lines()
            .find_map(|s| s.strip_prefix("Pid:\t"))
            .context("pidfd has no local PID")?
            .trim()
            .parse()?;
        let ns: Vec<i32> = info
            .lines()
            .find_map(|s| s.strip_prefix("NSpid:\t"))
            .context("pidfd has no namespace evidence")?
            .split_whitespace()
            .map(str::parse)
            .collect::<std::result::Result<_, _>>()?;
        ensure!(
            pid > 0 && pid == self.cred.pid && ns == [pid],
            "non-host or inconsistent sender coordinates"
        );
        let before = ProcIdentity::capture(pid as u32).map_err(anyhow::Error::msg)?;
        self.recheck(&before)?;
        Ok(before)
    }
    fn live(&self) -> Result<()> {
        let mut pfd = libc::pollfd {
            fd: self.pidfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll gets one live descriptor, with a zero timeout.
        ensure!(
            unsafe { libc::poll(&mut pfd, 1, 0) } == 0 && pfd.revents == 0,
            "sender exited or pidfd unavailable"
        );
        Ok(())
    }
    fn recheck(&self, expected: &ProcIdentity) -> Result<()> {
        self.live()?;
        let current = ProcIdentity::capture(expected.pid).map_err(anyhow::Error::msg)?;
        ensure!(&current == expected, "sender identity changed");
        self.live()
    }
}

pub(super) fn receive(fd: RawFd, until: Instant) -> Result<Message> {
    ready(fd, libc::POLLIN, until)?;
    let mut bytes = vec![0; MAX_FRAME];
    // usize alignment is sufficient for cmsghdr; all delivered descriptors are
    // adopted before any validation returns, including truncated/extra controls.
    let mut control = [0usize; 64];
    let mut iov = libc::iovec {
        iov_base: bytes.as_mut_ptr().cast(),
        iov_len: bytes.len(),
    };
    // SAFETY: zero is the documented empty msghdr initialization.
    let mut header: libc::msghdr = unsafe { std::mem::zeroed() };
    header.msg_iov = &mut iov;
    header.msg_iovlen = 1;
    header.msg_control = control.as_mut_ptr().cast();
    header.msg_controllen = size_of_val(&control);
    // SAFETY: all header buffers are writable and live until recvmsg returns.
    let n = unsafe { libc::recvmsg(fd, &mut header, libc::MSG_CMSG_CLOEXEC) };
    ensure!(n >= 0, "recvmsg: {}", std::io::Error::last_os_error());
    let mut cred = None;
    let mut pins = Vec::new();
    let mut extra_fds = Vec::new();
    let mut invalid = false;
    // SAFETY: kernel populated this bounded, aligned control buffer; CMSG macros
    // check message boundaries. Integer/credential payloads are read unaligned.
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&header);
        while !cmsg.is_null() {
            let base = libc::CMSG_LEN(0) as usize;
            let len = (*cmsg).cmsg_len;
            if len < base {
                invalid = true;
                break;
            }
            let payload = len - base;
            let data = libc::CMSG_DATA(cmsg);
            if (*cmsg).cmsg_level != libc::SOL_SOCKET {
                invalid = true;
            } else if (*cmsg).cmsg_type == SCM_PIDFD || (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                if payload % size_of::<i32>() != 0 {
                    invalid = true;
                }
                for index in 0..payload / size_of::<i32>() {
                    let raw =
                        std::ptr::read_unaligned(data.add(index * size_of::<i32>()).cast::<i32>());
                    let descriptor = owned(raw)?;
                    if (*cmsg).cmsg_type == SCM_PIDFD {
                        pins.push(descriptor);
                    } else {
                        extra_fds.push(descriptor);
                    }
                }
            } else if (*cmsg).cmsg_type == libc::SCM_CREDENTIALS {
                if payload != size_of::<libc::ucred>() || cred.is_some() {
                    invalid = true;
                } else {
                    cred = Some(std::ptr::read_unaligned(data.cast::<libc::ucred>()));
                }
            } else {
                invalid = true;
            }
            cmsg = libc::CMSG_NXTHDR(&header, cmsg);
        }
    }
    ensure!(
        !invalid
            && extra_fds.is_empty()
            && pins.len() == 1
            && cred.is_some()
            && header.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) == 0
            && n > 0,
        "missing, truncated or unexpected kernel message credentials"
    );
    bytes.truncate(n as usize);
    Ok(Message {
        bytes,
        cred: cred.unwrap(),
        pidfd: pins.pop().unwrap(),
    })
}

/// Pin every component without following symlinks. Intermediate root-owned
/// sticky directories (e.g. /tmp) are allowed only above the private final dir.
fn private_directory(path: &Path) -> Result<File> {
    ensure!(path.is_absolute(), "runtime directory must be absolute");
    let mut dir = File::open("/")?;
    let components: Vec<_> = path.components().collect();
    ensure!(
        components.len() > 1,
        "root is not a private runtime directory"
    );
    for (index, component) in components.iter().enumerate().skip(1) {
        let Component::Normal(name) = component else {
            bail!("unsafe runtime component");
        };
        let name = CString::new(name.as_bytes())?;
        // SAFETY: openat uses a live pinned directory and NUL-terminated name.
        dir = File::from(owned(unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        })?);
        let md = dir.metadata()?;
        let uid = unsafe { libc::geteuid() };
        ensure!(md.uid() == 0 || md.uid() == uid, "foreign runtime owner");
        if index == components.len() - 1 {
            ensure!(
                md.uid() == uid && md.mode() & 0o7777 == 0o700,
                "runtime must be owned by caller and mode 0700"
            );
        } else {
            ensure!(
                md.mode() & 0o022 == 0 || (md.uid() == 0 && md.mode() & 0o1000 != 0),
                "writable runtime ancestor"
            );
        }
    }
    Ok(dir)
}
fn identity(md: &std::fs::Metadata) -> (u64, u64) {
    (md.dev(), md.ino())
}

/// Unregistered staged service. Serial dispatch bounds concurrency to one
/// (below the protocol ceiling of 16); each request has one total deadline.
pub(super) struct Service {
    dir: File,
    lock: File,
    listener: OwnedFd,
    socket_identity: (u64, u64),
    runtime: PathBuf,
    project: String,
    store: String,
}
impl Service {
    // Private to this module and its tests. No production enable switch exists.
    pub(super) fn bind(runtime: &Path, project: String, store: String) -> Result<Self> {
        probe()?;
        let dir = private_directory(runtime)?;
        let lock_name = c"broker.lock";
        let lock = File::from(owned(unsafe {
            libc::openat(
                dir.as_raw_fd(),
                lock_name.as_ptr(),
                libc::O_RDWR
                    | libc::O_CREAT
                    | libc::O_NOFOLLOW
                    | libc::O_CLOEXEC
                    | libc::O_NONBLOCK,
                0o600,
            )
        })?);
        let md = lock.metadata()?;
        ensure!(
            md.is_file()
                && md.nlink() == 1
                && md.uid() == unsafe { libc::geteuid() }
                && md.mode() & 0o7777 == 0o600,
            "unsafe runtime lock"
        );
        ensure!(
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
            "broker runtime already locked"
        );
        let pinned = PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd()));
        let path = pinned.join("broker.sock");
        // Never unlink an existing endpoint, even a stale one: explicit host
        // recovery is required. This avoids removing a replacement socket.
        ensure!(
            std::fs::symlink_metadata(&path)
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
            "broker endpoint already exists or is unreadable"
        );
        let listener = socket()?;
        enable(listener.as_raw_fd())?;
        let (address, length) = address(&path)?;
        ensure!(
            unsafe {
                libc::bind(
                    listener.as_raw_fd(),
                    (&address as *const libc::sockaddr_un).cast(),
                    length,
                )
            } == 0,
            "bind failed: {}",
            std::io::Error::last_os_error()
        );
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
        let socket_identity = identity(&std::fs::symlink_metadata(&path)?);
        ensure!(
            unsafe { libc::listen(listener.as_raw_fd(), 16) } == 0,
            "listen failed"
        );
        let service = Self {
            dir,
            lock,
            listener,
            socket_identity,
            runtime: runtime.to_owned(),
            project,
            store,
        };
        service.validate()?;
        Ok(service)
    }
    fn pinned(&self, name: &str) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}/{}", self.dir.as_raw_fd(), name))
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            identity(&private_directory(&self.runtime)?.metadata()?)
                == identity(&self.dir.metadata()?),
            "runtime replaced"
        );
        ensure!(
            identity(&std::fs::symlink_metadata(self.pinned("broker.lock"))?)
                == identity(&self.lock.metadata()?),
            "lock replaced"
        );
        let md = std::fs::symlink_metadata(self.pinned("broker.sock"))?;
        ensure!(
            md.file_type().is_socket()
                && md.uid() == unsafe { libc::geteuid() }
                && md.mode() & 0o7777 == 0o600
                && identity(&md) == self.socket_identity,
            "socket replaced or unsafe"
        );
        Ok(())
    }
    pub(super) fn serve_one(&self) -> Result<()> {
        self.validate()?;
        let until = Instant::now() + DEADLINE;
        ready(self.listener.as_raw_fd(), libc::POLLIN, until)?;
        let client = owned(unsafe {
            libc::accept4(
                self.listener.as_raw_fd(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            )
        })?;
        // Probe connection support but NEVER use it to authenticate a message.
        let _probe = peer_pidfd(client.as_raw_fd())?;
        enable(client.as_raw_fd())?;
        let mut session = Session::new();
        let message = receive(client.as_raw_fd(), until)?;
        let sender = message.authenticate()?;
        let response = session.dispatch(&message.bytes, &sender, &self.project, &self.store);
        message.recheck(&sender)?;
        self.validate()?;
        ensure!(Instant::now() < until, "request deadline exceeded");
        send(client.as_raw_fd(), &serde_json::to_vec(&response)?, until)?;
        // Slice 1 serves one frame and closes; Session's bounded nonce policy
        // is tested ahead of any later persistent-session rollout.
        Ok(())
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        let path = self.pinned("broker.sock");
        if std::fs::symlink_metadata(&path).is_ok_and(|md| identity(&md) == self.socket_identity) {
            let _ = std::fs::remove_file(path);
        }
    }
}
fn address(path: &Path) -> Result<(libc::sockaddr_un, libc::socklen_t)> {
    let bytes = path.as_os_str().as_bytes();
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    ensure!(
        bytes.len() < addr.sun_path.len() && !bytes.contains(&0),
        "invalid socket path"
    );
    addr.sun_family = libc::AF_UNIX as _;
    for (dst, src) in addr.sun_path.iter_mut().zip(bytes) {
        *dst = *src as _;
    }
    let len = (std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1) as _;
    Ok((addr, len))
}

#[cfg(test)]
pub(super) fn connect(path: &Path) -> Result<OwnedFd> {
    let fd = socket()?;
    enable(fd.as_raw_fd())?;
    let (addr, len) = address(path)?;
    ensure!(
        unsafe {
            libc::connect(
                fd.as_raw_fd(),
                (&addr as *const libc::sockaddr_un).cast(),
                len,
            )
        } == 0,
        "test connect failed: {}",
        std::io::Error::last_os_error()
    );
    Ok(fd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_pid_and_uid_are_cross_checked_against_kernel_pin() {
        if !super::super::tests::kernel() {
            return;
        }
        let (receiver, sender) = pair().unwrap();
        let until = Instant::now() + DEADLINE;
        send(sender.as_raw_fd(), b"test", until).unwrap();
        let mut message = receive(receiver.as_raw_fd(), until).unwrap();
        assert!(message.authenticate().is_ok());
        let uid = message.cred.uid;
        message.cred.uid = uid.wrapping_add(1);
        assert!(message.authenticate().is_err());
        message.cred.uid = uid;
        message.cred.pid += 1;
        assert!(message.authenticate().is_err());
        message.cred.pid -= 1;
        assert!(message.authenticate().is_ok());
    }

    #[test]
    fn extra_rights_and_truncated_controls_refuse_without_poisoning_channel() {
        if !super::super::tests::kernel() {
            return;
        }
        let (receiver, sender) = pair().unwrap();
        let file = File::open("/dev/null").unwrap();
        for count in [1usize, 180] {
            let mut controls = [0usize; 128];
            let mut byte = b'x';
            let mut iov = libc::iovec {
                iov_base: (&mut byte as *mut u8).cast(),
                iov_len: 1,
            };
            let mut header: libc::msghdr = unsafe { std::mem::zeroed() };
            header.msg_iov = &mut iov;
            header.msg_iovlen = 1;
            header.msg_control = controls.as_mut_ptr().cast();
            // SAFETY: this aligned control buffer fits all count descriptors.
            unsafe {
                header.msg_controllen = libc::CMSG_SPACE((count * size_of::<i32>()) as _) as usize;
                let cmsg = libc::CMSG_FIRSTHDR(&header);
                (*cmsg).cmsg_level = libc::SOL_SOCKET;
                (*cmsg).cmsg_type = libc::SCM_RIGHTS;
                (*cmsg).cmsg_len = libc::CMSG_LEN((count * size_of::<i32>()) as _) as usize;
                for index in 0..count {
                    std::ptr::write_unaligned(
                        libc::CMSG_DATA(cmsg)
                            .add(index * size_of::<i32>())
                            .cast::<i32>(),
                        file.as_raw_fd(),
                    );
                }
                assert_eq!(
                    libc::sendmsg(sender.as_raw_fd(), &header, libc::MSG_NOSIGNAL),
                    1
                );
            }
            assert!(receive(receiver.as_raw_fd(), Instant::now() + DEADLINE).is_err());
            send(sender.as_raw_fd(), b"valid", Instant::now() + DEADLINE).unwrap();
            assert!(receive(receiver.as_raw_fd(), Instant::now() + DEADLINE)
                .unwrap()
                .authenticate()
                .is_ok());
        }
    }
}
