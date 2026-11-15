/// Render a registry glyph honoring the active profile. Default Unicode profile
/// reproduces the historical literals byte-for-byte. trace:TASK-840 | ai:claude
fn glyph(g: crate::glyphs::Glyph) -> &'static str {
    crate::glyphs::get(g, crate::find_project_root().ok().as_deref())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionMeta {
    pub agent: String,
    pub id: String,
    pub age_seconds: u64,
    pub role: Option<String>,
    pub spec: Option<String>,
    pub title: Option<String>,
    /// Timestamp of the first event in the .jsonl. Used for launch-log
    /// correlation in FR-1-044 — matches the session to a `aida session
    /// new` record so the user-chosen title and authoritative role can
    /// override the grep heuristic.
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Most-recent cwd recorded in the .jsonl. Each Claude Code message
    /// stores the cwd at message time; we sample the LAST one we see in
    /// our parse window so worktree switches mid-session show up.
    /// trace:STORY-59 | ai:claude
    pub last_cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<crate::agent_registry::TerminalIdentity>,
    /// Branch the worktree at `last_cwd` was on when we ran. Computed on
    /// demand at table-print time (one cheap `git branch --show-current`
    /// per unique cwd) — None if we can't resolve a branch (cwd missing,
    /// not a repo, etc.).
    /// trace:STORY-59 | ai:claude
    pub branch: Option<String>,
    /// Most-recent spec this session worked — the RECENT FOCUS column.
    /// Filled by `fill_recent_focus` from AIDA's per-session activity log
    /// (newest-first) with the manifest's planned items as fallback. None
    /// when AIDA never tracked the session (renders `-`). Distinct from
    /// `spec`, which stays the *first*-mentioned (launch) spec — together
    /// they show the session's spec evolution.
    /// trace:BUG-112 | ai:claude
    pub recent_focus: Option<String>,
    /// The transcript file this row was parsed from (`None` for rows built
    /// without a file, e.g. in tests). Read lazily by the STORY-993 process
    /// resolver for the last launch/resume hook event.
    // trace:STORY-993 | ai:claude
    pub path: Option<PathBuf>,
    /// STORY-993: the process (pid / tty / liveness) resolved for this row —
    /// lease pid, cwd+start-window proc-scan, or none. Never invented.
    // trace:STORY-993 | ai:claude
    pub process: crate::session_liveness::ProcessFacts,
}

/// STORY-59: liveness inferred from activity recency. Visual indicator
/// only — no PID tracking (Claude Code forks; child PIDs ≠ launch PID,
/// so PID-based liveness is unreliable across reads).
/// trace:STORY-59 | ai:claude
fn liveness_indicator(age_seconds: u64) -> &'static str {
    if age_seconds < 5 * 60 {
        "●" // live (active in last 5 minutes) — `●` is not a registry glyph.
    } else if age_seconds < 60 * 60 {
        // recent (last hour) — route the partial marker through the registry.
        glyph(crate::glyphs::Glyph::InFlight)
    } else {
        " " // idle
    }
}

const RECENT_WINDOW_SECS: u64 = 24 * 60 * 60;

