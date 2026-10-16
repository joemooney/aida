//! BUG-1918: who may approve a change.
//!
//! The review gate used to decide authorship from a free-text label. This
//! module binds both sides to identity instead:
//!
//! * a **recorder** (the process running `aida review record`) is described by
//!   identity tokens that persist across shell calls — its session ids, its
//!   process ancestry (with kernel start identities), and every lease it is
//!   bound to by working directory or `AIDA_SESSION_ID`;
//! * a spec's **authors** are its authoring leases — live ones, plus the
//!   durable authorship ledger written when a lease is taken, so releasing the
//!   lease does not erase who claimed the work;
//! * a recorded approval's **authority** (its seat grant, or the human-review
//!   receipt written when the recorder passed the human-at-TTY floor) is
//!   re-read from its durable store at ship time instead of trusting the
//!   booleans stored beside the verdict.
//!
//! Token format: `kind:value`. Process tokens are `kind:<pid>@<start>`, where
//! `<start>` is the kernel start identity (empty when unknown). Kinds:
//! `session` (a session id from the environment or a lease's manifest),
//! `lease` (a lease id), `shell` (the shell that ran the command — only the
//! immediate parent), and `proc` (any ancestor process; a lease's hosted agent
//! process is recorded under this kind), and `sid` (the POSIX session leader,
//! used only when no agent harness is recognised above the caller).
//!
//! # Threat model
//!
//! This gate stops an agent that self-approves **through AIDA's normal
//! commands**: claiming or implementing a spec and then running `aida review
//! record --verdict approved` (or shipping/merging) from the same session,
//! under any label. It does **not** stop a same-user adversary that forges
//! files or re-parents processes — hand-writing verdict JSON, receipts, or
//! ledger lines, back-dating `recorded_at`, editing the seat-grant store, or
//! detaching from its own ancestry with `setsid`/`script`/`tmux`/`nohup` so a
//! recorder no longer descends from the claiming harness. Everything here is
//! readable and writable by the same OS user; closing that gap needs signed
//! receipts and a separate trust domain (tracked as BUG-1934 and the
//! signed-receipts follow-up).
//!
//! Also out of scope: merging **around AIDA** — a raw `gh pr merge`, the
//! forge's web UI, or `git push` to the base branch. AIDA cannot gate a merge
//! it does not perform; that needs forge-side branch protection (a required
//! review from a distinct forge account).
//!
//! # Identity lifetime
//!
//! Process identity is tied to the agent **harness process instance**. A
//! harness restart or a resumed conversation in a new process is a new
//! identity: only the `session:` tokens (an agent session id carried across
//! the resume, or the lease id exported as `AIDA_SESSION_ID`) still bind it.
//! A recorder must therefore carry at least one *anchor* — a `session:`,
//! `proc:`/`shell:`, or `sid:` token — or it is refused: a lease token alone
//! says where a command ran, not who ran it.
// trace:BUG-1918 | ai:claude

use std::path::{Path, PathBuf};

use crate::SessionLease;

/// File (under the main worktree's `.aida/`) holding the authorship ledger.
const LEDGER_FILE: &str = "review-authorship.jsonl";
/// Directory (under the user's AIDA home) holding human-review receipts.
const RECEIPT_DIR: &str = "review-receipts";

// ── Identity tokens ─────────────────────────────────────────────────────────

/// `kind:<pid>@<start>`.
pub(crate) fn process_token(kind: &str, pid: u32, start: Option<&str>) -> String {
    format!("{kind}:{pid}@{}", start.unwrap_or_default())
}

/// Token kinds that name a live process. A `shell` (the process that ran a
/// command) and a `proc` (any ancestor) with the same pid and start identity
/// are the same process: the shell that took a lease, still alive and above
/// a later recorder, binds that recorder to the lease.
fn process_kind_class(kind: &str) -> &str {
    match kind {
        "shell" | "proc" => "proc",
        other => other,
    }
}

/// Do two identity tokens name the same identity? Plain tokens compare
/// exactly. Process tokens compare kind (`shell` and `proc` are one class)
/// and pid, and the start identities when both sides recorded one — so a
/// recycled pid of a long-dead process is not mistaken for it, while a legacy
/// record without a start identity keeps the historical pid-only match.
pub(crate) fn tokens_match(a: &str, b: &str) -> bool {
    let (Some((ka, va)), Some((kb, vb))) = (a.split_once(':'), b.split_once(':')) else {
        return false;
    };
    if process_kind_class(ka) != process_kind_class(kb) || va.is_empty() || vb.is_empty() {
        return false;
    }
    match (va.split_once('@'), vb.split_once('@')) {
        (Some((pa, sa)), Some((pb, sb))) => {
            !pa.is_empty() && pa == pb && (sa.is_empty() || sb.is_empty() || sa == sb)
        }
        (None, None) => va == vb,
        _ => false,
    }
}

