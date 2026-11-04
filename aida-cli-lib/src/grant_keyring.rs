//! BUG-1934 (SKETCH v2): possession of a seat grant's secret in the Linux
//! session keyring.
//!
//! A process-tree binding (`proc:<pid>@<start>`) cannot be seen from inside a
//! PID namespace: an agent harness's sandbox (bubblewrap) runs each tool call
//! in its own, so the bound shell or launcher never appears in the
//! presenter's ancestry. Keyrings belong to credentials, not namespaces, and
//! are inherited across `fork`/`exec`, `setsid` and a new PID or user
//! namespace. A grant's secret is kept as a possessor-only `user` key named
//! `aida-grant:<id>` in a fresh anonymous session keyring: every descendant of
//! the process that installed it can read it, and a same-user process outside
//! that tree (even in the same terminal session) gets `EACCES`. Only the
//! secret's SHA-256 is ever written to the grant record.
//!
//! Raw `keyctl(2)`/`add_key(2)` syscalls through `libc`: no keyutils library
//! or build script. Elsewhere than Linux the keyring is reported unavailable,
//! and a grant falls back to its process binding (never to "allowed").
// trace:BUG-1934 | ai:claude

use anyhow::Result;

/// Why a key could not be installed: the failing step and its `errno`, kept
/// so a caller can tell quota exhaustion (`EDQUOT`) from an unavailable
/// keyring instead of discarding the error (TASK-2195).
// trace:BUG-1934 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstallError {
    pub(crate) errno: i32,
    what: String,
}

impl InstallError {
    pub(crate) fn new(what: impl Into<String>, errno: i32) -> Self {
        Self {
            errno,
            what: what.into(),
        }
    }

    /// The per-user kernel key quota (`kernel.keys.maxkeys`/`maxbytes`) is
    /// exhausted.
    pub(crate) fn quota_exhausted(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.errno == libc::EDQUOT
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (errno {})", self.what, self.errno)?;
        // trace:BUG-2429 | ai:claude
        #[cfg(target_os = "linux")]
        if self.errno == libc::ELOOP {
            write!(f, ": the session keyring chain is nested too deeply")?;
        }
        Ok(())
    }
}

impl std::error::Error for InstallError {}

#[cfg(test)]
thread_local! {
    /// Test seam: make this thread's next [`install`] fail with this errno.
    pub(crate) static FORCE_INSTALL_ERRNO: std::cell::Cell<Option<i32>> =
        const { std::cell::Cell::new(None) };
    /// Test seam: make this thread's next `add_key` (after any keyring join)
    /// fail with this errno.
    pub(crate) static FORCE_ADD_KEY_ERRNO: std::cell::Cell<Option<i32>> =
        const { std::cell::Cell::new(None) };
    /// Test seam: make this thread's next join treat the previous session
    /// keyring as nested too deeply to link (`ELOOP`).
    pub(crate) static FORCE_HOLD_ELOOP: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

/// What a possession probe found for one grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KeyRead {
    /// This process possesses the key; its payload.
    Held(Vec<u8>),
    /// The keyring works, but this process does not possess the key.
    NotHeld,
    /// The kernel refused the keyring itself (no keyctl, or a seccomp filter
    /// such as Docker's default profile).
    Unavailable,
}

/// The key description for grant `id`.
pub(crate) fn description(id: &str) -> String {
    format!("aida-grant:{id}")
}

#[cfg(target_os = "linux")]
mod sys {
    use super::InstallError;
    use std::ffi::CString;

    const KEYCTL_GET_KEYRING_ID: libc::c_long = 0;
    const KEYCTL_JOIN_SESSION_KEYRING: libc::c_long = 1;
    const KEYCTL_SETPERM: libc::c_long = 5;
    const KEYCTL_LINK: libc::c_long = 8;
    const KEYCTL_UNLINK: libc::c_long = 9;
    const KEYCTL_SEARCH: libc::c_long = 10;
    const KEYCTL_READ: libc::c_long = 11;
    const KEYCTL_SET_TIMEOUT: libc::c_long = 15;
    const KEYCTL_SESSION_TO_PARENT: libc::c_long = 18;
    const KEYCTL_INVALIDATE: libc::c_long = 21;
    const KEY_SPEC_THREAD_KEYRING: libc::c_long = -1;
    const KEY_SPEC_SESSION_KEYRING: libc::c_long = -3;
    /// Possessor: view, read, write, search, link, setattr. Nobody else
    /// (not the owning user) gets anything.
    const POSSESSOR_ONLY: libc::c_long = 0x3f00_0000;

    fn errno() -> i32 {
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
    }