pub fn list(limit: usize, no_color: bool, all: bool) -> Result<()> {
    if no_color {
        colored::control::set_override(false);
    }
    let cwd = std::env::current_dir().context("could not determine cwd")?;
    let mut here = collect_sessions_from_cwd(&cwd, limit)?;

    // STORY-58: when this cwd is inside an active session worktree, also
    // walk the parent project's Claude Code session storage so the user
    // gets a merged view (their parent shell's sessions + this worktree's
    // sessions) instead of half a story.
    // trace:STORY-58 | ai:claude
    let parent_root = crate::parent_project_root_for_session(&cwd);
    let mut parent: Vec<SessionMeta> = match parent_root.as_ref() {
        Some(root) if root != &cwd => collect_sessions_from_cwd(root, limit).unwrap_or_default(),
        _ => Vec::new(),
    };

    let total_here = here.len();
    let total_parent = parent.len();
    // STORY-59: by default, hide sessions with no activity in the last
    // 24h — long-tail abandoned sessions clutter the everyday view. The
    // `--all` flag bypasses; we still note how many were elided.
    if !all {
        here.retain(|s| s.age_seconds < RECENT_WINDOW_SECS);
        parent.retain(|s| s.age_seconds < RECENT_WINDOW_SECS);
    }
    let hidden = (total_here - here.len()) + (total_parent - parent.len());

    if crate::output_format_is_json() && here.is_empty() && parent.is_empty() {
        println!(
            "{}",
            serde_json::to_string_pretty(&Vec::<SessionMeta>::new())?
        );
        return Ok(());
    }

    if here.is_empty() && parent.is_empty() {
        if hidden > 0 {
            eprintln!(
                "{}",
                format!(
                    "(no sessions active in the last 24h; {} older session{} hidden — pass --all to see them)",
                    hidden,
                    if hidden == 1 { "" } else { "s" }
                )
                .dimmed()
            );
        } else {
            eprintln!("{}", "(no past sessions in this directory)".dimmed());
        }
        print_leases_hint();
        return Ok(());
    }
    fill_branches(&mut here);
    fill_branches(&mut parent);
    enrich_from_agent_registry(&mut here);
    enrich_from_agent_registry(&mut parent);
    normalize_specs(&mut here);
    normalize_specs(&mut parent);
    fill_recent_focus(&mut here);
    fill_recent_focus(&mut parent);
    let resolver = ProcessResolver::new();
    resolver.fill(&mut here);
    resolver.fill(&mut parent);

    // STORY-993: `--format json` (or AIDA_OUTPUT_FORMAT=json) — the machine
    // projection carrying pid / pids / tty / liveness / resolution /
    // lease_role, the same rows the table renders.
    if crate::output_format_is_json() {
        return print_json(&here, &parent, parent_root.as_deref());
    }

    if crate::output_format_is_json() {
        let mut rows = here;
        rows.extend(parent);
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    // STORY-58: when there's nothing to merge, render the classic single
    // table — keep the existing one-group output untouched. Only switch
    // to grouped headers once we actually have a parent group to show.
    if parent.is_empty() {
        print_table(&here);
    } else {
        // Compute widths over the union so both tables align column-for-column.
        let widths = TableWidths::compute(here.iter().chain(parent.iter()));
        let here_label = group_label(&cwd);
        let parent_label = parent_root
            .as_ref()
            .map(|p| group_label(p))
            .unwrap_or_else(|| "parent".to_string());
        if !here.is_empty() {
            print_group_header(&format!("This worktree ({})", here_label));
            print_table_with_widths(&here, &widths);
            println!();
        }
        print_group_header(&format!("Parent project ({})", parent_label));
        print_table_with_widths(&parent, &widths);
    }
    if hidden > 0 {
        eprintln!(
            "{}",
            format!(
                "({} older session{} hidden — pass --all to see them)",
                hidden,
                if hidden == 1 { "" } else { "s" }
            )
            .dimmed()
        );
    }
    // BUG-112: RECENT FOCUS replaced the old INITIAL TOPIC column. It
    // tracks the latest spec each session worked (AIDA's per-session
    // activity log, newest-first) so it stays current as work moves,
    // instead of drifting like the conversation-start title did. A `-`
    // means AIDA isn't tracking a spec for that session.
    // trace:BUG-112 | ai:claude
    eprintln!(
        "{}",
        "(RECENT FOCUS is the latest spec a session worked — it updates as work moves; \
         `-` = no spec tracked)"
            .dimmed()
    );
    // STORY-993: the three-state liveness legend + what PID `?` means.
    eprintln!("{}", process_legend().dimmed());
    print_leases_hint();
    Ok(())
}

/// STORY-993: the legend for the liveness glyph, PID/TTY cells and the AGE
/// column's dual meaning.
// trace:STORY-993 | ai:claude
fn process_legend() -> String {
    format!(
        "(● = a process is alive for this session · {} = transcript written in the last 5m but \
         no process resolved · ⟳ = process alive but transcript is low-information/spinning · \
         blank = neither; PID `?` = two candidate processes — \
         `--format json` lists them; AGE = process elapsed for ● rows, time since last write otherwise)",
        glyph(crate::glyphs::Glyph::InFlight)
    )
}

/// STORY-993: the once-per-run inputs the process resolver needs — one
/// shared `/proc` walk (memoized with every other liveness consumer), one
/// lease read, one manifest read. Built once and applied to every table
/// group so the second group costs only its own rows, never a second
/// directory sweep.
// trace:STORY-993 | ai:claude
struct ProcessResolver {
    root: Option<std::path::PathBuf>,
    procs: Vec<crate::session_liveness::LiveAgentProcess>,
    leases: Vec<crate::SessionLease>,
    manifests: Vec<crate::session_manifest::SessionManifest>,
    now: chrono::DateTime<chrono::Utc>,
}

impl ProcessResolver {
    fn new() -> Self {
        let procs = aida_core::liveness::probe_live_agent_processes();
        let root = crate::find_main_worktree_root().ok();
        let leases = root.as_deref().map(crate::list_leases).unwrap_or_default();
        let manifests = root
            .as_deref()
            .map(crate::session_manifest::list_all)
            .unwrap_or_default();
        Self {
            root,
            procs,
            leases,
            manifests,
            now: chrono::Utc::now(),
        }
    }

    /// Resolve pid / tty / liveness for each row — no per-row spawn, no
    /// per-row directory read. A lease covers a transcript when a session
    /// manifest joins the lease id to the transcript's `claude_session_id`;
    /// its ownership judgment decides liveness. A cwd scan attributes only
    /// unleased historical conversations and cannot revive a dead owner.
    /// The lease role fills a missing transcript role.
    // trace:STORY-993 | ai:claude
    fn fill(&self, sessions: &mut [SessionMeta]) {
        use crate::session_liveness::{self as sl, AgentKind, LeaseFacts, TranscriptFacts};
        for s in sessions.iter_mut() {
            // trace:STORY-2325 | ai:codex — scans cannot revive recorded ownership.
            let owned_lease = self
                .manifests
                .iter()
                .find(|m| m.claude_session_id.as_deref() == Some(s.id.as_str()))
                .and_then(|m| self.leases.iter().find(|l| l.id == m.session_id));
            let lease = owned_lease.map(|l| LeaseFacts {
                pid: l.active_pid.or(l.creator_pid),
                role: l.role.clone(),
            });
            let cwd = s.last_cwd.as_deref().map(Path::new);
            let t = TranscriptFacts {
                agent: s.agent.as_str(),
                cwd,
                started_at: s.started_at,
                age_seconds: s.age_seconds,
            };
            let path = s.path.clone();
            let last_start = move || path.as_deref().and_then(sl::last_process_start_event);
            s.process = sl::resolve(
                &t,
                lease.as_ref(),
                if owned_lease.is_some() {
                    &[]
                } else {
                    &self.procs
                },
                &|pid| {
                    let Some(lease) = owned_lease else {
                        return false;
                    };
                    self.root.as_deref().is_some_and(|root| {
                        matches!(
                            crate::ownership::reader::lease_owner(root, lease),
                            crate::ownership::OwnerVerdict::Alive { .. }
                        ) && lease.active_pid.or(lease.creator_pid) == Some(pid)
                    })
                },
                &|pid| {
                    (
                        aida_core::liveness::process_start_time(pid),
                        aida_core::liveness::process_tty(pid),
                    )
                },
                &last_start,
                self.now,
            )
            .with_spinning(
                s.path
                    .as_deref()
                    .and_then(|path| sl::transcript_spinning(path, self.now)),
            );
            if owned_lease.is_some_and(|lease| {
                self.root.as_deref().is_none_or(|root| {
                    matches!(
                        crate::ownership::reader::lease_owner(root, lease),
                        crate::ownership::OwnerVerdict::Unknown { .. }
                    )
                })
            }) {
                s.process.liveness = sl::Liveness::Unknown;
            }
            if s.role.is_none() {
                s.role = s.process.lease_role.clone();
            }
            // Keep the walk's `AgentKind` spelling and the row's agent label in
            // one place so a future agent kind can't silently never match.
            debug_assert!(
                AgentKind::Claude.label() == "claude" && AgentKind::Codex.label() == "codex"
            );
        }
    }
}

/// STORY-993: the JSON projection of the conversations table.
// trace:STORY-993 | ai:claude
fn print_json(
    here: &[SessionMeta],
    parent: &[SessionMeta],
    parent_root: Option<&Path>,
) -> Result<()> {
    let row = |group: &str, s: &SessionMeta| {
        let p = &s.process;
        serde_json::json!({
            "group": group,
            "id": s.id,
            "agent": s.agent,
            "age_seconds": s.age_seconds,
            "elapsed_secs": p.elapsed_secs,
            "role": s.role,
            "lease_role": p.lease_role,
            "spec": s.spec,
            "title": s.title,
            "started_at": s.started_at.map(|t| t.to_rfc3339()),
            "cwd": s.last_cwd,
            "branch": s.branch,
            "recent_focus": s.recent_focus,
            "pid": p.pid,
            "pids": p.candidates,
            "tty": p.tty,
            "spinning": p.spinning,
            "terminal": s.terminal,
            "liveness": p.liveness.label(),
            "resolution": p.resolution.label(),
        })
    };
    let mut sessions: Vec<serde_json::Value> = here.iter().map(|s| row("here", s)).collect();
    sessions.extend(parent.iter().map(|s| row("parent", s)));
    let out = serde_json::json!({
        "parent_root": parent_root.map(|p| p.display().to_string()),
        "sessions": sessions,
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

/// BUG-98: append a one-line nudge pointing at `aida session leases`
/// when the project has any active scoped leases. `aida session list`
/// shows historical .jsonl conversations — not the same set as the
/// scoped-lease view — so users reaching here for "what's active"
/// otherwise miss the leases entirely. We only print when there's
/// something to point at; an empty-leases project gets no noise.
/// trace:BUG-98 | ai:claude
fn print_leases_hint() {
    let count = crate::active_lease_count_for_cwd();
    if count == 0 {
        return;
    }
    let label = if count == 1 { "lease" } else { "leases" };
    eprintln!(
        "{}",
        format!(
            "({} active session {} — run `aida session leases` for the live view)",
            count, label
        )
        .dimmed()
    );
}

/// STORY-58: a friendly label for a project-root path — the basename when
/// available, the full path otherwise. Used in `── This worktree (foo) ──`
/// headers so the user sees which dir each group represents.
/// trace:STORY-58 | ai:claude
fn group_label(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| path.display().to_string())
}

/// STORY-58: render a `── Label ──` separator above each group. Dimmed so
/// it doesn't compete with the table content.
/// trace:STORY-58 | ai:claude
fn print_group_header(label: &str) {
    println!("{}", format!("── {} ──", label).dimmed());
}

/// STORY-59: resolve `branch` per session by running `git -C <cwd>
/// branch --show-current` once per unique cwd. Cheap (one fork/exec per
/// distinct worktree), and the result is cached across sessions sharing
/// TASK-237: rewrite each session's stored SPEC value to its agreed-id
/// (short form) when the spec resolves to a requirement that carries
/// one. Sessions recorded before `aida db merge-gate` ran keep the
/// long-form node-aware id (`FR-1-042`); this normalizes the column to
/// the short form (`FR-42`) so it matches how `aida list` / `aida show`
/// / `aida history` render specs. A spec that no longer resolves to a
/// requirement is left as-is with an `(unresolved)` suffix.
/// trace:TASK-237 | ai:claude
fn normalize_specs(sessions: &mut [SessionMeta]) {
    if sessions.iter().all(|s| s.spec.is_none()) {
        return;
    }
    let Ok(root) = crate::find_project_root() else {
        return;
    };
    let Some(store) = crate::load_store_for_lookup(&root) else {
        return;
    };
    normalize_specs_with_store(sessions, &store);
}

/// TASK-237: the store-pure half of [`normalize_specs`] — split out so
/// the resolution rules are unit-testable without a project on disk.
/// trace:TASK-237 | ai:claude
fn normalize_specs_with_store(sessions: &mut [SessionMeta], store: &aida_core::RequirementsStore) {
    for s in sessions.iter_mut() {
        let Some(spec) = s.spec.as_deref() else {
            continue;
        };
        match store.get_requirement_by_spec_id(spec) {
            Some(req) => {
                if let Some(agreed) = req.agreed_id.as_deref() {
                    s.spec = Some(agreed.to_string());
                }
            }
            None => s.spec = Some(format!("{} (unresolved)", spec)),
        }
    }
}

/// BUG-112: populate `recent_focus` — the most-recent spec each session
/// worked, for the RECENT FOCUS column. Joins each Claude session to its
/// AIDA manifest on `claude_session_id`, then reads the session activity
/// log for the live "current spec", falling back to the manifest's most
/// recently picked-up planned item. Sessions AIDA never tracked keep
/// `recent_focus = None` and render `-` — absent signal, not the
/// misleading stale title BUG-112 set out to remove.
/// trace:BUG-112 | ai:claude
fn fill_recent_focus(sessions: &mut [SessionMeta]) {
    if sessions.is_empty() {
        return;
    }
    let Ok(root) = crate::find_project_root() else {
        return;
    };
    let manifests = crate::session_manifest::list_all(&root);
    if manifests.is_empty() {
        return;
    }
    for s in sessions.iter_mut() {
        // The manifest's `claude_session_id` is the only reliable join
        // back to a Claude session — lease ids and Claude UUIDs are
        // distinct id spaces. trace:TASK-112 | ai:claude
        let Some(manifest) = manifests
            .iter()
            .find(|m| m.claude_session_id.as_deref() == Some(s.id.as_str()))
        else {
            continue;
        };
        let activity_recent = crate::session_log_recent_spec(&root, &manifest.session_id);
        s.recent_focus = recent_focus_from(manifest, activity_recent.as_deref());
    }
}

/// BUG-112: the store-pure half of [`fill_recent_focus`] — derive the
/// RECENT FOCUS value from a session's manifest plus the optional
/// most-recent spec from its activity log. The activity log is the live
/// signal (actual `aida` spec interactions, newest-first); the manifest's
/// most recently picked-up planned item — latest `started_at`, else
/// highest `position` — is the fallback. Split out so the precedence is
/// unit-testable without a project on disk. trace:BUG-112 | ai:claude
fn recent_focus_from(
    manifest: &crate::session_manifest::SessionManifest,
    activity_recent: Option<&str>,
) -> Option<String> {
    if let Some(spec) = activity_recent {
        return Some(spec.to_string());
    }
    manifest
        .items
        .iter()
        .max_by(|a, b| match (a.started_at, b.started_at) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Greater,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (None, None) => a.position.cmp(&b.position),
        })
        .map(|it| it.spec_id.clone())
}

#[cfg(test)]
mod normalize_specs_tests {
    use super::*;

    fn meta(spec: Option<&str>) -> SessionMeta {
        SessionMeta {
            agent: "claude".to_string(),
            id: "deadbeef".to_string(),
            age_seconds: 10,
            role: None,
            spec: spec.map(|s| s.to_string()),
            title: None,
            started_at: None,
            last_cwd: None,
            terminal: None,
            branch: None,
            recent_focus: None,
            path: None,
            process: Default::default(),
        }
    }

    // trace:STORY-2325 | ai:codex
    #[test]
    fn recordless_leased_conversation_renders_unknown_even_when_old() {
        let tmp = tempfile::tempdir().unwrap();
        let mut row = meta(None);
        row.age_seconds = 86400;
        let lease: crate::SessionLease = serde_json::from_value(serde_json::json!({
            "id": "owner", "scope": uuid::Uuid::from_u128(7).to_string(),
            "slug": "owned", "owner": "tester", "worktree_path": tmp.path(),
            "branch": "work", "started_at": chrono::Utc::now(), "hostname": "test",
            "active_pid": std::process::id(), "role": "implementer",
        }))
        .unwrap();
        let manifest = serde_json::from_value(serde_json::json!({
            "session_id": "owner", "claude_session_id": row.id,
            "planned_at": chrono::Utc::now(), "plan_source": "test", "items": [],
        }))
        .unwrap();
        let resolver = ProcessResolver {
            root: Some(tmp.path().to_path_buf()),
            procs: vec![],
            leases: vec![lease],
            manifests: vec![manifest],
            now: chrono::Utc::now(),
        };
        resolver.fill(std::slice::from_mut(&mut row));
        assert_eq!(
            row.process.liveness,
            crate::session_liveness::Liveness::Unknown
        );
        assert_eq!(process_liveness_glyph(&row), "?");
        assert_eq!(row.process.liveness.label(), "unknown");
    }

    fn req(spec_id: &str, agreed_id: Option<&str>) -> aida_core::Requirement {
        let mut r = aida_core::Requirement::new(format!("test {spec_id}"), String::new());
        r.spec_id = Some(spec_id.to_string());
        r.agreed_id = agreed_id.map(|s| s.to_string());
        r
    }

    fn store(reqs: Vec<aida_core::Requirement>) -> aida_core::RequirementsStore {
        aida_core::RequirementsStore {
            requirements: reqs,
            ..Default::default()
        }
    }

    /// Long-form node-aware id rewrites to the agreed short id.
    #[test]
    fn long_form_resolves_to_agreed_id() {
        let st = store(vec![req("FR-1-042", Some("FR-42"))]);
        let mut sessions = vec![meta(Some("FR-1-042"))];
        normalize_specs_with_store(&mut sessions, &st);
        assert_eq!(sessions[0].spec.as_deref(), Some("FR-42"));
    }

    /// A spec already stored in short form (the agreed id) resolves and
    /// stays in short form — `get_requirement_by_spec_id` matches the
    /// agreed id too.
    #[test]
    fn short_form_stays_short() {
        let st = store(vec![req("FR-1-042", Some("FR-42"))]);
        let mut sessions = vec![meta(Some("FR-42"))];
        normalize_specs_with_store(&mut sessions, &st);
        assert_eq!(sessions[0].spec.as_deref(), Some("FR-42"));
    }

    /// A requirement with no agreed id keeps its long-form spec_id —
    /// that is the correct fallback, not an error.
    #[test]
    fn no_agreed_id_keeps_long_form() {
        let st = store(vec![req("FR-1-042", None)]);
        let mut sessions = vec![meta(Some("FR-1-042"))];
        normalize_specs_with_store(&mut sessions, &st);
        assert_eq!(sessions[0].spec.as_deref(), Some("FR-1-042"));
    }

    /// A spec that doesn't resolve (deleted, or never existed) is marked
    /// `(unresolved)` rather than silently shown as a live id.
    #[test]
    fn unresolvable_spec_is_flagged() {
        let st = store(vec![req("FR-1-042", Some("FR-42"))]);
        let mut sessions = vec![meta(Some("BUG-999"))];
        normalize_specs_with_store(&mut sessions, &st);
        assert_eq!(sessions[0].spec.as_deref(), Some("BUG-999 (unresolved)"));
    }

    /// A session with no spec is left untouched.
    #[test]
    fn missing_spec_is_left_alone() {
        let st = store(vec![req("FR-1-042", Some("FR-42"))]);
        let mut sessions = vec![meta(None)];
        normalize_specs_with_store(&mut sessions, &st);
        assert_eq!(sessions[0].spec, None);
    }
}

/// the same cwd.
/// trace:STORY-59 | ai:claude
fn fill_branches(sessions: &mut [SessionMeta]) {
    use std::collections::HashMap;
    let mut cache: HashMap<String, Option<String>> = HashMap::new();
    for s in sessions.iter_mut() {
        let Some(cwd) = s.last_cwd.clone() else {
            continue;
        };
        let branch = cache
            .entry(cwd.clone())
            .or_insert_with(|| resolve_branch(&cwd))
            .clone();
        s.branch = branch;
    }
}

// trace:BUG-2488 | ai:codex — status discovery also observes the host Git boundary.
fn resolve_branch(cwd: &str) -> Option<String> {
    let out = aida_core::git_ops::host_git_command(Path::new(cwd))
        .args(["branch", "--show-current"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// One line of `~/.aida/session-launches.log` matching the current cwd,
/// parsed back into structured form so `aida session list` can correlate
/// by timestamp. (The cwd field is dropped after the filter — every
/// LaunchRecord we keep matches the active project.)
#[derive(Debug, Clone)]
struct LaunchRecord {
    ts: chrono::DateTime<chrono::Utc>,
    role: String,
    title: String,
}

fn read_launches_for_cwd(cwd: &Path) -> Vec<LaunchRecord> {
    let Ok(path) = launch_log_path() else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let cwd_str = cwd.to_string_lossy().to_string();
    content
        .lines()
        .filter_map(|line| {
            let mut it = line.splitn(5, '\t');
            let ts = it.next()?;
            let role = it.next()?;
            let recorded_cwd = it.next()?;
            let _mode = it.next()?;
            let title = it.next().unwrap_or("");
            if recorded_cwd != cwd_str {
                return None;
            }
            let parsed_ts = chrono::DateTime::parse_from_rfc3339(ts)
                .ok()?
                .with_timezone(&chrono::Utc);
            Some(LaunchRecord {
                ts: parsed_ts,
                role: role.to_string(),
                title: title.to_string(),
            })
        })
        .collect()
}

/// Find the closest launch record (by timestamp) that's within `window`
/// seconds before-or-after the session's start time. Uses the .jsonl's
/// FIRST event timestamp (≈ launch time), not the file mtime (which
/// updates on every event).
/// trace:FR-1-044 | ai:claude
fn match_launch(
    launches: &[LaunchRecord],
    session_started_at: chrono::DateTime<chrono::Utc>,
    window_sec: i64,
) -> Option<&LaunchRecord> {
    launches
        .iter()
        .filter(|l| (session_started_at - l.ts).num_seconds().abs() <= window_sec)
        .min_by_key(|l| (session_started_at - l.ts).num_seconds().abs())
}

/// Walk Claude Code's per-project session directory for the current cwd
/// and return the most recent N parsed `SessionMeta` entries.
fn collect_sessions(limit: usize) -> Result<Vec<SessionMeta>> {
    let cwd = std::env::current_dir().context("could not determine cwd")?;
    collect_sessions_from_cwd(&cwd, limit)
}

/// STORY-58: same as `collect_sessions` but for an arbitrary project root,
/// so `aida session list` can pull a second batch from the parent project's
/// Claude Code session storage when it's invoked inside a session worktree.
/// trace:STORY-58 | ai:claude
fn collect_sessions_from_cwd(cwd: &Path, limit: usize) -> Result<Vec<SessionMeta>> {
    let mut entries = claude_entries_for_cwd(cwd)?;
    entries.extend(codex_entries()?);
    entries.extend(antigravity_entries()?);
    entries.sort_by(|a, b| b.mtime.cmp(&a.mtime));
    entries.truncate(session_scan_limit(limit));

    let now = SystemTime::now();
    let launches = read_launches_for_cwd(cwd);
    let mut metas: Vec<SessionMeta> = entries
        .into_iter()
        .filter_map(|entry| {
            let mut meta =
                parse_session_meta_for_agent(&entry.path, entry.mtime, now, entry.agent).ok()?;
            // FR-1-044: try to attribute the session to a launch
            // record. The .jsonl's FIRST event timestamp is the right
            // anchor — file mtime updates on every event so it drifts
            // away from the actual launch time as the session is used.
            if let Some(started) = meta.started_at {
                if let Some(launch) = match_launch(&launches, started, 60) {
                    if !launch.title.is_empty() {
                        meta.title = Some(launch.title.clone());
                    }
                    if launch.role != "-" {
                        meta.role = Some(launch.role.clone());
                    }
                }
            }
            Some(meta)
        })
        .filter(|m| {
            m.agent == "claude"
                || m.last_cwd
                    .as_deref()
                    .map(|session_cwd| session_cwd_in_project(session_cwd, cwd))
                    .unwrap_or(false)
        })
        .collect();
    enrich_from_agent_registry(&mut metas);
    metas.sort_by_key(|m| m.age_seconds);
    metas.truncate(limit);
    Ok(metas)
}

#[derive(Debug, Clone)]
struct SessionLogEntry {
    path: PathBuf,
    mtime: SystemTime,
    agent: &'static str,
}

fn jsonl_entries_in_dir(dir: &Path, agent: &'static str) -> Result<Vec<SessionLogEntry>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    Ok(std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                return None;
            }
            let mtime = e.metadata().ok()?.modified().ok()?;
            Some(SessionLogEntry { path, mtime, agent })
        })
        .collect())
}

fn claude_entries_for_cwd(cwd: &Path) -> Result<Vec<SessionLogEntry>> {
    jsonl_entries_in_dir(&claude_project_dir(cwd)?, "claude")
}

fn codex_entries() -> Result<Vec<SessionLogEntry>> {
    // BUG-2386: the same root resume classification reads, so the pickers
    // never list an id `recorded_resume` cannot place. trace:BUG-2386 | ai:claude
    let root = crate::vendor_activity::codex_sessions_root()
        .context("HOME not set; cannot locate Codex sessions")?;
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut dirs = vec![root];
    let mut entries = Vec::new();
    while let Some(dir) = dirs.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                if let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) {
                    entries.push(SessionLogEntry {
                        path,
                        mtime,
                        agent: "codex",
                    });
                }
            }
        }
    }
    Ok(entries)
}