/// BUG-1918: does `identity` carry an anchor — a session id, a process, or a
/// POSIX session — rather than nothing, or only lease/location tokens?
/// Approvals by an unanchored identity are refused at record and ship time.
pub(crate) fn has_identity_anchor(identity: &[String]) -> bool {
    identity.iter().any(|t| {
        t.split_once(':').is_some_and(|(kind, value)| {
            matches!(kind, "session" | "proc" | "shell" | "sid") && !value.trim().is_empty()
        })
    })
}

/// Does any token in `a` name the same identity as any token in `b`?
pub(crate) fn identities_overlap(a: &[String], b: &[String]) -> bool {
    a.iter().any(|x| b.iter().any(|y| tokens_match(x, y)))
}

fn push_unique(out: &mut Vec<String>, token: String) {
    if !out.contains(&token) {
        out.push(token);
    }
}

/// Agent harnesses whose process outlives an agent's separate shell calls:
/// the identity that binds a claim to a later review, whichever vendor runs
/// the agent.
/// `agy` is the Antigravity CLI's executable.
const AGENT_HARNESSES: &[&str] = &["claude", "codex", "gemini", "antigravity", "agy"];

/// Interpreters that run an agent harness as a script (`node …/gemini`).
const SCRIPT_RUNTIMES: &[&str] = &["node", "bun", "deno", "python", "python3"];

fn bare_command(arg: &str) -> String {
    let base = Path::new(arg)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(arg)
        .to_ascii_lowercase();
    [".exe", ".js", ".mjs", ".cjs"]
        .iter()
        .find_map(|ext| base.strip_suffix(ext).map(str::to_string))
        .unwrap_or(base)
}

fn is_harness_command(arg: &str) -> bool {
    let bare = bare_command(arg);
    bare == "claude code"
        || AGENT_HARNESSES
            .iter()
            .any(|h| bare == *h || bare.starts_with(&format!("{h}-")))
}

/// Is a process with this name and command line an agent harness (Claude
/// Code, Codex, Gemini CLI, Antigravity)? Matches the process name, the
/// executable, or — under a script runtime — the script; never an argument,
/// so an orchestrator's `--vendor claude` flag does not make it a harness.
pub(crate) fn is_agent_harness(name: &str, cmd: &[String]) -> bool {
    if is_harness_command(name) {
        return true;
    }
    let Some(first) = cmd.first() else {
        return false;
    };
    if is_harness_command(first) {
        return true;
    }
    SCRIPT_RUNTIMES.contains(&bare_command(first).as_str())
        && cmd
            .get(1)
            .filter(|script| !script.starts_with('-'))
            .is_some_and(|script| is_harness_command(script))
}

/// One process in an ancestry chain.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProcEntry {
    pub(crate) pid: u32,
    pub(crate) name: String,
    pub(crate) cmd: Vec<String>,
    /// Kernel start identity (empty/`None` when unknown).
    pub(crate) start: Option<String>,
}

impl ProcEntry {
    fn token(&self, kind: &str) -> String {
        process_token(kind, self.pid, self.start.as_deref())
    }
}

/// BUG-1918: everything identity is derived from for the calling process —
/// its agent session ids, its ancestry, and its POSIX session leader.
/// Production reads it live ([`IdentitySource::live`]); tests inject one
/// ([`with_identity_source`]) so the harness/ancestry paths are exercised
/// rather than compiled out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct IdentitySource {
    /// Agent session ids from the environment (`AIDA_SESSION_ID`,
    /// `CLAUDE_CODE_SESSION_ID`).
    pub(crate) session_ids: Vec<String>,
    /// The calling process's ancestry, nearest first, starting with the
    /// calling process itself.
    pub(crate) chain: Vec<ProcEntry>,
    /// The leader of the calling process's POSIX session.
    pub(crate) session_leader: Option<ProcEntry>,
}

#[cfg(test)]
thread_local! {
    static TEST_SOURCE: std::cell::RefCell<Option<IdentitySource>> =
        const { std::cell::RefCell::new(None) };
}

