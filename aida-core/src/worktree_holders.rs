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
//! 2. the whole process subtree of the *highest* ancestor in the unbroken run,
//!    starting at this process, whose cwd is inside the worktree. When an agent
//!    was started in the worktree and ends its own session, its own helpers
//!    (MCP servers, language servers, background builds) are part of the
//!    session being ended. The run must be unbroken: an orchestrator whose cwd
//!    is the main checkout does not pass this exemption to the implementers it
//!    spawned into other worktrees. That cross-worktree case is exactly what
//!    this guard exists to refuse.
//!
//! The session's own *recorded* owner pid gets no special treatment. If that
//! process is dead it is not in `/proc` and cannot block. If it is alive and
//! outside the caller's own chain, it is a live implementer, and the guard
//! refuses, as intended.
//!
//! ## Other platforms
//!
//! Without `/proc` the scan cannot run. [`check_worktree_removal`] then allows
//! the removal and prints a one-line warning that the check was skipped.
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
}

impl fmt::Display for WorktreeInUse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
        let pids: Vec<String> = self
            .holders
            .iter()
            .map(|h| format!("pid {} ({})", h.pid, h.command))
            .collect();
        format!("in use by live process(es): {}", pids.join(", "))
    }

    /// Full refusal text: the holder list plus how to override it with `flag`.
    pub fn refusal(&self, flag: &str) -> String {
        format!(
            "{self}\n\nRemoving it now would break the work in progress. Stop those \
             processes first, or pass {flag} to remove the worktree anyway."
        )
    }
}

/// Fail with [`WorktreeInUse`] when a live process, other than the caller's
/// own process chain, is using `worktree`. An empty or missing path never
/// blocks: nothing is on disk to protect.
// trace:BUG-1947 | ai:claude
pub fn check_worktree_removal(worktree: &Path) -> Result<(), WorktreeInUse> {
    #[cfg(not(target_os = "linux"))]
    {
        if !worktree.as_os_str().is_empty() && worktree.exists() {
            eprintln!(
                "warning: cannot check for live processes in {} on this platform; removing without that check",
                worktree.display()
            );
        }
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        let holders = live_worktree_holders(worktree);
        if holders.is_empty() {
            Ok(())
        } else {
            Err(WorktreeInUse {
                worktree: worktree.to_path_buf(),
                holders,
            })
        }
    }
}

/// Live processes using `worktree`, excluding the caller's own chain (see the
/// module docs). Always empty off Linux, and for an empty or missing path.
// trace:BUG-1947 | ai:claude
pub fn live_worktree_holders(worktree: &Path) -> Vec<WorktreeHolder> {
    #[cfg(target_os = "linux")]
    {
        scan_holders_in(Path::new("/proc"), worktree, std::process::id())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = worktree;
        Vec::new()
    }
}

/// The scan behind [`live_worktree_holders`], parameterised over the proc root
/// and the caller's pid so tests can drive it with a fake `/proc` tree.
// trace:BUG-1947 | ai:claude
pub fn scan_holders_in(proc_root: &Path, worktree: &Path, self_pid: u32) -> Vec<WorktreeHolder> {
    // An empty base would match every path under `starts_with`.
    if worktree.as_os_str().is_empty() {
        return Vec::new();
    }
    let Ok(canon) = worktree.canonicalize() else {
        return Vec::new(); // Missing worktree: nothing to protect.
    };
    let inside = |p: &Path| p.starts_with(&canon);

    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return Vec::new();
    };
    // Sorted so refusal messages list pids deterministically.
    let pids: BTreeSet<u32> = entries
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .collect();

    // The ppid map covers every visible process, so subtree membership can be
    // decided without a second pass over /proc.
    let mut ppid: BTreeMap<u32, u32> = BTreeMap::new();
    for &pid in &pids {
        if let Some(pp) = read_ppid(proc_root, pid) {
            ppid.insert(pid, pp);
        }
    }
    // Self's ppid may be missing from the map if /proc/self is hidden.
    if !ppid.contains_key(&self_pid) {
        if let Some(pp) = read_ppid(proc_root, self_pid) {
            ppid.insert(self_pid, pp);
        }
    }

    // Rule 1: self and ancestors.
    let chain = ancestry(&ppid, self_pid);
    let excluded: BTreeSet<u32> = chain.iter().copied().collect();

    // Rule 2: the highest ancestor in the unbroken in-worktree run from self.
    let mut subtree_root: Option<u32> = None;
    for &pid in &chain {
        match read_link(proc_root, pid, "cwd") {
            Some(cwd) if inside(&cwd) => subtree_root = Some(pid),
            _ => break,
        }
    }

    let mut holders = Vec::new();
    for &pid in &pids {
        if excluded.contains(&pid) {
            continue;
        }
        if let Some(root) = subtree_root {
            if ancestry(&ppid, pid).contains(&root) {
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
    holders
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

/// Parent pid from `/proc/<pid>/stat`. The comm field can contain spaces and
/// parentheses, so parse after the LAST `)`.
fn read_ppid(proc_root: &Path, pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(proc_root.join(pid.to_string()).join("stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 1..];
    // rest = " <state> <ppid> ..."
    rest.split_whitespace().nth(1)?.parse().ok()
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
        fn add(&self, pid: u32, ppid: u32, cmd: &str, cwd: &Path) -> PathBuf {
            let dir = self.root.join(pid.to_string());
            std::fs::create_dir_all(dir.join("fd")).unwrap();
            // comm containing ") " exercises the last-paren ppid parse.
            std::fs::write(dir.join("stat"), format!("{pid} (we) ird) S {ppid} 1 1 0")).unwrap();
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
        let h = scan_holders_in(&fake.root, &wt, 200);
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
        let h = scan_holders_in(&fake.root, &wt, 200);
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
        let h = scan_holders_in(&fake.root, &wt, 200);
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
        let h = scan_holders_in(&fake.root, &wt, 200);
        assert_eq!(pids(&h), vec![301, 302]);
        assert!(matches!(h[0].hold, HoldKind::OpenFile(_)));
        assert_eq!(h[1].hold, HoldKind::Executable);
    }

    // trace:BUG-1947 | ai:claude
    #[test]
    fn empty_or_missing_worktree_never_blocks() {
        let (_t, fake, wt, _outside) = fixture();
        fake.add(300, 1, "anything", Path::new("/"));
        assert!(scan_holders_in(&fake.root, Path::new(""), 1).is_empty());
        assert!(scan_holders_in(&fake.root, &wt.join("gone"), 1).is_empty());
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