fn antigravity_entries() -> Result<Vec<SessionLogEntry>> {
    let home = crate::home_dir().context("HOME not set; cannot locate Antigravity sessions")?;
    let candidates = [
        home.join(".antigravity").join("sessions"),
        home.join(".config").join("Antigravity").join("sessions"),
        home.join(".config").join("Antigravity").join("Sessions"),
    ];
    let mut entries = Vec::new();
    for dir in candidates {
        entries.extend(jsonl_entries_in_dir(&dir, "antigravity")?);
    }
    Ok(entries)
}

/// Encode a project path the way Claude Code does — replace each `/`
/// with `-` (and drop the leading separator). e.g. `/home/joe/ai/aida` →
/// `-home-joe-ai-aida`.
pub(crate) fn claude_project_dir(cwd: &Path) -> Result<PathBuf> {
    let s = cwd.to_string_lossy();
    // BUG-346: Windows paths can carry backslashes even when tests use
    // Unix-shaped fixture strings elsewhere. Normalize both spellings so
    // advisor fork destinations use the same slug convention cross-platform.
    // trace:BUG-346 | ai:codex
    let encoded = s.replace(['\\', '/'], "-");
    #[cfg(test)]
    let home = std::env::var_os("AIDA_TEST_HOME")
        .map(PathBuf::from)
        .or_else(crate::home_dir)
        .context("HOME not set; cannot locate Claude project dir")?;
    #[cfg(not(test))]
    let home = crate::home_dir().context("HOME not set; cannot locate Claude project dir")?;
    Ok(home.join(".claude/projects").join(encoded))
}