/// Run `f` with `source` as this thread's live identity source (tests only).
#[cfg(test)]
pub(crate) fn with_identity_source<R>(source: IdentitySource, f: impl FnOnce() -> R) -> R {
    struct Reset(Option<IdentitySource>);
    impl Drop for Reset {
        fn drop(&mut self) {
            let prev = self.0.take();
            TEST_SOURCE.with(|s| *s.borrow_mut() = prev);
        }
    }
    let prev = TEST_SOURCE.with(|s| s.borrow_mut().replace(source));
    let _reset = Reset(prev);
    f()
}

fn env_session_ids(vars: &[&str]) -> Vec<String> {
    vars.iter()
        .filter_map(|k| std::env::var(k).ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

impl IdentitySource {
    /// The calling process's identity source. Under `cfg(test)` only
    /// `AIDA_SESSION_ID` is read from the environment — the test runner's
    /// own Claude session and ancestry are ambient, not part of any fixture —
    /// plus whatever the test injected with [`with_identity_source`].
    pub(crate) fn live() -> Self {
        #[cfg(test)]
        {
            let mut src = TEST_SOURCE.with(|s| s.borrow().clone()).unwrap_or_default();
            for id in env_session_ids(&["AIDA_SESSION_ID"]) {
                push_unique(&mut src.session_ids, id);
            }
            src
        }
        #[cfg(not(test))]
        {
            Self {
                session_ids: env_session_ids(&["AIDA_SESSION_ID", "CLAUDE_CODE_SESSION_ID"]),
                chain: live_ancestry(),
                session_leader: live_session_leader(),
            }
        }
    }

    /// The nearest agent harness above the calling process.
    pub(crate) fn agent_root(&self) -> Option<&ProcEntry> {
        self.chain
            .iter()
            .skip(1)
            .find(|p| is_agent_harness(&p.name, &p.cmd))
    }

    fn session_tokens(&self) -> Vec<String> {
        self.session_ids
            .iter()
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
            .map(|v| format!("session:{v}"))
            .collect()
    }

    /// What a RECORDER is: its session ids, the shell that ran it, and every
    /// ancestor up to (and including) its nearest agent harness. Above the
    /// harness is a different session — an agent launched from an advisor's
    /// tree must not inherit the advisor's identity. With no recognised
    /// harness the walk runs to the top, and the POSIX session leader is
    /// added: an unrecognised agent's shell calls share it.
    pub(crate) fn recorder_tokens(&self) -> Vec<String> {
        let mut out = self.session_tokens();
        let root = self.agent_root().map(|p| p.pid);
        for (i, p) in self.chain.iter().enumerate().skip(1) {
            if p.pid <= 1 {
                continue;
            }
            if i == 1 {
                push_unique(&mut out, p.token("shell"));
            }
            push_unique(&mut out, p.token("proc"));
            if Some(p.pid) == root {
                break;
            }
        }
        if root.is_none() {
            if let Some(leader) = self.session_leader.as_ref().filter(|l| l.pid > 1) {
                push_unique(&mut out, leader.token("sid"));
            }
        }
        out
    }

    /// What a LAUNCH path leaves for the worker it claims for: only the
    /// claiming process itself, which an `exec` turns into the worker. The
    /// launcher's session ids, shell and harness are deliberately left out.
    pub(crate) fn launched_worker_tokens(&self) -> Vec<String> {
        self.chain
            .first()
            .filter(|p| p.pid > 1)
            .map(|me| vec![me.token("proc")])
            .unwrap_or_default()
    }

    /// What a CLAIMER leaves in the authorship ledger beyond its lease: its
    /// session ids, the claiming process itself, the shell that ran the
    /// claim, and its agent root — the nearest agent harness, vendor-neutral
    /// — or, with no recognised harness, its POSIX session leader. Both
    /// outlive the shell that took the lease, so the claimer stays
    /// identifiable after `cd`, a new shell, or lease release. Launch paths
    /// use [`Self::launched_worker_tokens`] instead.
    pub(crate) fn claimer_tokens(&self) -> Vec<String> {
        let mut out = self.session_tokens();
        if let Some(me) = self.chain.first().filter(|p| p.pid > 1) {
            push_unique(&mut out, me.token("proc"));
        }
        if let Some(shell) = self.chain.get(1).filter(|p| p.pid > 1) {
            push_unique(&mut out, shell.token("shell"));
        }
        match self.agent_root() {
            Some(root) => push_unique(&mut out, root.token("proc")),
            None => {
                if let Some(leader) = self.session_leader.as_ref().filter(|l| l.pid > 1) {
                    push_unique(&mut out, leader.token("sid"));
                }
            }
        }
        out
    }
}

/// The live ancestry of this process, nearest first, including itself.
/// Walks one pid at a time — a single-pid refresh per ancestor, never a
/// process-table scan (the BUG-613 hotspot pattern) — so the cost is the
/// depth of the tree, not its size.
#[cfg(not(test))]
fn live_ancestry() -> Vec<ProcEntry> {
    use sysinfo::{ProcessRefreshKind, System, UpdateKind};
    let mut out = Vec::new();
    let mut cur = sysinfo::Pid::from_u32(std::process::id());
    let mut seen = std::collections::HashSet::new();
    let mut sys = System::new();
    while seen.insert(cur) {
        sys.refresh_pids_specifics(
            &[cur],
            ProcessRefreshKind::new().with_cmd(UpdateKind::Always),
        );
        let Some(proc) = sys.process(cur) else { break };
        out.push(ProcEntry {
            pid: cur.as_u32(),
            name: proc.name().to_string(),
            cmd: proc.cmd().to_vec(),
            start: crate::process_probe::process_start_identity(cur.as_u32()),
        });
        let Some(parent) = proc.parent() else { break };
        if parent == sysinfo::Pid::from_u32(1) || parent == cur {
            break;
        }
        cur = parent;
    }
    out
}

/// The leader of this process's POSIX session (`getsid(0)`).
#[cfg(not(test))]
fn live_session_leader() -> Option<ProcEntry> {
    #[cfg(unix)]
    {
        // SAFETY: getsid(0) only reads the calling process's session id.
        let sid = unsafe { libc::getsid(0) };
        let pid = u32::try_from(sid).ok().filter(|p| *p > 1)?;
        Some(ProcEntry {
            pid,
            start: crate::process_probe::process_start_identity(pid),
            ..ProcEntry::default()
        })
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// The identity tokens a lease binds: its id, the shell that created it, its
/// hosted agent process, and the agent conversation its manifest recorded.
pub(crate) fn lease_tokens(lease: &SessionLease, manifest_session: Option<&str>) -> Vec<String> {
    let mut out = vec![format!("lease:{}", lease.id)];
    if let Some(pid) = lease.creator_pid {
        out.push(process_token(
            "shell",
            pid,
            lease.creator_pid_start_time.as_deref(),
        ));
    }
    if let Some(pid) = lease.active_pid {
        out.push(process_token(
            "proc",
            pid,
            lease.active_pid_start_time.as_deref(),
        ));
    }
    if let Some(id) = manifest_session.map(str::trim).filter(|s| !s.is_empty()) {
        out.push(format!("session:{id}"));
    }
    out
}

fn manifest_session_id(project_root: &Path, lease: &SessionLease) -> Option<String> {
    let root = lease
        .parent_project_root
        .clone()
        .unwrap_or_else(|| project_root.to_path_buf());
    crate::session_manifest::load(&crate::session_manifest::manifest_path(&root, &lease.id))
        .ok()
        .and_then(|m| m.claude_session_id)
}

/// Is `lease` bound to a process acting from `dirs` with `AIDA_SESSION_ID`
/// `session_id` — working inside its worktree, or carrying its id?
fn lease_bound_by_context(
    lease: &SessionLease,
    dirs: &[PathBuf],
    session_id: Option<&str>,
) -> bool {
    let in_worktree = dirs.iter().any(|d| crate::lease_covers_cwd(lease, d));
    let carries_id = session_id
        .map(str::trim)
        .filter(|s| s.len() >= 8)
        .is_some_and(|s| lease.id.starts_with(s) || s.starts_with(&lease.id));
    in_worktree || carries_id
}

/// BUG-1918: the identity of the process recording a review — what it is
/// (session ids, ancestry) plus the leases its context binds it to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RecorderIdentity {
    pub(crate) tokens: Vec<String>,
}

impl RecorderIdentity {
    /// Resolve the current process's identity against `leases`.
    pub(crate) fn current(project_root: &Path, leases: &[SessionLease]) -> Self {
        let dirs: Vec<PathBuf> = [
            std::env::current_dir().ok(),
            Some(project_root.to_path_buf()),
        ]
        .into_iter()
        .flatten()
        .map(|p| p.canonicalize().unwrap_or(p))
        .collect();
        let session_id = std::env::var("AIDA_SESSION_ID").ok();
        Self::with_context(
            IdentitySource::live().recorder_tokens(),
            leases,
            &dirs,
            session_id.as_deref(),
        )
    }

    /// Pure core of [`Self::current`]: `base` tokens plus a `lease:` token
    /// for every lease the context binds.
    pub(crate) fn with_context(
        base: Vec<String>,
        leases: &[SessionLease],
        dirs: &[PathBuf],
        session_id: Option<&str>,
    ) -> Self {
        let mut tokens = base;
        for lease in leases {
            if lease_bound_by_context(lease, dirs, session_id) {
                push_unique(&mut tokens, format!("lease:{}", lease.id));
            }
        }
        Self { tokens }
    }
}

// ── Authorship ──────────────────────────────────────────────────────────────

/// Is `lease` an authoring (implementer/claim) lease? Review leases and
/// PR-scoped leases belong to reviewers and never are.
pub(crate) fn is_authoring_lease(lease: &SessionLease) -> bool {
    let scope = lease.scope.trim();
    !lease.review_verb && !scope.is_empty() && !scope.to_ascii_uppercase().starts_with("PR-")
}

/// One durable authorship record: who took an authoring lease on `scope`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct AuthorshipRecord {
    pub(crate) scope: String,
    pub(crate) lease_id: String,
    #[serde(default)]
    pub(crate) branch: String,
    #[serde(default)]
    pub(crate) tokens: Vec<String>,
    pub(crate) recorded_at: String,
}

fn ledger_path(project_root: &Path) -> PathBuf {
    crate::main_worktree_root_from(project_root)
        .join(".aida")
        .join(LEDGER_FILE)
}

/// Append a durable authorship record for `lease` when it is an authoring
/// lease. The record carries the lease's own tokens plus the CLAIMING
/// process's identity ([`IdentitySource::claimer_tokens`]: session ids, the
/// claiming process and shell, and its vendor-neutral agent root or POSIX
/// session leader), so the claimer stays identifiable after the lease (and
/// the shell that took it) are gone. A failed write never blocks taking the
/// lease, but it is surfaced — a warning on stderr and an
/// `authorship-ledger` gate event — because the claimer is then
/// identifiable only while the lease lives.
pub(crate) fn record_authoring_lease(project_root: &Path, lease: &SessionLease) {
    if is_launching() {
        // A launch path claims FOR a worker: record the worker, never the
        // launcher (its session ids, shell and harness), so the advisor or
        // orchestrator that launched it can still review its PR.
        append_authorship_as(
            project_root,
            lease,
            IdentitySource::live().launched_worker_tokens(),
            false,
        );
    } else {
        append_authorship(project_root, lease, IdentitySource::live().claimer_tokens());
    }
}

thread_local! {
    static LAUNCHING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// BUG-1918: run `f` — the claim of a TRUE hand-off (`aida agent new`,
/// `aida queue work` and `aida session start --launch`, each of which starts
/// the worker itself) — so the authorship ledger records the WORKER the lease
/// is for, not the launcher. Never wrap a claim the caller works itself
/// (`aida worktree add` / `enter`, `do-guided`): that would drop the author's
/// own identity from the ledger.
///
/// Every launch path must also bind the worker it starts: export
/// [`WORKER_SESSION_ENV`] = lease id into the worker's environment
/// ([`export_worker_session_env`] before an `exec`, [`bind_worker_env`] on a
/// spawned `Command`), and record a spawned (not `exec`'d) worker's process
/// with [`record_launched_worker`]. An `exec`'d worker keeps the claiming
/// process's pid and start identity, which the claim already recorded.
// trace:BUG-1918 | ai:claude
pub(crate) fn as_launcher<R>(f: impl FnOnce() -> R) -> R {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            LAUNCHING.with(|l| l.set(self.0));
        }
    }
    let _reset = Reset(LAUNCHING.with(|l| l.replace(true)));
    f()
}

fn is_launching() -> bool {
    LAUNCHING.with(std::cell::Cell::get)
}

/// The environment variable a launched worker carries its lease id in. Every
/// authorship record holds `session:<lease-id>`, so a worker that keeps this
/// variable is its lease's author wherever it later runs from.
pub(crate) const WORKER_SESSION_ENV: &str = "AIDA_SESSION_ID";

/// BUG-1918: bind a worker about to be `exec`'d from this process to its
/// lease (see [`as_launcher`]).
// trace:BUG-1918 | ai:claude
pub(crate) fn export_worker_session_env(lease_id: &str) {
    std::env::set_var(WORKER_SESSION_ENV, lease_id);
}

/// BUG-1918: bind a worker spawned by `command` to its lease (see
/// [`as_launcher`]).
// trace:BUG-1918 | ai:claude
pub(crate) fn bind_worker_env(command: &mut std::process::Command, lease_id: &str) {
    command.env(WORKER_SESSION_ENV, lease_id);
}

/// BUG-1918: record a spawned worker's process (`pid` plus its kernel start
/// identity) as an author of lease `lease_id`, so it stays bound after it
/// leaves the worktree or the lease is released, even if it drops
/// [`WORKER_SESSION_ENV`].
// trace:BUG-1918 | ai:claude
pub(crate) fn record_launched_worker(project_root: &Path, lease_id: &str, pid: u32) {
    let start = crate::process_probe::process_start_identity(pid);
    record_lease_identity(
        project_root,
        lease_id,
        &[process_token("proc", pid, start.as_deref())],
    );
}

/// BUG-1918: append a further authorship record for lease `lease_id` once a
/// launcher learns who actually WORKS it — the worker's process
/// (`active_pid`) or agent session id stamped on the lease/manifest after
/// the claim. Without it the ledger names only the launcher, and a released
/// lease's worker could later approve its own work.
pub(crate) fn record_lease_identity(project_root: &Path, lease_id: &str, extra: &[String]) {
    let main = crate::main_worktree_root_from(project_root);
    let Some(lease) = crate::list_leases(project_root)
        .into_iter()
        .chain(crate::list_leases(&main))
        .find(|l| l.id == lease_id)
    else {
        return;
    };
    // A stamp names the worker, never the lease's creating shell: on a
    // launch path that shell is the launcher's. A claim from the worker's
    // own shell already recorded it at claim time.
    append_authorship_as(project_root, &lease, extra.to_vec(), false);
}

fn append_authorship(project_root: &Path, lease: &SessionLease, extra: Vec<String>) {
    append_authorship_as(project_root, lease, extra, true);
}

/// `with_creator_shell`: whether the lease's creating shell is part of the
/// author's identity — not on a launch path, where it is the launcher's.
fn append_authorship_as(
    project_root: &Path,
    lease: &SessionLease,
    extra: Vec<String>,
    with_creator_shell: bool,
) {
    if !is_authoring_lease(lease) {
        return;
    }
    let mut tokens = lease_tokens(lease, manifest_session_id(project_root, lease).as_deref());
    if !with_creator_shell {
        tokens.retain(|t| !t.starts_with("shell:"));
    }
    // A worker launched for the lease carries its id as `AIDA_SESSION_ID`.
    push_unique(&mut tokens, format!("session:{}", lease.id));
    for t in extra {
        push_unique(&mut tokens, t);
    }
    let record = AuthorshipRecord {
        scope: lease.scope.trim().to_string(),
        lease_id: lease.id.clone(),
        branch: lease.branch.clone(),
        tokens,
        recorded_at: chrono::Utc::now().to_rfc3339(),
    };
    let path = ledger_path(project_root);
    if let Err(e) = append_record(&path, &record) {
        let message = format!(
            "could not record who claimed {} in {} ({e}); review independence for this work \
             is checked only while its lease is live",
            record.scope,
            path.display()
        );
        eprintln!("  warning: {message}");
        crate::events::record_gate_held(
            project_root,
            crate::events::GATE_AUTHORSHIP_LEDGER,
            Some(record.scope.clone()),
            None,
            &message,
        );
    }
}

fn append_record(path: &Path, record: &AuthorshipRecord) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let line = serde_json::to_string(record).map_err(std::io::Error::other)?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{line}")
}

