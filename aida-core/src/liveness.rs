//! Shared work-liveness machinery: the `/proc` process probe plus the lease
//! and per-spec liveness classifiers.
//!
//! This module was lifted out of `aida-cli` (BUG-677) so BOTH surfaces that
//! need "is a live process working this?" can share ONE implementation:
//!
//!   - `aida-cli` — `aida ps`, `aida status <spec>`, `aida session leases`,
//!     the orchestrator's crash-corroboration checks, etc. re-export the probe
//!     from here (the module path `crate::process_probe` still resolves via a
//!     re-export, so the ~80 existing call sites are untouched).
//!   - `aida-tui` — the cockpit's per-row liveness glyph. Before BUG-677 the
//!     TUI shelled out to `aida ps --json` (a ~1.3s subprocess) to borrow the
//!     already-computed verdict, because it must NOT depend on `aida-cli`. Now
//!     it calls [`spec_liveness_map`] in-process — same probe, same
//!     classifiers, no subprocess.
//!
//! The verdict a caller gets here is byte-for-byte the verdict the old
//! aida-cli path produced: the classifiers ([`classify_lease_state`],
//! [`classify_spec_liveness`]) and the `/proc` probe are the SAME code, and
//! [`spec_liveness_map`] reads the SAME `.aida/sessions/*.toml` lease files
//! `aida ps` reads, so the CLI table and the TUI glyph can never disagree.
//!
//! trace:BUG-677 | ai:claude

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sysinfo::{ProcessRefreshKind, RefreshKind, System};

// ============================================================================
// /proc process probe (moved verbatim from aida-cli/src/process_probe.rs).
// STORY-69 foundation: enumerate live `claude` (Claude Code) processes, map
// each to its working directory, and best-effort surface the active session
// jsonl. trace:STORY-69 | ai:claude
// ============================================================================

/// Live `claude` process candidate paired with its (best-effort) project cwd
/// and recently-touched session jsonl.
#[derive(Debug, Clone)]
pub struct LiveSession {
    pub pid: u32,
    /// Cwd of the process. If `stale_cwd` is true, this is the path with the
    /// `(deleted)` suffix stripped — i.e., the path the process WAS in before
    /// the inode was unlinked.
    pub cwd: PathBuf,
    /// Best-effort: the most-recently-touched session jsonl under
    /// `~/.claude/projects/<encoded-cwd>/`, if mtime within
    /// [`RECENT_JSONL_WINDOW`].
    pub jsonl: Option<PathBuf>,
    /// True if /proc reported the cwd as `<path> (deleted)` — i.e., the
    /// directory inode is unlinked but the process still holds it open.
    /// This is the signature of BUG-61 (session end removed the worktree
    /// but didn't terminate claude).
    pub stale_cwd: bool,
}

/// Window for "this jsonl was just written" — short enough that a quiescent
/// session won't be classified as live, long enough to absorb a normal
/// inter-tool-call gap.
pub const RECENT_JSONL_WINDOW: Duration = Duration::from_secs(60);

/// Enumerate live `claude` Claude Code processes on this host.
///
/// Returns an empty vec on platforms where sysinfo can't read process info
/// (Windows pre-Vista, restricted procfs). Never panics — degrades to mtime-
/// only callers without crashing.
///
/// BUG-613: the full `sysinfo` process refresh is expensive — on Linux it
/// enumerates every process AND every thread under `/proc/<pid>/task/...`
/// (tens of thousands of `openat`/`read` on a busy host). A single `aida
/// status` calls this from several sections (agent classify, worktree rows,
/// the cross-substrate Claude Code view), so the raw walk used to fire 3+
/// times per invocation. Process liveness does not meaningfully change within
/// one short-lived CLI run, so we memoize the first result for the lifetime of
/// the process and hand every later caller a cheap clone. The uncached walk is
/// still available as [`probe_live_claude_sessions_uncached`] for the rare
/// caller that must observe fresh state.
// trace:BUG-613 | ai:claude
pub fn probe_live_claude_sessions() -> Vec<LiveSession> {
    use std::sync::OnceLock;
    static CACHE: OnceLock<Vec<LiveSession>> = OnceLock::new();
    CACHE
        .get_or_init(probe_live_claude_sessions_uncached)
        .clone()
}