    fn keyctl(op: libc::c_long, a: libc::c_long, b: libc::c_long, c: libc::c_long) -> libc::c_long {
        // SAFETY: keyctl takes integer/pointer arguments by value; every
        // pointer passed through `a..c` below is a live NUL-terminated
        // string or a buffer of the stated length.
        unsafe { libc::syscall(libc::SYS_keyctl, op, a, b, c, 0 as libc::c_long) }
    }

    pub(super) fn available() -> bool {
        keyctl(KEYCTL_GET_KEYRING_ID, KEY_SPEC_SESSION_KEYRING, 0, 0) >= 0
    }

    /// The possessed key named `desc`, `Ok(None)` when it is not possessed,
    /// `Err(errno)` when the keyring itself refused.
    pub(super) fn search(desc: &str) -> Result<Option<libc::c_long>, i32> {
        let ty = CString::new("user").map_err(|_| libc::EINVAL)?;
        let desc = CString::new(desc).map_err(|_| libc::EINVAL)?;
        let id = keyctl(
            KEYCTL_SEARCH,
            KEY_SPEC_SESSION_KEYRING,
            ty.as_ptr() as libc::c_long,
            desc.as_ptr() as libc::c_long,
        );
        if id >= 0 {
            return Ok(Some(id));
        }
        match errno() {
            libc::ENOKEY | libc::EACCES | libc::EKEYREVOKED | libc::EKEYEXPIRED => Ok(None),
            e => Err(e),
        }
    }

    pub(super) fn read(key: libc::c_long) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; 256];
        let n = keyctl(
            KEYCTL_READ,
            key,
            buf.as_mut_ptr() as libc::c_long,
            buf.len() as libc::c_long,
        );
        if n < 0 || n as usize > buf.len() {
            return None;
        }
        buf.truncate(n as usize);
        Some(buf)
    }

    /// This thread's session keyring id, or `None` when it is unavailable.
    pub(super) fn session_keyring() -> Option<libc::c_long> {
        let id = keyctl(KEYCTL_GET_KEYRING_ID, KEY_SPEC_SESSION_KEYRING, 0, 0);
        (id >= 0).then_some(id)
    }

    fn fail(what: &str) -> InstallError {
        InstallError::new(what, errno())
    }

    /// The keys and keyrings linked directly into keyring `ring`.
    fn members(ring: libc::c_long) -> Option<Vec<libc::c_long>> {
        let mut buf: Vec<i32> = vec![0; 16];
        loop {
            let cap = buf.len() * std::mem::size_of::<i32>();
            let n = keyctl(
                KEYCTL_READ,
                ring,
                buf.as_mut_ptr() as libc::c_long,
                cap as libc::c_long,
            );
            if n < 0 {
                return None;
            }
            let n = n as usize;
            if n <= cap {
                buf.truncate(n / std::mem::size_of::<i32>());
                return Some(buf.into_iter().map(libc::c_long::from).collect());
            }
            buf.resize(n.div_ceil(std::mem::size_of::<i32>()), 0);
        }
    }

    /// Link the previous session keyring `old` into the thread keyring so it
    /// stays possessed across a join, and return what was held. Each launch
    /// nests the previous session keyring one level deeper; once that chain
    /// reaches the kernel's keyring nesting limit (a launcher started by a
    /// launcher started by a launcher...), linking `old` anywhere fails with
    /// `ELOOP`. Then hold `old`'s direct members instead, so the new keyring
    /// is no deeper than `old` and the chain stops growing. A member that
    /// still cannot be linked is dropped: the kernel's search cannot reach a
    /// key that deep anyway.
    // trace:BUG-2429 | ai:claude
    fn hold_previous(old: libc::c_long) -> Result<Vec<libc::c_long>, InstallError> {
        #[cfg(test)]
        let forced = super::FORCE_HOLD_ELOOP.with(std::cell::Cell::take);
        #[cfg(not(test))]
        let forced = false;
        if !forced && keyctl(KEYCTL_LINK, old, KEY_SPEC_THREAD_KEYRING, 0) >= 0 {
            return Ok(vec![old]);
        }
        if !forced && errno() != libc::ELOOP {
            return Err(fail("cannot hold the previous session keyring"));
        }
        let Some(members) = members(old) else {
            return Err(fail("cannot read the previous session keyring"));
        };
        Ok(members
            .into_iter()
            .filter(|&k| keyctl(KEYCTL_LINK, k, KEY_SPEC_THREAD_KEYRING, 0) >= 0)
            .collect())
    }

    /// Add `desc` to a session keyring and return that keyring's id. `reuse`
    /// names a keyring this thread joined earlier: while it is still the
    /// session keyring the key goes straight into it; otherwise a fresh
    /// anonymous keyring is joined (the previous one linked in). `joined`
    /// is set as soon as a join succeeds, even when a later step fails: the
    /// thread's session keyring has changed either way (TASK-2211).
    pub(super) fn install(
        desc: &str,
        secret: &[u8],
        ttl_secs: u32,
        to_parent: bool,
        reuse: Option<libc::c_long>,
        joined: &mut Option<libc::c_long>,
    ) -> Result<libc::c_long, InstallError> {
        let Some(old) = session_keyring() else {
            return Err(fail("the session keyring is unavailable"));
        };
        let ring = if reuse == Some(old) {
            old
        } else {
            // Keep the keys this process already possessed (a login keyring,
            // a parent grant's key) reachable from the new keyring. Linking
            // needs possession of the old keyring, which joining a new
            // session keyring ends, so the thread keyring holds it across
            // the join.
            let held = hold_previous(old)?;
            let new = keyctl(KEYCTL_JOIN_SESSION_KEYRING, 0, 0, 0);
            let linked = new >= 0
                && held
                    .iter()
                    .all(|&k| k == new || keyctl(KEYCTL_LINK, k, new, 0) >= 0);
            let e = errno();
            for &k in &held {
                keyctl(KEYCTL_UNLINK, k, KEY_SPEC_THREAD_KEYRING, 0);
            }
            if new < 0 {
                return Err(InstallError::new("cannot create a session keyring", e));
            }
            // trace:TASK-1955 | ai:claude (TASK-2211)
            *joined = Some(new);
            if !linked {
                return Err(InstallError::new(
                    "cannot link the previous session keyring",
                    e,
                ));
            }
            if keyctl(KEYCTL_SETPERM, new, POSSESSOR_ONLY, 0) < 0 {
                return Err(fail("cannot restrict the session keyring"));
            }
            new
        };
        let invalid = || InstallError::new("the key description holds a NUL byte", libc::EINVAL);
        let ty = CString::new("user").map_err(|_| invalid())?;
        let d = CString::new(desc).map_err(|_| invalid())?;
        #[cfg(test)]
        if let Some(errno) = super::FORCE_ADD_KEY_ERRNO.with(std::cell::Cell::take) {
            return Err(InstallError::new("cannot add the grant key", errno));
        }
        // SAFETY: add_key reads `secret.len()` bytes from a live slice and two
        // NUL-terminated strings.
        let key = unsafe {
            libc::syscall(
                libc::SYS_add_key,
                ty.as_ptr(),
                d.as_ptr(),
                secret.as_ptr(),
                secret.len(),
                KEY_SPEC_SESSION_KEYRING,
            )
        };
        if key < 0 {
            return Err(fail("cannot add the grant key"));
        }
        if keyctl(KEYCTL_SETPERM, key, POSSESSOR_ONLY, 0) < 0
            || keyctl(KEYCTL_SET_TIMEOUT, key, ttl_secs.max(1) as libc::c_long, 0) < 0
        {
            let e = errno();
            keyctl(KEYCTL_INVALIDATE, key, 0, 0);
            return Err(InstallError::new("cannot restrict the grant key", e));
        }
        if to_parent && keyctl(KEYCTL_SESSION_TO_PARENT, 0, 0, 0) < 0 {
            return Err(fail(
                "cannot install the session keyring into the calling shell",
            ));
        }
        Ok(ring)
    }

    pub(super) fn invalidate(key: libc::c_long) -> bool {
        keyctl(KEYCTL_INVALIDATE, key, 0, 0) >= 0
    }
}