/// Every parseable record in the authorship ledger.
pub(crate) fn load_ledger(project_root: &Path) -> Vec<AuthorshipRecord> {
    load_ledger_checked(project_root).0
}

/// The authorship ledger's parseable records, plus a warning when it is
/// missing, unreadable, or has malformed lines — authors it would have named
/// are then identified only while their leases live.
pub(crate) fn load_ledger_checked(project_root: &Path) -> (Vec<AuthorshipRecord>, Option<String>) {
    let path = ledger_path(project_root);
    let body = match std::fs::read_to_string(&path) {
        Ok(body) => body,
        Err(e) => {
            return (
                Vec::new(),
                Some(format!(
                    "the authorship ledger {} could not be read ({e}); authors are identified \
                     only from live leases",
                    path.display()
                )),
            )
        }
    };
    let mut bad = 0usize;
    let records = body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            let parsed = serde_json::from_str(l).ok();
            bad += usize::from(parsed.is_none());
            parsed
        })
        .collect();
    let warning = (bad > 0).then(|| {
        format!(
            "{bad} malformed line(s) in the authorship ledger {} were skipped; the authors \
             they recorded are identified only while their leases live",
            path.display()
        )
    });
    (records, warning)
}

/// Print [`load_ledger_checked`]'s warning, if any, to stderr.
pub(crate) fn warn_on_ledger_read(project_root: &Path) {
    if let (_, Some(warning)) = load_ledger_checked(project_root) {
        eprintln!("  warning: {warning}");
    }
}