/// BUG-613: the uncached single-walk probe. Prefer [`probe_live_claude_sessions`]
/// (process-lifetime memoized) on the read-mostly status/listing paths; reach
/// for this only when fresh liveness is required mid-process.
// trace:BUG-613
pub fn probe_live_claude_sessions_uncached() -> Vec<LiveSession> {
    // One refresh, not two: `new_with_specifics` already performs the initial
    // process walk for the given `RefreshKind`, so the previous extra
    // `refresh_processes_specifics` call doubled the `/proc` scan for no gain.
    // trace:BUG-613 | ai:claude
    let sys = System::new_with_specifics(
        RefreshKind::new().with_processes(
            ProcessRefreshKind::new()
                .with_cwd(sysinfo::UpdateKind::Always)
                .with_cmd(sysinfo::UpdateKind::Always),
        ),
    );

    let mut out = Vec::new();
    for proc in sys.processes().values() {
        // sysinfo on Linux enumerates per-thread entries from /proc/<tgid>/task/*.
        // We only want process leaders — `thread_kind()` returns Some(_) for
        // worker threads, None for the actual process. Without this filter a
        // single multi-threaded `claude` shows up 16+ times.
        if proc.thread_kind().is_some() {
            continue;
        }
        if !is_claude_process(proc.name(), proc.cmd()) {
            continue;
        }
        let raw_cwd = match proc.cwd() {
            Some(p) => p.to_path_buf(),
            None => continue,
        };
        let (cwd, stale_cwd) = strip_deleted_suffix(&raw_cwd);
        let jsonl = if stale_cwd {
            None
        } else {
            recent_jsonl_in_project(&cwd)
        };
        out.push(LiveSession {
            pid: proc.pid().as_u32(),
            cwd,
            jsonl,
            stale_cwd,
        });
    }
    out
}

/// Heuristic: does this look like a Claude Code process? `claude` matches by
/// name on Linux (the binary is literally named `claude`); on macOS the name
/// can be truncated. Falling back to scanning the command line for a
/// `claude` token catches edge cases without false-positiving on `clauded`
/// or `claude-something`.
fn is_claude_process(name: &str, cmd: &[String]) -> bool {
    if name == "claude" || name == "Claude" || name == "Claude Code" {
        return true;
    }
    cmd.iter().any(|arg| {
        let bare = Path::new(arg)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(arg);
        bare == "claude" || bare == "Claude" || bare == "Claude Code"
    })
}

/// Linux's procfs reports the cwd of a process whose directory has been
/// unlinked as `<original-path> (deleted)`. sysinfo preserves this in the
/// PathBuf verbatim. Detect it, strip the suffix, and signal staleness so
/// the caller can warn (BUG-61) or skip.
fn strip_deleted_suffix(p: &Path) -> (PathBuf, bool) {
    let s = p.to_string_lossy();
    if let Some(real) = s.strip_suffix(" (deleted)") {
        (PathBuf::from(real), true)
    } else {
        (p.to_path_buf(), false)
    }
}

/// Find the most-recently-modified `*.jsonl` under
/// `~/.claude/projects/<encoded-cwd>/`, if its mtime is within
/// [`RECENT_JSONL_WINDOW`] of now. Returns `None` when no project dir
/// exists, no jsonl is present, or the newest one is older than the window.
///
/// Encoding follows Claude Code's own convention: replace each `/` in the
/// absolute cwd with `-`, including the leading slash. So
/// `/home/joe/ai/aida-epic-20` → `-home-joe-ai-aida-epic-20`. The decoding
/// is ambiguous (a literal `-` in the path is indistinguishable from `/`),
/// but we only need to encode forwards, not decode.
pub fn recent_jsonl_in_project(cwd: &Path) -> Option<PathBuf> {
    let proj_dir = claude_projects_dir_for_cwd(cwd)?;
    let now = SystemTime::now();
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    let entries = std::fs::read_dir(&proj_dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let mtime = entry.metadata().and_then(|m| m.modified()).ok();
        if let Some(t) = mtime {
            if newest.as_ref().map(|(prev, _)| t > *prev).unwrap_or(true) {
                newest = Some((t, p));
            }
        }
    }
    let (mtime, path) = newest?;
    let age = now.duration_since(mtime).unwrap_or(Duration::ZERO);
    if age <= RECENT_JSONL_WINDOW {
        Some(path)
    } else {
        None
    }
}

/// Path to `~/.claude/projects/<encoded-cwd>/` for a given absolute cwd, or
/// `None` if HOME isn't set or the directory doesn't exist.
pub fn claude_projects_dir_for_cwd(cwd: &Path) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let encoded = encode_cwd_for_projects(cwd);
    let candidate = home.join(".claude").join("projects").join(encoded);
    if candidate.is_dir() {
        Some(candidate)
    } else {
        None
    }
}