/// Whether this process can use a session keyring at all.
pub(crate) fn available() -> bool {
    #[cfg(target_os = "linux")]
    {
        sys::available()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Does this process possess grant `id`'s key, and what is in it?
pub(crate) fn read(id: &str) -> KeyRead {
    #[cfg(target_os = "linux")]
    {
        if !sys::available() {
            return KeyRead::Unavailable;
        }
        match sys::search(&description(id)) {
            Ok(Some(key)) => sys::read(key).map_or(KeyRead::NotHeld, KeyRead::Held),
            Ok(None) => KeyRead::NotHeld,
            Err(_) => KeyRead::Unavailable,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = id;
        KeyRead::Unavailable
    }
}

#[cfg(target_os = "linux")]
thread_local! {
    /// TASK-2195: the session keyring [`install`] joined on this thread.
    /// Later installs on the same thread add their key to it instead of
    /// joining (and pinning) one keyring per launch against the per-user
    /// kernel key quota.
    static LAUNCHER_KEYRING: std::cell::Cell<Option<libc::c_long>> =
        const { std::cell::Cell::new(None) };
}

/// This thread's session keyring id; `None` when the keyring is unavailable.
pub(crate) fn session_keyring_id() -> Option<i64> {
    #[cfg(target_os = "linux")]
    {
        sys::session_keyring().map(i64::from)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Put grant `id`'s secret into a session keyring for THIS thread and every
/// process it later spawns, expiring after `ttl_secs`. The first install on a
/// thread joins a fresh keyring (the previous one stays linked in); later
/// ones add to that same keyring while it is still this thread's session
/// keyring, so a long-lived launcher holds one keyring, not one per launch
/// (TASK-2195). A revoked child's key is invalidated, which frees its quota.
// trace:BUG-1934 | ai:claude
pub(crate) fn install(id: &str, secret: &[u8], ttl_secs: u32) -> Result<(), InstallError> {
    #[cfg(test)]
    if let Some(errno) = FORCE_INSTALL_ERRNO.with(std::cell::Cell::take) {
        return Err(InstallError::new("cannot add the grant key", errno));
    }
    #[cfg(target_os = "linux")]
    {
        let reuse = LAUNCHER_KEYRING.with(std::cell::Cell::get);
        let mut joined = None;
        let installed = sys::install(
            &description(id),
            secret,
            ttl_secs,
            false,
            reuse,
            &mut joined,
        );
        // TASK-2211: remember a joined keyring even when a later step failed
        // (EDQUOT on add_key), so the next launch adds to it instead of
        // joining and chaining yet another keyring against the quota.
        // trace:TASK-1955 | ai:claude (TASK-2211)
        if let Some(ring) = installed.as_ref().ok().copied().or(joined) {
            LAUNCHER_KEYRING.with(|k| k.set(Some(ring)));
        }
        installed.map(|_| ())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (id, secret, ttl_secs);
        Err(InstallError::new(
            "the session keyring is only available on Linux",
            0,
        ))
    }
}

/// Join a fresh session keyring holding grant `id`'s key, then replace the
/// PARENT process's session keyring with it (`KEYCTL_SESSION_TO_PARENT`):
/// how `aida role adopt-key`, run by the shell that eval'd `aida role enter`,
/// hands the key to that shell.
pub(crate) fn install_to_parent(id: &str, secret: &[u8], ttl_secs: u32) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        sys::install(&description(id), secret, ttl_secs, true, None, &mut None)?;
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (id, secret, ttl_secs);
        anyhow::bail!("the session keyring is only available on Linux")
    }
}

/// Invalidate grant `id`'s key when this process can reach it (a revoke from
/// the shell or launcher that holds it). `false` when it is not reachable.
pub(crate) fn invalidate(id: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        matches!(sys::search(&description(id)), Ok(Some(key)) if sys::invalidate(key))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = id;
        false
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    //! BUG-2429: a launcher whose session keyring is already deeply nested
    //! (launched by a launcher launched by a launcher) still joins a fresh
    //! keyring instead of failing with `ELOOP`, and keeps its parent's key.
    // trace:BUG-2429 | ai:claude
    use super::*;

    fn held(id: &str) -> bool {
        matches!(read(id), KeyRead::Held(_))
    }

    /// Install `id` the way a fresh launcher process does: always join a new
    /// keyring with the previous one linked in.
    fn launch(id: &str) -> Result<(), InstallError> {
        sys::install(&description(id), b"secret", 30, false, None, &mut None).map(|_| ())
    }

    #[test]
    fn nested_launch_chain_never_fails_with_eloop() {
        if !available() {
            eprintln!("SKIP: the session keyring is unavailable here");
            return;
        }
        // Well past the kernel's keyring nesting limit (6).
        for depth in 0..12 {
            let id = format!("bug-2429-chain-{depth}");
            launch(&id).unwrap_or_else(|e| panic!("launch at depth {depth}: {e}"));
            assert!(held(&id), "the new key at depth {depth}");
            if depth > 0 {
                let parent = format!("bug-2429-chain-{}", depth - 1);
                assert!(held(&parent), "the parent key at depth {depth}");
            }
        }
    }

    #[test]
    fn eloop_flattens_the_previous_keyring_and_keeps_its_keys() {
        if !available() {
            eprintln!("SKIP: the session keyring is unavailable here");
            return;
        }
        launch("bug-2429-flat-parent").unwrap();
        let before = sys::session_keyring().unwrap();
        FORCE_HOLD_ELOOP.with(|f| f.set(true));
        launch("bug-2429-flat-child").unwrap();
        assert_ne!(
            sys::session_keyring().unwrap(),
            before,
            "joined a new keyring"
        );
        assert!(
            held("bug-2429-flat-parent"),
            "the parent key stays possessed"
        );
        assert!(held("bug-2429-flat-child"));
    }

    #[test]
    fn eloop_error_names_the_cause() {
        let e = InstallError::new("cannot hold the previous session keyring", libc::ELOOP);
        assert!(e.to_string().contains("nested too deeply"), "{e}");
        let e = InstallError::new("cannot add the grant key", libc::EDQUOT);
        assert!(!e.to_string().contains("nested"), "{e}");
    }
}