/// Is `lease` an authoring lease scoped to one of `spec_ids`?
pub(crate) fn is_authoring_lease_for(lease: &SessionLease, spec_ids: &[String]) -> bool {
    is_authoring_lease(lease) && scope_in(&lease.scope, spec_ids)
}

fn scope_in(scope: &str, spec_ids: &[String]) -> bool {
    let scope = scope.trim();
    !scope.is_empty()
        && spec_ids
            .iter()
            .any(|id| scope.eq_ignore_ascii_case(id.trim()))
}

fn branch_is(branch: &str, head_branch: Option<&str>) -> bool {
    head_branch
        .map(str::trim)
        .is_some_and(|b| !b.is_empty() && branch.trim() == b)
}

/// The author sessions of the work under review: every authoring lease (live,
/// or durably recorded in the ledger) scoped to one of `spec_ids` or working
/// on `head_branch`.
pub(crate) fn author_sessions(
    leases: &[SessionLease],
    ledger: &[AuthorshipRecord],
    spec_ids: &[String],
    head_branch: Option<&str>,
    manifest_session: impl Fn(&SessionLease) -> Option<String>,
) -> Vec<crate::pr_ship::AuthorSession> {
    let mut out: Vec<crate::pr_ship::AuthorSession> = Vec::new();
    for lease in leases {
        if is_authoring_lease_for(lease, spec_ids)
            || (is_authoring_lease(lease) && branch_is(&lease.branch, head_branch))
        {
            let mut tokens = lease_tokens(lease, manifest_session(lease).as_deref());
            // The claim record says whether the creating shell is the
            // author's (a claim) or a launcher's (`agent new`, `session
            // start --launch`). With no record, it counts: fail closed.
            // trace:BUG-1918 | ai:claude
            if ledger.iter().any(|r| r.lease_id == lease.id) {
                tokens.retain(|t| !t.starts_with("shell:"));
            }
            out.push(crate::pr_ship::AuthorSession {
                label: format!("lease {} on {}", lease.id, lease.scope),
                tokens,
            });
        }
    }
    for rec in ledger {
        if scope_in(&rec.scope, spec_ids) || branch_is(&rec.branch, head_branch) {
            out.push(crate::pr_ship::AuthorSession {
                label: format!("lease {} on {} (claim record)", rec.lease_id, rec.scope),
                tokens: rec.tokens.clone(),
            });
        }
    }
    out
}