/// Encode a cwd to Claude Code's project-dir naming convention: replace each
/// `/` (including leading) with `-`.
pub fn encode_cwd_for_projects(cwd: &Path) -> String {
    let s = cwd.to_string_lossy().to_string();
    // Claude Code's project directory slug replaces path separators with
    // hyphens. Tests and cross-platform paths can contain either separator
    // spelling, so normalize both rather than keying off MAIN_SEPARATOR.
    // trace:BUG-346 | ai:codex
    s.replace(['\\', '/'], "-")
}

/// Walk the chain of parent PIDs starting from `start` (typically the PID of
/// the calling shell — get with `std::process::id()`'s parent if you can, or
/// just `start = std::process::id()` and accept that the immediate parent is
/// usually `aida` itself which is fine: ancestors of aida are ancestors of
/// the shell that invoked it).
///
/// The chain INCLUDES the start pid. Stops at PID 1 (init) or when a PPid
/// can't be read. Used by STORY-73 to match a calling shell against any
/// session lease's `creator_pid`.
pub fn walk_ancestor_pids(start: u32) -> Vec<u32> {
    let mut sys =
        System::new_with_specifics(RefreshKind::new().with_processes(ProcessRefreshKind::new()));
    sys.refresh_processes_specifics(ProcessRefreshKind::new());
    let mut chain = Vec::new();
    let mut cur = sysinfo::Pid::from_u32(start);
    let mut seen = std::collections::HashSet::new();
    while seen.insert(cur) {
        chain.push(cur.as_u32());
        let Some(proc) = sys.process(cur) else { break };
        let Some(parent) = proc.parent() else { break };
        if parent == sysinfo::Pid::from_u32(1) || parent == cur {
            chain.push(parent.as_u32());
            break;
        }
        cur = parent;
    }
    chain
}

/// Is process `pid` currently alive? A thin wrapper over `sysinfo`'s process
/// table — `true` iff the kernel still has an entry for `pid`.
///
/// Used by BUG-233's orchestrator-run corroboration: a child trusts its
/// `AIDA_AUTO_COMPLETE_TOKEN` only when the marker file it names records a PID
/// that is still running. Returns `false` on platforms where the process table
/// can't be read — corroboration fails safe (treat as not-orchestrated).
// trace:BUG-233 | ai:claude
pub fn pid_is_alive(pid: u32) -> bool {
    // pid 0 is never a probeable user process, and it means different dangerous
    // things per platform: on Unix `kill(0, …)` addresses the caller's WHOLE
    // process group; on Windows pid 0 is the System Idle Process, which sysinfo
    // reports as existing. Reject it up front so liveness reads `false` on every
    // platform (the Windows fallback below has no other way to exclude it).
    // trace:BUG-613 | ai:claude
    if pid == 0 {
        return false;
    }
    pid_is_alive_impl(pid)
}

/// BUG-613: liveness must be O(1), not a full process-table walk. The old
/// implementation built a fresh `sysinfo::System` and refreshed EVERY process
/// (and, on Linux, every thread via `/proc/<pid>/task/<tid>/...`) just to test
/// whether a single pid exists. Callers like `agent_registry::list_agent_views`
/// invoke this once per registered agent/session record, so on a long-lived
/// machine with many accumulated session manifests `aida status` fanned out to
/// hundreds of full `/proc` scans (~182 on the reporter's host → ~13s of
/// system time, dominated by `/proc/*/task/*` recursion). A single-pid probe
/// removes the whole hotspot.
///
/// Unix: `kill(pid, 0)` sends no signal but performs the existence + permission
/// check. `0` → alive and signalable; `EPERM` → the pid exists but is owned by
/// another user (still alive); `ESRCH` → no such process. Any other errno is
/// treated as not-alive (fail-safe, matching the prior platform-degrade
/// contract). `pid == 0` is rejected by the `pid_is_alive` wrapper before it
/// reaches here, so `kill` never addresses the caller's whole process group.
// trace:BUG-613 | ai:claude
#[cfg(unix)]
fn pid_is_alive_impl(pid: u32) -> bool {
    // SAFETY: `kill` with signal 0 only probes; it never delivers a signal.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return true;
    }
    // errno == EPERM means the process exists but we may not signal it.
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Non-Unix fallback: refresh ONLY the target pid rather than the whole table.
/// `refresh_pids_specifics` (sysinfo 0.30) scopes the scan to the single pid,
/// so this stays O(1) on Windows too.
// trace:BUG-613 | ai:claude
#[cfg(not(unix))]
fn pid_is_alive_impl(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessRefreshKind, System};
    let target = Pid::from_u32(pid);
    let mut sys = System::new();
    sys.refresh_pids_specifics(&[target], ProcessRefreshKind::new());
    sys.process(target).is_some()
}

