//! Private terminal transport for the shared boundary. Only stdio reaches the lane.
// trace:TASK-3219 | ai:codex
use anyhow::{Context, Result};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::process::{Child, ExitStatus, Stdio};

pub(crate) struct PrivatePty {
    master: File,
    slave: File,
}
impl PrivatePty {
    pub(crate) fn open() -> Result<Self> {
        let (mut master, mut slave) = (-1, -1);
        anyhow::ensure!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            } == 0,
            "private PTY unavailable"
        );
        let pty = Self {
            master: unsafe { File::from_raw_fd(master) },
            slave: unsafe { File::from_raw_fd(slave) },
        };
        for fd in [master, slave] {
            anyhow::ensure!(
                unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == 0,
                "private PTY descriptor protection failed"
            );
        }
        Ok(pty)
    }
    pub(crate) fn stdio(&self) -> Result<[Stdio; 3]> {
        Ok([
            self.slave.try_clone()?.into(),
            self.slave.try_clone()?.into(),
            self.slave.try_clone()?.into(),
        ])
    }
    pub(crate) fn report_refusal(&self) {
        // Before receipt acceptance the relay has not started. Preserve helper
        // diagnostics on refusal, with a bounded nonblocking drain.
        if unsafe { libc::fcntl(self.master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } < 0 {
            return;
        }
        let mut master = &self.master;
        let mut buf = [0; 8192];
        if let Ok(n) = master.read(&mut buf) {
            let _ = std::io::stderr().write_all(&buf[..n]);
        }
    }
    pub(crate) fn resize_from(&self, input: &File) -> Result<()> {
        let mut size = unsafe { std::mem::zeroed::<libc::winsize>() };
        anyhow::ensure!(
            unsafe { libc::ioctl(input.as_raw_fd(), libc::TIOCGWINSZ, &mut size) } == 0,
            "terminal size unavailable"
        );
        anyhow::ensure!(
            unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &size) } == 0,
            "private PTY resize failed"
        );
        Ok(())
    }
    pub(crate) fn wait(
        mut self,
        child: &mut Child,
        mut input: File,
        mut output: File,
    ) -> Result<ExitStatus> {
        let _terminal = RawTerminal::new(&input)?;
        self.resize_from(&input)?;
        // Nonblocking master bounds reads and writes; backpressure never changes host fd flags.
        anyhow::ensure!(
            unsafe { libc::fcntl(self.master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } == 0,
            "PTY nonblocking mode failed"
        );
        drop(self.slave);
        let mut pending = Vec::new();
        loop {
            // Polling the window also covers coalesced/missed SIGWINCH without a global handler.
            let mut size = unsafe { std::mem::zeroed::<libc::winsize>() };
            anyhow::ensure!(
                unsafe { libc::ioctl(input.as_raw_fd(), libc::TIOCGWINSZ, &mut size) } == 0,
                "terminal size unavailable"
            );
            anyhow::ensure!(
                unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &size) } == 0,
                "private PTY resize failed"
            );
            let mut fds = [
                libc::pollfd {
                    fd: input.as_raw_fd(),
                    events: if pending.is_empty() { libc::POLLIN } else { 0 },
                    revents: 0,
                },
                libc::pollfd {
                    fd: self.master.as_raw_fd(),
                    events: libc::POLLIN | if pending.is_empty() { 0 } else { libc::POLLOUT },
                    revents: 0,
                },
            ];
            let rc = unsafe { libc::poll(fds.as_mut_ptr(), 2, 50) };
            if rc < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(std::io::Error::last_os_error().into());
            }
            anyhow::ensure!(
                fds[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) == 0,
                "interactive terminal disconnected"
            );
            let mut buf = [0; 8192];
            if fds[0].revents & libc::POLLIN != 0 {
                let n = input.read(&mut buf)?;
                anyhow::ensure!(n > 0, "interactive terminal closed");
                pending.extend_from_slice(&buf[..n]);
            }
            if !pending.is_empty() {
                match self.master.write(&pending) {
                    Ok(n) => {
                        pending.drain(..n);
                    }
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                        ) => {}
                    Err(e) => return Err(e.into()),
                }
            }
            for _ in 0..16 {
                match self.master.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        output.write_all(&buf[..n])?;
                        output.flush()?;
                    }
                    Err(e)
                        if e.raw_os_error() == Some(libc::EIO)
                            || e.kind() == std::io::ErrorKind::WouldBlock =>
                    {
                        break
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e.into()),
                }
            }
            if let Some(status) = child.try_wait()? {
                // Exit can race the bounded relay reads above. Keep forwarding
                // queued bytes, including terminal reset sequences, until the
                // final slave closes; WouldBlock is not end of output.
                drain_exited_master(&mut self.master, &mut output)?;
                return Ok(status);
            }
        }
    }
}