/// [`author_sessions`] read from `project_root`'s lease store (and its main
/// worktree's) and authorship ledger.
pub(crate) fn author_sessions_at(
    project_root: &Path,
    spec_ids: &[String],
    head_branch: Option<&str>,
) -> Vec<crate::pr_ship::AuthorSession> {
    author_sessions(
        &visible_leases(project_root),
        &load_ledger(project_root),
        spec_ids,
        head_branch,
        |l| manifest_session_id(project_root, l),
    )
}

/// BUG-1918: the fail-closed author set, used when the work under review
/// maps to no spec or branch — every live authoring lease and every
/// authorship-ledger record, whatever it claimed.
pub(crate) fn all_author_sessions(project_root: &Path) -> Vec<crate::pr_ship::AuthorSession> {
    let leases: Vec<SessionLease> = visible_leases(project_root)
        .into_iter()
        .filter(is_authoring_lease)
        .collect();
    let ledger = load_ledger(project_root);
    let mut ids: Vec<String> = leases
        .iter()
        .map(|l| l.scope.trim().to_string())
        .chain(ledger.iter().map(|r| r.scope.trim().to_string()))
        .collect();
    ids.sort();
    ids.dedup();
    author_sessions(&leases, &ledger, &ids, None, |l| {
        manifest_session_id(project_root, l)
    })
}

