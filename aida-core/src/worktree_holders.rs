//! Live-process guard for worktree removal.
//!
//! Before any command deletes a worktree directory (`git worktree remove`, or
//! a teardown that ends in one), it asks [`check_worktree_removal`] whether a
//! live process is still using the tree. On 2026-10-10 a stale-session sweep
//! salvage-committed and then removed worktrees in which implementers were
//! running cargo, which killed their builds mid-flight. The salvage commit was
//! harmless; the deletion was not.
//!
//! ## What counts as "using" a worktree (Linux)
//!
//! For every readable `/proc/<pid>`, a process holds the worktree when any of
//! these resolve to a path at or under the canonical worktree path:
//!
//! - `cwd`: its working directory (a shell, an agent, `cargo build`),
//! - `root`: its root directory (a chroot inside the tree),
//! - `exe`: its executable (a test binary built into the tree's `target/`),
//! - any `fd/*` link: an open file (a cargo lock, an editor buffer).
//!
//! Entries the caller cannot read, such as other users' processes or ones that
//! exit mid-scan, are skipped. The scan is bounded: at most
//! [`MAX_FDS_PER_PROCESS`] fd links are read per process.
//!
//! ## Who is NOT counted (the self-exclusion rule)
//!
//! A session is legitimately ended from inside its own worktree: the operator's
//! shell, or the agent that owns the session, runs `aida session end` with its
//! cwd in the tree. Counting those processes would make ending your own
//! session impossible. So the scan excludes:
//!
//! 1. the current process and every ancestor (the `aida` invocation, the
//!    shell that ran it, the agent that ran the shell, and so on up to init);
//! 2. descendants of `T`, the *highest* ancestor in the unbroken run (starting
//!    at this process) whose cwd is inside the worktree, but ONLY those whose
//!    session id (`/proc/<pid>/stat` field 6) is the session of some process
//!    on the chain from the caller up to `T`. That covers the caller's own
//!    shell and its background jobs (the shell's session), and the agent that
//!    ran the shell together with its helpers (MCP servers, language servers),
//!    which the agent forked into its own session.
//!
//! The session bound keeps a terminal multiplexer or editor that was started
//! in the worktree from exempting everything under it. A tmux server calls
//! `setsid`, and so does every pane it opens; VS Code's integrated terminals
//! each get their own session too. A codex or `cargo build` running in a
//! SIBLING pane or terminal is therefore in a different session from both the
//! caller and `T`, so it still counts as a holder. Claude Code's Bash tool
//! also runs every command under `setsid`: the tool shell leads its own
//! session, while the agent stays in the session it was started in, and a
//! daemonized agent sits under a pty host that has a session of its own. That
//! is why every session on the chain is exempt, not just the caller's and
//! `T`'s. Without that, the agent's own MCP servers would block it from
//! ending its own session. A sibling pane is never on the caller's chain, so
//! its session stays counted. A background command the agent started from a
//! separate tool call has its own session, so it DOES block; stop it first,
//! or pass the override flag.
//!
//! The run must be unbroken: an orchestrator whose cwd is the main checkout
//! does not pass the exemption to the implementers it spawned into other
//! worktrees. That cross-worktree case is exactly what this guard exists to
//! refuse.
//!
//! The session's own *recorded* owner pid gets no special treatment. If that
//! process is dead it is not in `/proc` and cannot block. If it is alive and
//! outside the caller's own chain, it is a live implementer, and the guard
//! refuses, as intended.
//!
//! ## When the scan cannot be trusted
//!
//! A single unreadable entry (another user's process, or one that exited
//! mid-scan) is skipped. It cannot be attributed, and refusing on it would
//! block every removal on a busy machine. The scan as a WHOLE is
//! "unverified" when `/proc` itself cannot be listed, when the caller
//! cannot see its own pid there (a `/proc` belonging to another pid
//! namespace, or a restricted mount), or when the caller runs in a pid
//! namespace other than the host's. The last is read from the inode of
//! `/proc/<self>/ns/pid`: the kernel reserves a fixed inode for the initial
//! namespace, and that is the one signal that still holds when the namespace
//! mounts its own `/proc` (there it looks complete and `NSpid` shows a single
//! level). Setting `AIDA_TRUST_PID_NAMESPACE=1` waives the namespace check
//! for a container that owns its worktrees (nothing outside it can be using
//! them); the other two unverified cases still apply. A `hidepid` mount that still shows the caller's own pid is NOT
//! detected; other users' processes are then skipped like any unreadable
//! entry. [`check_worktree_removal`], which every bulk or
//! automatic path uses, treats an unverified scan as a refusal, so those
//! paths keep the tree. [`check_worktree_removal_interactive`], used when an
//! operator ends one session by name, warns and proceeds.
//!
//! ## Other platforms
//!
//! Without `/proc` the scan cannot run. Both checks then allow the removal
//! and print a one-line warning that the check was skipped.
//!
// trace:BUG-1947 | ai:claude

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