// ============================================================================
// Lease-state classifier (moved from aida-cli, TASK-55).
// ============================================================================

/// TASK-55: lease liveness classification used by `session leases`, `aida ps`,
/// and the TUI's per-spec liveness glyph.
// trace:TASK-55 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseState {
    /// Live claude found inside the worktree — actively working.
    Live,
    /// Worktree exists, no live claude inside, <24h old. Could be a
    /// shell-only session or a paused claude — not yet abandoned.
    Dormant,
    /// Worktree missing OR (no live claude AND >24h old) — the lease
    /// is almost certainly leaked.
    Stale,
}

impl LeaseState {
    /// Human label. Pure (no glyph rendering — that stays in aida-cli, which
    /// owns the glyph palette).
    // trace:TASK-55 | ai:claude
    pub fn label(self) -> &'static str {
        match self {
            LeaseState::Live => "live",
            LeaseState::Dormant => "dormant",
            LeaseState::Stale => "stale",
        }
    }
}

/// TASK-55: classify a lease using its worktree existence, live-claude
/// probe result, and age. Pure given pre-collected inputs so the
/// decision matrix is unit-testable.
// trace:TASK-55 | ai:claude
pub fn classify_lease_state(
    worktree_exists: bool,
    has_live_claude: bool,
    age_hours: i64,
) -> LeaseState {
    if !worktree_exists {
        return LeaseState::Stale;
    }
    if has_live_claude {
        return LeaseState::Live;
    }
    if age_hours >= 24 {
        return LeaseState::Stale;
    }
    LeaseState::Dormant
}

// ============================================================================
// Per-spec liveness classifier (moved from aida-cli, STORY-694).
// ============================================================================

/// STORY-694: is a spec's In-Progress flag actually backed by a live process?
// trace:STORY-694 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecLiveness {
    /// A spec-scoped lease exists and its holder process is alive (a live
    /// claude is in the worktree). The In-Progress flag is liveness-backed.
    Live,
    /// A spec-scoped lease exists but no live process backs it (pid dead, or
    /// the worktree has gone idle past the threshold with no spec movement).
    /// The In-Progress flag is orphaned.
    // trace:BUG-623
    Stale,
    /// No spec-scoped lease is linked to this spec. When the spec is
    /// In-Progress this is DRIFT — the status flag is not liveness-backed.
    /// This is also the CORRECT honest signal for advisor Agent-tool
    /// fan-outs, which take generic `harness-worktree`-scoped leases that are
    /// NOT spec-linked (making the spec-to-session link is a documented
    /// follow-up, not this change).
    // trace:STORY-694
    FlagOnly,
    /// The spec is not In-Progress — no live session is expected.
    NoSession,
}

/// Pure verdict — given the spec-scoped lease's classified [`LeaseState`] (when
/// one was found) and whether the spec is In-Progress, decide the
/// [`SpecLiveness`]. Kept side-effect-free so the matrix (live lease → Live;
/// dead/stale lease → Stale; in-progress + no lease → FlagOnly; not-in-progress
/// → NoSession) is unit-testable without a store, a lease dir, or a real
/// process. A `Dormant` lease (worktree present, no live claude, <24h) counts
/// as Stale here: the operator asked specifically "is a LIVE process working
/// it?", and dormant means no live process.
// trace:STORY-694 | ai:claude
pub fn classify_spec_liveness(lease_state: Option<LeaseState>, in_progress: bool) -> SpecLiveness {
    match lease_state {
        Some(LeaseState::Live) => SpecLiveness::Live,
        Some(LeaseState::Dormant) | Some(LeaseState::Stale) => SpecLiveness::Stale,
        None if in_progress => SpecLiveness::FlagOnly,
        None => SpecLiveness::NoSession,
    }
}

// ============================================================================
// In-process spec-liveness map (BUG-677): the TUI's replacement for shelling
// out to `aida ps --json`.
// ============================================================================