/// TASK-112: scan every Claude Code project directory under
/// `~/.claude/projects/` and return recorded sessions whose first-mentioned
/// SPEC-ID matches `scope` (case-insensitive), most recent first. Used by
/// `aida queue work --resume` / `--list-sessions` to find a prior
/// conversation to continue for a given scope — sessions launched in a
/// since-removed worktree still surface, because the `.jsonl` files persist
/// after `aida session end` removes the worktree.
///
/// Bounded: across all project dirs we parse only the 150
/// most-recently-modified `.jsonl` files, so the scan stays fast even with
/// hundreds of historical sessions. trace:TASK-112 | ai:claude
pub fn list_scope_sessions(scope: &str) -> Result<Vec<SessionMeta>> {
    let want = scope.trim();
    // BUG-447: scope the machine-global scan to the CURRENT project. Onboarding
    // seeds the same id (TASK-007) in every project, and the scan keyed on
    // spec-id alone would surface a same-id session from a DIFFERENT (or
    // since-deleted) project on this machine — resuming it would replay another
    // project's context. Filter to sessions whose recorded cwd is within the
    // current project root or a sibling `<root>-<slug>` worktree. When we can't
    // resolve a project root (not in a project), keep the old global behaviour.
    // trace:BUG-447 | ai:claude
    let mut out: Vec<SessionMeta> =
        collect_global_project_sessions(150, Some(SESSION_SCAN_BYTE_BUDGET), None, None, |m| {
            m.spec
                .as_deref()
                .map(|s| s.eq_ignore_ascii_case(want))
                .unwrap_or(false)
        })?
        .collect();
    out.sort_by_key(|m| m.age_seconds);
    Ok(out)
}

/// STORY-821: recent Claude conversations whose detected AIDA role matches
/// `role`, newest first, scoped to the current project/worktree family. This is
/// the role-enter view over the same session metadata used by `session resume`.
/// BUG-1789: bounded — stops once `limit` matches are found, and never reads
/// more than `SESSION_SCAN_BYTE_BUDGET` in total.
// trace:STORY-821 | ai:codex
// trace:BUG-1789 | ai:antigravity
pub fn list_role_sessions(role: &str, limit: usize) -> Result<Vec<SessionMeta>> {
    let want = canonical_session_role(role);
    let mut out: Vec<SessionMeta> = collect_global_project_sessions(
        limit,
        Some(SESSION_SCAN_BYTE_BUDGET),
        Some(&want),
        None,
        |m| m.role.as_deref().map(canonical_session_role).as_deref() == Some(want.as_str()),
    )?
    .collect();
    out.sort_by_key(|m| m.age_seconds);
    fill_branches(&mut out);
    normalize_specs(&mut out);
    fill_recent_focus(&mut out);
    Ok(out)
}