/// Upper bound on fd links read per process. This keeps a process with a huge
/// descriptor table (a database, a browser) from dominating the scan.
// trace:BUG-1947 | ai:claude
pub const MAX_FDS_PER_PROCESS: usize = 4096;

/// Upper bound on parent-chain hops. It protects against a corrupt or cyclic
/// ppid map.
const MAX_ANCESTRY_DEPTH: usize = 128;

/// How a process holds the worktree.
// trace:BUG-1947 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HoldKind {
    /// Its working directory is inside the worktree.
    Cwd,
    /// Its root directory is inside the worktree.
    Root,
    /// Its executable lives inside the worktree.
    Executable,
    /// It has this file under the worktree open.
    OpenFile(PathBuf),
}

impl HoldKind {
    fn describe(&self) -> String {
        match self {
            HoldKind::Cwd => "working directory is inside the worktree".to_string(),
            HoldKind::Root => "root directory is inside the worktree".to_string(),
            HoldKind::Executable => "running a program from inside the worktree".to_string(),
            HoldKind::OpenFile(p) => format!("has {} open", p.display()),
        }
    }
}

/// One live process that is using a worktree.
// trace:BUG-1947 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeHolder {
    pub pid: u32,
    /// Command line, falling back to the short process name. Truncated for display.
    pub command: String,
    pub hold: HoldKind,
}

/// Removal refused because live processes are using the worktree. `Display`
/// names every pid and command. The caller appends the override flag for its
/// own command.
// trace:BUG-1947 | ai:claude
#[derive(Debug, Clone)]
pub struct WorktreeInUse {
    pub worktree: PathBuf,
    pub holders: Vec<WorktreeHolder>,
    /// Set when the scan itself could not be trusted, so `holders` is empty
    /// and the tree is NOT known to be free. The text says why. Bulk and
    /// automatic paths keep the tree in that case.
    pub unverified: Option<String>,
    /// True when the scan was unverified because this process runs in a
    /// separate pid namespace, the one case [`TRUST_PID_NAMESPACE_ENV`]
    /// waives. Callers print the opt-out hint only then.
    // trace:BUG-1947 | ai:claude
    pub pid_namespace: bool,
}

/// Why a live-process scan could not be trusted. `pid_namespace` marks the
/// separate-pid-namespace case, the only one [`TRUST_PID_NAMESPACE_ENV`]
/// waives; `Display` adds the opt-out hint only for that case.
// trace:BUG-1947 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanUnverified {
    pub reason: String,
    pub pid_namespace: bool,
}

impl ScanUnverified {
    fn other(reason: String) -> Self {
        Self {
            reason,
            pid_namespace: false,
        }
    }
}

impl fmt::Display for ScanUnverified {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason)?;
        if self.pid_namespace {
            f.write_str(&trust_pid_namespace_hint())?;
        }
        Ok(())
    }
}

/// The opt-out hint appended to a pid-namespace refusal, starting with `; `.
// trace:BUG-1947 | ai:claude
pub fn trust_pid_namespace_hint() -> String {
    format!(
        "; if nothing outside this pid namespace uses these worktrees, set \
         {TRUST_PID_NAMESPACE_ENV}=1 to trust the scan"
    )
}