/// A minimal read of a session lease TOML (`.aida/sessions/*.toml`), carrying
/// ONLY the fields the liveness matrix needs. Deserialized from the SAME files
/// `aida-cli`'s full `SessionLease` reads; serde ignores the other ~20 fields,
/// so the `LeaseState` this yields is identical to the one `aida ps` computes.
// trace:BUG-677 | ai:claude
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SessionLeaseLite {
    /// Raw scope string the user passed to `--owns` (matched against spec ids).
    pub scope: String,
    /// Worktree path (canonicalized at write time). Empty for advisory leases.
    #[serde(default)]
    pub worktree_path: PathBuf,
    /// ISO-8601 UTC session-start time.
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// PID of the process that minted the lease (the liveness signal for the
    /// worktree-less review/claim leases).
    #[serde(default)]
    pub creator_pid: Option<u32>,
    /// BUG-511: a review lease (`aida review`) is a worktree-less advisory lock.
    #[serde(default)]
    pub review_verb: bool,
    /// TASK-957: a claim lease (`aida claim`) is a worktree-less advisory lock.
    #[serde(default)]
    pub claim_verb: bool,
}

/// One work-item spec projected for the liveness pass — the display id, the two
/// id forms a lease scope can match against, whether it is In-Progress (an
/// orphan-pass candidate), and whether it is a rollup/stateless type (epic /
/// folder / meta) that never holds a spec-scoped lease and so is excluded from
/// the orphan pass. The caller (e.g. the TUI, from its already-loaded store)
/// builds this list.
// trace:BUG-677 | ai:claude
#[derive(Debug, Clone)]
pub struct SpecLivenessInput {
    /// agreed_id || spec_id || uuid — the id the row displays and the map keys.
    pub disp: String,
    pub agreed_id: Option<String>,
    pub spec_id: Option<String>,
    /// The spec is currently In Progress (orphan-pass candidate).
    pub in_progress: bool,
    /// Rollup / stateless type (epic / folder / meta) — excluded from the
    /// orphan pass because it never holds a spec-scoped lease.
    pub orphan_excluded: bool,
}

/// The liveness dir for a project root — `.aida/sessions/`, the same dir
/// `aida-cli::leases_dir` writes to.
// trace:BUG-677 | ai:claude
pub fn leases_dir(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("sessions")
}

/// Read the session leases at `.aida/sessions/*.toml` into the liveness-only
/// [`SessionLeaseLite`] view. Mirrors `aida-cli::list_leases`: same dir, same
/// `.toml` filter, same tolerant parse (a malformed lease is skipped, never
/// fatal).
// trace:BUG-677 | ai:claude
pub fn read_session_leases(project_root: &Path) -> Vec<SessionLeaseLite> {
    let dir = leases_dir(project_root);
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&p) {
            if let Ok(lease) = toml::from_str::<SessionLeaseLite>(&content) {
                out.push(lease);
            }
        }
    }
    out.sort_by(|a, b| a.started_at.cmp(&b.started_at));
    out
}

/// The lease-state of one lease, given the ONE already-computed live-session
/// slice and `now`. Mirrors `aida-cli::lease_state_for` exactly:
///   - a review/claim lease (worktree-less advisory lock) is Live iff its
///     `creator_pid` is alive, else Stale (never Dormant);
///   - otherwise the worktree/live-claude/age matrix via [`classify_lease_state`].
// trace:BUG-677 trace:BUG-511 trace:TASK-957 | ai:claude
pub fn lease_state_for(
    l: &SessionLeaseLite,
    live_sessions: &[LiveSession],
    now: chrono::DateTime<chrono::Utc>,
) -> LeaseState {
    if l.review_verb || l.claim_verb {
        let alive = l.creator_pid.map(pid_is_alive).unwrap_or(false);
        return if alive {
            LeaseState::Live
        } else {
            LeaseState::Stale
        };
    }
    let worktree_exists = l.worktree_path.exists();
    let has_live_claude = live_sessions
        .iter()
        .any(|s| !s.stale_cwd && (s.cwd == l.worktree_path || s.cwd.starts_with(&l.worktree_path)));
    let age_hours = now.signed_duration_since(l.started_at).num_hours();
    classify_lease_state(worktree_exists, has_live_claude, age_hours)
}

/// Does `lease`'s scope resolve to `spec`? True iff the raw scope string equals
/// (case-insensitively) the spec's agreed id OR its raw spec id — the same
/// match `aida-cli::spec_scoped_lease` / the `aida ps` row pass use. A generic
/// `harness-worktree` fan-out lease matches no spec, so its liveness is not
/// attributed to any row.
// trace:BUG-677 | ai:claude
fn scope_matches_spec(scope: &str, spec: &SpecLivenessInput) -> bool {
    [spec.agreed_id.as_deref(), spec.spec_id.as_deref()]
        .into_iter()
        .flatten()
        .any(|id| scope.eq_ignore_ascii_case(id))
}