/// Every lease visible from `project_root` and its main worktree,
/// deduplicated by id.
pub(crate) fn visible_leases(project_root: &Path) -> Vec<SessionLease> {
    let main = crate::main_worktree_root_from(project_root);
    let mut seen = std::collections::HashSet::new();
    crate::list_leases(&main)
        .into_iter()
        .chain(crate::list_leases(project_root))
        .filter(|l| seen.insert(l.id.clone()))
        .collect()
}

// ── Human-review receipts ──────────────────────────────────────────────────

/// BUG-1918: a durable record, in the user's AIDA home, that a human passed
/// the human-at-TTY floor when recording an approval of `sha`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct HumanReviewReceipt {
    pub(crate) id: String,
    pub(crate) sha: String,
    pub(crate) subject: String,
    pub(crate) recorded_at: String,
    /// The recorder's identity tokens when the human approved. The ship gate
    /// checks THIS copy against the author sessions, not the verdict's.
    #[serde(default)]
    pub(crate) identity: Vec<String>,
}

fn receipt_dir() -> Option<PathBuf> {
    crate::aida_home_dir().map(|h| h.join(".aida").join(RECEIPT_DIR))
}

fn receipt_path(dir: &Path, id: &str) -> Option<PathBuf> {
    uuid::Uuid::parse_str(id).ok()?;
    Some(dir.join(format!("{id}.json")))
}