/// Newest-first, project-scoped session rows that pass `filter_fn`; see
/// [`select_recent_sessions`] for the bounds. `byte_budget: None` means only
/// the file-count bound applies (used by exact id-prefix resume lookups).
// trace:BUG-1789 | ai:antigravity
// trace:TASK-1578 | ai:antigravity
fn collect_global_project_sessions<F>(
    limit: usize,
    byte_budget: Option<u64>,
    expected_role: Option<&str>,
    id_prefix: Option<&str>,
    mut filter_fn: F,
) -> Result<impl Iterator<Item = SessionMeta>>
where
    F: FnMut(&SessionMeta) -> bool,
{
    let project_root = crate::find_main_worktree_root().ok();
    let encoded_cwd = project_root
        .as_deref()
        .map(crate::process_probe::encode_cwd_for_projects);
    let home = crate::home_dir().context("HOME not set; cannot locate sessions")?;
    let projects = home.join(".claude").join("projects");
    let mut entries: Vec<SessionLogEntry> = Vec::new();
    if projects.is_dir() {
        if let Ok(dirs) = std::fs::read_dir(&projects) {
            for dir in dirs.flatten() {
                let p = dir.path();
                if !p.is_dir() {
                    continue;
                }
                if let Some(enc) = &encoded_cwd {
                    if p.file_name().and_then(|s| s.to_str()) != Some(enc) {
                        continue;
                    }
                }
                let Ok(files) = std::fs::read_dir(&p) else {
                    continue;
                };
                for f in files.flatten() {
                    let fp = f.path();
                    if fp.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                        continue;
                    }
                    if let Some(prefix) = id_prefix {
                        if !fp
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .is_some_and(|s| s.starts_with(prefix))
                        {
                            continue;
                        }
                    }
                    if let Ok(mtime) = f.metadata().and_then(|m| m.modified()) {
                        entries.push(SessionLogEntry {
                            path: fp,
                            mtime,
                            agent: "claude",
                        });
                    }
                }
            }
        }
    }
    let mut codex = codex_entries()?;
    let mut antigravity = antigravity_entries()?;
    if let Some(prefix) = id_prefix {
        let f = |e: &SessionLogEntry| {
            e.path
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| {
                    s.starts_with(prefix)
                        || (e.agent == "codex"
                            && codex_id_from_rollout_stem(s).is_some_and(|s| s.starts_with(prefix)))
                })
        };
        codex.retain(f);
        antigravity.retain(|e: &SessionLogEntry| {
            e.path
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.starts_with(prefix))
        });
    }
    entries.extend(codex);
    entries.extend(antigravity);
    entries.sort_by(|a, b| b.mtime.cmp(&a.mtime));

    let now = SystemTime::now();
    let agents = load_agent_views_for_enrich();
    let (sessions, _stats) = select_recent_sessions(
        entries,
        limit,
        byte_budget,
        expected_role,
        now,
        project_root.as_deref(),
        &agents,
        &mut filter_fn,
    );
    Ok(sessions.into_iter())
}

/// BUG-1789: what a bounded session scan actually consumed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct SessionScanStats {
    files_opened: usize,
    bytes_read: u64,
}

/// BUG-1789: walk `entries` (already sorted newest-first) and stop as soon as
/// any bound trips: `limit` rows survived the project-scope + caller filters,
/// `session_scan_limit(limit.max(150))` files were opened, or `byte_budget` bytes were
/// read. Each file read is capped at `SESSION_META_MAX_BYTES`, and a file is
/// only opened while a full cap still fits, so `bytes_read <= byte_budget` is
/// a hard guarantee. Registry
/// enrichment runs per row *before* the filter so registry-only roles/specs
/// still match. Returns the selected rows (newest first) plus scan stats.
// trace:BUG-1789 | ai:antigravity
fn select_recent_sessions<F>(
    entries: Vec<SessionLogEntry>,
    limit: usize,
    byte_budget: Option<u64>,
    expected_role: Option<&str>,
    now: SystemTime,
    project_root: Option<&Path>,
    agents: &[crate::agent_registry::AgentRegistryView],
    filter_fn: &mut F,
) -> (Vec<SessionMeta>, SessionScanStats)
where
    F: FnMut(&SessionMeta) -> bool,
{
    // Keep the historical >=1500-file window (`session_scan_limit(150)`);
    // the byte budget, not the file count, is the real I/O bound.
    let max_open = session_scan_limit(limit.max(150));
    let mut stats = SessionScanStats::default();
    let mut sessions = Vec::new();
    for entry in entries {
        if sessions.len() >= limit
            || stats.files_opened >= max_open
            || byte_budget.is_some_and(|b| stats.bytes_read + SESSION_META_MAX_BYTES > b)
        {
            break;
        }
        stats.files_opened += 1;
        let in_scope = |cwd: &str| {
            project_root
                .map(|root| session_cwd_in_project(cwd, root))
                .unwrap_or(true)
        };
        let scope: Option<&dyn Fn(&str) -> bool> = project_root.map(|_| &in_scope as _);
        let Ok((mut m, n)) = parse_session_meta_counted(
            &entry.path,
            entry.mtime,
            now,
            entry.agent,
            expected_role,
            scope,
        ) else {
            continue;
        };
        stats.bytes_read += n;
        let in_project = match project_root {
            Some(root) => m
                .last_cwd
                .as_deref()
                .map(|cwd| session_cwd_in_project(cwd, root))
                .unwrap_or(false),
            None => true,
        };
        if !in_project {
            continue;
        }
        enrich_one_from_agent_views(&mut m, agents);
        if filter_fn(&m) {
            sessions.push(m);
        }
    }
    sessions.sort_by_key(|m| m.age_seconds);
    (sessions, stats)
}

/// BUG-1789: `Read` adapter counting bytes actually pulled from the file
/// (including `BufReader` read-ahead) — the injected counter for the scan
/// budget.
struct CountingRead<R> {
    inner: R,
    count: std::rc::Rc<std::cell::Cell<u64>>,
}

impl<R: std::io::Read> std::io::Read for CountingRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count.set(self.count.get() + n as u64);
        Ok(n)
    }
}

/// BUG-1789: per-file read cap for `parse_session_meta_for_agent`. Sized from
/// measured logs: the first AIDA role marker sits at p50 ~100 KB, p99 ~1.4 MB
/// (max seen ~2.8 MB) — a 256 KB cap would have hidden ~1 in 5 sessions.
const SESSION_META_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// BUG-1789: a scoped parse gives up after this many bytes with no cwd seen.
const SESSION_META_NO_CWD_PROBE_BYTES: u64 = 64 * 1024;

/// BUG-1789: total read budget for interactive scans (role-enter picker,
/// scope resume lookup). A hard ceiling (~47 MB) so a cold scan stays under
/// 50 MB even on a contended disk.
const SESSION_SCAN_BYTE_BUDGET: u64 = 45 * 1024 * 1024;

fn session_scan_limit(limit: usize) -> usize {
    limit.saturating_mul(10).max(150)
}

/// Registry records know the authoritative launcher agent and native session id
/// for AIDA-spawned sessions. Let that beat store-scan heuristics when a row
/// correlates.
// trace:STORY-822 | ai:codex
fn enrich_from_agent_registry(sessions: &mut [SessionMeta]) {
    let agents = load_agent_views_for_enrich();
    for session in sessions {
        enrich_one_from_agent_views(session, &agents);
    }
}

fn load_agent_views_for_enrich() -> Vec<crate::agent_registry::AgentRegistryView> {
    let Ok(project_root) = crate::find_main_worktree_root() else {
        return Vec::new();
    };
    // trace:TASK-2272 | ai:claude
    let ctx = crate::agent_registry::AgentClassifyContext::new(chrono::Utc::now(), 30, vec![])
        .for_display();
    crate::agent_registry::list_agent_views(&project_root, &ctx)
}

fn enrich_one_from_agent_views(
    session: &mut SessionMeta,
    agents: &[crate::agent_registry::AgentRegistryView],
) {
    let Some(view) = agents.iter().find(|agent| {
        agent
            .native_session_id
            .as_deref()
            .map(|id| id == session.id)
            .unwrap_or(false)
    }) else {
        return;
    };
    session.agent = view.agent_type.clone();
    if session.role.is_none() {
        session.role = view.role.clone();
    }
    if session.spec.is_none() {
        session.spec = view.current_spec.clone();
    }
    if session.terminal.is_none() {
        session.terminal = view.terminal.clone();
    }
}