/// Pure core of [`spec_liveness_map`]: given the spec projections, the session
/// leases, the ONE live-session slice, and `now`, build the per-spec verdict
/// map keyed by UPPER-CASED display id.
///
/// This reproduces `aida ps`'s picture collapsed to the per-spec liveness the
/// TUI renders (Live / Stale). Two passes, mirroring `build_running_work`:
///
///   1. **Row pass.** Every lease whose scope resolves to a known spec marks
///      that spec Live (if the lease is [`LeaseState::Live`]) else Stale. Live
///      is the strongest verdict and never downgrades.
///   2. **Orphan pass.** Every In-Progress work-item spec (rollup/stateless
///      types excluded) with no LIVE spec-scoped lease is Stale — the
///      flag-only / crashed-session case. `aida ps`'s "likely fan-out" framing
///      is informational only and does not change the Live/Stale verdict, so
///      it is intentionally not modelled here.
///
/// A spec absent from the map is Idle (the caller's lookup default).
// trace:BUG-677 | ai:claude
pub fn spec_verdicts(
    specs: &[SpecLivenessInput],
    leases: &[SessionLeaseLite],
    live: &[LiveSession],
    now: chrono::DateTime<chrono::Utc>,
) -> HashMap<String, SpecLiveness> {
    let mut map: HashMap<String, SpecLiveness> = HashMap::new();

    // Live wins: never let a later Stale overwrite an already-recorded Live.
    let upsert = |map: &mut HashMap<String, SpecLiveness>, disp: &str, state: SpecLiveness| {
        let key = disp.trim().to_ascii_uppercase();
        if key.is_empty() {
            return;
        }
        match map.get(&key) {
            Some(SpecLiveness::Live) => {}
            _ => {
                map.insert(key, state);
            }
        }
    };

    // Row pass: one entry per lease that resolves to a known spec.
    for l in leases {
        let state = lease_state_for(l, live, now);
        if let Some(spec) = specs.iter().find(|s| scope_matches_spec(&l.scope, s)) {
            let verdict = if state == LeaseState::Live {
                SpecLiveness::Live
            } else {
                SpecLiveness::Stale
            };
            upsert(&mut map, &spec.disp, verdict);
        }
    }

    // Orphan pass: In-Progress work-item specs with no LIVE spec-scoped lease.
    for s in specs {
        if !s.in_progress || s.orphan_excluded {
            continue;
        }
        let lease_state = leases
            .iter()
            .filter(|l| scope_matches_spec(&l.scope, s))
            .map(|l| lease_state_for(l, live, now))
            // If several leases claim the spec, the strongest (Live) wins — the
            // spec is genuinely being worked. Order: Live < Dormant < Stale in
            // the enum, and Live is what we care about, so take the min.
            .min_by_key(|st| match st {
                LeaseState::Live => 0,
                LeaseState::Dormant => 1,
                LeaseState::Stale => 2,
            });
        match classify_spec_liveness(lease_state, true) {
            SpecLiveness::Live => upsert(&mut map, &s.disp, SpecLiveness::Live),
            // Stale lease OR flag-only (no lease) both surface as Stale on the
            // TUI glyph — parity with `parse_ps_json`, which maps every orphaned
            // entry to Stale regardless of the stale-vs-flag-only sub-reason.
            SpecLiveness::Stale | SpecLiveness::FlagOnly => {
                upsert(&mut map, &s.disp, SpecLiveness::Stale)
            }
            // in_progress == true, so NoSession is unreachable here.
            SpecLiveness::NoSession => {}
        }
    }

    map
}