// trace:TASK-3219 | ai:codex
fn drain_exited_master(master: &mut File, output: &mut File) -> Result<()> {
    let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
    anyhow::ensure!(flags >= 0, "PTY flags unavailable");
    anyhow::ensure!(
        unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags & !libc::O_NONBLOCK) } == 0,
        "PTY final drain mode failed"
    );
    let mut buf = [0; 8192];
    loop {
        match master.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => output.write_all(&buf[..n])?,
            Err(e) if e.raw_os_error() == Some(libc::EIO) => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    output.flush()?;
    Ok(())
}

struct RawTerminal {
    input: File,
    original: libc::termios,
}
impl RawTerminal {
    fn new(input: &File) -> Result<Self> {
        let input = input.try_clone()?;
        let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
        anyhow::ensure!(
            unsafe { libc::tcgetattr(input.as_raw_fd(), &mut original) } == 0,
            "interactive wrapper requires a terminal"
        );
        let guard = Self { input, original };
        let mut raw = original;
        unsafe {
            libc::cfmakeraw(&mut raw);
        }
        anyhow::ensure!(
            unsafe { libc::tcsetattr(guard.input.as_raw_fd(), libc::TCSANOW, &raw) } == 0,
            "cannot enter terminal relay mode"
        );
        Ok(guard)
    }
}
impl Drop for RawTerminal {
    fn drop(&mut self) {
        loop {
            let rc =
                unsafe { libc::tcsetattr(self.input.as_raw_fd(), libc::TCSANOW, &self.original) };
            if rc == 0 || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
            {
                break;
            }
        }
    }
}

pub(crate) fn host_stdio() -> Result<(File, File)> {
    #[cfg(test)]
    if let Some(io) = TEST_STDIO.with(|s| {
        s.borrow()
            .as_ref()
            .map(|(i, o)| (i.try_clone().unwrap(), o.try_clone().unwrap()))
    }) {
        return Ok(io);
    }
    // Duplicate only the selected endpoints, never open or mount host /dev/tty.
    let duplicate = |fd| -> Result<File> {
        let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
        anyhow::ensure!(copy >= 0, "terminal descriptor unavailable");
        Ok(unsafe { File::from_raw_fd(copy) })
    };
    let input = duplicate(0)?;
    anyhow::ensure!(
        unsafe { libc::isatty(input.as_raw_fd()) } == 1,
        "interactive wrapper requires a terminal"
    );
    Ok((input, duplicate(1).context("terminal output unavailable")?))
}