/// Write a receipt for a human-at-TTY approval of `sha` on `subject` by a
/// recorder with `identity`; returns its id.
pub(crate) fn write_human_receipt(
    sha: &str,
    subject: &str,
    identity: &[String],
) -> anyhow::Result<String> {
    let dir = receipt_dir().ok_or_else(|| anyhow::anyhow!("cannot locate the AIDA home"))?;
    write_human_receipt_in(&dir, sha, subject, identity)
}

pub(crate) fn write_human_receipt_in(
    dir: &Path,
    sha: &str,
    subject: &str,
    identity: &[String],
) -> anyhow::Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let receipt = HumanReviewReceipt {
        id: id.clone(),
        sha: sha.trim().to_string(),
        subject: subject.trim().to_string(),
        recorded_at: chrono::Utc::now().to_rfc3339(),
        identity: identity.to_vec(),
    };
    std::fs::create_dir_all(dir)?;
    let path = receipt_path(dir, &id).ok_or_else(|| anyhow::anyhow!("invalid receipt id"))?;
    aida_core::write_atomic(&path, serde_json::to_string_pretty(&receipt)?)?;
    Ok(id)
}

/// Receipt `id`, when it exists and vouches for exactly `sha`.
pub(crate) fn human_receipt_for(id: &str, sha: &str) -> Option<HumanReviewReceipt> {
    receipt_dir().and_then(|dir| human_receipt_in(&dir, id, sha))
}

pub(crate) fn human_receipt_in(dir: &Path, id: &str, sha: &str) -> Option<HumanReviewReceipt> {
    receipt_path(dir, id)
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<HumanReviewReceipt>(&b).ok())
        .filter(|r| {
            r.id == id
                && !sha.trim().is_empty()
                && crate::review_verdict::same_reviewed_sha(&r.sha, sha)
        })
}

/// Does receipt `id` exist and vouch for exactly `sha`?
pub(crate) fn human_receipt_valid_in(dir: &Path, id: &str, sha: &str) -> bool {
    human_receipt_in(dir, id, sha).is_some()
}

// ── Ship-time re-validation ────────────────────────────────────────────────

/// BUG-1918: the authority keys (see
/// [`crate::review_verdict::RecorderAttestation::authority_key`]) of every
/// attested approval among `candidates` whose authority re-validates against
/// its durable store — the seat grant was valid for that (non-implementer)
/// seat at the verdict's `recorded_at`, or the human-review receipt exists
/// for the attested commit — each mapped to the recorder identity the ship
/// gate checks against the author sessions: the receipt's own copy for a
/// human approval, the attestation's for a seat grant.
pub(crate) fn verified_authority_keys(
    project_root: &Path,
    candidates: &[crate::review_verdict::RecordedVerdict],
) -> std::collections::BTreeMap<String, Vec<String>> {
    let mut out = std::collections::BTreeMap::new();
    for v in candidates {
        let Some(att) = v.attestation.as_ref() else {
            continue;
        };
        let Some(key) = att.authority_key(v.recorded_at.as_deref()) else {
            continue;
        };
        let identity = match (att.grant_id.as_deref(), att.seat.as_deref()) {
            (Some(grant), Some(seat)) => v
                .recorded_at
                .as_deref()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t.trim()).ok())
                .is_some_and(|at| {
                    crate::seat_authority::grant_authorized_at(
                        project_root,
                        grant,
                        seat,
                        at.with_timezone(&chrono::Utc),
                    )
                })
                .then(|| att.identity.clone()),
            _ => att
                .receipt_id
                .as_deref()
                .and_then(|id| human_receipt_for(id, &att.sha))
                .map(|r| r.identity),
        };
        if let Some(identity) = identity {
            out.insert(key, identity);
        }
    }
    out
}

#[cfg(test)]
#[path = "tests/bug_1918_review_authority_tests.rs"]
mod bug_1918_review_authority_tests;