/// The in-process replacement for `aida ps --json`: probe `/proc` ONCE, read
/// the session leases, and classify each `specs` entry into its per-spec
/// [`SpecLiveness`] verdict. Keyed by UPPER-CASED display id; a spec absent
/// from the map is Idle.
///
/// The caller supplies `specs` from its already-loaded requirement store (the
/// TUI does — it need not re-open a backend on the probe thread). Read-only and
/// non-blocking beyond the one `/proc` walk + lease-dir read.
// trace:BUG-677
pub fn spec_liveness_map(
    project_root: &Path,
    specs: &[SpecLivenessInput],
) -> HashMap<String, SpecLiveness> {
    let now = chrono::Utc::now();
    let live = probe_live_claude_sessions();
    let leases = read_session_leases(project_root);
    spec_verdicts(specs, &leases, &live, now)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- probe helpers (moved from aida-cli/src/process_probe.rs) ----------

    #[test]
    fn encode_cwd_replaces_slashes() {
        let p = Path::new("/home/joe/ai/aida-epic-20");
        assert_eq!(encode_cwd_for_projects(p), "-home-joe-ai-aida-epic-20");
    }

    #[test]
    fn encode_cwd_replaces_windows_backslashes() {
        let p = Path::new(r"C:\Users\joe\ai\aida-epic-20");
        assert_eq!(encode_cwd_for_projects(p), "C:-Users-joe-ai-aida-epic-20");
    }

    #[test]
    fn encode_cwd_handles_root() {
        let p = Path::new("/");
        assert_eq!(encode_cwd_for_projects(p), "-");
    }

    #[test]
    fn strip_deleted_recognises_proc_suffix() {
        let (real, stale) = strip_deleted_suffix(Path::new("/foo/bar (deleted)"));
        assert_eq!(real, Path::new("/foo/bar"));
        assert!(stale);
    }

    #[test]
    fn strip_deleted_passthrough_when_clean() {
        let (real, stale) = strip_deleted_suffix(Path::new("/foo/bar"));
        assert_eq!(real, Path::new("/foo/bar"));
        assert!(!stale);
    }

    #[test]
    fn is_claude_process_matches_name() {
        assert!(is_claude_process("claude", &[]));
        assert!(is_claude_process("Claude", &[]));
        assert!(!is_claude_process("clauded", &[]));
        assert!(!is_claude_process("claude-helper", &[]));
    }

    #[test]
    fn is_claude_process_falls_back_to_cmd() {
        assert!(is_claude_process(
            "node",
            &[
                "/usr/local/bin/claude".to_string(),
                "--something".to_string()
            ],
        ));
        assert!(!is_claude_process(
            "node",
            &["/usr/local/bin/clauded".to_string()]
        ));
    }

    #[test]
    fn walk_ancestor_pids_includes_self_and_terminates() {
        let me = std::process::id();
        let chain = walk_ancestor_pids(me);
        assert!(!chain.is_empty());
        assert_eq!(chain[0], me);
        assert!(chain.len() < 100);
    }

    #[test]
    fn pid_is_alive_true_for_self() {
        assert!(pid_is_alive(std::process::id()));
    }

    #[test]
    fn pid_is_alive_false_for_unused_pid() {
        assert!(!pid_is_alive(u32::MAX - 1));
    }

    // BUG-613: pid 0 must never read as alive. trace:BUG-613 | ai:claude
    #[test]
    fn pid_is_alive_false_for_pid_zero() {
        assert!(!pid_is_alive(0));
    }

    // BUG-613: a reaped child is dead. trace:BUG-613
    #[cfg(unix)]
    #[test]
    fn pid_is_alive_false_after_child_reaped() {
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn `true`");
        let pid = child.id();
        child.wait().expect("reap child");
        assert!(!pid_is_alive(pid));
    }

    // ---- lease-state classifier -------------------------------------------

    #[test]
    fn classify_lease_state_matrix() {
        // Live claude in the worktree → Live regardless of age.
        assert_eq!(classify_lease_state(true, true, 0), LeaseState::Live);
        assert_eq!(classify_lease_state(true, true, 100), LeaseState::Live);
        // No claude, young worktree → Dormant.
        assert_eq!(classify_lease_state(true, false, 1), LeaseState::Dormant);
        // No claude, old worktree → Stale.
        assert_eq!(classify_lease_state(true, false, 24), LeaseState::Stale);
        // Missing worktree → Stale regardless.
        assert_eq!(classify_lease_state(false, false, 0), LeaseState::Stale);
    }

    #[test]
    fn lease_state_label_is_pure() {
        assert_eq!(LeaseState::Live.label(), "live");
        assert_eq!(LeaseState::Dormant.label(), "dormant");
        assert_eq!(LeaseState::Stale.label(), "stale");
    }

    // ---- per-spec liveness classifier (acceptance criterion 4) -------------

    // BUG-677 acceptance: the aida-core classifier returns the SAME verdict for
    // a LIVE session, a STALE session, and a FLAG-ONLY session — the exact
    // matrix the prior aida-cli `classify_spec_liveness` produced.
    // trace:BUG-677 | ai:claude
    #[test]
    fn classify_spec_liveness_live_stale_flag_only() {
        // A live spec-scoped lease → Live.
        assert_eq!(
            classify_spec_liveness(Some(LeaseState::Live), true),
            SpecLiveness::Live
        );
        // A stale (or dormant) spec-scoped lease → Stale.
        assert_eq!(
            classify_spec_liveness(Some(LeaseState::Stale), true),
            SpecLiveness::Stale
        );
        assert_eq!(
            classify_spec_liveness(Some(LeaseState::Dormant), true),
            SpecLiveness::Stale
        );
        // In-Progress with NO lease → FlagOnly (orphaned status flag).
        assert_eq!(classify_spec_liveness(None, true), SpecLiveness::FlagOnly);
        // Not In-Progress with no lease → NoSession.
        assert_eq!(classify_spec_liveness(None, false), SpecLiveness::NoSession);
    }

    // ---- spec_verdicts (in-process parity with `aida ps` → parse_ps_json) --

    fn spec(disp: &str, in_progress: bool) -> SpecLivenessInput {
        SpecLivenessInput {
            disp: disp.to_string(),
            agreed_id: Some(disp.to_string()),
            spec_id: Some(disp.to_string()),
            in_progress,
            orphan_excluded: false,
        }
    }

    fn lease(scope: &str, worktree: &str) -> SessionLeaseLite {
        SessionLeaseLite {
            scope: scope.to_string(),
            worktree_path: PathBuf::from(worktree),
            started_at: chrono::Utc::now(),
            creator_pid: None,
            review_verb: false,
            claim_verb: false,
        }
    }

    #[test]
    fn spec_verdicts_marks_live_stale_and_flag_only() {
        // TASK-1 has a live lease (a live claude sits in its worktree).
        // TASK-2 has a spec-scoped lease but no live claude and a missing
        //        worktree → the lease is Stale.
        // TASK-3 is In-Progress with no lease at all → flag-only → Stale.
        // TASK-9 is not In-Progress and unleased → absent (Idle).
        let now = chrono::Utc::now();
        let live_wt = std::env::temp_dir().join("aida-bug677-live");
        std::fs::create_dir_all(&live_wt).unwrap();

        let specs = vec![
            spec("TASK-1", true),
            spec("TASK-2", true),
            spec("TASK-3", true),
            spec("TASK-9", false),
        ];
        let leases = vec![
            lease("TASK-1", live_wt.to_str().unwrap()),
            lease("TASK-2", "/nonexistent/task-2-worktree"),
        ];
        let live = vec![LiveSession {
            pid: 4242,
            cwd: live_wt.clone(),
            jsonl: None,
            stale_cwd: false,
        }];

        let map = spec_verdicts(&specs, &leases, &live, now);
        assert_eq!(map.get("TASK-1"), Some(&SpecLiveness::Live));
        assert_eq!(map.get("TASK-2"), Some(&SpecLiveness::Stale));
        assert_eq!(map.get("TASK-3"), Some(&SpecLiveness::Stale));
        assert!(map.get("TASK-9").is_none());

        let _ = std::fs::remove_dir_all(&live_wt);
    }

    #[test]
    fn spec_verdicts_live_wins_over_stale_and_folds_case() {
        // A lowercase-scoped live lease AND an In-Progress flag for the same
        // spec: Live must win, and the key must fold to the upper-cased disp.
        let now = chrono::Utc::now();
        let live_wt = std::env::temp_dir().join("aida-bug677-livewins");
        std::fs::create_dir_all(&live_wt).unwrap();

        let specs = vec![SpecLivenessInput {
            disp: "TASK-7".to_string(),
            agreed_id: Some("TASK-7".to_string()),
            spec_id: Some("TASK-7".to_string()),
            in_progress: true,
            orphan_excluded: false,
        }];
        let leases = vec![lease("task-7", live_wt.to_str().unwrap())];
        let live = vec![LiveSession {
            pid: 99,
            cwd: live_wt.clone(),
            jsonl: None,
            stale_cwd: false,
        }];

        let map = spec_verdicts(&specs, &leases, &live, now);
        assert_eq!(map.get("TASK-7"), Some(&SpecLiveness::Live));

        let _ = std::fs::remove_dir_all(&live_wt);
    }

    #[test]
    fn spec_verdicts_excludes_rollup_types_from_orphan_pass() {
        // An In-Progress epic (rollup type) with no lease must NOT read Stale —
        // it never holds a spec-scoped lease, so `aida ps` skips it. Parity:
        // absent from the map → Idle.
        let now = chrono::Utc::now();
        let specs = vec![SpecLivenessInput {
            disp: "EPIC-1".to_string(),
            agreed_id: Some("EPIC-1".to_string()),
            spec_id: Some("EPIC-1".to_string()),
            in_progress: true,
            orphan_excluded: true,
        }];
        let map = spec_verdicts(&specs, &[], &[], now);
        assert!(map.get("EPIC-1").is_none());
    }

    #[test]
    fn read_session_leases_missing_dir_is_empty() {
        let dir = std::env::temp_dir().join("aida-bug677-nonexistent-XYZ");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(read_session_leases(&dir).is_empty());
    }
}