#[cfg(test)]
thread_local! { static TEST_STDIO: std::cell::RefCell<Option<(File, File)>> = const { std::cell::RefCell::new(None) }; }
#[cfg(test)]
pub(crate) struct TestStdio;
#[cfg(test)]
impl Drop for TestStdio {
    fn drop(&mut self) {
        TEST_STDIO.with(|s| {
            s.borrow_mut().take();
        });
    }
}
#[cfg(test)]
pub(crate) fn test_stdio(input: File, output: File) -> TestStdio {
    TEST_STDIO.with(|s| {
        assert!(s.borrow_mut().replace((input, output)).is_none());
    });
    TestStdio
}
#[cfg(test)]
impl PrivatePty {
    pub(crate) fn test_endpoints(&self) -> (File, File) {
        (
            self.master.try_clone().unwrap(),
            self.slave.try_clone().unwrap(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn task_3219_pty_restores_on_error_and_unwind() {
        let pty = PrivatePty::open().unwrap();
        let (_, slave) = pty.test_endpoints();
        let flags = || {
            let mut t = unsafe { std::mem::zeroed::<libc::termios>() };
            assert_eq!(unsafe { libc::tcgetattr(slave.as_raw_fd(), &mut t) }, 0);
            (t.c_iflag, t.c_oflag, t.c_cflag, t.c_lflag, t.c_cc)
        };
        let original = flags();
        {
            let _raw = RawTerminal::new(&slave).unwrap();
            assert_eq!(flags().3 & (libc::ICANON | libc::ECHO | libc::ISIG), 0);
        }
        assert_eq!(flags(), original);
        let _ = std::panic::catch_unwind(|| {
            let _raw = RawTerminal::new(&slave).unwrap();
            panic!("restore");
        });
        assert_eq!(flags(), original);
        assert!(RawTerminal::new(&File::open("/dev/null").unwrap()).is_err());
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn task_3219_pty_restores_on_relay_write_failure() {
        let host = PrivatePty::open().unwrap();
        let (_, slave) = host.test_endpoints();
        let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(
            unsafe { libc::tcgetattr(slave.as_raw_fd(), &mut original) },
            0
        );
        let lane = PrivatePty::open().unwrap();
        let [input, output, error] = lane.stdio().unwrap();
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "echo relay-output"])
            .stdin(input)
            .stdout(output)
            .stderr(error)
            .spawn()
            .unwrap();
        assert!(lane
            .wait(
                &mut child,
                slave.try_clone().unwrap(),
                std::fs::OpenOptions::new()
                    .write(true)
                    .open("/dev/full")
                    .unwrap()
            )
            .is_err());
        let _ = child.wait();
        let mut restored = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(
            unsafe { libc::tcgetattr(slave.as_raw_fd(), &mut restored) },
            0
        );
        assert_eq!(
            (
                restored.c_iflag,
                restored.c_oflag,
                restored.c_lflag,
                restored.c_cc
            ),
            (
                original.c_iflag,
                original.c_oflag,
                original.c_lflag,
                original.c_cc
            )
        );
    }
    // An already-reaped child can still have unread terminal reset bytes.
    // Also exercise more than one relay iteration's output budget.
    // trace:TASK-3219 | ai:codex
    #[test]
    fn task_3219_pty_drains_exit_sequences_and_large_output() {
        let reset = b"\x1b[?1049l\x1b[?25h\x1b[?2004l";
        for exited in [true, false] {
            let host = PrivatePty::open().unwrap();
            let (_, slave) = host.test_endpoints();
            let mut lane = PrivatePty::open().unwrap();
            let [input, output, error] = lane.stdio().unwrap();
            let count = if exited { 1024 } else { 256 * 1024 };
            let reset_hex = reset.iter().map(|b| format!("{b:02x}")).collect::<String>();
            let script = format!(
                "import sys; sys.stdout.buffer.write(b'x'*{count}+bytes.fromhex('{reset_hex}')); sys.stdout.buffer.flush()"
            );
            let mut child = std::process::Command::new("/usr/bin/python3")
                .args(["-c", &script])
                .stdin(input)
                .stdout(output)
                .stderr(error)
                .spawn()
                .unwrap();
            let mut captured = tempfile::tempfile().unwrap();
            if exited {
                assert!(child.wait().unwrap().success());
                drop(lane.slave);
                drain_exited_master(&mut lane.master, &mut captured).unwrap();
            } else {
                assert!(lane
                    .wait(&mut child, slave, captured.try_clone().unwrap())
                    .unwrap()
                    .success());
            }
            use std::io::{Seek, SeekFrom};
            captured.seek(SeekFrom::Start(0)).unwrap();
            let mut actual = Vec::new();
            captured.read_to_end(&mut actual).unwrap();
            let mut expected = vec![b'x'; count];
            expected.extend_from_slice(reset);
            assert_eq!(actual.len(), expected.len());
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn task_3219_pty_resize_and_cloexec() {
        let host = PrivatePty::open().unwrap();
        let lane = PrivatePty::open().unwrap();
        let (_, slave) = host.test_endpoints();
        let size = libc::winsize {
            ws_row: 37,
            ws_col: 113,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(
            unsafe { libc::ioctl(slave.as_raw_fd(), libc::TIOCSWINSZ, &size) },
            0
        );
        lane.resize_from(&slave).unwrap();
        let mut actual = unsafe { std::mem::zeroed::<libc::winsize>() };
        assert_eq!(
            unsafe { libc::ioctl(lane.slave.as_raw_fd(), libc::TIOCGWINSZ, &mut actual) },
            0
        );
        assert_eq!((actual.ws_row, actual.ws_col), (37, 113));
        for fd in [&lane.master, &lane.slave] {
            assert_ne!(
                unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
                0
            );
        }
    }
}

/// RAII signal registration: errors and early refusals also close the listener.
// trace:TASK-3219 | ai:codex
pub(crate) struct WorkerSignals {
    handle: signal_hook::iterator::Handle,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl WorkerSignals {
    pub(crate) fn install(worker: u32) -> Result<Self> {
        // Pin the attested process so a late signal can never reach a reused PID.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, worker, 0) };
        anyhow::ensure!(fd >= 0, "contained worker signal handle unavailable");
        let worker = unsafe { File::from_raw_fd(fd as i32) };
        let mut signals = signal_hook::iterator::Signals::new([
            libc::SIGINT,
            libc::SIGTERM,
            libc::SIGHUP,
            libc::SIGQUIT,
        ])?;
        let handle = signals.handle();
        let thread = std::thread::spawn(move || {
            for signal in signals.forever() {
                unsafe {
                    libc::syscall(
                        libc::SYS_pidfd_send_signal,
                        worker.as_raw_fd(),
                        signal,
                        std::ptr::null::<libc::siginfo_t>(),
                        0,
                    );
                }
            }
        });
        Ok(Self {
            handle,
            thread: Some(thread),
        })
    }
}
impl Drop for WorkerSignals {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

// Observe the actual receipt-bound grant while the exec barrier is held, and
// inject a refusal there without changing production launch decisions.
// trace:TASK-3219 | ai:codex
#[cfg(test)]
type BeforeRelease = Box<dyn FnMut(&str, u32) -> Result<()>>;
#[cfg(test)]
thread_local! { static BEFORE_RELEASE: std::cell::RefCell<Option<BeforeRelease>> = const { std::cell::RefCell::new(None) }; }
#[cfg(test)]
pub(crate) fn before_release(grant: &str, pid: u32) -> Result<()> {
    BEFORE_RELEASE.with(|hook| match hook.borrow_mut().as_mut() {
        Some(hook) => hook(grant, pid),
        None => Ok(()),
    })
}
#[cfg(test)]
pub(crate) struct TestBeforeRelease;
#[cfg(test)]
impl Drop for TestBeforeRelease {
    fn drop(&mut self) {
        BEFORE_RELEASE.with(|hook| {
            hook.borrow_mut().take();
        });
    }
}
#[cfg(test)]
pub(crate) fn test_before_release(hook: BeforeRelease) -> TestBeforeRelease {
    BEFORE_RELEASE.with(|slot| assert!(slot.borrow_mut().replace(hook).is_none()));
    TestBeforeRelease
}