/// BUG-447: is a recorded session's `cwd` within the current project's scope —
/// the project-root subtree, or a sibling `<root>-<slug>` worktree that
/// `aida queue work` creates (`<parent>/<repo_name>-<slug>`)? `Path::starts_with`
/// is component-wise, so a sibling worktree (`…/ai-task-007`) does NOT match the
/// root (`…/ai`); the explicit sibling check handles it. trace:BUG-447 | ai:claude
fn session_cwd_in_project(cwd: &str, project_root: &Path) -> bool {
    let cwd = Path::new(cwd.trim());
    if cwd == project_root || cwd.starts_with(project_root) {
        return true;
    }
    let (Some(parent), Some(repo_name)) = (
        project_root.parent(),
        project_root.file_name().and_then(|s| s.to_str()),
    ) else {
        return false;
    };
    match (cwd.parent(), cwd.file_name().and_then(|s| s.to_str())) {
        (Some(cwd_parent), Some(cwd_name)) => {
            cwd_parent == parent && cwd_name.starts_with(&format!("{repo_name}-"))
        }
        _ => false,
    }
}

/// TASK-402: canonicalize a role name parsed out of a session's JSONL so
/// `--list-sessions` reports the project-wide identity, not a deprecated
/// alias. `dialog` is the legacy token (TASK-279) for `advisor` (TASK-586);
/// normalizing here keeps the recorded role aligned with what the orchestrator
/// actually launched (`--role implementer`/`advisor`) and removes the
/// "did the orchestrator launch the wrong role?" confusion during a
/// resume-after-failure recovery. Pure + case-insensitive on the alias.
/// trace:TASK-402 | ai:claude
fn canonical_session_role(raw: &str) -> String {
    if raw.eq_ignore_ascii_case("dialog") {
        "advisor".to_string()
    } else {
        raw.to_string()
    }
}

/// TASK-112: one-line summary of a recorded Claude session, for the
/// `aida queue work --list-sessions` output. `<liveness> <id8>  <age>
/// <role>  <title>`. trace:TASK-112 | ai:claude
pub fn format_session_line(m: &SessionMeta) -> String {
    format!(
        "{} {:<8}  {:>4}  {:<11}  {:<11}  {}",
        liveness_indicator(m.age_seconds),
        &m.id[..m.id.len().min(8)],
        humanize_age(m.age_seconds),
        m.agent,
        m.role.as_deref().unwrap_or("-"),
        m.title.as_deref().unwrap_or("(untitled)"),
    )
}

/// Resolve only the AIDA role from a session JSONL. The `aida ps` live table
/// uses this when a lease and a currently-active transcript point at the same
/// pid: the transcript role names what the session is actually doing now,
/// while the lease role remains provenance.
// trace:TASK-152 | ai:codex
pub fn role_from_jsonl(path: &Path, agent: &'static str) -> Result<Option<String>> {
    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or_else(|_| SystemTime::now());
    Ok(parse_session_meta_for_agent(path, mtime, SystemTime::now(), agent)?.role)
}

/// Resolve a Claude Code transcript role by its native session UUID.
///
/// `aida ps` uses this through the lease-manifest `claude_session_id` join when
/// a long-lived/resumed transcript is no longer attached to the live-process
/// JSONL probe. Exact filename lookup keeps the scan cheap and avoids guessing.
// trace:TASK-153 | ai:codex
pub fn role_from_claude_session_id(session_id: &str) -> Option<String> {
    let path = claude_session_jsonl_path_by_id(session_id)?;
    role_from_jsonl(&path, "claude").ok().flatten()
}