impl fmt::Display for WorktreeInUse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(why) = &self.unverified {
            write!(
                f,
                "could not check worktree {} for live processes: {why}",
                self.worktree.display()
            )?;
            if self.pid_namespace {
                f.write_str(&trust_pid_namespace_hint())?;
            }
            return Ok(());
        }
        let n = self.holders.len();
        write!(
            f,
            "worktree {} is in use by {} live process{}:",
            self.worktree.display(),
            n,
            if n == 1 { "" } else { "es" }
        )?;
        for h in &self.holders {
            write!(
                f,
                "\n  pid {} ({}): {}",
                h.pid,
                h.command,
                h.hold.describe()
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for WorktreeInUse {}

impl WorktreeInUse {
    /// Short single-line form for table rows and skip reasons, e.g.
    /// `in use by live process(es): pid 123 (sleep 600), pid 456 (cargo build)`.
    pub fn summary(&self) -> String {
        if let Some(why) = &self.unverified {
            return format!("not verified free of live processes ({why})");
        }
        let pids: Vec<String> = self
            .holders
            .iter()
            .map(|h| format!("pid {} ({})", h.pid, h.command))
            .collect();
        format!("in use by live process(es): {}", pids.join(", "))
    }

    /// Full refusal text: the holder list plus how to override it with `flag`.
    pub fn refusal(&self, flag: &str) -> String {
        if self.unverified.is_some() {
            return format!(
                "{self}\n\nThe worktree was kept because a live process may still be \
                 using it. Pass {flag} to remove it anyway."
            );
        }
        format!(
            "{self}\n\nRemoving it now would break the work in progress. Stop those \
             processes first, or pass {flag} to remove the worktree anyway."
        )
    }
}

/// Fail with [`WorktreeInUse`] when a live process, other than the caller's
/// own process chain, is using `worktree`, OR when the scan could not be
/// trusted (see the module docs). Every bulk or automatic removal path uses
/// this strict form. An empty or missing path never blocks: nothing is on
/// disk to protect.
// trace:BUG-1947 | ai:claude
pub fn check_worktree_removal(worktree: &Path) -> Result<(), WorktreeInUse> {
    #[cfg(not(target_os = "linux"))]
    {
        warn_unsupported_platform(worktree);
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        match live_worktree_holders(worktree) {
            Ok(holders) if holders.is_empty() => Ok(()),
            Ok(holders) => Err(WorktreeInUse {
                worktree: worktree.to_path_buf(),
                holders,
                unverified: None,
                pid_namespace: false,
            }),
            Err(why) => Err(WorktreeInUse {
                worktree: worktree.to_path_buf(),
                holders: Vec::new(),
                unverified: Some(why.reason),
                pid_namespace: why.pid_namespace,
            }),
        }
    }
}

/// Like [`check_worktree_removal`], for an operator ending ONE session by
/// name: an unverified scan prints a warning and allows the removal instead
/// of refusing. Live holders still refuse.
// trace:BUG-1947 | ai:claude
pub fn check_worktree_removal_interactive(worktree: &Path) -> Result<(), WorktreeInUse> {
    match check_worktree_removal(worktree) {
        Err(e) if e.unverified.is_some() => {
            eprintln!("warning: {e}; removing it without that check");
            Ok(())
        }
        other => other,
    }
}

#[cfg(not(target_os = "linux"))]
fn warn_unsupported_platform(worktree: &Path) {
    if !worktree.as_os_str().is_empty() && worktree.exists() {
        eprintln!(
            "warning: cannot check for live processes in {} on this platform; removing without that check",
            worktree.display()
        );
    }
}

/// Live processes using `worktree`, excluding the caller's own chain (see the
/// module docs). `Err` carries the reason the scan could not be trusted.
/// Always `Ok(empty)` off Linux, and for an empty or missing path.
// trace:BUG-1947 | ai:claude
pub fn live_worktree_holders(worktree: &Path) -> Result<Vec<WorktreeHolder>, ScanUnverified> {
    #[cfg(target_os = "linux")]
    {
        scan_holders_in(Path::new("/proc"), worktree, std::process::id())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = worktree;
        Ok(Vec::new())
    }
}

/// The scan behind [`live_worktree_holders`], parameterised over the proc root
/// and the caller's pid so tests can drive it with a fake `/proc` tree.
/// `Err` means the scan as a whole is unverified: `proc_root` cannot be
/// listed, or `self_pid` is not visible in it.
// trace:BUG-1947 | ai:claude
pub fn scan_holders_in(
    proc_root: &Path,
    worktree: &Path,
    self_pid: u32,
) -> Result<Vec<WorktreeHolder>, ScanUnverified> {
    // An empty base would match every path under `starts_with`.
    if worktree.as_os_str().is_empty() {
        return Ok(Vec::new());
    }
    let Ok(canon) = worktree.canonicalize() else {
        return Ok(Vec::new()); // Missing worktree: nothing to protect.
    };
    let inside = |p: &Path| p.starts_with(&canon);

    let entries = std::fs::read_dir(proc_root).map_err(|e| {
        ScanUnverified::other(format!("{} is not readable ({e})", proc_root.display()))
    })?;
    // Sorted so refusal messages list pids deterministically.
    let pids: BTreeSet<u32> = entries
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .collect();
    // If this process cannot see itself, it is looking at a different pid
    // namespace (or a restricted mount) and other processes may be hidden.
    if !pids.contains(&self_pid) {
        return Err(ScanUnverified::other(format!(
            "this process (pid {self_pid}) is not visible in {}, so other processes may be hidden",
            proc_root.display()
        )));
    }

    // The operator can vouch that nothing outside this pid namespace uses
    // the worktrees (a dev container that owns its checkout), which would
    // otherwise leave every automatic cleanup refusing. trace:BUG-1947 | ai:claude
    if !trust_pid_namespace() {
        if let Some(reason) = separate_pid_namespace(proc_root, self_pid) {
            return Err(ScanUnverified {
                reason,
                pid_namespace: true,
            });
        }
    }

    // The ppid/session maps cover every visible process, so subtree
    // membership can be decided without a second pass over /proc.
    let mut ppid: BTreeMap<u32, u32> = BTreeMap::new();
    let mut sid: BTreeMap<u32, u32> = BTreeMap::new();
    for &pid in &pids {
        if let Some((pp, s)) = read_ppid_sid(proc_root, pid) {
            ppid.insert(pid, pp);
            sid.insert(pid, s);
        }
    }

    // Rule 1: self and ancestors.
    let chain = ancestry(&ppid, self_pid);
    let excluded: BTreeSet<u32> = chain.iter().copied().collect();

    // Rule 2: T is the highest ancestor in the unbroken in-worktree run from
    // self. Only T's descendants in a session that some process on the chain
    // from self up to T belongs to are exempt (see the module docs).
    let mut subtree_root: Option<u32> = None;
    let mut run_len = 0usize;
    for &pid in &chain {
        match read_link(proc_root, pid, "cwd") {
            Some(cwd) if inside(&cwd) => {
                subtree_root = Some(pid);
                run_len += 1;
            }
            _ => break,
        }
    }
    // Self is always in its own session's set, even when its cwd is outside
    // (then there is no T and nothing below is exempt anyway).
    let exempt_sessions: BTreeSet<u32> = chain[..run_len.max(1)]
        .iter()
        .filter_map(|p| sid.get(p).copied())
        .collect();

    let mut holders = Vec::new();
    for &pid in &pids {
        if excluded.contains(&pid) {
            continue;
        }
        if let Some(root) = subtree_root {
            let same_session = sid.get(&pid).is_some_and(|s| exempt_sessions.contains(s));
            if same_session && ancestry(&ppid, pid).contains(&root) {
                continue;
            }
        }
        if let Some(hold) = holds(proc_root, pid, &inside) {
            holders.push(WorktreeHolder {
                pid,
                command: command_of(proc_root, pid),
                hold,
            });
        }
    }
    Ok(holders)
}

/// Inode of the initial (host) pid namespace. The kernel reserves it
/// (`PROC_PID_INIT_INO` in `include/linux/proc_ns.h`); every other pid
/// namespace gets a dynamically allocated inode, which never equals it.
const INITIAL_PID_NAMESPACE_INODE: u64 = 0xEFFF_FFFC;

/// `Some(reason)` when this process runs in a pid namespace other than the
/// host's. Processes outside the namespace (a host-side build in a
/// bind-mounted worktree) are then invisible, even though `/proc` looks
/// complete: a namespace that mounts its own `/proc` sees itself as pid 1's
/// world, and `NSpid` in its status file shows only one level. The namespace
/// link's inode is the one signal that holds there. A missing or unparsable
/// link (a kernel without pid namespaces, or a test's fake `/proc`) gives no
/// signal, so the scan proceeds.
// trace:BUG-1947 | ai:claude
fn separate_pid_namespace(proc_root: &Path, self_pid: u32) -> Option<String> {
    let link =
        std::fs::read_link(proc_root.join(self_pid.to_string()).join("ns").join("pid")).ok()?;
    let inode: u64 = link
        .to_str()?
        .strip_prefix("pid:[")?
        .strip_suffix(']')?
        .parse()
        .ok()?;
    (inode != INITIAL_PID_NAMESPACE_INODE).then(|| {
        "this process runs in a separate pid namespace (a container?), so processes \
         outside it are hidden"
            .to_string()
    })
}

/// Environment opt-out for the separate-pid-namespace refusal. Set to `1`
/// (or `true`/`yes`) when no process outside this pid namespace can be using
/// the worktrees, e.g. a dev container or CI job that owns its checkout.
/// Without it, a scan from inside a container is unverified and every
/// automatic cleanup (pool reuse, gc, reap, doctor heals, stale-lease
/// release) keeps the tree. It does NOT trust a `/proc` in which this
/// process cannot see itself.
// trace:BUG-1947 | ai:claude
pub const TRUST_PID_NAMESPACE_ENV: &str = "AIDA_TRUST_PID_NAMESPACE";

/// True when [`TRUST_PID_NAMESPACE_ENV`] opts out of the pid-namespace check.
// trace:BUG-1947 | ai:claude
pub fn trust_pid_namespace() -> bool {
    std::env::var(TRUST_PID_NAMESPACE_ENV)
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

/// `pid` followed by its ancestors, nearest first, bounded in depth.
fn ancestry(ppid: &BTreeMap<u32, u32>, pid: u32) -> Vec<u32> {
    let mut out = vec![pid];
    let mut cur = pid;
    for _ in 0..MAX_ANCESTRY_DEPTH {
        match ppid.get(&cur) {
            Some(&p) if p != 0 && p != cur && !out.contains(&p) => {
                out.push(p);
                cur = p;
            }
            _ => break,
        }
    }
    out
}

/// The first way `pid` holds the worktree, checked from cheapest to costliest.
fn holds(proc_root: &Path, pid: u32, inside: &dyn Fn(&Path) -> bool) -> Option<HoldKind> {
    if read_link(proc_root, pid, "cwd").is_some_and(|p| inside(&p)) {
        return Some(HoldKind::Cwd);
    }
    if read_link(proc_root, pid, "root").is_some_and(|p| inside(&p) && p != Path::new("/")) {
        return Some(HoldKind::Root);
    }
    if read_link(proc_root, pid, "exe").is_some_and(|p| inside(&p)) {
        return Some(HoldKind::Executable);
    }
    let fd_dir = proc_root.join(pid.to_string()).join("fd");
    let fds = std::fs::read_dir(fd_dir).ok()?; // Unreadable: another user's process.
    for fd in fds.flatten().take(MAX_FDS_PER_PROCESS) {
        if let Ok(target) = std::fs::read_link(fd.path()) {
            if inside(&target) {
                return Some(HoldKind::OpenFile(target));
            }
        }
    }
    None
}

fn read_link(proc_root: &Path, pid: u32, name: &str) -> Option<PathBuf> {
    std::fs::read_link(proc_root.join(pid.to_string()).join(name)).ok()
}

/// Parent pid and session id (fields 4 and 6) from `/proc/<pid>/stat`. The
/// comm field can contain spaces and parentheses, so parse after the LAST `)`.
fn read_ppid_sid(proc_root: &Path, pid: u32) -> Option<(u32, u32)> {
    let stat = std::fs::read_to_string(proc_root.join(pid.to_string()).join("stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 1..];
    // rest = " <state> <ppid> <pgrp> <session> ..."
    let mut f = rest.split_whitespace();
    let ppid = f.nth(1)?.parse().ok()?;
    let session = f.nth(1)?.parse().ok()?;
    Some((ppid, session))
}

/// Display command: the NUL-separated cmdline joined with spaces, falling back
/// to `comm`, truncated to keep refusal messages readable.
fn command_of(proc_root: &Path, pid: u32) -> String {
    let dir = proc_root.join(pid.to_string());
    let from_cmdline = std::fs::read(dir.join("cmdline")).ok().map(|raw| {
        raw.split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    });
    let cmd = from_cmdline
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::fs::read_to_string(dir.join("comm"))
                .ok()
                .map(|s| s.trim().to_string())
        })
        .unwrap_or_else(|| "?".to_string());
    const MAX: usize = 120;
    if cmd.chars().count() > MAX {
        format!("{}…", cmd.chars().take(MAX).collect::<String>())
    } else {
        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    /// Builds a fake `/proc` tree: one dir per pid with `stat`, `cmdline`,
    /// and optional `cwd`/`exe` symlinks and `fd/` links.
    struct FakeProc {
        root: PathBuf,
    }

    impl FakeProc {
        fn new(base: &Path) -> Self {
            let root = base.join("proc");
            std::fs::create_dir_all(&root).unwrap();
            Self { root }
        }
        /// A process in session 1 (every test that does not care about
        /// sessions puts everything in one).
        fn add(&self, pid: u32, ppid: u32, cmd: &str, cwd: &Path) -> PathBuf {
            self.add_in(1, pid, ppid, cmd, cwd)
        }
        /// A process in session `sid` (stat field 6).
        fn add_in(&self, sid: u32, pid: u32, ppid: u32, cmd: &str, cwd: &Path) -> PathBuf {
            let dir = self.root.join(pid.to_string());
            std::fs::create_dir_all(dir.join("fd")).unwrap();
            // comm containing ") " exercises the last-paren field parse.
            std::fs::write(
                dir.join("stat"),
                format!("{pid} (we) ird) S {ppid} {sid} {sid} 0"),
            )
            .unwrap();
            std::fs::write(dir.join("cmdline"), cmd.replace(' ', "\0") + "\0").unwrap();
            symlink(cwd, dir.join("cwd")).unwrap();
            dir
        }
    }

    fn fixture() -> (tempfile::TempDir, FakeProc, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let wt = base.join("wt");
        let outside = base.join("main");
        std::fs::create_dir_all(wt.join("target")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let fake = FakeProc::new(&base);
        (tmp, fake, wt, outside)
    }

    fn pids(h: &[WorktreeHolder]) -> Vec<u32> {
        h.iter().map(|h| h.pid).collect()
    }

    // trace:BUG-1947 | ai:claude
    #[test]
    fn unrelated_process_with_cwd_inside_is_a_holder_named_by_command() {
        let (_t, fake, wt, outside) = fixture();
        fake.add(1, 0, "init", Path::new("/"));
        fake.add(200, 1, "aida session end", &outside);
        fake.add(300, 1, "cargo build", &wt.join("target"));
        let h = scan_holders_in(&fake.root, &wt, 200).unwrap();
        assert_eq!(pids(&h), vec![300]);
        assert_eq!(h[0].command, "cargo build");
        assert_eq!(h[0].hold, HoldKind::Cwd);
    }

    // trace:BUG-1947 | ai:claude
    #[test]
    fn self_and_ancestors_are_excluded_but_an_orchestrators_other_children_are_not() {
        let (_t, fake, wt, outside) = fixture();
        fake.add(1, 0, "init", Path::new("/"));
        // Orchestrator in the main checkout spawned two agents.
        fake.add(50, 1, "orchestrator", &outside);
        fake.add(100, 50, "claude", &wt); // the session's own agent
        fake.add(150, 100, "bash", &wt); // its tool shell
        fake.add(200, 150, "aida session end", &wt); // self
        fake.add(160, 100, "aida mcp", &wt); // the agent's own helper
        fake.add(300, 50, "claude other", &wt); // a different live implementer
        let h = scan_holders_in(&fake.root, &wt, 200).unwrap();
        // 200/150/100/50/1 are self+ancestors; 160 is in the own-session
        // subtree rooted at 100; 300 descends from 50, which sits outside the
        // worktree, so it is NOT exempt.
        assert_eq!(pids(&h), vec![300]);
    }

    // trace:BUG-1947 | ai:claude
    #[test]
    fn subtree_exemption_needs_an_unbroken_in_worktree_run_from_self() {
        let (_t, fake, wt, outside) = fixture();
        fake.add(1, 0, "init", Path::new("/"));
        fake.add(100, 1, "shell", &wt); // ancestor in the worktree
        fake.add(200, 100, "aida", &outside); // self is OUTSIDE: run broken
        fake.add(110, 100, "cargo test", &wt); // sibling of self
        let h = scan_holders_in(&fake.root, &wt, 200).unwrap();
        assert_eq!(
            pids(&h),
            vec![110],
            "ancestor excluded, its other child is not"
        );
    }

    // trace:BUG-1947 | ai:claude
    #[test]
    fn open_files_and_executables_under_the_tree_count_and_prefix_siblings_do_not() {
        let (_t, fake, wt, outside) = fixture();
        let sibling = wt.with_file_name("wt-other");
        std::fs::create_dir_all(&sibling).unwrap();
        fake.add(1, 0, "init", Path::new("/"));
        fake.add(200, 1, "aida", &outside);
        let p = fake.add(301, 1, "editor", &outside);
        symlink(wt.join("target/.cargo-lock"), p.join("fd/7")).unwrap();
        let p = fake.add(302, 1, "testbin", &outside);
        symlink(wt.join("target/debug/testbin"), p.join("exe")).unwrap();
        fake.add(303, 1, "neighbour", &sibling); // `/wt-other` is not under `/wt`
                                                 // A pid dir with no readable links (another user's process) is skipped.
        std::fs::create_dir_all(fake.root.join("304")).unwrap();
        let h = scan_holders_in(&fake.root, &wt, 200).unwrap();
        assert_eq!(pids(&h), vec![301, 302]);
        assert!(matches!(h[0].hold, HoldKind::OpenFile(_)));
        assert_eq!(h[1].hold, HoldKind::Executable);
    }

    // trace:BUG-1947 | ai:claude
    #[test]
    fn empty_or_missing_worktree_never_blocks() {
        let (_t, fake, wt, _outside) = fixture();
        fake.add(300, 1, "anything", Path::new("/"));
        assert!(scan_holders_in(&fake.root, Path::new(""), 1)
            .unwrap()
            .is_empty());
        assert!(scan_holders_in(&fake.root, &wt.join("gone"), 1)
            .unwrap()
            .is_empty());
    }

    // trace:BUG-1947 | ai:claude
    #[test]
    fn tmux_started_in_the_worktree_does_not_exempt_a_sibling_pane() {
        // The operator started tmux from the worktree. Pane A runs codex and
        // a build; in pane B the operator runs aida. tmux's server and every
        // pane lead their own sessions, as real tmux does.
        let (_t, fake, wt, _outside) = fixture();
        fake.add(1, 0, "init", Path::new("/"));
        fake.add_in(500, 500, 1, "tmux new-session", &wt); // server
        fake.add_in(510, 510, 500, "bash", &wt); // pane A
        fake.add_in(510, 511, 510, "codex", &wt);
        fake.add_in(510, 512, 511, "cargo build", &wt.join("target"));
        fake.add_in(520, 520, 500, "bash", &wt); // pane B
        fake.add_in(520, 521, 520, "aida session reap", &wt); // self
        fake.add_in(520, 522, 520, "cargo check", &wt); // pane B's own job
        let h = scan_holders_in(&fake.root, &wt, 521).unwrap();
        // T is the tmux server (500); nothing else is in its session, and
        // pane A is in a different session from the caller, so pane A's whole
        // tree is held. Pane B's own job shares the caller's session.
        assert_eq!(pids(&h), vec![510, 511, 512]);
    }

    // trace:BUG-1947 | ai:claude
    #[test]
    fn claude_code_shape_keeps_its_helpers_exempt_but_not_a_detached_build() {
        // An agent started from a terminal in the worktree. Claude Code's
        // Bash tool runs each command under setsid, so the tool shell (and
        // the caller) lead their own session while the agent and its MCP
        // servers stay in the terminal's session.
        let (_t, fake, wt, outside) = fixture();
        fake.add(1, 0, "init", Path::new("/"));
        fake.add_in(10, 10, 1, "terminal", &outside);
        fake.add_in(20, 20, 10, "bash", &wt); // interactive shell, T
        fake.add_in(20, 100, 20, "claude", &wt);
        fake.add_in(20, 110, 100, "aida mcp-serve", &wt); // agent helper
        fake.add_in(200, 200, 100, "bash -c", &wt); // tool shell
        fake.add_in(200, 201, 200, "aida session end", &wt); // self
        fake.add_in(300, 300, 100, "cargo test", &wt); // separate tool call
        let h = scan_holders_in(&fake.root, &wt, 201).unwrap();
        assert_eq!(pids(&h), vec![300]);
    }

    // trace:BUG-1947 | ai:claude
    #[test]
    fn daemonized_agent_keeps_its_own_helpers_and_jobs_exempt() {
        // Claude Code in daemon mode: a pty host started in the worktree leads
        // its own session, the agent under it leads ANOTHER, and each tool
        // shell is setsid'd. T is the pty host, so exempting only the
        // caller's and T's sessions would count the agent's own MCP server.
        let (_t, fake, wt, outside) = fixture();
        fake.add(1, 0, "init", Path::new("/"));
        fake.add_in(5, 5, 1, "claude daemon", &outside);
        fake.add_in(50, 50, 5, "bg-pty-host", &wt); // T
        fake.add_in(100, 100, 50, "claude", &wt);
        fake.add_in(100, 110, 100, "aida mcp-serve", &wt); // agent helper
        fake.add_in(200, 200, 100, "bash -c", &wt); // tool shell
        fake.add_in(200, 201, 200, "aida session end", &wt); // self
        fake.add_in(200, 202, 200, "tail -f log", &wt); // shell's bg job
        fake.add_in(300, 300, 100, "cargo test", &wt); // separate tool call
        fake.add_in(600, 600, 5, "other agent", &wt); // not under T
        fake.add_in(700, 700, 50, "detached build", &wt); // under T, own sid
        let h = scan_holders_in(&fake.root, &wt, 201).unwrap();
        assert_eq!(pids(&h), vec![300, 600, 700]);
    }

    // trace:BUG-1947 | ai:claude
    #[test]
    fn a_separate_pid_namespace_is_unverified_but_the_host_namespace_is_not() {
        let (_t, fake, wt, _outside) = fixture();
        fake.add(1, 0, "init", Path::new("/"));
        let me = fake.add(200, 1, "aida session reap", &wt);
        std::fs::create_dir_all(me.join("ns")).unwrap();

        // Host namespace: the kernel's reserved inode.
        symlink("pid:[4026531836]", me.join("ns").join("pid")).unwrap();
        assert!(scan_holders_in(&fake.root, &wt, 200).unwrap().is_empty());

        // A container that mounted its own /proc: pid list looks complete,
        // but the namespace inode is a dynamically allocated one.
        std::fs::remove_file(me.join("ns").join("pid")).unwrap();
        symlink("pid:[4026533285]", me.join("ns").join("pid")).unwrap();
        let err = scan_holders_in(&fake.root, &wt, 200).unwrap_err();
        assert!(err.pid_namespace, "{err}");
        assert!(err.reason.contains("separate pid namespace"), "{err}");
        // The refusal names its opt-out, once.
        let text = err.to_string();
        assert_eq!(
            text.matches("AIDA_TRUST_PID_NAMESPACE=1").count(),
            1,
            "{text}"
        );

        // Unparsable link: no signal, scan proceeds.
        std::fs::remove_file(me.join("ns").join("pid")).unwrap();
        symlink("garbage", me.join("ns").join("pid")).unwrap();
        assert!(scan_holders_in(&fake.root, &wt, 200).is_ok());
    }

    /// The opt-out trusts a container's own pid namespace (the scan runs and
    /// still finds holders inside it), but never a /proc that hides the
    /// caller itself.
    // trace:BUG-1947 | ai:claude
    #[test]
    fn trust_pid_namespace_opt_out_verifies_a_container_scan() {
        let (_t, fake, wt, _outside) = fixture();
        fake.add(1, 0, "init", Path::new("/"));
        let me = fake.add(200, 1, "aida session reap", Path::new("/"));
        fake.add(300, 1, "cargo build", &wt);
        std::fs::create_dir_all(me.join("ns")).unwrap();
        symlink("pid:[4026533285]", me.join("ns").join("pid")).unwrap();

        {
            let _env = crate::test_env::EnvVarGuard::unset(TRUST_PID_NAMESPACE_ENV);
            assert!(!trust_pid_namespace());
            assert!(scan_holders_in(&fake.root, &wt, 200).is_err());
        }
        {
            let _env = crate::test_env::EnvVarGuard::set(TRUST_PID_NAMESPACE_ENV, "1");
            assert!(trust_pid_namespace());
            let h = scan_holders_in(&fake.root, &wt, 200).expect("trusted namespace");
            assert_eq!(pids(&h), vec![300], "holders inside are still found");
            // Invisible self stays unverified even with the opt-out.
            let err = scan_holders_in(&fake.root, &wt, 999).unwrap_err();
            assert!(err.reason.contains("pid 999"), "{err}");
            assert!(!err.pid_namespace, "{err}");
        }
        {
            let _env = crate::test_env::EnvVarGuard::set(TRUST_PID_NAMESPACE_ENV, "0");
            assert!(!trust_pid_namespace());
        }
    }

    // trace:BUG-1947 | ai:claude
    #[test]
    fn unreadable_proc_or_invisible_self_is_unverified_but_one_bad_entry_is_not() {
        let (_t, fake, wt, _outside) = fixture();
        let missing = fake.root.with_file_name("no-proc");
        let err = scan_holders_in(&missing, &wt, 200).unwrap_err();
        assert!(err.reason.contains("not readable"), "{err}");
        // The opt-out does nothing here, so it is not suggested.
        assert!(!err.pid_namespace, "{err}");
        assert!(!err.to_string().contains(TRUST_PID_NAMESPACE_ENV), "{err}");

        fake.add(1, 0, "init", Path::new("/"));
        fake.add(300, 1, "cargo build", &wt);
        // Self (200) is not in this proc tree: a pid namespace.
        let err = scan_holders_in(&fake.root, &wt, 200).unwrap_err();
        assert!(err.reason.contains("pid 200"), "{err}");
        assert!(!err.to_string().contains(TRUST_PID_NAMESPACE_ENV), "{err}");

        // Self visible, plus an entry with nothing readable: still verified.
        fake.add(200, 1, "aida", Path::new("/"));
        std::fs::create_dir_all(fake.root.join("400")).unwrap();
        assert_eq!(
            pids(&scan_holders_in(&fake.root, &wt, 200).unwrap()),
            vec![300]
        );
    }

    // trace:BUG-1947 | ai:claude
    #[test]
    fn unverified_result_names_the_reason_and_the_override() {
        let e = WorktreeInUse {
            worktree: PathBuf::from("/w"),
            holders: Vec::new(),
            unverified: Some("/proc is not readable".into()),
            pid_namespace: false,
        };
        assert!(e.summary().contains("not verified"), "{}", e.summary());
        let r = e.refusal("--force");
        assert!(
            r.contains("/proc is not readable") && r.contains("--force"),
            "{r}"
        );
        assert!(!r.contains(TRUST_PID_NAMESPACE_ENV), "{r}");
        let ns = WorktreeInUse {
            unverified: Some("this process runs in a separate pid namespace".into()),
            pid_namespace: true,
            ..e
        };
        let r = ns.refusal("--force");
        assert_eq!(r.matches("AIDA_TRUST_PID_NAMESPACE=1").count(), 1, "{r}");
    }

    /// Kills the child on drop so a failed assertion never leaks a `sleep`.
    struct KillOnDrop(std::process::Child);
    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    // trace:BUG-1947 | ai:claude
    #[cfg(target_os = "linux")]
    #[test]
    fn real_proc_scan_finds_a_live_sleep_inside_the_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        assert!(check_worktree_removal(&wt).is_ok(), "no holder yet");
        let child = KillOnDrop(
            std::process::Command::new("sleep")
                .arg("600")
                .current_dir(&wt)
                .spawn()
                .unwrap(),
        );
        let pid = child.0.id();
        let err = check_worktree_removal(&wt).expect_err("live sleep must block");
        assert!(err
            .holders
            .iter()
            .any(|h| h.pid == pid && h.hold == HoldKind::Cwd));
        let msg = err.refusal("--force");
        assert!(msg.contains(&format!("pid {pid}")), "{msg}");
        assert!(msg.contains("sleep 600"), "{msg}");
        assert!(msg.contains("--force"), "{msg}");
        drop(child);
        assert!(
            check_worktree_removal(&wt).is_ok(),
            "holder gone after kill"
        );
    }
}