// trace:TASK-153 | ai:codex
fn claude_session_jsonl_path_by_id(session_id: &str) -> Option<PathBuf> {
    if session_id.trim().is_empty() || session_id.contains(['/', '\\']) {
        return None;
    }
    #[cfg(test)]
    let home = std::env::var_os("AIDA_TEST_HOME")
        .map(PathBuf::from)
        .or_else(crate::home_dir)?;
    #[cfg(not(test))]
    let home = crate::home_dir()?;
    let projects = home.join(".claude").join("projects");
    let dirs = std::fs::read_dir(projects).ok()?;
    let filename = format!("{session_id}.jsonl");
    for dir in dirs.flatten() {
        let project_dir = dir.path();
        if !project_dir.is_dir() {
            continue;
        }
        let candidate = project_dir.join(&filename);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
fn parse_session_meta(path: &Path, mtime: SystemTime, now: SystemTime) -> Result<SessionMeta> {
    parse_session_meta_for_agent(path, mtime, now, "claude")
}

fn parse_session_meta_for_agent(
    path: &Path,
    mtime: SystemTime,
    now: SystemTime,
    agent: &'static str,
) -> Result<SessionMeta> {
    parse_session_meta_counted(path, mtime, now, agent, None, None).map(|(meta, _)| meta)
}

/// Same as [`parse_session_meta_for_agent`], also returning the number of
/// bytes actually read from the file (BUG-1789 scan budget accounting).
///
/// `in_scope`: when given, the parse stops as soon as the FIRST recorded cwd
/// is out of scope, so a foreign-project log costs only its head (Codex puts
/// cwd in line-1 `session_meta`; Claude events carry it from the first event)
/// instead of a full per-file cap. Trade-off: a session launched outside the
/// project that later moved into it is no longer matched.
// trace:BUG-1789 | ai:antigravity
// trace:TASK-1578 | ai:antigravity
fn parse_session_meta_counted(
    path: &Path,
    mtime: SystemTime,
    now: SystemTime,
    agent: &'static str,
    expected_role: Option<&str>,
    in_scope: Option<&dyn Fn(&str) -> bool>,
) -> Result<(SessionMeta, u64)> {
    use std::io::{BufRead, BufReader};
    let mut file_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("?")
        .to_string();
    if agent == "codex" {
        if let Some(id) = codex_id_from_rollout_stem(&file_name) {
            file_name = id;
        }
    }
    let age_seconds = now.duration_since(mtime).map(|d| d.as_secs()).unwrap_or(0);

    // Parse only the first chunk of lines — `ai-title` appears within
    // the first few hundred events and the SessionStart hook output
    // (the source of role + first @SPEC mentions) is even earlier. Reads
    // stay sub-millisecond per file even on multi-MB session logs.
    const MAX_LINES: usize = 400;
    let file = std::fs::File::open(path)?;
    // BUG-1789: the line cap is no byte bound — a Codex rollout event is one
    // multi-MB line — so also cap the bytes read per file. Every marker this
    // parser needs lives near the head of the log.
    // trace:BUG-1789 | ai:antigravity
    let counter = std::rc::Rc::new(std::cell::Cell::new(0u64));
    let counted = CountingRead {
        inner: file,
        count: counter.clone(),
    };
    let reader = BufReader::new(std::io::Read::take(counted, SESSION_META_MAX_BYTES));
    let mut scope_checked = in_scope.is_none();
    let mut role: Option<String> = None;
    let mut title: Option<String> = None;
    let mut spec: Option<String> = None;
    let mut started_at: Option<chrono::DateTime<chrono::Utc>> = None;
    let mut last_cwd: Option<String> = None;
    let mut role_resolved = false;
    // BUG-1593 (rework, PROXY DECISION): two marker tiers within the
    // unconditional cap. Tier 1 (structural, checked first) is written
    // only by AIDA's own launch/hook context and always wins when
    // present anywhere within the cap. Tier 2 (bare `Role: `) is the
    // fallback used ONLY when no tier-1 marker appears at all within
    // the cap — `.claude/skills/aida-pickup.md`, `aida-queue.md`, and
    // `aida role show` all echo a bare `Role: <role>` line with no
    // structural anchor, and dropping it entirely (the first rework
    // attempt) regressed those sessions back to `unknown`.
    let mut tier2_role: Option<String> = None;
    let mut tier2_resolved = false;
    // BUG-1593: the cap is honoured unconditionally. A role unresolved by
    // MAX_LINES is `unknown` (None), never a late match found by scanning
    // further into a transcript that may still be growing.
    // trace:BUG-1593 | ai:claude
    for (i, line) in reader.split(b'\n').enumerate() {
        if i >= MAX_LINES {
            break;
        }
        // Lossy decode: the byte cap can split a multibyte char on the last
        // line, which must not discard the whole (often first) line.
        // Lines with invalid UTF-8 (previously skipped by `lines()`) are now
        // parsed with U+FFFD replacements.
        let Ok(line) = line else { continue };
        let line = String::from_utf8_lossy(&line);
        let line = line.strip_suffix('\r').unwrap_or(&line);

        // STORY-59: capture the most recent cwd we see in the parse
        // window. Each event in Claude Code's .jsonl carries `"cwd":"..."`
        // — we read the last occurrence so a session that switched
        // worktrees mid-flight reports its current cwd, not its launch
        // cwd. Cheap because it's just a substring scan per line.
        if let Some(cwd) = extract_str(&line, "\"cwd\":\"") {
            last_cwd = Some(cwd);
        }

        // Codex emits a structured `session_meta` payload on the first line.
        // Prefer it over filename heuristics when present. trace:STORY-822 | ai:codex
        if agent == "codex" && i == 0 {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
                if let Some(payload) = value.get("payload") {
                    if let Some(id) = payload
                        .get("session_id")
                        .or_else(|| payload.get("id"))
                        .and_then(|v| v.as_str())
                    {
                        file_name = id.to_string();
                    }
                    if let Some(cwd) = payload.get("cwd").and_then(|v| v.as_str()) {
                        last_cwd = Some(cwd.to_string());
                    }
                    if let Some(ts) = payload.get("timestamp").and_then(|v| v.as_str()) {
                        if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(ts) {
                            started_at = Some(parsed.with_timezone(&chrono::Utc));
                        }
                    }
                }
            }
        }

        // BUG-1789: bail on a foreign-project log once its first cwd is known.
        // A scoped parse that has seen no cwd in its first 64 KiB would be
        // dropped by the caller anyway (no cwd => out of scope), so stop.
        if !scope_checked {
            if let (Some(cwd), Some(f)) = (last_cwd.as_deref(), in_scope) {
                scope_checked = true;
                if !f(cwd) {
                    break;
                }
            } else if counter.get() > SESSION_META_NO_CWD_PROBE_BYTES {
                break;
            }
        }

        // First event with a `"timestamp":"..."` field — gives us a
        // close-to-launch-time anchor for FR-1-044's launches.log
        // correlation. Skip the file-history-snapshot's timestamp
        // (which has its own, slightly different timestamp).
        if started_at.is_none() {
            if let Some(ts_str) = extract_str(&line, "\"timestamp\":\"") {
                if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(&ts_str) {
                    started_at = Some(parsed.with_timezone(&chrono::Utc));
                }
            }
        }

        // ai-title event — what Claude Code's resume picker shows.
        if title.is_none() {
            if let Some(t) = extract_str(&line, "\"aiTitle\":\"") {
                title = Some(t);
            }
        }

        // Role markers, two tiers. Tier 1 (structural) is written only by
        // AIDA's own launch/hook context and is never an arbitrary
        // substring that happened to be echoed back through ordinary tool
        // output or quoted text:
        //   - `AIDA_SESSION_ROLE=implementer`  (env, set at session start)
        //   - `- Role: implementer`            (render_agent_launch_context's
        //                                        launch-context line, the
        //                                        dash anchors it to that
        //                                        exact structural line)
        //   - `AIDA · role: implementer`       (aida-session-context.sh hook)
        //   - `AIDA active role: implementer`  (aida-role-context.sh hook)
        // The FIRST tier-1 match anywhere within the cap always wins.
        //
        // Tier 2 (fallback) is a bare `Role: ` with no structural anchor —
        // emitted by `.claude/skills/aida-pickup.md`, `aida-queue.md`, and
        // `aida role show` output piped through a tool call. It is used
        // ONLY when the whole capped scan finds no tier-1 marker at all;
        // dropping it outright (rather than demoting it to a fallback)
        // regressed those sessions to `unknown` — that was the root cause
        // of the review BLOCK on the first rework of this bug. A tier-2
        // candidate is only recorded on a line that did NOT already match
        // a tier-1 marker, so a transcript quoting a foreign `Role: X`
        // elsewhere can never outrank a real tier-1 launch marker.
        // trace:FR-1-043 | ai:claude
        // trace:BUG-837 | ai:codex
        // trace:BUG-1593 | ai:claude
        if !role_resolved {
            let mut matched_tier1 = false;
            for marker in [
                "AIDA_SESSION_ROLE=",
                "- Role: ",
                "AIDA · role: ",
                "AIDA active role: ",
            ] {
                if let Some(idx) = line.find(marker) {
                    let after = &line[idx + marker.len()..];
                    let name: String = after
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                        .collect();
                    // Reject empty matches and the literal `(none active)`
                    // / `${role}` cases that show up in shell-template
                    // output or unset-env echos.
                    //
                    // TASK-402 (friction #2): a session the orchestrator
                    // launched with `--role implementer` could be tagged
                    // `dialog` in `--list-sessions` because the early JSONL
                    // scan caught the deprecated alias (a not-yet-migrated
                    // role file / shell echo). Canonicalize the parsed name
                    // so the recorded role reflects the project-wide identity
                    // (`dialog` is the deprecated alias for `advisor`) instead
                    // of confusing the operator mid-recovery.
                    // trace:TASK-402 | ai:claude
                    if !name.is_empty() {
                        matched_tier1 = true;
                        role_resolved = true;
                        if name != "none" {
                            role = Some(canonical_session_role(&name));
                        }
                        break;
                    }
                }
            }
            if role_resolved {
                if let (Some(expected), Some(r)) = (expected_role, &role) {
                    if r != expected {
                        break;
                    }
                }
            }
            if !matched_tier1 && !tier2_resolved {
                const TIER2_MARKER: &str = "Role: ";
                if let Some(idx) = line.find(TIER2_MARKER) {
                    let after = &line[idx + TIER2_MARKER.len()..];
                    let name: String = after
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                        .collect();
                    if !name.is_empty() {
                        tier2_resolved = true;
                        if name != "none" {
                            tier2_role = Some(canonical_session_role(&name));
                        }
                    }
                }
            }
        }

        // First mentioned SPEC-ID (heuristic: regex-ish scan for typical
        // patterns FR-N, BUG-N-M, TASK-N, etc. in a user message body).
        if spec.is_none() {
            if let Some(s) = first_spec_id(&line) {
                spec = Some(s);
            }
        }
    }

    // No tier-1 (structural) marker anywhere within the cap: fall back to
    // the tier-2 bare `Role: ` match, if any.
    let role = role.or(tier2_role);

    let meta = SessionMeta {
        agent: agent.to_string(),
        id: file_name,
        age_seconds,
        role,
        spec,
        title,
        started_at,
        last_cwd,
        terminal: None,
        branch: None,
        recent_focus: None,
        path: Some(path.to_path_buf()),
        process: Default::default(),
    };
    Ok((meta, counter.get()))
}

fn codex_id_from_rollout_stem(stem: &str) -> Option<String> {
    let rest = stem.strip_prefix("rollout-")?;
    let parts: Vec<&str> = rest.rsplitn(6, '-').collect();
    if parts.len() < 5 {
        return None;
    }
    let mut uuid_parts = parts[..5].to_vec();
    uuid_parts.reverse();
    Some(uuid_parts.join("-"))
}

/// Find a JSON string-value for `"<key>":"...` and return the (unescaped
/// only for backslash-quote) value up to the closing quote. Good enough
/// for the simple `"aiTitle":"..."` event we emit ourselves; not a full
/// JSON parser.
fn extract_str(line: &str, marker: &str) -> Option<String> {
    let start = line.find(marker)? + marker.len();
    let mut end = start;
    let bytes = line.as_bytes();
    while end < bytes.len() {
        let b = bytes[end];
        if b == b'\\' && end + 1 < bytes.len() {
            end += 2;
            continue;
        }
        if b == b'"' {
            break;
        }
        end += 1;
    }
    Some(line[start..end].replace("\\\"", "\"").replace("\\\\", "\\"))
}

/// Pull the first SPEC-ID-shaped token (FR-1-042, BUG-1-038, TASK-001,
/// EPIC-2, …) from a line. Skips negatives like ANSI codes and `1-2`
/// number ranges.
fn first_spec_id(line: &str) -> Option<String> {
    // Manual scan: walk the line looking for an alpha run of length ≥2,
    // followed immediately by `-` and digits. Optionally followed by `-`
    // and digits again (node-aware form). Return the first match.
    let bytes = line.as_bytes();
    let n = bytes.len();
    let mut i = 0usize;
    while i < n {
        if !bytes[i].is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        let start = i;
        while i < n && bytes[i].is_ascii_alphabetic() {
            i += 1;
        }
        if i - start < 2 {
            continue;
        }
        if i >= n || bytes[i] != b'-' {
            continue;
        }
        let after_dash = i + 1;
        if after_dash >= n || !bytes[after_dash].is_ascii_digit() {
            continue;
        }
        // Walk through digits, optionally another -digits segment.
        let mut j = after_dash;
        while j < n && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j < n && bytes[j] == b'-' && j + 1 < n && bytes[j + 1].is_ascii_digit() {
            j += 1;
            while j < n && bytes[j].is_ascii_digit() {
                j += 1;
            }
        }
        // Reject very short prefixes like "v-1" or "x-2" — require ≥2
        // alpha chars (already guaranteed) AND the prefix to be one of
        // the conventional SPEC-ID prefixes. Cheap check:
        let prefix = &line[start..start + (i - start)];
        if matches!(
            prefix,
            "FR" | "BUG"
                | "TASK"
                | "EPIC"
                | "STORY"
                | "SPIKE"
                | "SPRINT"
                | "FOLDER"
                | "META"
                | "UR"
                | "SR"
                | "CR"
                | "REQ"
                | "NFR"
                | "SPEC"
        ) {
            return Some(line[start..j].to_string());
        }
    }
    None
}

/// STORY-58: column widths for the session-list table, computed over a
/// chosen set of rows. Lifted out of `print_table` so the grouped path can
/// compute one shared width set from the union of "this worktree" + parent
/// rows, keeping columns aligned across both sections.
/// trace:STORY-58 | ai:claude
struct TableWidths {
    id_w: usize,
    age_w: usize,
    pid_w: usize,
    tty_w: usize,
    agent_w: usize,
    role_w: usize,
    spec_w: usize,
    terminal_w: usize,
    worktree_w: usize,
}

impl TableWidths {
    /// Worktree column = `<basename> @ <branch>` (truncated). 28 chars is
    /// enough for "aida-epic-20 @ epic-20-batch3" without squeezing TITLE.
    const WORKTREE_W: usize = 28;

    fn compute<'a, I: Iterator<Item = &'a SessionMeta>>(sessions: I) -> Self {
        let mut role_w = 4usize;
        let mut spec_w = 4usize;
        let mut agent_w = 5usize;
        let mut pid_w = 3usize;
        let mut tty_w = 3usize;
        let mut terminal_w = 3usize;
        for s in sessions {
            agent_w = agent_w.max(s.agent.len());
            role_w = role_w.max(s.role.as_deref().unwrap_or("-").len());
            spec_w = spec_w.max(s.spec.as_deref().unwrap_or("-").len());
            pid_w = pid_w.max(s.process.pid_cell().len());
            tty_w = tty_w.max(s.process.tty_cell().len());
            terminal_w = terminal_w
                .max(crate::agent_registry::terminal_cell(s.terminal.as_ref(), None).len());
        }
        Self {
            id_w: 8,
            age_w: 6,
            pid_w,
            tty_w,
            agent_w,
            role_w,
            spec_w,
            terminal_w: terminal_w.min(20),
            worktree_w: Self::WORKTREE_W,
        }
    }
}

fn print_table(sessions: &[SessionMeta]) {
    let widths = TableWidths::compute(sessions.iter());
    print_table_with_widths(sessions, &widths);
}

/// STORY-993: the AGE cell — process elapsed for a `●` row (the process
/// resolved and its start time is known), time since the transcript's last
/// write otherwise.
// trace:STORY-993 | ai:claude
fn age_cell(s: &SessionMeta) -> String {
    match (s.process.liveness, s.process.elapsed_secs) {
        (crate::session_liveness::Liveness::Alive, Some(e))
        | (crate::session_liveness::Liveness::Spinning, Some(e)) => humanize_age(e),
        _ => humanize_age(s.age_seconds),
    }
}

/// STORY-993: the liveness glyph for a row — `●` only when a process was
/// resolved and is alive; the registry's partial glyph when the transcript
/// was merely written recently; a blank otherwise.
// trace:STORY-993 | ai:claude
fn process_liveness_glyph(s: &SessionMeta) -> &'static str {
    use crate::session_liveness::Liveness;
    match s.process.liveness {
        Liveness::Alive => "●",
        Liveness::Spinning => "⟳",
        Liveness::FileTouched => glyph(crate::glyphs::Glyph::InFlight),
        Liveness::None => " ",
        Liveness::Unknown => "?",
    }
}

/// STORY-58: render the session-list table using caller-supplied widths
/// (so two grouped sections can share one column layout).
/// trace:STORY-58 | ai:claude
fn print_table_with_widths(sessions: &[SessionMeta], w: &TableWidths) {
    println!(
        "{}",
        format!(
            " {:<id_w$}  {:<age_w$}  {:<pid_w$}  {:<tty_w$}  {:<agent_w$}  {:<role_w$}  {:<spec_w$}  {:<term_w$}  {:<wt_w$}  {}",
            "ID",
            "AGE",
            "PID",
            "TTY",
            "AGENT",
            "ROLE",
            "SPEC",
            "TERMINAL",
            "WORKTREE",
            "RECENT FOCUS",
            id_w = w.id_w,
            age_w = w.age_w,
            pid_w = w.pid_w,
            tty_w = w.tty_w,
            agent_w = w.agent_w,
            role_w = w.role_w,
            spec_w = w.spec_w,
            term_w = w.terminal_w,
            wt_w = w.worktree_w,
        )
        .dimmed()
    );

    for s in sessions {
        let id_short = &s.id[..s.id.len().min(w.id_w)];
        let age = age_cell(s);
        let pid = s.process.pid_cell();
        let tty = s.process.tty_cell();
        let role = s.role.as_deref().unwrap_or("-");
        let spec = s.spec.as_deref().unwrap_or("-");
        let terminal = crate::agent_registry::terminal_cell(s.terminal.as_ref(), None);
        let focus = s.recent_focus.as_deref().unwrap_or("-");
        let worktree =
            format_worktree_label(s.last_cwd.as_deref(), s.branch.as_deref(), w.worktree_w);
        // STORY-993: three-state glyph — bright green only for a resolved,
        // alive process; yellow for file-touched; dim blank otherwise.
        let live = process_liveness_glyph(s);
        let live_colored = match s.process.liveness {
            crate::session_liveness::Liveness::Alive => live.green().bold().to_string(),
            crate::session_liveness::Liveness::Spinning => live.yellow().bold().to_string(),
            crate::session_liveness::Liveness::FileTouched => live.yellow().to_string(),
            crate::session_liveness::Liveness::None => live.dimmed().to_string(),
            crate::session_liveness::Liveness::Unknown => live.yellow().to_string(),
        };
        println!(
            "{} {:<id_w$}  {:<age_w$}  {:<pid_w$}  {:<tty_w$}  {:<agent_w$}  {:<role_w$}  {:<spec_w$}  {:<term_w$}  {:<wt_w$}  {}",
            live_colored,
            id_short.bold(),
            age,
            pid,
            tty,
            s.agent.as_str().blue(),
            role.yellow(),
            spec.cyan(),
            terminal.cyan(),
            worktree.cyan(),
            focus.cyan(),
            id_w = w.id_w,
            age_w = w.age_w,
            pid_w = w.pid_w,
            tty_w = w.tty_w,
            agent_w = w.agent_w,
            role_w = w.role_w,
            spec_w = w.spec_w,
            term_w = w.terminal_w,
            wt_w = w.worktree_w,
        )
    }
}

/// STORY-59: render `<basename> @ <branch>` with truncation. Empty/no
/// cwd shows a dash; missing branch shows just the basename.
/// trace:STORY-59 | ai:claude
fn format_worktree_label(cwd: Option<&str>, branch: Option<&str>, max: usize) -> String {
    let Some(cwd) = cwd else {
        return "-".to_string();
    };
    let basename = std::path::Path::new(cwd)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(cwd);
    let label = match branch {
        Some(b) => format!("{} @ {}", basename, b),
        None => basename.to_string(),
    };
    if label.chars().count() <= max {
        return label;
    }
    let mut out: String = label.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

pub fn format_role_resume_session_line(m: &SessionMeta) -> String {
    format!(
        "{} {:<8}  {:<6}  {:<11}  {:<11}  {:<12}  {:<20}  {:<24}  {}",
        liveness_indicator(m.age_seconds),
        &m.id[..m.id.len().min(8)],
        humanize_age(m.age_seconds),
        m.agent,
        m.role.as_deref().unwrap_or("-"),
        m.spec.as_deref().unwrap_or("-"),
        crate::agent_registry::terminal_cell(m.terminal.as_ref(), None),
        format_worktree_label(m.last_cwd.as_deref(), m.branch.as_deref(), 24),
        m.title.as_deref().unwrap_or("(untitled)"),
    )
}

fn humanize_age(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else if secs < 7 * 86_400 {
        format!("{}d", secs / 86_400)
    } else {
        format!("{}w", secs / (7 * 86_400))
    }
}
