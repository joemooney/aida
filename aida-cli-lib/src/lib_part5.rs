/// SPIKE-30 helper: render a compact status chip from a Claude agent entry.
/// Uses `status` when set; falls back to `kind` (e.g. "interactive") when
/// not — the live JSON for a freshly-spawned session typically omits status
/// for the first few seconds.
pub(crate) fn claude_status_chip(entry: &claude_agents::ClaudeAgentEntry) -> String {
    let label = entry.status.as_deref().unwrap_or(entry.kind.as_str());
    let painted = match label {
        "busy" | "working" | "Working" => label.green().to_string(),
        "idle" | "Idle" => label.dimmed().to_string(),
        "blocked" | "Needs input" => label.yellow().to_string(),
        "failed" | "Failed" => label.red().to_string(),
        _ => label.normal().to_string(),
    };
    format!("{} {}", crate::glyph(crate::glyphs::Glyph::Bullet), painted)
}

pub(crate) fn print_status_short(ctx: &UserStatusContext) {
    let role = ctx.role.as_deref().unwrap_or("-");
    let scope = ctx
        .session
        .as_ref()
        .map(|l| l.scope.as_str())
        .unwrap_or("-");
    let branch = ctx.branch.as_ref().map(|b| b.name.as_str()).unwrap_or("-");
    let dirty = ctx
        .branch
        .as_ref()
        .map(|b| if b.dirty { "*" } else { "" })
        .unwrap_or("");
    let queue = ctx.queue_total;
    let pr_chip = match &ctx.pr {
        Some(p) if matches!(p.gh_status, GhStatus::Ok) && p.number > 0 => {
            let ci = p
                .ci_rollup
                .as_deref()
                .map(|r| {
                    if r == "SUCCESS" {
                        format!("ci{}", crate::glyph(crate::glyphs::Glyph::Check))
                    } else if r.contains("FAIL") || r.contains("CANCEL") {
                        format!("ci{}", crate::glyph(crate::glyphs::Glyph::Cross))
                    } else if r.contains("PENDING") || r.contains("IN_PROGRESS") {
                        "ci⏱".to_string()
                    } else {
                        "ci?".to_string()
                    }
                })
                .unwrap_or_else(|| "ci?".to_string());
            format!(" PR#{} {}", p.number, ci)
        }
        _ => String::new(),
    };
    println!(
        "role:{} scope:{} branch:{}{} queue:{}{}",
        role, scope, branch, dirty, queue, pr_chip
    );
}

pub(crate) fn print_status_json(
    ctx: &UserStatusContext,
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
    project_root: &std::path::Path,
    queue_only: bool,
    ci_only: bool,
    awaiting: &awaiting_you::AwaitingReport,
    absence: Option<serde_json::Value>,
) -> Result<()> {
    use serde_json::json;

    let session = ctx.session.as_ref().map(|l| {
        json!({
            "id": l.id,
            "scope": l.scope,
            "branch": l.branch,
            "worktree": l.worktree_path.display().to_string(),
            "role": l.role,
            "started_at": l.started_at.to_rfc3339(),
        })
    });
    let branch = ctx.branch.as_ref().map(|b| {
        json!({
            "name": b.name,
            "dirty": b.dirty,
            "ahead_main": b.ahead_main,
            "behind_main": b.behind_main,
            "ahead_upstream": b.ahead_upstream,
            "behind_upstream": b.behind_upstream,
            "has_upstream": b.has_upstream,
        })
    });
    let pr = ctx.pr.as_ref().map(|p| match &p.gh_status {
        GhStatus::Ok if p.number > 0 => json!({
            "number": p.number,
            "title": p.title,
            "url": p.url,
            "state": p.state,
            "ci_rollup": p.ci_rollup,
        }),
        GhStatus::Ok => json!({ "state": "none" }),
        GhStatus::Missing => json!({ "error": "gh-missing" }),
        GhStatus::Failed(r) => json!({ "error": "gh-failed", "reason": r }),
        GhStatus::Skipped => json!({ "skipped": true }),
        // BUG-560: non-GitHub remote — report the forge, not a gh error.
        GhStatus::NotGitHub(kind) => json!({
            "forge": kind.config_token(),
            "note": "PR/CI section is GitHub-only today; the requirement store works fully",
        }),
    });
    let head_json = |r: &QueueRow| {
        let mut obj = json!({
            "spec_id": r.spec_id,
            "title": r.title,
            "status": r.status,
            "for_role": r.for_role,
            "in_progress": r.in_progress,
        });
        if r.in_progress {
            if let Some(id) = &r.lease_id {
                obj["lease_id"] = json!(id);
            }
            if let Some(started) = r.lease_started_at {
                obj["lease_started_at"] = json!(started.to_rfc3339());
            }
        }
        obj
    };
    let queue_split = split_queue_view(&ctx.queue_head, ctx.queue_total);
    let queue = json!({
        "role": ctx.role,
        "total": ctx.queue_total,
        "in_progress_count": queue_split.in_progress_total,
        "next_up_count": queue_split.next_up_total,
        "head": ctx.queue_head.iter().map(head_json).collect::<Vec<_>>(),
    });

    let mut out = serde_json::Map::new();
    if !queue_only && !ci_only {
        // STORY-465: lead the JSON the same way the text view leads —
        // human-gate items first. Always present (even when empty) so
        // consumers can detect "no items" without a key-absence guard.
        let absence_json = absence.unwrap_or_else(|| {
            json!({
                "absence_days": serde_json::Value::Null,
                "last_activity_at": serde_json::Value::Null,
            })
        });
        out.insert(
            "absence_days".to_string(),
            absence_json
                .get("absence_days")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        );
        out.insert(
            "last_activity_at".to_string(),
            absence_json
                .get("last_activity_at")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        );
        out.insert("absence".to_string(), absence_json);
        out.insert("awaiting".to_string(), awaiting.to_json());
        out.insert(
            "session".to_string(),
            session.unwrap_or(serde_json::Value::Null),
        );
        out.insert(
            "role".to_string(),
            serde_json::Value::from(ctx.role.clone()),
        );
        out.insert(
            "branch".to_string(),
            branch.unwrap_or(serde_json::Value::Null),
        );
    }
    if !queue_only {
        out.insert("pr".to_string(), pr.unwrap_or(serde_json::Value::Null));
    }
    if !ci_only {
        out.insert("queue".to_string(), queue);
    }
    if !queue_only && !ci_only {
        out.insert(
            "agents".to_string(),
            json!(ctx
                .agents
                .iter()
                .map(|a| {
                    json!({
                        "id": a.id,
                        "agent_type": a.agent_type,
                        "pid": a.pid,
                        "tty": a.tty,
                        "started_at": a.started_at,
                        "last_active_at": a.last_active_at,
                        "role": a.role,
                        "current_spec": a.current_spec,
                        "status": a.status.as_str(),
                        "worktree_path": a.worktree_path.display().to_string(),
                        "source": a.source,
                        "binary_version": a.binary_version,
                        "build_sha": a.build_sha,
                    })
                })
                .collect::<Vec<_>>()),
        );
        let cache = backend.cache();
        let recorded = cache.source_head_sha().ok().flatten().unwrap_or_default();
        let actual = aida_core::git_ops::head_sha(store_path).unwrap_or_default();
        let stale = recorded != actual || recorded.is_empty();
        out.insert(
            "cache".to_string(),
            json!({
                "fresh": !stale || actual.is_empty(),
                "rows": backend.requirement_count().unwrap_or(0),
            }),
        );

        // TASK-662: machine-readable findings detail (count + per-finding
        // rows), mirroring the text Findings section — draft-filtered so the
        // count matches `aida findings list`. trace:TASK-662 | ai:claude
        {
            let f_filter = aida_core::ListFilter {
                status: Some("draft".to_string()),
                ..Default::default()
            };
            if let Ok(summaries) = backend.list_summaries(&f_filter) {
                let sections = crate::findings::build_findings_view(
                    &summaries,
                    &crate::findings::FindingsFilter::default(),
                );
                let total = crate::findings::count_findings(&sections);
                let mut items = Vec::new();
                let mut current_ids: Vec<String> = Vec::new();
                for section in &sections {
                    let source = section.source.label();
                    for group in &section.groups {
                        for row in &group.rows {
                            current_ids.push(row.display_id.clone());
                            items.push(json!({
                                "id": row.display_id,
                                "source": source,
                                "origin": group.origin,
                                "severity": row.severity.label(),
                                "kind": row.kind,
                                "recurrence": row.recurrence,
                                "title": row.title,
                            }));
                        }
                    }
                }
                // TASK-662: delta-since-last-run. Diff the current finding IDs
                // against the snapshot persisted on the previous `aida status`
                // run, then re-baseline. `delta` is null on the very first run
                // (no prior snapshot to diff). trace:TASK-662 | ai:claude
                let previous = read_last_findings(project_root);
                let delta = compute_findings_delta(previous.as_deref(), &current_ids);
                write_last_findings(project_root, &current_ids);
                let delta_json = match &delta {
                    Some(d) => serde_json::to_value(d).unwrap_or(serde_json::Value::Null),
                    None => serde_json::Value::Null,
                };
                out.insert(
                    "findings".to_string(),
                    json!({ "pending": total, "items": items, "delta": delta_json }),
                );
            }
        }

        // SPIKE-30: emit the same Claude Code cross-substrate view machine-
        // readable when the operator passes `--json`. Absent when the
        // `claude` binary isn't on PATH — graceful no-op, not an error key.
        if let Some(value) = claude_code_status_json() {
            out.insert("claude_code".to_string(), value);
        }

        // STORY-456: unified worktrees + open-PRs + recently-merged panes,
        // mirroring the text sections so machine consumers get the same merge
        // (worktree + lease + liveness + ahead + PR + obsolescence verdict).
        // Always present (possibly empty arrays) so consumers needn't guard on
        // key-absence. trace:STORY-456 | ai:claude
        let main_root = main_worktree_root_from(project_root);
        let worktree_rows = collect_worktree_status_rows(&main_root);
        out.insert(
            "worktrees".to_string(),
            json!(worktree_rows
                .iter()
                .map(|r| json!({
                    "path": r.path.display().to_string(),
                    "branch": r.branch,
                    "tied_spec": r.tied_spec,
                    "lease_scope": r.lease_scope,
                    "live": r.has_live,
                    "dirty_count": r.dirty_count,
                    "ahead_main": r.ahead,
                    "pr_number": r.pr_number,
                    "pr_ci": r.pr_ci,
                    "pr_mergeable": r.pr_mergeable,
                    "obsolete": r.obsolete,
                }))
                .collect::<Vec<_>>()),
        );
        let open_pr_snapshot = collect_open_prs(&main_root);
        let mut open_prs: Vec<&status_cleanup::OpenPrItem> =
            open_pr_snapshot.by_branch.values().collect();
        open_prs.sort_by_key(|p| p.number);
        out.insert(
            "open_prs".to_string(),
            json!(open_prs
                .iter()
                .map(|p| {
                    let merge = p.mergeable.as_deref().unwrap_or("UNKNOWN");
                    json!({
                        "number": p.number,
                        "title": p.title,
                        "head_branch": p.head_branch,
                        "ci": p.ci_rollup,
                        "mergeable": p.mergeable,
                        "review_decision": p.review_decision,
                        "next_step": open_pr_next_step(
                            p.ci_rollup.as_deref().unwrap_or("?"),
                            &merge.to_ascii_uppercase(),
                            p.review_decision.as_deref(),
                        ),
                    })
                })
                .collect::<Vec<_>>()),
        );
        out.insert(
            "recently_merged".to_string(),
            json!(collect_recently_merged_prs(&main_root, 5)
                .iter()
                .map(|(number, title, when)| json!({
                    "number": number,
                    "title": title,
                    "merged_at": when,
                }))
                .collect::<Vec<_>>()),
        );
    }
    println!("{}", crate::cache_output::json_pretty(&out)?);
    Ok(())
}

/// SPIKE-30 JSON projection: same cross-substrate view as
/// `print_status_claude_code_section`, structured for machine consumers.
/// Returns `None` (caller omits the key) when `claude` isn't available.
pub(crate) fn claude_code_status_json() -> Option<serde_json::Value> {
    use serde_json::json;

    let entries = claude_agents::list_agents()?;
    let project_root = std::env::current_dir().ok()?;
    let leases = list_leases(&project_root);
    let worktree_paths: Vec<std::path::PathBuf> =
        leases.iter().map(|l| l.worktree_path.clone()).collect();
    let (in_scope, elsewhere) =
        claude_agents::partition_by_project(&entries, &project_root, &worktree_paths);
    let manifests = session_manifest::list_all(&project_root);

    let mut linked = Vec::new();
    let mut shells = 0usize;
    let mut drift = Vec::new();
    for entry in &in_scope {
        let manifest = manifests
            .iter()
            .find(|m| m.claude_session_id.as_deref() == Some(entry.session_id.as_str()));
        let lease = manifest
            .and_then(|m| {
                leases
                    .iter()
                    .find(|l| l.id.starts_with(&m.session_id) || m.session_id.starts_with(&l.id))
            })
            .or_else(|| leases.iter().find(|l| l.worktree_path == entry.cwd));
        match lease {
            Some(l) => linked.push(json!({
                "session_id": entry.session_id,
                "pid": entry.pid,
                "cwd": entry.cwd.display().to_string(),
                "kind": entry.kind,
                "status": entry.status,
                "started_at_ms": entry.started_at_ms,
                "lease_id": l.id,
                "scope": l.scope,
                "role": l.role,
                "branch": l.branch,
            })),
            None if entry.cwd == project_root => shells += 1,
            None => drift.push(json!({
                "session_id": entry.session_id,
                "pid": entry.pid,
                "cwd": entry.cwd.display().to_string(),
                "kind": entry.kind,
                "status": entry.status,
                "started_at_ms": entry.started_at_ms,
            })),
        }
    }

    Some(json!({
        "live_in_project": in_scope.len(),
        "live_elsewhere": elsewhere.len(),
        "linked": linked,
        "interactive_shells_in_root": shells,
        "drift_in_worktree": drift,
    }))
}

#[cfg(test)]
#[path = "tests/bug_560_status_forge_tests.rs"]
mod bug_560_status_forge_tests;

#[cfg(test)]
#[path = "tests/task_490_status_in_progress_tests.rs"]
mod task_490_status_in_progress_tests;

/// One record from `git worktree list --porcelain`. The main worktree is
/// always the first record; linked worktrees follow. Detached worktrees
/// produce `None` for `branch`.
/// trace:STORY-385 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct WorktreeRecord {
    pub(crate) path: std::path::PathBuf,
    pub(crate) branch: Option<String>,
}

/// Parse `git worktree list --porcelain` output. One record per worktree,
/// separated by blank lines.
pub(crate) fn list_worktrees(project_root: &std::path::Path) -> Vec<WorktreeRecord> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["worktree", "list", "--porcelain"])
        .output();
    let Ok(out) = out else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    let mut records = Vec::new();
    let mut cur_path: Option<std::path::PathBuf> = None;
    let mut cur_branch: Option<String> = None;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            // Flush previous record if any.
            if let Some(path) = cur_path.take() {
                records.push(WorktreeRecord {
                    path,
                    branch: cur_branch.take(),
                });
            }
            cur_path = Some(std::path::PathBuf::from(p));
        } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
            cur_branch = Some(b.to_string());
        }
    }
    if let Some(path) = cur_path {
        records.push(WorktreeRecord {
            path,
            branch: cur_branch,
        });
    }
    records
}

/// Open-PR snapshot keyed by head branch — one `gh` invocation feeds the
/// "open PRs" and "branches ahead with no PR" detectors. Empty when gh is
/// missing or the call fails (every dependent detector silently
/// degrades).
/// trace:STORY-385 | ai:claude
#[derive(Debug, Clone, Default)]
pub(crate) struct OpenPrSnapshot {
    pub(crate) by_branch: std::collections::HashMap<String, status_cleanup::OpenPrItem>,
}

pub(crate) fn collect_open_prs(project_root: &std::path::Path) -> OpenPrSnapshot {
    // BUG-613: one `aida status` resolves the open-PR snapshot from three
    // sections (the JSON `open_prs` block, the worktree-rows PR merge, and the
    // awaiting-you mergeable filter), each previously firing its own
    // `gh pr list` — ~0.4s of network latency apiece. PR state does not change
    // within a single short-lived `status` run, so memoize the snapshot for the
    // process lifetime, keyed by the canonicalized repo root (so the rare
    // cross-repo caller still gets the right answer). trace:BUG-613 | ai:claude
    use std::sync::Mutex;
    use std::sync::OnceLock;
    static CACHE: OnceLock<Mutex<std::collections::HashMap<std::path::PathBuf, OpenPrSnapshot>>> =
        OnceLock::new();
    let key = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Ok(guard) = cache.lock() {
        if let Some(hit) = guard.get(&key) {
            return hit.clone();
        }
    }
    let snapshot = collect_open_prs_uncached(project_root);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(key, snapshot.clone());
    }
    snapshot
}

/// BUG-613: the uncached single-`gh pr list` snapshot fetch behind the
/// process-lifetime memo in [`collect_open_prs`]. trace:BUG-613 | ai:claude
pub(crate) fn collect_open_prs_uncached(project_root: &std::path::Path) -> OpenPrSnapshot {
    // TASK-833: mirror the `collect_pr_facts` forge-aware degrade (BUG-560) — on
    // a non-GitHub forge `gh` errors with a raw "not a known GitHub host" auth
    // message; skip the spawn entirely and degrade to an empty snapshot so every
    // dependent surface (status-cleanup detectors + the burndown-status open-PR
    // section) renders nothing rather than leaking that error.
    // trace:TASK-833 | ai:claude
    // BUG-1454 F3, DECIDED AND KEPT: this empty-on-unknown is the same SHAPE as
    // the `specs_with_open_prs` defect, but not the same consequence, so it is
    // deliberately left alone rather than widened into an Option.
    //
    // The distinction that matters is what a consumer DOES with emptiness, not
    // whether emptiness is ambiguous. Every consumer here degrades toward
    // silence in a surface that only informs: `active_reviewer_unmerged_pr`
    // (STORY-127) withholds a WARNING, the release check at the STORY-127
    // detector withholds a WARNING, and the `aida status` open-PR section
    // renders no rows. None of them change a spec's state. The defect in
    // `specs_with_open_prs` was that emptiness AUTHORISED an irreversible
    // Done -> Completed bump, hiding unfinished work.
    //
    // Note also that this default is already returned on four paths (non-GitHub
    // forge, gh missing, gh failure, unparseable output), so the forge guard
    // joins an existing conflation rather than creating one; typing "unknown"
    // here would mean typing all four and every consumer.
    // trace:BUG-1454 | ai:claude
    if forge::resolve_forge_kind(project_root) != forge::ForgeKind::GitHub {
        return OpenPrSnapshot::default();
    }
    let gh_bin = match resolve_gh_binary() {
        Some(p) => p,
        None => return OpenPrSnapshot::default(),
    };
    let mut cmd = std::process::Command::new(&gh_bin);
    cmd.current_dir(project_root).args([
        "pr",
        "list",
        "--state",
        "open",
        "--limit",
        "50",
        "--json",
        "number,title,headRefName,headRefOid,statusCheckRollup,mergeable,reviewDecision,labels,createdAt",
    ]);
    // trace:BUG-1288 | ai:claude — bounded like every other gh call this
    // machine-readable pipeline makes; see FORGE_CLI_CALL_TIMEOUT.
    let Some(out) = command_output_with_timeout(cmd, FORGE_CLI_CALL_TIMEOUT) else {
        return OpenPrSnapshot::default();
    };
    if !out.status.success() {
        return OpenPrSnapshot::default();
    }
    // BUG-1481: fetch the base branch's required status checks once per
    // snapshot and thread them through so `ci_rollup` can't read "pass" off
    // a head that is simply missing a required check's row entirely.
    // trace:BUG-1481 | ai:claude
    let required = required_status_checks(project_root);
    parse_open_pr_snapshot(&String::from_utf8_lossy(&out.stdout), required.as_deref())
}

/// BUG-1481: the base branch's required status-check names (branch
/// protection's `required_status_checks.contexts`), so `ci_rollup` can tell
/// "nothing is required" apart from "a required check never showed up on
/// this head". `None` means the set itself could not be determined — gh
/// missing, offline, or no permission to read protection — and callers must
/// treat that as *unknown*, never as "nothing required" (PRIN-5: absent
/// evidence is not good evidence). A branch that is genuinely unprotected
/// (the API's 404 "Branch not protected") maps to `Some(vec![])`, which is a
/// real, positive answer, not an unreadable one. Cached once per process per
/// project root, mirroring `collect_open_prs`'s BUG-613 memo, so one
/// `aida awaiting` run pays for this API call once regardless of PR count.
// trace:BUG-1481 | ai:claude
pub(crate) fn required_status_checks(project_root: &std::path::Path) -> Option<Vec<String>> {
    use std::sync::Mutex;
    use std::sync::OnceLock;
    static CACHE: OnceLock<
        Mutex<std::collections::HashMap<std::path::PathBuf, Option<Vec<String>>>>,
    > = OnceLock::new();
    let key = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Ok(guard) = cache.lock() {
        if let Some(hit) = guard.get(&key) {
            return hit.clone();
        }
    }
    let result = required_status_checks_uncached(project_root);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(key, result.clone());
    }
    result
}

/// The uncached lookup behind [`required_status_checks`].
// trace:BUG-1481 | ai:claude
pub(crate) fn required_status_checks_uncached(
    project_root: &std::path::Path,
) -> Option<Vec<String>> {
    // Same forge guard as `collect_open_prs_uncached`: no gh-shaped
    // protection API to ask on a non-GitHub forge, so there is nothing to
    // treat as "required" — legitimately empty, not unknown.
    if forge::resolve_forge_kind(project_root) != forge::ForgeKind::GitHub {
        return Some(Vec::new());
    }
    let gh_bin = resolve_gh_binary()?;
    let default_branch = detect_default_branch_ref(project_root)
        .and_then(|r| r.rsplit('/').next().map(str::to_string))
        .unwrap_or_else(|| "main".to_string());
    let mut cmd = std::process::Command::new(&gh_bin);
    cmd.current_dir(project_root).args([
        "api",
        &format!("repos/{{owner}}/{{repo}}/branches/{default_branch}/protection"),
        "--jq",
        ".required_status_checks.contexts // []",
    ]);
    // BUG-1288: this specific call measured 10.5s of wall clock in this
    // repository — a single slow branch-protection lookup was enough to blow
    // the whole `aida awaiting --json` / `aida status --full` budget on its
    // own. `None` here degrades exactly like every other unreachable-forge
    // path this function already handles (PRIN-5: unknown, not "nothing
    // required"). trace:BUG-1288 | ai:claude
    let out = command_output_with_timeout(cmd, FORGE_CLI_CALL_TIMEOUT)?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return required_status_checks_outcome_from_stderr(&stderr);
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    Some(
        parsed
            .as_array()?
            .iter()
            .filter_map(|v| v.as_str())
            .map(str::to_string)
            .collect(),
    )
}

/// BUG-1481: classify a failed `gh api .../protection` call's stderr into
/// "nothing is required" vs "unknown". GitHub returns HTTP 404 for BOTH a
/// genuinely unprotected branch (body `{"message":"Branch not protected",...}`)
/// AND a protected branch the caller lacks permission to read protection on
/// (body `{"message":"Not Found",...}`) — so a bare "404"/"not found"
/// substring match conflates the two and can silently report a protected
/// branch as having no required checks (the exact PR-2009 false-green shape).
/// Only the literal "Branch not protected" message is a real, positive
/// "nothing required" answer; everything else (permission denied, network
/// failure, the ambiguous plain "not found") is genuinely unknown and must
/// never be treated as "nothing required" (PRIN-5: absent evidence is not
/// good evidence).
// trace:BUG-1481 | ai:claude
pub(crate) fn required_status_checks_outcome_from_stderr(stderr: &str) -> Option<Vec<String>> {
    let stderr = stderr.to_ascii_lowercase();
    // prose-ok: matches gh's literal 'Branch not protected' answer (BUG-1481)
    if stderr.contains("branch not protected") {
        return Some(Vec::new());
    }
    None
}

/// BUG-1291: bounded safety net for runs killed before their normal reviewer
/// handoff. The scheduler tick calls this once over the forge's already-bounded
/// open-PR list. It only claims review work; it never reviews or merges.
// trace:BUG-1291 trace:TASK-1284 | ai:codex
pub(crate) fn sweep_orphaned_reviews(project_root: &std::path::Path) -> Vec<String> {
    let live_branches = live_owned_branches(project_root);

    let mut lines = Vec::new();
    for pr in collect_open_prs_uncached(project_root)
        .by_branch
        .into_values()
    {
        let clean = pr.mergeable.as_deref() == Some("MERGEABLE");
        let green = matches!(pr.ci_rollup.as_deref(), None | Some("pass"));
        let no_verdict =
            pr.review_decision.is_none() && !pr_has_local_verdict(project_root, pr.number);
        let no_hold = !pr_has_merge_hold(project_root, &pr);
        let unowned = !live_branches.contains(&pr.head_branch);
        if !orphaned_review_is_claimable(clean, green, no_verdict, no_hold, unowned) {
            continue;
        }

        let outcome = try_auto_queue_pr_review(
            project_root,
            &pr.head_branch,
            "orphan-sweep",
            AutoQueueOrigin::PrSkill,
        );
        if matches!(
            outcome.status,
            AutoQueueStatus::Filed | AutoQueueStatus::AlreadyExists
        ) {
            // Filed now means queue insertion succeeded; AlreadyExists means
            // reviewer_queue_story_ids verified an existing queue owner.
            lines.push(format!(
                "schedule tick: claimed orphaned PR #{} for reviewer",
                pr.number
            ));
        } else if matches!(outcome.status, AutoQueueStatus::SkippedNeedsAttention) {
            lines.push(format!(
                "schedule tick: orphaned PR #{} still unclaimed: {}",
                pr.number, outcome.summary
            ));
        }
    }
    lines
}

// TASK-192: shared ownership fact for the green orphan-review sweep and the
// red unowned-repair awaiting channel. A lease is ownership only while either
// recorded process identity is still alive.
// trace:TASK-192 | ai:codex
pub(crate) fn live_owned_branches(
    project_root: &std::path::Path,
) -> std::collections::HashSet<String> {
    list_leases(project_root)
        .into_iter()
        .filter(|lease| {
            lease.active_pid.is_some_and(|pid| {
                process_probe::process_identity_is_alive(
                    pid,
                    lease.active_pid_start_time.as_deref(),
                )
            }) || lease.creator_pid.is_some_and(|pid| {
                process_probe::process_identity_is_alive(
                    pid,
                    lease.creator_pid_start_time.as_deref(),
                )
            })
        })
        .map(|lease| lease.branch)
        .filter(|branch| !branch.is_empty())
        .collect()
}

// trace:BUG-1291 | ai:codex
pub(crate) fn orphaned_review_is_claimable(
    clean: bool,
    green: bool,
    no_verdict: bool,
    no_hold: bool,
    unowned: bool,
) -> bool {
    clean && green && no_verdict && no_hold && unowned
}

#[cfg(test)]
mod bug_1291_orphan_sweep_tests {
    use super::*;

    fn write_verdict(root: &std::path::Path, verdict: &str) {
        let dir = root.join(".aida/review-verdicts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("PR-1970.json"),
            format!(r#"{{"verdict":"{verdict}"}}"#),
        )
        .unwrap();
    }

    #[test]
    fn every_valid_local_verdict_blocks_historical_pr_1970_requeue() {
        for verdict in ["Approved", "RequestChanges", "Rejected"] {
            let root = tempfile::tempdir().unwrap();
            write_verdict(root.path(), verdict);
            assert!(
                pr_has_local_verdict(root.path(), 1970),
                "{verdict} must keep PR-1970 / STORY-1354 out of the orphan sweep"
            );
        }
    }

    #[test]
    fn sweep_predicate_requires_no_hold_and_no_live_owner() {
        assert!(orphaned_review_is_claimable(true, true, true, true, true));
        assert!(!orphaned_review_is_claimable(true, true, true, false, true));
        assert!(!orphaned_review_is_claimable(true, true, true, true, false));
    }

    #[test]
    fn malformed_or_missing_verdict_remains_sweep_eligible() {
        let root = tempfile::tempdir().unwrap();
        assert!(!pr_has_local_verdict(root.path(), 1970));
        let dir = root.path().join(".aida/review-verdicts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("PR-1970.json"), "not-json").unwrap();
        assert!(!pr_has_local_verdict(root.path(), 1970));
    }

    // BUG-1549: end-to-end through the production readers — verdict files on
    // disk (spec-keyed for every title spec + PR-keyed) → `local_suppressed_prs`
    // → `classify_open_prs`, and the same PRs → `pr_review_rows`. The rule
    // itself is table-tested in `awaiting_you::tests::classify_pr_review_table`;
    // this pins the plumbing and the suppressed-iff-row invariant at the
    // report surface.
    // trace:BUG-1490 trace:BUG-1549 | ai:claude
    fn write_verdict_at(root: &std::path::Path, key: &str, verdict: &str, sha: &str, at: &str) {
        let dir = root.join(".aida/review-verdicts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{key}.json")),
            format!(r#"{{"verdict":"{verdict}","reviewed_sha":"{sha}","recorded_at":"{at}"}}"#),
        )
        .unwrap();
    }

    fn open_pr(number: u64, title: &str, head_sha: Option<&str>) -> status_cleanup::OpenPrItem {
        status_cleanup::OpenPrItem {
            number,
            is_draft: false,
            title: title.to_string(),
            head_branch: format!("claude/pr-{number}"),
            ci_rollup: Some("pass".to_string()),
            mergeable: Some("MERGEABLE".to_string()),
            review_decision: None,
            head_sha: head_sha.map(str::to_string),
            labels: Vec::new(),
            created_at: None,
        }
    }

    #[test]
    fn pr_review_suppression_and_rows_agree_end_to_end() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        let head = "cafef00d0000000001";
        let old = "deadbeef0000000001";
        let (t1, t2) = ("2026-09-01T00:00:00+00:00", "2026-09-02T00:00:00+00:00");

        // 5001: RequestChanges at head (spec-keyed) → blocked.
        write_verdict_at(r, "BUG-5001", "request-changes", head, t1);
        // 5002: refusal moved past → mergeable, rework-ready row.
        write_verdict_at(r, "BUG-5002", "request-changes", old, t1);
        // 5003: spec-keyed refusal t1 superseded by PR-keyed approval at head t2.
        write_verdict_at(r, "BUG-5003", "request-changes", head, t1);
        write_verdict_at(r, "PR-5003", "approved", head, t2);
        // 5004: two title specs — first approves at head t1, SECOND refuses sha-less
        // at t2 (newer, so not superseded).
        write_verdict_at(r, "BUG-5004", "approved", head, t1);
        write_verdict_at(r, "BUG-5014", "request-changes", "", t2);
        // 5005: spec-keyed approval at head t1, PR-keyed sha-less approval t2.
        write_verdict_at(r, "BUG-5005", "approved", head, t1);
        write_verdict_at(r, "PR-5005", "approved", "", t2);
        // 5006: stale approval.
        write_verdict_at(r, "BUG-5006", "approved", old, t1);
        // 5007: approval at head → mergeable, no row.
        write_verdict_at(r, "BUG-5007", "approved", head, t1);
        // 5008: head-less PR with a sha'd approval → unverifiable.
        write_verdict_at(r, "BUG-5008", "approved", head, t1);
        // 5009: no verdict at all → mergeable, no row.

        let prs = vec![
            open_pr(5001, "fix: a (BUG-5001)", Some(head)),
            open_pr(5002, "fix: b (BUG-5002)", Some(head)),
            open_pr(5003, "fix: c (BUG-5003)", Some(head)),
            open_pr(5004, "fix: d (BUG-5004) (BUG-5014)", Some(head)),
            open_pr(5005, "fix: e (BUG-5005)", Some(head)),
            open_pr(5006, "fix: f (BUG-5006)", Some(head)),
            open_pr(5007, "fix: g (BUG-5007)", Some(head)),
            open_pr(5008, "fix: h (BUG-5008)", None),
            open_pr(5009, "fix: i (BUG-5009)", Some(head)),
        ];

        let suppressed = local_suppressed_prs(r, &prs);
        let mut want: Vec<u64> = vec![5001, 5004, 5005, 5006, 5008];
        let mut got: Vec<u64> = suppressed.iter().copied().collect();
        got.sort_unstable();
        want.sort_unstable();
        assert_eq!(got, want, "suppressed set");

        let mergeable: Vec<u64> = awaiting_you::classify_open_prs(&prs, &suppressed)
            .iter()
            .map(|m| m.number)
            .collect();
        assert_eq!(mergeable, vec![5002, 5003, 5007, 5009], "mergeable set");

        // Rows under a seat that recorded nothing: explaining rows are never
        // seat-filtered, so the invariant holds at the report surface.
        let rows = pr_review_rows(r, &prs, Some("nobody-in-particular"));
        let mut explained: Vec<u64> = rows
            .blocked_reviews
            .iter()
            .map(|b| b.pr)
            .chain(rows.stale_approvals.iter().map(|s| s.pr))
            .collect();
        explained.sort_unstable();
        assert_eq!(explained, want, "suppressed iff an explaining row exists");

        let blocked = |n: u64| {
            rows.blocked_reviews
                .iter()
                .find(|b| b.pr == n)
                .unwrap()
                .reason
        };
        assert_eq!(blocked(5001), awaiting_you::BlockedReason::AtHead);
        assert_eq!(blocked(5004), awaiting_you::BlockedReason::Unverifiable);
        let stale = |n: u64| {
            rows.stale_approvals
                .iter()
                .find(|s| s.pr == n)
                .unwrap()
                .reason
        };
        assert_eq!(stale(5005), awaiting_you::StaleApprovalReason::Unverifiable);
        assert_eq!(stale(5006), awaiting_you::StaleApprovalReason::Stale);
        assert_eq!(stale(5008), awaiting_you::StaleApprovalReason::Unverifiable);
        assert_eq!(
            rows.rework_ready.iter().map(|w| w.pr).collect::<Vec<_>>(),
            vec![5002],
            "a moved-only refusal keeps its rework-ready row"
        );
    }

    #[cfg(unix)]
    #[test]
    fn real_phase_driver_shelve_handoff_makes_story_1354_claimable() {
        let root = tempfile::tempdir().unwrap();
        let fake_aida = root.path().join("aida");
        let script = r#"#!/bin/sh
if [ "$1" = "list" ]; then
  printf '%s\n' '[{"spec_id":"STORY-1354","title":"Review PR-1970: BUG-1268 stale-base recovery","status":"Approved"}]'
  exit 0
fi
if [ "$1" = "queue" ] && [ "$2" = "add" ]; then
  printf '%s\n' "$3" > .claimed-review
  exit 0
fi
exit 1
"#;
        crate::test_exec::write_executable(&fake_aida, script);

        let mut driver = RealPhaseDriver::new(
            root.path().to_path_buf(),
            "BUG-1268".into(),
            "reviewer-test".into(),
            None,
            true,
            None,
            AutonomyMode::Default,
            "bug-1291-test".into(),
            false,
            false,
            false,
            false,
            auto_complete::LifecycleSkip::none(),
            auto_complete::AutoCompleteVariant::Full,
        );
        driver.aida_exe = fake_aida;
        driver.pr_number = Some(1970);

        auto_complete::PhaseDriver::handoff_open_pr_after_shelve(
            &mut driver,
            "BUG-1268",
            auto_complete::Phase::Reviewer,
            &auto_complete::PhaseFailure::new("reviewer phase shelved"),
        );

        assert_eq!(
            std::fs::read_to_string(root.path().join(".claimed-review"))
                .unwrap()
                .trim(),
            "STORY-1354",
            "the historical review story must be present in the reviewer queue after shelving"
        );
    }

    #[cfg(unix)]
    #[test]
    fn queue_add_failure_is_propagated() {
        let root = tempfile::tempdir().unwrap();
        let fake = root.path().join("failing-aida");
        crate::test_exec::write_executable(
            &fake,
            "#!/bin/sh\necho queue unavailable >&2\nexit 23\n",
        );
        let err = aida_subcmd_queue_add_for_reviewer_using(
            root.path(),
            "STORY-1354",
            "BUG-1291 test",
            &fake,
        )
        .unwrap_err();
        assert!(err.to_string().contains("queue unavailable"), "{err}");
    }

    /// AC3: the retry engages at *this* call site, not merely inside the helper's
    /// own unit test. A live writer descriptor on the freshly written fixture is a
    /// real `ETXTBSY` (the bare-spawn assertion below proves the window is open),
    /// which before BUG-1735 took the `Err(e)` arm and reported
    /// `could not invoke \`aida queue add\``. The retry must instead wait the
    /// window out and land on the `Ok(o)` non-zero-exit arm the caller cares
    /// about, so the assertion is on the *propagated stderr*, not on "no error".
    // trace:BUG-1735 | ai:claude
    #[cfg(target_os = "linux")]
    #[test]
    fn queue_add_waits_out_a_writer_descriptor_instead_of_reporting_could_not_invoke() {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;

        let body = "#!/bin/sh\necho queue unavailable >&2\nexit 23\n";
        let root = tempfile::tempdir().unwrap();
        let fake = root.path().join("failing-aida");
        crate::test_exec::write_executable(&fake, body);

        let mut writer = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(&fake)
            .unwrap();
        writer.write_all(body.as_bytes()).unwrap();
        writer.sync_all().unwrap();

        assert_eq!(
            std::process::Command::new(&fake)
                .spawn()
                .err()
                .and_then(|e| e.raw_os_error()),
            Some(libc::ETXTBSY),
            "the writer descriptor must make a bare spawn fail, or this test proves nothing"
        );

        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(80));
            drop(writer);
        });

        let err = aida_subcmd_queue_add_for_reviewer_using(
            root.path(),
            "STORY-1354",
            "BUG-1735 test",
            &fake,
        )
        .unwrap_err();
        release.join().unwrap();

        assert!(
            err.to_string().contains("queue unavailable"),
            "the spawn must be retried past ETXTBSY and reach the non-zero-exit arm, got: {err}"
        );
        assert!(
            !err.to_string().contains("could not invoke"),
            "a transient ETXTBSY must not surface as a spawn failure, got: {err}"
        );
    }
}

/// TASK-833: pure parse of a `gh pr list --json
/// number,title,headRefName,statusCheckRollup,mergeable,reviewDecision` payload
/// into an `OpenPrSnapshot`. Split out of `collect_open_prs` so it's
/// unit-testable without shelling out; malformed JSON / a missing `number`
/// field degrades silently (empty snapshot / skip the row).
///
/// `required_checks` (BUG-1481) is the base branch's required status-check
/// name set, from [`required_status_checks`]: `None` when it could not be
/// determined, `Some(list)` (possibly empty) when it's known. It only ever
/// pulls a `ci_rollup` of `"pass"` DOWN to `"missing"` (a required check's
/// row never showed up on this head) or `"unknown"` (the required set
/// itself couldn't be read) — it never turns a `"fail"`/`"pending"`/`"?"`
/// into something greener.
// trace:TASK-833 trace:BUG-1481 | ai:claude
pub(crate) fn parse_open_pr_snapshot(
    json: &str,
    required_checks: Option<&[String]>,
) -> OpenPrSnapshot {
    let parsed: serde_json::Value = match serde_json::from_str(json.trim()) {
        Ok(v) => v,
        Err(_) => return OpenPrSnapshot::default(),
    };
    let mut by_branch = std::collections::HashMap::new();
    for pr in parsed.as_array().cloned().unwrap_or_default() {
        let number = pr.get("number").and_then(|v| v.as_u64()).unwrap_or(0);
        if number == 0 {
            continue;
        }
        let title = pr
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let head_branch = pr
            .get("headRefName")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let head_sha = pr
            .get("headRefOid")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let mergeable = pr
            .get("mergeable")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let review_decision = pr
            .get("reviewDecision")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let ci_rollup = pr
            .get("statusCheckRollup")
            .and_then(|v| v.as_array())
            .map(|arr| summarize_status_check_rollup_with_required(arr, required_checks));
        let labels = pr
            .get("labels")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|label| label.get("name").and_then(|v| v.as_str()))
            .map(str::to_owned)
            .collect();
        // BUG-1514: same `gh pr list` call, no extra request — drives the
        // unowned-failing-PR age gate. Malformed/missing → None (fail open).
        // trace:BUG-1514 | ai:claude
        let created_at = pr
            .get("createdAt")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));
        by_branch.insert(
            head_branch.clone(),
            status_cleanup::OpenPrItem {
                is_draft: false,
                number,
                title,
                head_branch,
                ci_rollup,
                mergeable,
                review_decision,
                head_sha,
                labels,
                created_at,
            },
        );
    }
    OpenPrSnapshot { by_branch }
}

/// Every branch that has ever been a PR head, across all states
/// (open / closed / merged). Used by the branches-ahead-no-PR detector
/// to distinguish "never PR'd" branches from squash-merged ones that
/// git still considers ahead. Empty when gh is missing or the call
/// fails. trace:STORY-385 | ai:claude
pub(crate) fn collect_all_pr_head_branches(
    project_root: &std::path::Path,
) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    let gh_bin = match resolve_gh_binary() {
        Some(p) => p,
        None => return set,
    };
    let out = std::process::Command::new(&gh_bin)
        .current_dir(project_root)
        .args([
            "pr",
            "list",
            "--state",
            "all",
            "--limit",
            "500",
            "--json",
            "headRefName",
        ])
        .output_retrying_etxtbsy();
    let Ok(out) = out else { return set };
    if !out.status.success() {
        return set;
    }
    let parsed: serde_json::Value = match serde_json::from_slice(&out.stdout) {
        Ok(v) => v,
        Err(_) => return set,
    };
    for pr in parsed.as_array().cloned().unwrap_or_default() {
        if let Some(b) = pr.get("headRefName").and_then(|v| v.as_str()) {
            set.insert(b.to_string());
        }
    }
    set
}

#[derive(Debug, Clone)]
pub(crate) struct PrHeadEvidence {
    pub(crate) state: String,
    pub(crate) head_sha: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PrHeadStateSnapshot {
    pub(crate) by_branch: std::collections::HashMap<String, PrHeadEvidence>,
    pub(crate) open_heads_by_spec: std::collections::HashMap<String, Vec<String>>,
    pub(crate) merged_heads_by_spec: std::collections::HashMap<String, Vec<String>>,
    // BUG-1288: branches the per-branch `--head` query in
    // `collect_pr_head_state_snapshot_bounded` actually asked about. A branch
    // present here with no `by_branch` entry has an affirmatively CONFIRMED
    // absence of any PR ("absent"); a branch absent from both was never asked
    // (deadline-truncated) and must read "unknown", never "absent" — a
    // skipped check is not evidence of nothing (PRIN-5). trace:BUG-1288 | ai:claude
    pub(crate) queried_heads: std::collections::HashSet<String>,
}

// BUG-1576: query each bounded candidate by head name instead of sampling the
// first N PRs from repository history. This remains proportional to active
// work, while finding an arbitrarily old merged PR in a repository with more
// than 1,000 PRs. The recorded head SHA identifies the exact branch
// incarnation reviewed and merged; `git cherry` cannot prove an N-to-1 squash.
/// BUG-1288: the per-branch `gh pr list --head` loop below is one network
/// round trip per candidate, so a large candidate population can add several
/// real seconds even though each individual call is fast. `deadline`, when
/// set, stops issuing NEW per-branch queries once it passes; branches not yet
/// queried simply keep whatever `by_branch`/`open_heads_by_spec`/
/// `merged_heads_by_spec` state the initial batched "open" query already gave
/// them (state "unknown" downstream, never a false "absent" — PRIN-5). The
/// initial batched query always runs uncapped: it is one call regardless of
/// candidate count, so there is nothing to bound there. `deadline: None`
/// (every caller but the machine-readable awaiting/status paths) probes every
/// candidate exactly as before this change.
// trace:BUG-1288 | ai:claude
pub(crate) fn collect_pr_head_state_snapshot_bounded(
    project_root: &std::path::Path,
    candidate_branches: &[String],
    deadline: Option<std::time::Instant>,
) -> Option<PrHeadStateSnapshot> {
    let gh_bin = resolve_gh_binary()?;
    let query = |args: &[&str]| -> Option<PrHeadStateSnapshot> {
        let mut cmd = std::process::Command::new(&gh_bin);
        cmd.current_dir(project_root).args(args);
        // trace:BUG-1288 | ai:claude — bounded like every other gh call this
        // machine-readable pipeline makes; see FORGE_CLI_CALL_TIMEOUT.
        let out = command_output_with_timeout(cmd, FORGE_CLI_CALL_TIMEOUT)?;
        out.status
            .success()
            .then(|| parse_pr_head_state_snapshot(&String::from_utf8_lossy(&out.stdout)))?
    };
    let mut snapshot = query(&[
        "pr",
        "list",
        "--state",
        "open",
        "--limit",
        "1000",
        "--json",
        "state,title,headRefName,headRefOid",
    ])?;
    for branch in candidate_branches {
        if deadline.is_some_and(|dl| std::time::Instant::now() >= dl) {
            break;
        }
        // trace:BUG-1288 | ai:claude — record the attempt regardless of
        // outcome, same as the pre-BUG-1288 code implicitly did for every
        // branch (it always attempted every one); only a deadline-skipped
        // branch is now excluded from this set.
        snapshot.queried_heads.insert(branch.clone());
        if let Some(found) = query_pr_head_history(project_root, branch) {
            snapshot.merge(found);
        }
    }
    Some(snapshot)
}

/// One `gh pr list --head <branch> --state all` round trip: the full PR
/// history (open/closed/merged) recorded against this exact head name.
/// BUG-1756: split out of [`collect_pr_head_state_snapshot_bounded`]'s
/// per-candidate loop so the unshipped-work detector can issue it lazily —
/// only for candidates that survive the cheap local git classification —
/// instead of paying one network round trip per candidate up front.
// trace:BUG-1756 | ai:claude
pub(crate) fn query_pr_head_history(
    project_root: &std::path::Path,
    branch: &str,
) -> Option<PrHeadStateSnapshot> {
    let gh_bin = resolve_gh_binary()?;
    let mut cmd = std::process::Command::new(&gh_bin);
    cmd.current_dir(project_root).args([
        "pr",
        "list",
        "--head",
        branch,
        "--state",
        "all",
        "--limit",
        "100",
        "--json",
        "state,title,headRefName,headRefOid",
    ]);
    // trace:BUG-1288 | ai:claude — bounded like every other gh call this
    // machine-readable pipeline makes; see FORGE_CLI_CALL_TIMEOUT.
    let out = command_output_with_timeout(cmd, FORGE_CLI_CALL_TIMEOUT)?;
    out.status
        .success()
        .then(|| parse_pr_head_state_snapshot(&String::from_utf8_lossy(&out.stdout)))?
}

impl PrHeadStateSnapshot {
    pub(crate) fn merge(&mut self, other: Self) {
        for (branch, evidence) in other.by_branch {
            let replace = self.by_branch.get(&branch).is_none_or(|old| {
                (evidence.state == "open" && old.state != "open")
                    || (evidence.state == "merged" && old.state == "closed")
            });
            if replace {
                self.by_branch.insert(branch, evidence);
            }
        }
        for (spec, mut heads) in other.open_heads_by_spec {
            self.open_heads_by_spec
                .entry(spec)
                .or_default()
                .append(&mut heads);
        }
        for (spec, mut heads) in other.merged_heads_by_spec {
            self.merged_heads_by_spec
                .entry(spec)
                .or_default()
                .append(&mut heads);
        }
        for heads in self.open_heads_by_spec.values_mut() {
            heads.sort();
            heads.dedup();
        }
        for heads in self.merged_heads_by_spec.values_mut() {
            heads.sort();
            heads.dedup();
        }
    }
}

pub(crate) fn parse_pr_head_state_snapshot(json: &str) -> Option<PrHeadStateSnapshot> {
    let rows: Vec<serde_json::Value> = serde_json::from_str(json).ok()?;
    let mut snapshot = PrHeadStateSnapshot::default();
    for row in rows {
        let Some(branch) = row.get("headRefName").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(state) = row.get("state").and_then(|v| v.as_str()) else {
            continue;
        };
        let state = state.to_ascii_lowercase();
        if branch.is_empty() || !matches!(state.as_str(), "open" | "closed" | "merged") {
            continue;
        }
        let title = row.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let mut spec_ids = work_spec_id_from_branch(branch)
            .into_iter()
            .collect::<Vec<_>>();
        spec_ids.extend(extract_spec_ids_from_commit(title));
        spec_ids.sort();
        spec_ids.dedup();
        if state == "open" {
            for spec in &spec_ids {
                snapshot
                    .open_heads_by_spec
                    .entry(spec.to_ascii_uppercase())
                    .or_default()
                    .push(branch.to_string());
            }
        }
        let head_sha = row
            .get("headRefOid")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .map(str::to_string);
        if state == "merged" {
            if let Some(sha) = &head_sha {
                for spec in &spec_ids {
                    snapshot
                        .merged_heads_by_spec
                        .entry(spec.to_ascii_uppercase())
                        .or_default()
                        .push(sha.clone());
                }
            }
        }
        let evidence = PrHeadEvidence { state, head_sha };
        let replace = snapshot.by_branch.get(branch).is_none_or(|old| {
            (evidence.state == "open" && old.state != "open")
                || (evidence.state == "merged" && old.state == "closed")
        });
        if replace {
            snapshot.by_branch.insert(branch.to_string(), evidence);
        }
    }
    for heads in snapshot.open_heads_by_spec.values_mut() {
        heads.sort();
        heads.dedup();
    }
    for heads in snapshot.merged_heads_by_spec.values_mut() {
        heads.sort();
        heads.dedup();
    }
    Some(snapshot)
}

/// Roll up `statusCheckRollup` into one of `pass`, `fail`, `pending`, or
/// `?`. Counts FAILURE/CANCELLED/TIMED_OUT/ACTION_REQUIRED as fail,
/// IN_PROGRESS/QUEUED/PENDING as pending; SUCCESS only when every check
/// reports success.
pub(crate) fn summarize_status_check_rollup(checks: &[serde_json::Value]) -> String {
    if checks.is_empty() {
        return "?".to_string();
    }
    let mut has_fail = false;
    let mut has_pending = false;
    let mut total = 0usize;
    let mut passed = 0usize;
    for c in checks {
        total += 1;
        let conclusion = c
            .get("conclusion")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_uppercase();
        let status = c
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_uppercase();
        match conclusion.as_str() {
            "SUCCESS" => passed += 1,
            "FAILURE" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED" => has_fail = true,
            _ => {
                if matches!(status.as_str(), "IN_PROGRESS" | "QUEUED" | "PENDING") {
                    has_pending = true;
                }
            }
        }
    }
    if has_fail {
        "fail".to_string()
    } else if has_pending {
        "pending".to_string()
    } else if passed == total {
        "pass".to_string()
    } else {
        "?".to_string()
    }
}

/// BUG-1481: [`summarize_status_check_rollup`], corrected for the checks
/// that never ran at all. `ci_rollup: "pass"` means "every check present is
/// green" — that's true of a head carrying one green trivial check and
/// missing the required build entirely, which is exactly the false-green
/// PR-2009 shape. This wrapper never lets a required check's *absence* read
/// as a pass:
///
/// - `required_checks` unresolved (`None`, e.g. branch protection unreadable)
///   downgrades a `"pass"` verdict to `"unknown"` — the required set itself
///   is absent evidence, not evidence of nothing required (PRIN-5).
/// - `required_checks` resolved but naming a check whose row is missing from
///   `checks` downgrades to `"missing"`.
/// - Otherwise the base rollup passes through unchanged, so a `"fail"` /
///   `"pending"` never gets *greener* just because the required set is
///   unknown or incomplete.
// trace:BUG-1481 | ai:claude
pub(crate) fn summarize_status_check_rollup_with_required(
    checks: &[serde_json::Value],
    required_checks: Option<&[String]>,
) -> String {
    let base = summarize_status_check_rollup(checks);
    if base != "pass" {
        return base;
    }
    let Some(required) = required_checks else {
        return "unknown".to_string();
    };
    if required.is_empty() {
        return base;
    }
    let present: std::collections::HashSet<&str> = checks
        .iter()
        .filter_map(|c| {
            c.get("name")
                .and_then(|v| v.as_str())
                .or_else(|| c.get("context").and_then(|v| v.as_str()))
        })
        .collect();
    if required.iter().any(|r| !present.contains(r.as_str())) {
        return "missing".to_string();
    }
    base
}

#[cfg(test)]
mod bug_1481_ci_rollup_required_checks_tests {
    use super::*;

    fn checks(json: &str) -> Vec<serde_json::Value> {
        serde_json::from_str(json).unwrap()
    }

    /// The exact PR-2009 shape from the bug report: one green non-required
    /// check (`merge-hold-gate`) and no row at all for the required `Build
    /// (ubuntu-latest)` check. Must NOT read as "pass".
    // trace:BUG-1481 | ai:claude
    #[test]
    fn missing_required_check_is_not_pass() {
        let arr =
            checks(r#"[{"name":"merge-hold-gate","status":"COMPLETED","conclusion":"SUCCESS"}]"#);
        let required = vec![
            "merge-hold-gate".to_string(),
            "Build (ubuntu-latest)".to_string(),
        ];
        assert_eq!(
            summarize_status_check_rollup_with_required(&arr, Some(&required)),
            "missing"
        );
    }

    /// Every required check present and green → pass, unchanged from today.
    // trace:BUG-1481 | ai:claude
    #[test]
    fn all_required_present_and_green_is_pass() {
        let arr = checks(
            r#"[{"name":"merge-hold-gate","status":"COMPLETED","conclusion":"SUCCESS"},
                {"name":"Build (ubuntu-latest)","status":"COMPLETED","conclusion":"SUCCESS"}]"#,
        );
        let required = vec![
            "merge-hold-gate".to_string(),
            "Build (ubuntu-latest)".to_string(),
        ];
        assert_eq!(
            summarize_status_check_rollup_with_required(&arr, Some(&required)),
            "pass"
        );
    }

    /// Branch protection couldn't be read at all: never claim pass over an
    /// unknown required set.
    // trace:BUG-1481 | ai:claude
    #[test]
    fn unreadable_protection_is_unknown_not_pass() {
        let arr =
            checks(r#"[{"name":"merge-hold-gate","status":"COMPLETED","conclusion":"SUCCESS"}]"#);
        assert_eq!(
            summarize_status_check_rollup_with_required(&arr, None),
            "unknown"
        );
    }

    /// A genuinely unprotected branch (`Some(vec![])`, e.g. the API's 404)
    /// keeps today's behavior — nothing is required, so all-green is pass.
    // trace:BUG-1481 | ai:claude
    #[test]
    fn no_required_checks_configured_is_unchanged() {
        let arr =
            checks(r#"[{"name":"merge-hold-gate","status":"COMPLETED","conclusion":"SUCCESS"}]"#);
        assert_eq!(
            summarize_status_check_rollup_with_required(&arr, Some(&[])),
            "pass"
        );
    }

    /// A required set that can't be read never makes a failing/pending head
    /// look better than it is.
    // trace:BUG-1481 | ai:claude
    #[test]
    fn unreadable_protection_does_not_upgrade_fail_or_pending() {
        let failing = checks(r#"[{"name":"x","status":"COMPLETED","conclusion":"FAILURE"}]"#);
        assert_eq!(
            summarize_status_check_rollup_with_required(&failing, None),
            "fail"
        );
        let pending = checks(r#"[{"name":"x","status":"IN_PROGRESS","conclusion":""}]"#);
        assert_eq!(
            summarize_status_check_rollup_with_required(&pending, None),
            "pending"
        );
    }

    /// The literal "Branch not protected" message is the ONLY real, positive
    /// "nothing required" answer.
    // trace:BUG-1481 | ai:claude
    #[test]
    fn branch_not_protected_message_is_nothing_required() {
        assert_eq!(
            required_status_checks_outcome_from_stderr(
                "gh: Branch not protected (HTTP 404)\n{\"message\":\"Branch not protected\"}"
            ),
            Some(Vec::new())
        );
    }

    /// A bare 404 with a DIFFERENT message (e.g. a protected branch the
    /// caller lacks permission to read protection on) must NOT be read as
    /// "nothing required" — that is the exact PR-2009 false-green shape via
    /// the "404"/"not found" substring match this replaces.
    // trace:BUG-1481 | ai:claude
    #[test]
    fn bare_404_with_other_message_is_unknown() {
        assert_eq!(
            required_status_checks_outcome_from_stderr(
                "gh: Not Found (HTTP 404)\n{\"message\":\"Not Found\"}"
            ),
            None
        );
    }

    /// Permission-denied / network failure: unknown, never "nothing
    /// required".
    // trace:BUG-1481 | ai:claude
    #[test]
    fn permission_denied_is_unknown() {
        assert_eq!(
            required_status_checks_outcome_from_stderr(
                "gh: Must have admin rights to Repository. (HTTP 403)"
            ),
            None
        );
    }
}

/// Walk recent default-branch commits and recover the `spec_id → sha`
/// pairs that would auto-bump a Done spec to Completed. Returns the
/// flips that *would* land — the caller filters them against current
/// store state. Cheaper than `auto_bump_done_to_completed` because it
/// doesn't require being checked out on main.
/// trace:STORY-385 | ai:claude
pub(crate) fn scan_default_branch_for_spec_landings(
    project_root: &std::path::Path,
    limit: u32,
) -> Vec<(String, String)> {
    let default = match detect_default_branch_ref(project_root) {
        Some(r) => r,
        None => return Vec::new(),
    };
    let log = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "log",
            "--pretty=format:%H%x09%s",
            &format!("--max-count={limit}"),
            &default,
        ])
        .output();
    let Ok(log) = log else { return Vec::new() };
    if !log.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&log.stdout);
    // Spec → first-seen sha (matches auto-bump "earliest landing wins").
    let mut seen: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for line in text.lines() {
        let mut parts = line.splitn(2, '\t');
        let sha = parts.next().unwrap_or("").trim();
        let subject = parts.next().unwrap_or("").trim();
        if sha.is_empty() || subject.is_empty() {
            continue;
        }
        for spec_id in extract_spec_ids_from_commit(subject) {
            seen.entry(spec_id).or_insert_with(|| sha.to_string());
        }
    }
    seen.into_iter().collect()
}

/// True when `branch` is ahead of `target_ref` (defaults to the project's
/// detected default ref).
pub(crate) fn branch_ahead_of(
    project_root: &std::path::Path,
    branch: &str,
    target_ref: &str,
) -> Option<u32> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "rev-list",
            "--count",
            &format!("{}..{}", target_ref, branch),
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// TASK-1056: batch every local branch's commits-ahead-of-`target_ref` count
/// into ONE `git for-each-ref` (the `ahead-behind` field, git ≥ 2.41) instead
/// of a `git rev-list --count <target>..<branch>` per branch. On a fleet repo
/// the per-branch loop spawned a hundred-plus rev-list processes; this is one.
/// The map's value equals `branch_ahead_of(.., branch, target_ref)` for the
/// same branch, so callers that swap the lookup in render byte-identical
/// output. Returns an EMPTY map when the field is unsupported (older git makes
/// `for-each-ref` exit non-zero on the unknown atom) so callers transparently
/// fall back to the per-branch probe.
// trace:TASK-1056 | ai:claude
pub(crate) fn collect_branch_ahead_of(
    project_root: &std::path::Path,
    target_ref: &str,
) -> std::collections::HashMap<String, u32> {
    let mut map = std::collections::HashMap::new();
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "for-each-ref",
            &format!("--format=%(refname:short)%09%(ahead-behind:{target_ref})"),
            "refs/heads/",
        ])
        .output();
    let Ok(out) = out else { return map };
    if !out.status.success() {
        // Older git: the `ahead-behind` atom is unknown and the whole call
        // fails — leave the map empty so callers fall back per-branch.
        return map;
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.splitn(2, '\t');
        let Some(name) = parts.next() else { continue };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        // The `ahead-behind` field renders "<ahead> <behind>"; we want ahead.
        // An empty field (e.g. target_ref unresolved for this ref) is skipped
        // so the caller falls back rather than recording a bogus 0.
        let Some(ahead) = parts
            .next()
            .and_then(|ab| ab.split_whitespace().next().map(|s| s.to_string()))
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        map.insert(name.to_string(), ahead);
    }
    map
}

/// `git for-each-ref refs/heads/` — just the branch names.
pub(crate) fn list_local_branches(project_root: &std::path::Path) -> Vec<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["for-each-ref", "--format=%(refname:short)", "refs/heads/"])
        .output();
    let Ok(out) = out else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// One `git for-each-ref` over `refs/heads/` returning each local branch's name
/// paired with its tip-commit time (unix seconds). Collapses a per-branch
/// `git log -1 --format=%ct` fan-out (one process per branch — thousands on a
/// fleet repo) into a single git invocation that yields the same data.
// trace:TASK-1056 | ai:claude
pub(crate) fn collect_local_branch_commit_times(
    project_root: &std::path::Path,
) -> std::collections::HashMap<String, i64> {
    let mut map = std::collections::HashMap::new();
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "for-each-ref",
            "--format=%(refname:short)%09%(committerdate:unix)",
            "refs/heads/",
        ])
        .output();
    let Ok(out) = out else { return map };
    if !out.status.success() {
        return map;
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.splitn(2, '\t');
        let Some(name) = parts.next() else { continue };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let ts = parts
            .next()
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(0);
        map.insert(name.to_string(), ts);
    }
    map
}

/// One `git for-each-ref` over `refs/remotes/origin` returning the set of
/// remote-tracking branch short names with the `origin/` prefix stripped (and
/// the symbolic `origin/HEAD` excluded). Collapses a per-branch
/// `git rev-parse --verify --quiet origin/<branch>` existence fan-out into a
/// single git invocation; membership in the set is equivalent to a successful
/// verify.
// trace:TASK-1056 | ai:claude
pub(crate) fn collect_remote_branch_name_set(
    project_root: &std::path::Path,
) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/remotes/origin",
        ])
        .output();
    let Ok(out) = out else { return set };
    if !out.status.success() {
        return set;
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let name = line.trim();
        let short = name.strip_prefix("origin/").unwrap_or(name);
        if short.is_empty() || short == "HEAD" {
            continue;
        }
        set.insert(short.to_string());
    }
    set
}

/// One `git for-each-ref` over `refs/heads/` AND `refs/remotes/origin`
/// returning each ref's short name (exactly the display spelling the
/// unshipped-work candidate list uses: `bug-x` locally, `origin/bug-x` for
/// remote-tracking refs) paired with its tip-commit time (unix seconds).
/// BUG-1756: drives the newest-first probe order of the bounded scan.
// trace:BUG-1756 | ai:claude
pub(crate) fn collect_candidate_tip_times(
    project_root: &std::path::Path,
) -> std::collections::HashMap<String, i64> {
    let mut map = std::collections::HashMap::new();
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "for-each-ref",
            "--format=%(refname:short)%09%(committerdate:unix)",
            "refs/heads/",
            "refs/remotes/origin",
        ])
        .output();
    let Ok(out) = out else { return map };
    if !out.status.success() {
        return map;
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.splitn(2, '\t');
        let Some(name) = parts.next() else { continue };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let ts = parts
            .next()
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(0);
        map.insert(name.to_string(), ts);
    }
    map
}

// trace:STORY-1043 | ai:codex
#[derive(Debug, Clone)]
pub(crate) struct UnshippedBranchCandidate {
    pub(crate) branch: String,
    pub(crate) refname: String,
    pub(crate) local_branch: String,
    pub(crate) spec_id: String,
    pub(crate) commits_ahead: u32,
    pub(crate) age: String,
    pub(crate) has_local: bool,
    // BUG-1531: the candidate's tip commit, used to bind a "do not ship"
    // refusal to the exact commit a reviewer looked at rather than to a PR
    // number or a branch-naming convention. `None` only when `git rev-parse`
    // itself fails, in which case the sha-bound checks are skipped rather
    // than guessed.
    // trace:BUG-1531 | ai:claude
    pub(crate) tip_sha: Option<String>,
    // BUG-1531: the branch's name has the `pr-N`/`mr-N` review-snapshot
    // shape, but the forge did NOT confirm it (no forge, lookup failure, or
    // a head sha that doesn't match this branch's tip) — so it is kept in
    // the report rather than silently excluded (PRIN-5: never hide on name
    // alone), labelled as unverified with no ship hint.
    // trace:BUG-1531 | ai:claude
    pub(crate) possible_review_snapshot: bool,
    // BUG-1756: whether a remote counterpart of this branch exists
    // (`origin/<name>`; trivially true for a remote-only row). `false` means
    // the commits exist on exactly one machine — the most strandable
    // unshipped state — and drives a push-first recovery hint instead of a
    // bare `aida pr ship`. trace:BUG-1756 | ai:claude
    pub(crate) pushed: bool,
}

/// BUG-1756: a candidate that survived the cheap local classification pass
/// and is waiting for (or has received) its per-head forge evidence. The
/// review-snapshot NAME detection happens in the local pass; the forge
/// CONFIRMATION (a network call) happens in the bounded evidence pass, so
/// `review_shape` carries the parsed shape between the two.
// trace:BUG-1756 | ai:claude
pub(crate) struct UnshippedSurvivor {
    pub(crate) cand: UnshippedBranchCandidate,
    pub(crate) review_shape: Option<(forge::ForgeKind, u64)>,
    /// The forge confirmed PR/MR N exists with a head sha equal to this
    /// branch's tip — the one condition BUG-1531 allows the row to be
    /// excluded on.
    pub(crate) confirmed_review_snapshot: bool,
}

// BUG-1288: a bounded candidate gate for the unshipped-work detector. Kept
// pure so a large stale-ref population can be pinned without timing-sensitive
// process tests. trace:BUG-1288 | ai:codex
pub(crate) fn branch_belongs_to_active_work(branch: &str, active_prefixes: &[String]) -> bool {
    active_prefixes.iter().any(|prefix| {
        branch == prefix
            || branch
                .strip_prefix(prefix)
                .is_some_and(|suffix| suffix.starts_with('-'))
    })
}

// trace:BUG-1187 | ai:codex
pub(crate) fn all_requirement_summaries(
    project_root: &std::path::Path,
) -> Vec<aida_core::RequirementSummary> {
    let Some(store_path) = detect_distributed_store_from(project_root) else {
        return Vec::new();
    };
    let Ok(dispenser) = load_dispenser(&store_path) else {
        return Vec::new();
    };
    let Ok(inner) = aida_core::GitBackend::new(&store_path).map(|b| b.with_dispenser(dispenser))
    else {
        return Vec::new();
    };
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let Ok(backend) = aida_core::CachedGitBackend::with_inner(inner, &cache_path) else {
        return Vec::new();
    };
    backend
        .list_summaries(&aida_core::ListFilter {
            archive: aida_core::ArchiveFilter::Both,
            defer: aida_core::DeferFilter::Both,
            ..Default::default()
        })
        .unwrap_or_default()
}

// trace:BUG-1187 | ai:codex
pub(crate) fn insert_summary_statuses(
    status_by_spec: &mut std::collections::HashMap<String, String>,
    summaries: &[aida_core::RequirementSummary],
) {
    for s in summaries {
        let Some(id) = s.agreed_id.clone().or_else(|| s.spec_id.clone()) else {
            continue;
        };
        status_by_spec.insert(id.to_ascii_uppercase(), s.status.to_ascii_lowercase());
    }
}

// trace:STORY-1043 | ai:codex
pub(crate) fn collect_unshipped_work_items(
    project_root: &std::path::Path,
    summaries: &[aida_core::RequirementSummary],
    no_forge: bool,
    emit_detected_events: bool,
) -> Vec<awaiting_you::UnshippedWorkItem> {
    collect_unshipped_work_items_bounded(
        project_root,
        summaries,
        no_forge,
        emit_detected_events,
        None,
    )
    .0
}

/// BUG-1288: time-boxed sibling of [`collect_unshipped_work_items`], used by
/// the machine-readable polling paths (`aida awaiting --json`, `aida status
/// --full`). The candidate-branch SET is computed exactly as before (PR
/// #1999 / STORY-1368's exclude-list, unchanged) — this only bounds how long
/// probing that set may run. `deadline == None` behaves identically to the
/// unbounded original (every other caller, including tests and
/// `session_reap`, which need the exhaustive answer regardless of cost).
///
/// Two independent probes can consume wall clock per candidate: the local
/// git commit/patch-equivalence walk and the per-branch `gh pr list --head`
/// network round trip ([`query_pr_head_history`]). Both consult the same
/// `deadline`, so the combined budget is shared rather than doubled — and
/// since BUG-1756 the CHEAP local walk runs first for every candidate, with
/// the network round trips paid only for the candidates that survive it.
/// (Before that reorder the up-front per-candidate network calls consumed
/// the whole budget on a working repository, so the bounded scan always
/// truncated at 0/N and the channel was effectively dead.) Once the deadline
/// passes, remaining candidate branches are simply not probed — the returned
/// [`awaiting_you::UnshippedScanStatus`] records `complete: false` plus how
/// many of the total candidates were actually scanned, so a truncated run is
/// never presented as an exhaustive one (PRIN-5). A survivor whose evidence
/// query the deadline skipped counts as UNSCANNED and is dropped rather than
/// reported on evidence that was never fetched. What IS returned is
/// unaffected: nothing already found is dropped, and nothing is hidden by
/// widening or narrowing which branches count as candidates.
// trace:BUG-1288 | ai:claude
pub(crate) fn collect_unshipped_work_items_bounded(
    project_root: &std::path::Path,
    summaries: &[aida_core::RequirementSummary],
    no_forge: bool,
    emit_detected_events: bool,
    deadline: Option<std::time::Instant>,
) -> (
    Vec<awaiting_you::UnshippedWorkItem>,
    awaiting_you::UnshippedScanStatus,
) {
    let Some(default_ref) = detect_default_branch_ref(project_root) else {
        return (
            Vec::new(),
            awaiting_you::UnshippedScanStatus {
                complete: true,
                scanned: 0,
                candidates: 0,
            },
        );
    };

    let mut status_by_spec = std::collections::HashMap::new();
    insert_summary_statuses(&mut status_by_spec, summaries);
    insert_summary_statuses(
        &mut status_by_spec,
        &all_requirement_summaries(project_root),
    );

    let live = process_probe::probe_live_claude_sessions();
    let now = chrono::Utc::now();
    let leases = list_leases(project_root);
    let live_leases: Vec<SessionLease> = leases
        .iter()
        .filter(|l| matches!(lease_state_for(l, &live, now), LeaseState::Live))
        .cloned()
        .collect();
    let live_scopes: std::collections::HashSet<String> = live_leases
        .iter()
        .map(|l| l.scope.to_ascii_uppercase())
        .collect();
    let live_branches: std::collections::HashSet<String> =
        live_leases.iter().map(|l| l.branch.clone()).collect();

    let mut seen = std::collections::HashSet::new();
    let local: std::collections::HashSet<String> =
        list_local_branches(project_root).into_iter().collect();
    let remote = collect_remote_branch_name_set(project_root);
    // BUG-1288: only run the expensive commit/patch-equivalence probes for
    // branches attributable to active requirement work or a recorded session.
    // Listing refs remains two batched git calls, but stale unrelated refs no
    // longer cause one or more subprocesses each. A requirement branch may
    // carry a suffix (`bug-1288-work`), so prefix matching deliberately keeps
    // those session-created variants while excluding another numeric id.
    //
    // PR #1999 rework: this was originally an ALLOW-list of only
    // "inprogress"/"in-progress"/"done", which silently dropped a lease-less
    // branch whose spec sat in NeedsAttention (shelved-but-not-abandoned),
    // Approved, Draft, or Planned — each a status a branch can legitimately
    // carry real unshipped commits under, before the commit/patch probe ever
    // ran. The bound this exists for is "don't probe every stale ref in the
    // repo" (301+ of them), not "only probe two statuses" — so the gate is
    // an EXCLUDE-list of the terminal statuses instead: everything that
    // isn't Completed/Rejected/Superseded is still eligible for the probe.
    // The exclusion set intentionally mirrors the terminal-status check
    // applied to the resolved branch spec_id further below in this
    // function, so a branch is never filtered here for a reason that
    // wouldn't also filter it there. trace:BUG-1288 | ai:claude
    let active_prefixes: Vec<String> = status_by_spec
        .iter()
        .filter(|(_, status)| !matches!(status.as_str(), "completed" | "rejected" | "superseded"))
        .map(|(spec, _)| spec.to_ascii_lowercase())
        .collect();
    let lease_branches: std::collections::HashSet<&str> =
        leases.iter().map(|lease| lease.branch.as_str()).collect();
    let is_candidate = |branch: &str| {
        lease_branches.contains(branch) || branch_belongs_to_active_work(branch, &active_prefixes)
    };
    let mut branches: Vec<(String, String, bool)> = local
        .iter()
        .filter(|branch| is_candidate(branch))
        .map(|b| (b.clone(), b.clone(), true))
        .chain(
            remote
                .iter()
                .filter(|b| !local.contains(*b) && is_candidate(b))
                .map(|b| (format!("origin/{b}"), format!("origin/{b}"), false)),
        )
        .collect();
    // BUG-1756: probe NEWEST tip first (name as the deterministic
    // tie-break), not alphabetically. The scan is wall-clock-bounded and a
    // single anciently-forked ref can cost tens of seconds in `git cherry`
    // (patch-id over every default-branch commit since the fork), so
    // whatever the budget cannot cover must be the OLD tail — never the
    // day-old branch a dead drain just stranded, which is the state this
    // channel exists to report. Tip times come from one batched
    // `for-each-ref`; a branch missing from it sorts last.
    // trace:BUG-1756 | ai:claude
    let tip_times = collect_candidate_tip_times(project_root);
    branches.sort_by(|a, b| {
        let ta = tip_times.get(&a.0).copied().unwrap_or(i64::MIN);
        let tb = tip_times.get(&b.0).copied().unwrap_or(i64::MIN);
        tb.cmp(&ta).then_with(|| a.0.cmp(&b.0))
    });

    // trace:BUG-1576 | ai:codex
    // BUG-1756: the snapshot starts BATCHED-ONLY — one `gh pr list --state
    // open` call regardless of candidate count. The per-candidate `--head`
    // history queries that used to run here up front moved into the evidence
    // pass below and run only for candidates that survive the cheap local
    // git classification. On a working repository the up-front per-candidate
    // network round trips consumed the entire wall-clock budget before a
    // single branch was locally probed, so the bounded scan always truncated
    // at 0/N and the channel reported nothing at all — which is exactly how
    // a local-only committed branch (the state that exists on one machine
    // and nowhere else) stayed invisible. Local evidence is cheap; it goes
    // first. trace:BUG-1756 | ai:claude
    let mut pr_head_states = if no_forge {
        None
    } else {
        collect_pr_head_state_snapshot_bounded(project_root, &[], deadline)
    };

    // BUG-1288: the total candidate-branch population identified above,
    // before the deadline can truncate how much of it actually gets probed
    // below. This is the denominator `UnshippedScanStatus::candidates`
    // reports, so "scan incomplete" always names how much was left unscanned
    // rather than just how much was scanned. trace:BUG-1288 | ai:claude
    let total_candidates = branches.len();
    let mut scanned = 0usize;
    let mut truncated = false;

    // BUG-1756: the most a single candidate's local diff+cherry probe may
    // spend when the scan is deadline-bounded. ~10× the loaded-host cost of
    // a normal candidate, so it only trips on genuine monsters (an
    // anciently-forked ref whose `git cherry` patch-ids thousands of
    // default-branch commits). trace:BUG-1756 | ai:claude
    const UNSHIPPED_PROBE_SLICE: std::time::Duration = std::time::Duration::from_millis(2000);

    // BUG-1756: resolve the patch-count base ref ONCE for the whole scan
    // instead of once per candidate inside
    // `branch_unshipped_patch_count_default` — the per-branch resolution was
    // 1–2 extra git subprocesses per candidate charged against the same
    // wall-clock budget. Resolution failure keeps the exact pre-existing
    // behavior: the candidate is skipped. trace:BUG-1756 | ai:claude
    let patch_count_base = resolve_default_branch_ref(project_root);

    // ── Pass 1: LOCAL classification — git only, no network ──────────────
    // Everything that can disqualify a candidate without per-branch forge
    // calls runs first, in the pre-existing predicate order: the batched
    // open-PR evidence, spec resolution, live leases, terminal status, and
    // the commit/patch-equivalence probe. Only the survivors — typically
    // zero to a handful — pay a per-head network query in the evidence pass
    // below, so the wall-clock budget is spent on work the local evidence
    // already says matters. trace:BUG-1756 | ai:claude
    let mut survivors: Vec<UnshippedSurvivor> = Vec::new();
    // A locally shipped ref can carry the merged-PR proof needed to suppress
    // a stale same-spec ancestor. Keep it for pass 2, but only query it when
    // that spec also has a survivor. trace:TASK-1575 | ai:codex
    let mut shipped_evidence_carriers: Vec<(String, String)> = Vec::new();
    for (display_branch, refname, has_local) in branches {
        if deadline.is_some_and(|dl| std::time::Instant::now() >= dl) {
            truncated = true;
            break;
        }
        scanned += 1;
        let short_branch = display_branch
            .strip_prefix("origin/")
            .unwrap_or(display_branch.as_str())
            .to_string();
        if matches!(
            short_branch.as_str(),
            "main" | "master" | "aida-store" | "HEAD"
        ) {
            continue;
        }
        // BUG-1531 criterion 1: a branch fetched from refs/pull/N/head (or its
        // GitLab mr-N twin) is the one shape `ReviewForge::local_branch_for`
        // creates, per TASK-1312's `parse_review_snapshot_branch` — but the
        // NAME alone is not proof (a real unpushed feature branch can happen
        // to be named `pr-123`). The NAME detection happens here; the forge
        // CONFIRMATION (`change_metadata`, a network call) happens in the
        // bounded evidence pass below. Only a confirmed match (head sha
        // equals this branch's tip) is excluded; anything less is kept and
        // labelled unverified rather than hidden on name alone (PRIN-5).
        // trace:BUG-1531 | ai:claude
        let review_shape = pr_cmd::parse_review_snapshot_branch(&short_branch);
        let pr_evidence = pr_head_states
            .as_ref()
            .and_then(|s| s.by_branch.get(&short_branch));
        if pr_evidence.is_some_and(|pr| pr.state == "open") {
            continue;
        }
        let Some(spec_id) = work_spec_id_from_branch(&short_branch)
            .or_else(|| first_spec_id_in_branch_commits(project_root, &default_ref, &refname))
        else {
            continue;
        };
        let spec_key = spec_id.to_ascii_uppercase();
        // A unique open PR's headRefName is authoritative for its spec. Never
        // suggest shipping an abandoned local branch as a duplicate PR.
        if pr_head_states
            .as_ref()
            .and_then(|s| s.open_heads_by_spec.get(&spec_key))
            .is_some_and(|heads| heads.len() == 1 && heads[0] != short_branch)
        {
            continue;
        }
        if live_scopes.contains(&spec_key) || live_branches.contains(&short_branch) {
            continue;
        }
        if status_by_spec
            .get(&spec_key)
            .map(|status| matches!(status.as_str(), "completed" | "rejected" | "superseded"))
            .unwrap_or(false)
        {
            continue;
        }
        // BUG-1756: one candidate gets at most UNSHIPPED_PROBE_SLICE (and
        // never more than the remaining budget) for its diff+cherry probe. A
        // candidate whose probe times out was never classified: it counts as
        // unscanned and marks the scan incomplete, instead of either eating
        // the whole budget (starving every candidate after it) or being
        // silently dispositioned as "nothing". trace:BUG-1756 | ai:claude
        let probe_slice = deadline.map(|dl| {
            dl.saturating_duration_since(std::time::Instant::now())
                .min(UNSHIPPED_PROBE_SLICE)
        });
        let probe = match patch_count_base.as_deref() {
            Some(base) => {
                branch_unshipped_patch_count_probe(project_root, base, &refname, probe_slice)
            }
            None => PatchCountProbe::NoSignal,
        };
        let commits_ahead = match probe {
            PatchCountProbe::Counted(n) if n > 0 => n,
            PatchCountProbe::Counted(0) => {
                shipped_evidence_carriers.push((spec_key, short_branch));
                continue;
            }
            PatchCountProbe::TimedOut => {
                scanned -= 1;
                truncated = true;
                continue;
            }
            PatchCountProbe::Counted(_) | PatchCountProbe::NoSignal => continue,
        };
        if !seen.insert(display_branch.clone()) {
            continue;
        }
        let tip_sha = git_output_checked(project_root, &["rev-parse", &refname])
            .ok()
            .map(|s| s.trim().to_string());
        // BUG-1756: a local branch with no `origin/<name>` counterpart exists
        // on exactly one machine. (A remote-only row is trivially pushed.)
        // Deliberately the EXISTENCE of the remote ref, not tip equality: a
        // stale-pushed branch still has a durable copy of most of its work,
        // and `aida pr ship` pushes the remainder. trace:BUG-1756 | ai:claude
        let pushed = !has_local || remote.contains(&short_branch);
        let age = branch_tip_age(project_root, &refname);
        survivors.push(UnshippedSurvivor {
            cand: UnshippedBranchCandidate {
                branch: display_branch,
                refname,
                local_branch: short_branch,
                spec_id,
                commits_ahead,
                age,
                has_local,
                tip_sha,
                possible_review_snapshot: false,
                pushed,
            },
            review_shape,
            confirmed_review_snapshot: false,
        });
    }

    // ── Pass 2: bounded per-survivor forge evidence ───────────────────────
    // BUG-1576's targeted `--head` history lookup and BUG-1531's review-
    // snapshot confirmation, one survivor at a time, still under the same
    // deadline. A survivor the budget cannot cover is counted as UNSCANNED
    // and dropped from the report — truncation stays a lower bound (PRIN-5),
    // never a row built on evidence that was never fetched.
    // trace:BUG-1756 | ai:claude
    // BUG-1756: evidence the LOCAL-ONLY survivors first (stable sort keeps
    // the newest-first order within each group). A pushed survivor the
    // budget cannot cover is durable and findable from any machine; a
    // local-only one is the state this channel exists to report, so
    // truncation must never be able to hide it behind pushed rows.
    // trace:BUG-1756 | ai:claude
    survivors.sort_by_key(|s| s.cand.pushed);
    let mut evidenced = survivors.len();
    if no_forge {
        for survivor in survivors.iter_mut() {
            if survivor.review_shape.is_some() {
                survivor.cand.possible_review_snapshot = true;
            }
        }
    } else {
        for (idx, survivor) in survivors.iter_mut().enumerate() {
            if deadline.is_some_and(|dl| std::time::Instant::now() >= dl) {
                truncated = true;
                evidenced = idx;
                break;
            }
            if let Some(states) = pr_head_states.as_mut() {
                states
                    .queried_heads
                    .insert(survivor.cand.local_branch.clone());
                if let Some(found) =
                    query_pr_head_history(project_root, &survivor.cand.local_branch)
                {
                    states.merge(found);
                }
            }
            if let Some((kind, n)) = survivor.review_shape {
                match forge::forge_for_kind(project_root, kind)
                    .change_metadata(n, &mut network_retry::NoopSink)
                {
                    Ok(meta) if !meta.head_sha.is_empty() => {
                        if survivor.cand.tip_sha.as_deref() == Some(meta.head_sha.as_str()) {
                            survivor.confirmed_review_snapshot = true;
                        } else {
                            survivor.cand.possible_review_snapshot = true;
                        }
                    }
                    _ => {
                        survivor.cand.possible_review_snapshot = true;
                    }
                }
            }
        }
        // Apply pass-2 truncation before carrier filtering. `evidenced` is a
        // prefix length; filtering first could shift a never-evidenced row
        // into that prefix and report it without forge evidence. trace:TASK-1575 | ai:codex
        if evidenced < survivors.len() {
            scanned -= survivors.len() - evidenced;
            survivors.truncate(evidenced);
        }
        // Query excluded same-spec carriers only after survivor queries. This
        // preserves lazy evidence while restoring merged-head representation
        // proof for the ancestor branch case. If the deadline prevents a
        // needed carrier query, suppress that spec's survivors rather than
        // report on incomplete representation evidence. trace:TASK-1575 | ai:codex
        let survivor_specs: std::collections::HashSet<String> = survivors
            .iter()
            .map(|s| s.cand.spec_id.to_ascii_uppercase())
            .collect();
        let mut incomplete_carrier_specs = std::collections::HashSet::new();
        for (spec_key, carrier) in shipped_evidence_carriers {
            if !survivor_specs.contains(&spec_key) {
                continue;
            }
            if deadline.is_some_and(|dl| std::time::Instant::now() >= dl) {
                truncated = true;
                incomplete_carrier_specs.insert(spec_key);
                continue;
            }
            if let Some(states) = pr_head_states.as_mut() {
                states.queried_heads.insert(carrier.clone());
                if let Some(found) = query_pr_head_history(project_root, &carrier) {
                    states.merge(found);
                } else {
                    truncated = true;
                    incomplete_carrier_specs.insert(spec_key);
                }
            } else {
                truncated = true;
                incomplete_carrier_specs.insert(spec_key);
            }
        }
        if !incomplete_carrier_specs.is_empty() {
            let before = survivors.len();
            survivors.retain(|s| {
                !incomplete_carrier_specs.contains(&s.cand.spec_id.to_ascii_uppercase())
            });
            let removed = before - survivors.len();
            scanned = scanned.saturating_sub(removed);
        }
    }
    // ── Final classification against the completed evidence ──────────────
    // The forge-dependent exclusions, in their pre-existing order, applied
    // once every surviving candidate's history query has been merged — so a
    // merged head discovered through one survivor can still represent (and
    // exclude) another survivor of the same spec, exactly as when every
    // candidate was queried up front. trace:BUG-1756 | ai:claude
    let mut candidates = Vec::new();
    for survivor in survivors {
        let UnshippedSurvivor {
            cand: c,
            confirmed_review_snapshot,
            ..
        } = survivor;
        // trace:BUG-1531 | ai:claude — the one exclusion a review-snapshot
        // NAME may earn: the forge confirmed the PR/MR and its head sha
        // equals this branch's tip.
        if confirmed_review_snapshot {
            continue;
        }
        let pr_evidence = pr_head_states
            .as_ref()
            .and_then(|s| s.by_branch.get(&c.local_branch));
        // Re-applied with the full evidence: a per-head history query above
        // can surface an open PR the batched open-PR snapshot did not.
        if pr_evidence.is_some_and(|pr| pr.state == "open") {
            continue;
        }
        let spec_key = c.spec_id.to_ascii_uppercase();
        if pr_head_states
            .as_ref()
            .and_then(|s| s.open_heads_by_spec.get(&spec_key))
            .is_some_and(|heads| heads.len() == 1 && heads[0] != c.local_branch)
        {
            continue;
        }
        if pr_evidence.is_some_and(|pr| {
            if pr.state != "merged" {
                return false;
            }
            let tip_matches = pr
                .head_sha
                .as_ref()
                .is_some_and(|sha| c.tip_sha.as_deref() == Some(sha.as_str()));
            tip_matches
                || doctor_cmd::branch_content_fully_landed(project_root, &default_ref, &c.refname)
        }) {
            continue;
        }
        // A rework ref may have a different name from the PR head while
        // pointing at that reviewed head (or one of its ancestors). This is
        // commit-representation proof, unlike branch age: a divergent branch
        // for the same spec is not an ancestor and remains visible.
        let represented_by_merged_head = pr_head_states
            .as_ref()
            .and_then(|s| s.merged_heads_by_spec.get(&spec_key))
            .is_some_and(|merged_heads| {
                merged_heads.iter().any(|head| {
                    let is_ancestor = std::process::Command::new("git")
                        .arg("-C")
                        .arg(project_root)
                        .args([
                            "merge-base",
                            "--is-ancestor",
                            git_arg_guard::END_OF_OPTIONS,
                            &c.refname,
                            head,
                        ]) // trace:BUG-1622 | ai:claude
                        .status()
                        .is_ok_and(|status| status.success());
                    is_ancestor
                        || doctor_cmd::branch_content_fully_landed(project_root, head, &c.refname)
                })
            });
        if represented_by_merged_head {
            continue;
        }
        // BUG-1531 criterion 2 (the safety floor, independent of criterion 1):
        // never suggest shipping a commit that is ALREADY the head of an open
        // PR, even when this branch's own name doesn't match that PR's
        // headRefName (a rework ref, a hand-fetched investigation branch,
        // …). Matched by sha across every open PR the snapshot knows about,
        // not just the by-name lookup above.
        // trace:BUG-1531 | ai:claude
        if let (Some(sha), Some(states)) = (c.tip_sha.as_deref(), pr_head_states.as_ref()) {
            let already_open_elsewhere = states.by_branch.values().any(|evidence| {
                evidence.state == "open" && evidence.head_sha.as_deref() == Some(sha)
            });
            if already_open_elsewhere {
                continue;
            }
        }
        candidates.push(c);
    }

    candidates.sort_by(|a, b| b.commits_ahead.cmp(&a.commits_ahead));
    let items: Vec<awaiting_you::UnshippedWorkItem> = candidates
        .into_iter()
        .map(|c| {
            let first_seen = if emit_detected_events {
                record_unshipped_work_detected(project_root, &c.spec_id, &c.branch)
            } else {
                String::new()
            };
            let pr_state = if no_forge || pr_head_states.is_none() {
                "unknown".to_string()
            } else {
                let states = pr_head_states.as_ref().expect("checked above");
                match states
                    .by_branch
                    .get(&c.local_branch)
                    .map(|pr| pr.state.as_str())
                {
                    Some("open") => "open",
                    Some("merged") => "merged",
                    // BUG-1288: only report "absent" (no PR ever existed for
                    // this head) when the per-branch query actually ran. A
                    // branch the deadline skipped was never asked, so it must
                    // read "unknown" rather than a false "absent" — PRIN-5.
                    // trace:BUG-1288 | ai:claude
                    _ if states.queried_heads.contains(&c.local_branch) => "absent",
                    _ => "unknown",
                }
                .to_string()
            };
            // BUG-1531 criterion 3 (the general form of the hole, independent
            // of both branch shape and PR number): a refusal binds to a
            // COMMIT. If this branch's tip is the exact sha an outstanding
            // (unclosed, not superseded by a Completed spec) RequestChanges
            // or Rejected verdict names, no surface may recommend shipping
            // it — regardless of which ref happens to reach that commit.
            // trace:BUG-1531 | ai:claude
            let outstanding_verdict = c.tip_sha.as_deref().and_then(|sha| {
                let verdict = review_verdict::read_recorded_verdict(project_root, &c.spec_id)?;
                let sha_matches = verdict
                    .reviewed_sha
                    .as_deref()
                    .is_some_and(|reviewed| review_verdict::same_reviewed_sha(reviewed, sha));
                let spec_completed = status_by_spec
                    .get(&c.spec_id.to_ascii_uppercase())
                    .is_some_and(|status| status.as_str() == "completed");
                (sha_matches && review_verdict::is_outstanding_refusal(&verdict, spec_completed))
                    .then_some(verdict)
            });
            let recovery = if let Some(verdict) = &outstanding_verdict {
                format!(
                    "do not ship — this commit carries an unresolved reviewer verdict ({}); resolve the review first",
                    verdict.raw
                )
            } else if c.possible_review_snapshot {
                // BUG-1531: name-shaped like a review snapshot but the forge
                // never confirmed it — kept visible, no ship hint (PRIN-5).
                format!(
                    "possible review snapshot ({}), unverified",
                    c.local_branch
                )
            } else if c.has_local && !c.pushed {
                // BUG-1756 AC3: a branch with NO remote ref must be pushed
                // before anything can ship it — the hint names the push
                // first, never a bare `aida pr ship` against a ref the forge
                // cannot see. trace:BUG-1756 | ai:claude
                format!(
                    "git push -u origin {b} && aida pr ship {b}",
                    b = &c.local_branch
                )
            } else if c.has_local {
                format!("aida pr ship {}", c.branch)
            } else {
                format!(
                    "git switch -c {} {} && aida pr ship {}",
                    &c.local_branch, c.refname, &c.local_branch
                )
            };
            let age = if first_seen.is_empty() {
                c.age
            } else {
                format!("{} (first seen {})", c.age, first_seen)
            };
            awaiting_you::UnshippedWorkItem {
                spec_id: c.spec_id,
                branch: c.branch,
                commits_ahead: c.commits_ahead,
                age,
                recovery,
                pr_state,
                pushed: c.pushed, // trace:BUG-1756 | ai:claude
            }
        })
        .collect();
    (
        items,
        awaiting_you::UnshippedScanStatus {
            complete: !truncated,
            scanned,
            candidates: total_candidates,
        },
    )
}

// trace:STORY-1043 | ai:codex
pub(crate) fn first_spec_id_in_branch_commits(
    project_root: &std::path::Path,
    default_ref: &str,
    refname: &str,
) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "log",
            "--format=%s",
            "-n",
            "25",
            &format!("{default_ref}..{refname}"),
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|line| extract_spec_ids_from_commit(line).into_iter().next())
}

// trace:STORY-1043 | ai:codex
pub(crate) fn branch_tip_age(project_root: &std::path::Path, refname: &str) -> String {
    let ts = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["log", "-1", "--format=%ct", refname])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<i64>()
                .ok()
        });
    let Some(ts) = ts else {
        return "unknown".to_string();
    };
    let age = chrono::Utc::now().timestamp().saturating_sub(ts);
    if age < 3600 {
        format!("{}m", (age / 60).max(1))
    } else if age < 86400 {
        format!("{}h", age / 3600)
    } else {
        format!("{}d", age / 86400)
    }
}

// trace:STORY-1043 | ai:codex
pub(crate) fn record_unshipped_work_detected(
    project_root: &std::path::Path,
    spec_id: &str,
    branch: &str,
) -> String {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    branch.hash(&mut hasher);
    let safe: String = branch
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = format!("{safe}-{:016x}", hasher.finish());
    let path = project_root
        .join(".aida")
        .join("unshipped-work-seen")
        .join(safe);
    if let Ok(prev) = std::fs::read_to_string(&path) {
        return prev.trim().to_string();
    }
    let first_seen = chrono::Utc::now().to_rfc3339();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::write(&path, &first_seen).is_ok() {
        events::emit(
            project_root,
            &events::Event::new(
                Some(spec_id.to_string()),
                "",
                events::EventKind::UnshippedWorkDetected {
                    spec: spec_id.to_string(),
                    branch: branch.to_string(),
                    first_seen: first_seen.clone(),
                },
            ),
        );
    }
    first_seen
}

// trace:STORY-1043 | ai:codex
#[cfg(test)]
mod story_1043_unshipped_work_tests {
    use super::*;

    fn git(root: &std::path::Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git should run");
        assert!(
            output.status.success(),
            "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn commit_file(root: &std::path::Path, path: &str, body: &str, subject: &str) {
        let full = root.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&full, body).unwrap();
        git(root, &["add", path]);
        git(root, &["commit", "-m", subject]);
    }

    fn branch_with_commit(root: &std::path::Path, branch: &str, spec: &str) {
        git(root, &["checkout", "-b", branch, "main"]);
        commit_file(
            root,
            &format!("{branch}.txt"),
            branch,
            &format!("[AI:codex] feat: branch work ({spec})"),
        );
        git(root, &["checkout", "main"]);
    }

    fn init_repo(root: &std::path::Path) {
        git(root, &["init"]);
        git(root, &["checkout", "-b", "main"]);
        git(root, &["config", "user.email", "codex@example.test"]);
        git(root, &["config", "user.name", "Codex"]);
        git(
            root,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/example/aida-fixture.git",
            ],
        );
        commit_file(root, "README.md", "fixture", "chore: init");
    }

    fn executable_fake_gh(root: &std::path::Path, body: &str) -> std::path::PathBuf {
        let fake_gh = root.join("fake-gh");
        crate::test_exec::write_executable(&fake_gh, body);
        fake_gh
    }

    fn summary(spec_id: &str, status: &str) -> aida_core::RequirementSummary {
        aida_core::RequirementSummary {
            id: uuid::Uuid::new_v4(),
            spec_id: Some(spec_id.to_string()),
            agreed_id: Some(spec_id.to_string()),
            title: format!("{spec_id} title"),
            description: String::new(),
            status: status.to_string(),
            priority: "high".to_string(),
            owner: String::new(),
            assignee: None,
            feature: String::new(),
            req_type: "Story".to_string(),
            tags: Vec::new(),
            created_at: String::new(),
            modified_at: chrono::Utc::now().to_rfc3339(),
            archived: false,
            archived_at: None,
            deferred: false,
            deferred_at: None,
            deferred_until: None,
            deferred_reason: None,
            in_degree: 0,
            out_degree: 0,
            heft: 0,
            blocked: false,
            has_pending_decision: false,
            execution_mode: None,
            weight: None,
            origin: None,
            completed_at: None, // trace:TASK-1474 | ai:claude
            yaml_path: String::new(),
        }
    }

    // BUG-1288: stale refs must not enlarge the expensive probe set. This is
    // a deterministic operation-count regression test rather than a flaky
    // wall-clock assertion. trace:BUG-1288 | ai:codex
    #[test]
    fn unshipped_candidate_gate_is_constant_with_stale_branch_count() {
        let active = vec!["bug-1288".to_string(), "story-42".to_string()];
        let mut refs: Vec<String> = (0..10_000).map(|n| format!("old-feature-{n}")).collect();
        refs.extend([
            "bug-1288".to_string(),
            "bug-1288-work".to_string(),
            "story-42-retry".to_string(),
            "story-420".to_string(),
        ]);

        let candidates: Vec<_> = refs
            .iter()
            .filter(|branch| branch_belongs_to_active_work(branch, &active))
            .cloned()
            .collect();
        assert_eq!(
            candidates,
            ["bug-1288", "bug-1288-work", "story-42-retry"],
            "only active spec branches reach per-branch git probes"
        );
    }

    fn write_live_lease(root: &std::path::Path, spec: &str, branch: &str) {
        let lease = SessionLease {
            id: "live1043".to_string(),
            scope: spec.to_string(),
            slug: slugify(spec),
            owner: "codex@example.test".to_string(),
            worktree_path: root.canonicalize().unwrap(),
            branch: branch.to_string(),
            started_at: chrono::Utc::now(),
            hostname: hostname(),
            role: Some("implementer".to_string()),
            creator_pid: None,
            creator_pid_start_time: None,
            active_pid: Some(std::process::id()),
            active_pid_start_time: process_probe::process_start_identity(std::process::id()),
            cargo_target_dir: None,
            parent_project_root: Some(root.to_path_buf()),
            pr_head_sha: None,
            pr_base_sha: None,
            pr_base_ref: None,
            zen_intent_token: None,
            escalated_to_human: None,
            parent_branch: None,
            parent_branch_sha: None,
            review_verb: false,
            claim_verb: false,
            manual_enter_at: None,
            interrupted_at: None,
            interrupted_reason: None,
        };
        std::fs::create_dir_all(leases_dir(root)).unwrap();
        std::fs::write(
            lease_path(root, "live1043"),
            toml::to_string_pretty(&lease).unwrap(),
        )
        .unwrap();
    }

    // The fake `gh` used here is a `#!/usr/bin/env bash` script stubbed via
    // AIDA_TEST_GH_BINARY; Windows cannot execute it through a shebang, so the
    // open-PR branch is misread as unshipped. The detector itself is
    // platform-agnostic (real gh.exe works); only this harness is Unix-only.
    // trace:STORY-1043 | ai:claude
    #[cfg_attr(
        windows,
        ignore = "fake-gh harness is a bash script; gh cannot be stubbed via shebang on Windows"
    )]
    #[test]
    fn detector_lists_unshipped_work_and_skips_live_terminal_and_open_pr_branches() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);

        branch_with_commit(root, "story-1043-unshipped", "STORY-1043");
        branch_with_commit(root, "story-1044-live", "STORY-1044");
        branch_with_commit(root, "story-1045-done", "STORY-1045");
        branch_with_commit(root, "story-1046-open-pr", "STORY-1046");
        branch_with_commit(root, "story-1047-remote", "STORY-1047");
        git(
            root,
            &[
                "update-ref",
                "refs/remotes/origin/story-1047-remote",
                "refs/heads/story-1047-remote",
            ],
        );
        git(root, &["branch", "-D", "story-1047-remote"]);
        write_live_lease(root, "STORY-1044", "story-1044-live");

        let fake_gh = executable_fake_gh(
            root,
            r#"#!/usr/bin/env bash
if [[ "$*" == *"pr list"* ]]; then
  printf '[{"number":46,"title":"open","headRefName":"story-1046-open-pr","state":"OPEN","statusCheckRollup":[],"mergeable":"MERGEABLE","reviewDecision":""}]'
  exit 0
fi
exit 1
"#,
        );

        let _env = crate::test_env::EnvVarsGuard::set(&[(
            "AIDA_TEST_GH_BINARY",
            fake_gh.to_str().unwrap(),
        )]);
        let rows = collect_unshipped_work_items(
            root,
            &[
                summary("STORY-1043", "InProgress"),
                summary("STORY-1044", "InProgress"),
                summary("STORY-1045", "Completed"),
                summary("STORY-1046", "InProgress"),
                summary("STORY-1047", "InProgress"),
            ],
            false,
            false,
        );

        assert_eq!(rows.len(), 2);
        let local = rows
            .iter()
            .find(|row| row.branch == "story-1043-unshipped")
            .unwrap();
        assert_eq!(local.spec_id, "STORY-1043");
        assert_eq!(local.commits_ahead, 1);
        assert_eq!(local.pr_state, "absent");
        // BUG-1756: this fixture branch was never pushed (no
        // refs/remotes/origin counterpart), so it is the local-only state and
        // its hint must name the push first. trace:BUG-1756 | ai:claude
        assert!(!local.pushed);
        assert_eq!(
            local.recovery,
            "git push -u origin story-1043-unshipped && aida pr ship story-1043-unshipped"
        );

        let remote = rows
            .iter()
            .find(|row| row.branch == "origin/story-1047-remote")
            .unwrap();
        assert_eq!(remote.spec_id, "STORY-1047");
        assert_eq!(remote.commits_ahead, 1);
        assert!(remote.pushed); // trace:BUG-1756 | ai:claude
        assert_ne!(remote.age, "unknown");
        assert_eq!(
            remote.recovery,
            "git switch -c story-1047-remote origin/story-1047-remote && aida pr ship story-1047-remote"
        );
    }

    // BUG-1756 AC1 + AC3 + AC4: a LOCAL-ONLY committed branch (ahead of
    // main, ≥1 commit, no PR, never pushed) is reported as unshipped work,
    // with its state distinguished from a pushed-no-PR branch — the two need
    // different next actions (push then open a PR vs open a PR), and the
    // local-only hint must name the push first: a reader must never be told
    // to `aida pr ship` a ref the forge cannot see. The pushed branch is the
    // AC4 control — its classification must stay byte-identical to what the
    // detector reported before this fix. trace:BUG-1756 | ai:claude
    #[cfg_attr(
        windows,
        ignore = "fake-gh harness is a bash script; gh cannot be stubbed via shebang on Windows"
    )]
    #[test]
    fn detector_distinguishes_local_only_from_pushed_no_pr_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);

        branch_with_commit(root, "bug-1756-localonly", "BUG-1756");
        branch_with_commit(root, "bug-1757-pushed", "BUG-1757");
        // Simulate a pushed branch: the remote-tracking ref exists alongside
        // the local one (what a real `git push -u origin` leaves behind).
        git(
            root,
            &[
                "update-ref",
                "refs/remotes/origin/bug-1757-pushed",
                "refs/heads/bug-1757-pushed",
            ],
        );

        let fake_gh = executable_fake_gh(
            root,
            r#"#!/usr/bin/env bash
printf '[]'
"#,
        );
        let _env = crate::test_env::EnvVarsGuard::set(&[(
            "AIDA_TEST_GH_BINARY",
            fake_gh.to_str().unwrap(),
        )]);
        let rows = collect_unshipped_work_items(
            root,
            &[
                summary("BUG-1756", "InProgress"),
                summary("BUG-1757", "InProgress"),
            ],
            false,
            false,
        );

        assert_eq!(
            rows.len(),
            2,
            "both unshipped branches must report: {rows:?}"
        );
        let local_only = rows
            .iter()
            .find(|row| row.branch == "bug-1756-localonly")
            .expect("the never-pushed branch is the state the detector most needs to name");
        assert!(!local_only.pushed);
        assert_eq!(local_only.pr_state, "absent");
        assert_eq!(
            local_only.recovery,
            "git push -u origin bug-1756-localonly && aida pr ship bug-1756-localonly"
        );

        let pushed = rows
            .iter()
            .find(|row| row.branch == "bug-1757-pushed")
            .expect("a pushed-no-PR branch keeps reporting exactly as before");
        assert!(pushed.pushed);
        assert_eq!(pushed.pr_state, "absent");
        assert_eq!(pushed.recovery, "aida pr ship bug-1757-pushed");
    }

    // BUG-1756 (the observed incident): the per-candidate `--head` network
    // round trips used to run UP FRONT for every candidate branch, which on
    // a working repository consumed the entire wall-clock budget before a
    // single branch was locally probed — `aida awaiting` reported
    // `unshipped_work[0]` / `scanned: 0` while a finished local-only commit
    // sat on exactly one machine. Deterministic operation-count pin (no
    // wall-clock flakiness, same style as the BUG-1288 gate test): with
    // three candidates of which only one survives the local classification,
    // exactly ONE `--head` history query is issued, and it names the
    // survivor. trace:BUG-1756 | ai:claude
    #[cfg_attr(
        windows,
        ignore = "fake-gh harness is a bash script; gh cannot be stubbed via shebang on Windows"
    )]
    #[test]
    fn per_head_history_queries_run_only_for_surviving_candidates() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);

        branch_with_commit(root, "bug-9101-survivor", "BUG-9101");
        // A candidate the LOCAL pass excludes: live lease on its branch.
        branch_with_commit(root, "bug-9102-live", "BUG-9102");
        write_live_lease(root, "BUG-9102", "bug-9102-live");
        // A candidate the LOCAL pass excludes: no content beyond main.
        git(root, &["branch", "bug-9103-empty", "main"]);

        let call_log = root.join("gh-calls.log");
        let fake_gh = executable_fake_gh(
            root,
            &format!(
                r#"#!/usr/bin/env bash
echo "$@" >> "{}"
printf '[]'
"#,
                call_log.display()
            ),
        );
        let _env = crate::test_env::EnvVarsGuard::set(&[(
            "AIDA_TEST_GH_BINARY",
            fake_gh.to_str().unwrap(),
        )]);
        let rows = collect_unshipped_work_items(
            root,
            &[
                summary("BUG-9101", "InProgress"),
                summary("BUG-9102", "InProgress"),
                summary("BUG-9103", "InProgress"),
            ],
            false,
            false,
        );

        assert_eq!(rows.len(), 1, "only the survivor reports: {rows:?}");
        assert_eq!(rows[0].spec_id, "BUG-9101");

        let calls = std::fs::read_to_string(&call_log).unwrap_or_default();
        let head_queries: Vec<&str> = calls.lines().filter(|l| l.contains("--head")).collect();
        assert_eq!(
            head_queries.len(),
            1,
            "a per-head network query is paid only for candidates that survive \
             the local classification, never per candidate up front: {calls}"
        );
        assert!(
            head_queries[0].contains("bug-9101-survivor"),
            "the one --head query names the survivor: {calls}"
        );
    }

    // The merged PR can be on a sibling ref that pass 1 classifies as already
    // shipped (tree-identical to main), while the old ancestor still survives
    // git cherry. Its merged-head evidence must be fetched lazily for both
    // detector entry points. trace:TASK-1575 | ai:codex
    #[cfg_attr(windows, ignore = "fake-gh harness requires a Unix shebang")]
    #[test]
    fn shipped_sibling_carrier_represents_stale_ancestor_in_both_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);
        git(root, &["checkout", "-b", "task-1575"]);
        commit_file(
            root,
            "ancestor.txt",
            "ancestor\n",
            "fix: ancestor (TASK-1575)",
        );
        let ancestor = git_output_checked(root, &["rev-parse", "HEAD"]).unwrap();
        git(root, &["checkout", "-b", "task-1575-merged"]);
        commit_file(root, "carrier.txt", "carrier\n", "fix: carrier (TASK-1575)");
        let carrier = git_output_checked(root, &["rev-parse", "HEAD"]).unwrap();
        git(root, &["checkout", "main"]);
        std::fs::write(root.join("ancestor.txt"), "ancestor\n").unwrap();
        std::fs::write(root.join("carrier.txt"), "carrier\n").unwrap();
        git(root, &["add", "ancestor.txt", "carrier.txt"]);
        git(root, &["commit", "-m", "fix: squash (TASK-1575)"]);
        assert_eq!(
            git_output_checked(root, &["rev-parse", "main^{tree}"]).unwrap(),
            git_output_checked(root, &["rev-parse", "task-1575-merged^{tree}"]).unwrap()
        );

        let calls = root.join("gh-calls");
        let fake = executable_fake_gh(
            root,
            &format!(
                r#"#!/usr/bin/env bash
printf '%s\\n' "$*" >> '{}'
if [[ "$*" == *"--state open"* ]]; then printf '[]'; exit 0; fi
if [[ "$*" == *"--head task-1575-merged"* ]]; then
  printf '[{{"state":"MERGED","title":"fix (TASK-1575)","headRefName":"task-1575-merged","headRefOid":"{}"}}]'
else
  printf '[]'
fi
"#,
                calls.display(),
                carrier.trim()
            ),
        );
        let _env =
            crate::test_env::EnvVarsGuard::set(&[("AIDA_TEST_GH_BINARY", fake.to_str().unwrap())]);
        let summaries = [summary("TASK-1575", "InProgress")];

        let rows = collect_unshipped_work_items(root, &summaries, false, false);
        assert!(
            rows.iter().all(|row| row.branch != "task-1575"),
            "ancestor is represented: {rows:?}; calls: {}",
            std::fs::read_to_string(&calls).unwrap_or_default()
        );

        let (rows, scan) = collect_unshipped_work_items_bounded(
            root,
            &summaries,
            false,
            false,
            Some(std::time::Instant::now() + std::time::Duration::from_secs(30)),
        );
        assert!(scan.complete, "generous deadline should finish: {scan:?}");
        assert!(
            rows.iter().all(|row| row.branch != "task-1575"),
            "ancestor is represented: {rows:?}"
        );
        assert_eq!(ancestor.trim().len(), 40);
        let calls = std::fs::read_to_string(calls).unwrap();
        assert!(
            calls.contains("--head task-1575-merged --state all"),
            "carrier history queried: {calls}"
        );
    }

    // When pass 2 truncates after an evidenced local-only survivor, an
    // incomplete carrier must not filter that survivor and shift a later,
    // never-evidenced pushed survivor into the count-based kept prefix.
    // trace:TASK-1575 | ai:codex
    #[cfg_attr(windows, ignore = "fake-gh harness requires a Unix shebang")]
    #[test]
    fn incomplete_carrier_cannot_promote_never_evidenced_survivor() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);
        git(root, &["checkout", "-b", "task-1575"]);
        commit_file(
            root,
            "ancestor.txt",
            "ancestor\n",
            "fix: ancestor (TASK-1575)",
        );
        git(root, &["checkout", "-b", "task-1575-carrier"]);
        commit_file(root, "carrier.txt", "carrier\n", "fix: carrier (TASK-1575)");
        let carrier = git_output_checked(root, &["rev-parse", "HEAD"]).unwrap();
        git(root, &["checkout", "main"]);
        std::fs::write(root.join("ancestor.txt"), "ancestor\n").unwrap();
        std::fs::write(root.join("carrier.txt"), "carrier\n").unwrap();
        git(root, &["add", "ancestor.txt", "carrier.txt"]);
        git(root, &["commit", "-m", "fix: squash (TASK-1575)"]);
        assert_eq!(
            git_output_checked(root, &["rev-parse", "main^{tree}"]).unwrap(),
            git_output_checked(root, &["rev-parse", "task-1575-carrier^{tree}"]).unwrap()
        );
        branch_with_commit(root, "task-1582-pushed", "TASK-1582");
        let pushed = git_output_checked(root, &["rev-parse", "task-1582-pushed"]).unwrap();
        git(
            root,
            &[
                "update-ref",
                "refs/remotes/origin/task-1582-pushed",
                pushed.trim(),
            ],
        );

        let calls = root.join("gh-calls");
        let fake = executable_fake_gh(
            root,
            &format!(
                r#"#!/usr/bin/env bash
printf '%s\\n' "$*" >> '{}'
if [[ "$*" == *"--state open"* ]]; then printf '[]'; exit 0; fi
if [[ "$*" == *"--head task-1575"* ]]; then sleep 2.5; fi
printf '[]'
"#,
                calls.display()
            ),
        );
        let _env =
            crate::test_env::EnvVarsGuard::set(&[("AIDA_TEST_GH_BINARY", fake.to_str().unwrap())]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let (rows, scan) = collect_unshipped_work_items_bounded(
            root,
            &[
                summary("TASK-1575", "InProgress"),
                summary("TASK-1582", "InProgress"),
            ],
            false,
            false,
            Some(deadline),
        );

        assert!(
            !scan.complete,
            "deadline and incomplete carrier evidence must be reported: {scan:?}"
        );
        assert!(
            rows.iter().all(|row| row.branch != "task-1582-pushed"),
            "the later survivor never received --head evidence and must not be reported: {rows:?}"
        );
        assert_eq!(carrier.trim().len(), 40);
        let calls = std::fs::read_to_string(calls).unwrap();
        assert!(
            calls.contains("--head task-1575 --state all"),
            "first survivor queried: {calls}"
        );
        assert!(
            !calls.contains("--head task-1582-pushed"),
            "later survivor skipped: {calls}"
        );
    }

    // Failure of the batched forge snapshot also makes carrier evidence
    // incomplete; dropping same-spec survivors must mark the scan incomplete.
    // trace:TASK-1575 | ai:codex
    #[cfg_attr(windows, ignore = "fake-gh harness requires a Unix shebang")]
    #[test]
    fn missing_forge_snapshot_marks_carrier_scan_incomplete() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);
        git(root, &["checkout", "-b", "task-1575"]);
        commit_file(
            root,
            "ancestor.txt",
            "ancestor\n",
            "fix: ancestor (TASK-1575)",
        );
        git(root, &["checkout", "-b", "task-1575-carrier"]);
        commit_file(root, "carrier.txt", "carrier\n", "fix: carrier (TASK-1575)");
        git(root, &["checkout", "main"]);
        std::fs::write(root.join("ancestor.txt"), "ancestor\n").unwrap();
        std::fs::write(root.join("carrier.txt"), "carrier\n").unwrap();
        git(root, &["add", "ancestor.txt", "carrier.txt"]);
        git(root, &["commit", "-m", "fix: squash (TASK-1575)"]);

        let fake = executable_fake_gh(
            root,
            "#!/usr/bin/env bash\nif [[ \"$*\" == *\"--state open\"* ]]; then exit 1; fi\nprintf '[]'\n",
        );
        let _env =
            crate::test_env::EnvVarsGuard::set(&[("AIDA_TEST_GH_BINARY", fake.to_str().unwrap())]);
        let (rows, scan) = collect_unshipped_work_items_bounded(
            root,
            &[summary("TASK-1575", "InProgress")],
            false,
            false,
            Some(std::time::Instant::now() + std::time::Duration::from_secs(30)),
        );

        assert!(
            rows.is_empty(),
            "missing representation evidence suppresses the spec: {rows:?}"
        );
        assert!(
            !scan.complete,
            "a missing forge snapshot must make the scan honestly incomplete: {scan:?}"
        );
    }

    // BUG-1756: a single anciently-forked candidate can cost `git cherry`
    // tens of seconds — several times the whole scan budget — and before the
    // per-candidate slice it starved every candidate after it (observed live:
    // the scan stuck at the same truncation point under 5s, 10s and 15s
    // budgets because one review-snapshot ref cost ~30s). The monster must
    // burn at most its slice, count as UNSCANNED (incomplete scan, PRIN-5),
    // and the normal candidates around it must still classify and report.
    // trace:BUG-1756 | ai:claude
    #[cfg_attr(
        windows,
        ignore = "fake-git harness is a bash script; git cannot be shimmed via shebang on Windows"
    )]
    #[test]
    fn one_expensive_candidate_cannot_starve_the_scan() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);
        branch_with_commit(root, "bug-9105-monster", "BUG-9105");
        branch_with_commit(root, "bug-9106-normal", "BUG-9106");

        // A PATH-shim git that stalls ONLY the monster's `cherry` probe and
        // execs the real git for everything else.
        let real_git = std::env::var_os("PATH")
            .and_then(|paths| {
                std::env::split_paths(&paths)
                    .map(|dir| dir.join("git"))
                    .find(|p| p.is_file())
            })
            .expect("git on PATH");
        let real_git = real_git.display();
        let shim_dir = root.join("git-shim");
        std::fs::create_dir_all(&shim_dir).unwrap();
        crate::test_exec::write_executable(
            &shim_dir.join("git"),
            &format!(
                r#"#!/usr/bin/env bash
is_cherry=""
is_monster=""
for a in "$@"; do
  [ "$a" = "cherry" ] && is_cherry=1
  [ "$a" = "bug-9105-monster" ] && is_monster=1
done
if [ -n "$is_cherry" ] && [ -n "$is_monster" ]; then
  sleep 30
fi
exec "{real_git}" "$@"
"#
            ),
        );
        let path = format!(
            "{}:{}",
            shim_dir.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let _env = crate::test_env::EnvVarsGuard::set(&[("PATH", path.as_str())]);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        let (rows, scan) = collect_unshipped_work_items_bounded(
            root,
            &[
                summary("BUG-9105", "InProgress"),
                summary("BUG-9106", "InProgress"),
            ],
            true, // no_forge — this pin is about the LOCAL pass
            false,
            Some(deadline),
        );

        assert!(
            rows.iter().any(|r| r.branch == "bug-9106-normal"),
            "a normal candidate must still classify and report next to the monster: {rows:?}"
        );
        assert!(
            rows.iter().all(|r| r.branch != "bug-9105-monster"),
            "the timed-out candidate was never classified and must not be reported: {rows:?}"
        );
        assert!(
            !scan.complete,
            "a probe-capped candidate means the scan did NOT cover everything"
        );
        assert_eq!(
            scan.scanned,
            scan.candidates - 1,
            "exactly the monster counts as unscanned: {scan:?}"
        );
    }

    // BUG-1288: `collect_unshipped_work_items_bounded`'s deadline must be
    // honest about a truncated scan, never collapse it into an empty/zero
    // result. An already-elapsed deadline is the deterministic way to pin
    // this — no wall-clock race, no flakiness — and stands in for what a
    // slow, candidate-heavy repo does to the real 2s production budget.
    // trace:BUG-1288 | ai:claude
    #[test]
    fn detector_reports_an_incomplete_scan_honestly_instead_of_hiding_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);
        branch_with_commit(root, "story-9001-first", "STORY-9001");
        branch_with_commit(root, "story-9002-second", "STORY-9002");

        // Already in the past — the bounded scan must not probe a single
        // candidate branch.
        let deadline = std::time::Instant::now();
        let (rows, scan) = collect_unshipped_work_items_bounded(
            root,
            &[
                summary("STORY-9001", "InProgress"),
                summary("STORY-9002", "InProgress"),
            ],
            true, // no_forge — no gh dependency for this assertion
            false,
            Some(deadline),
        );

        assert!(
            rows.is_empty(),
            "an elapsed-before-start deadline must probe nothing: {rows:?}"
        );
        assert!(
            !scan.complete,
            "a truncated scan must say so, not read as an exhaustive empty result"
        );
        assert_eq!(scan.scanned, 0);
        assert_eq!(
            scan.candidates, 2,
            "the candidate COUNT must still reflect the full eligible set, even though \
             none of it was actually probed — narrowing what's reported is not the same \
             as narrowing what's eligible"
        );
    }

    #[cfg_attr(
        windows,
        ignore = "fake-gh harness is a bash script; gh cannot be stubbed via shebang on Windows"
    )]
    #[test]
    fn detector_skips_merged_pr_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);
        branch_with_commit(root, "story-1187-merged-pr", "STORY-1187");
        let head = git_output_checked(root, &["rev-parse", "story-1187-merged-pr"]).unwrap();

        let fake_gh = executable_fake_gh(
            root,
            &format!(
                r#"#!/usr/bin/env bash
if [[ "$*" == *"pr list"* ]]; then
  printf '[{{"number":1877,"title":"merged","headRefName":"story-1187-merged-pr","headRefOid":"{}","state":"MERGED","statusCheckRollup":[],"mergeable":"UNKNOWN","reviewDecision":""}}]'
  exit 0
fi
exit 1
"#,
                head.trim()
            ),
        );

        let _env = crate::test_env::EnvVarsGuard::set(&[(
            "AIDA_TEST_GH_BINARY",
            fake_gh.to_str().unwrap(),
        )]);
        let rows = collect_unshipped_work_items(
            root,
            &[summary("STORY-1187", "InProgress")],
            false,
            false,
        );

        assert!(
            rows.iter().all(|row| row.branch != "story-1187-merged-pr"),
            "merged PR heads must not be reported as unshipped: {rows:?}"
        );
    }

    #[test]
    fn detector_skips_patch_equivalent_squash_merged_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);

        git(root, &["checkout", "-b", "story-1187-squash", "main"]);
        commit_file(
            root,
            "squash.txt",
            "same patch\n",
            "[AI:codex] feat: branch work (STORY-1187)",
        );
        git(root, &["checkout", "main"]);
        commit_file(
            root,
            "squash.txt",
            "same patch\n",
            "[AI:codex] feat: landed equivalent patch (STORY-1187)",
        );

        assert_eq!(
            branch_ahead_of(root, "story-1187-squash", "main"),
            Some(1),
            "raw rev-list still sees the old branch as ahead"
        );
        assert_eq!(
            branch_unshipped_patch_count_default(root, "story-1187-squash"),
            Some(0),
            "patch-equivalence count must see it as shipped"
        );

        let rows =
            collect_unshipped_work_items(root, &[summary("STORY-1187", "InProgress")], true, false);

        assert!(
            rows.iter().all(|row| row.branch != "story-1187-squash"),
            "patch-equivalent branches must not be reported as unshipped: {rows:?}"
        );
    }

    // BUG-1531 criterion 1 + 4: a local `pr-<digits>` review-snapshot branch
    // (the one shape `ReviewForge::local_branch_for` creates, per TASK-1312's
    // parser) is a PUBLISHED snapshot and must never be reported as unshipped
    // work — regardless of naming collisions with other branches. A
    // genuinely unshipped, non-snapshot branch for a different spec must
    // still report, so the fix does not suppress the whole channel. The
    // "PR-2035" summary/branch is a self-contained fixture (no real PR
    // needed) matching the advisor's note that this shape should be
    // constructed, not depended on an accidental branch.
    // trace:BUG-1531 | ai:claude
    #[cfg_attr(
        windows,
        ignore = "fake-gh harness is a bash script; gh cannot be stubbed via shebang on Windows"
    )]
    #[test]
    fn detector_excludes_review_snapshot_branch_but_still_reports_genuine_unshipped_work() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);

        // The snapshot: a spec whose own id happens to be "PR-2035" so the
        // branch `pr-2035` clears the active-work candidate gate on its own,
        // exactly the way a real snapshot's spec id clears it via commit
        // trailers.
        branch_with_commit(root, "pr-2035", "PR-2035");
        // A genuinely unshipped, unrelated branch that must keep reporting.
        branch_with_commit(root, "story-1531-unshipped", "STORY-1531");
        let pr_2035_tip = git_output_checked(root, &["rev-parse", "pr-2035"]).unwrap();

        // The forge CONFIRMS PR 2035 exists and its head sha equals the
        // branch tip — the only condition BUG-1531's rework allows the
        // name-shaped `pr-2035` branch to be excluded on.
        let fake_gh = executable_fake_gh(
            root,
            &format!(
                r#"#!/usr/bin/env bash
if [[ "$*" == *"pr list"* ]]; then
  printf '[]'
  exit 0
fi
if [[ "$*" == *"pr view 2035"* ]]; then
  printf '{{"state":"MERGED","title":"t","mergedAt":null,"baseRefName":"main","headRefName":"pr-2035","headRefOid":"{}","isCrossRepository":false,"headRepository":null,"isDraft":false}}'
  exit 0
fi
exit 1
"#,
                pr_2035_tip.trim()
            ),
        );
        let _env = crate::test_env::EnvVarsGuard::set(&[(
            "AIDA_TEST_GH_BINARY",
            fake_gh.to_str().unwrap(),
        )]);
        let rows = collect_unshipped_work_items(
            root,
            &[
                summary("PR-2035", "InProgress"),
                summary("STORY-1531", "InProgress"),
            ],
            false,
            false,
        );

        assert!(
            rows.iter().all(|row| row.branch != "pr-2035"),
            "a forge-confirmed review snapshot (matching head sha) must never be reported as unshipped: {rows:?}"
        );
        let genuine = rows
            .iter()
            .find(|row| row.branch == "story-1531-unshipped")
            .expect("a genuinely unshipped branch must still report");
        assert_eq!(genuine.spec_id, "STORY-1531");
        // BUG-1756: never pushed in this fixture → push-first hint.
        assert_eq!(
            genuine.recovery,
            "git push -u origin story-1531-unshipped && aida pr ship story-1531-unshipped"
        );
    }

    // BUG-1531 PROXY DECISION: with NO forge available (`no_forge = true`),
    // a name-shaped `pr-N` branch cannot be confirmed, so it must be KEPT —
    // never hidden on name alone (PRIN-5) — and labelled unverified with no
    // "aida pr ship" hint.
    // trace:BUG-1531 | ai:claude
    #[test]
    fn detector_keeps_review_snapshot_named_branch_unverified_with_no_forge() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);

        branch_with_commit(root, "pr-2036", "PR-2036");

        let rows = collect_unshipped_work_items(
            root,
            &[summary("PR-2036", "InProgress")],
            true, // no_forge
            false,
        );

        let row = rows
            .iter()
            .find(|row| row.branch == "pr-2036")
            .expect("with no forge, a name-shaped pr-N branch must be kept, not hidden");
        assert_eq!(row.spec_id, "PR-2036");
        assert_eq!(
            row.recovery,
            "possible review snapshot (pr-2036), unverified"
        );
        assert!(
            !row.recovery.contains("aida pr ship"),
            "an unverified row must carry no ship hint: {row:?}"
        );
    }

    // BUG-1531 PROXY DECISION: the forge resolves PR 2037, but its recorded
    // head sha does NOT match this branch's tip (e.g. the change moved since
    // the fetch, or the name is a coincidence). The branch must be KEPT and
    // labelled unverified rather than excluded.
    // trace:BUG-1531 | ai:claude
    #[cfg_attr(
        windows,
        ignore = "fake-gh harness is a bash script; gh cannot be stubbed via shebang on Windows"
    )]
    #[test]
    fn detector_keeps_review_snapshot_named_branch_unverified_when_forge_head_differs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);

        branch_with_commit(root, "pr-2037", "PR-2037");

        let fake_gh = executable_fake_gh(
            root,
            r#"#!/usr/bin/env bash
if [[ "$*" == *"pr list"* ]]; then
  printf '[]'
  exit 0
fi
if [[ "$*" == *"pr view 2037"* ]]; then
  printf '{"state":"OPEN","title":"t","mergedAt":null,"baseRefName":"main","headRefName":"pr-2037","headRefOid":"0000000000000000000000000000000000dead","isCrossRepository":false,"headRepository":null,"isDraft":false}'
  exit 0
fi
exit 1
"#,
        );
        let _env = crate::test_env::EnvVarsGuard::set(&[(
            "AIDA_TEST_GH_BINARY",
            fake_gh.to_str().unwrap(),
        )]);
        let rows =
            collect_unshipped_work_items(root, &[summary("PR-2037", "InProgress")], false, false);

        let row = rows.iter().find(|row| row.branch == "pr-2037").expect(
            "a forge-resolved PR whose head sha differs from the branch tip must be kept, not excluded",
        );
        assert_eq!(row.spec_id, "PR-2037");
        assert_eq!(
            row.recovery,
            "possible review snapshot (pr-2037), unverified"
        );
        assert!(
            !row.recovery.contains("aida pr ship"),
            "an unverified row must carry no ship hint: {row:?}"
        );
    }

    // BUG-1531 criterion 3 + 6: a refusal binds to a COMMIT, not to a PR
    // number or a branch-naming convention. A branch whose tip is the exact
    // sha an outstanding (unclosed) RequestChanges verdict names must never
    // carry a "ship it" recommendation — driven from a verdict fixture
    // rather than from any particular PR number, so the check generalizes
    // past the specific pr2035-review incident that surfaced it.
    // trace:BUG-1531 | ai:claude
    #[test]
    fn detector_omits_ship_hint_for_commit_carrying_unresolved_review_verdict() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);

        branch_with_commit(root, "bug-1470-work", "BUG-1470");
        let tip = git_output_checked(root, &["rev-parse", "bug-1470-work"]).unwrap();

        let verdict_dir = root.join(".aida").join("review-verdicts");
        std::fs::create_dir_all(&verdict_dir).unwrap();
        std::fs::write(
            verdict_dir.join("BUG-1470.json"),
            format!(
                r#"{{"verdict":"request-changes","reviewed_sha":"{}","reviewed_branch":"bug-1470-work","recorded_at":"2026-09-21T05:00:00Z","summary":"blocking defects found"}}"#,
                tip.trim()
            ),
        )
        .unwrap();

        let rows =
            collect_unshipped_work_items(root, &[summary("BUG-1470", "InProgress")], true, false);

        for row in &rows {
            assert!(
                !row.recovery.contains("aida pr ship"),
                "a commit carrying an unresolved review verdict must never get a ship hint: {row:?}"
            );
        }
        assert!(
            rows.iter().any(|row| row.branch == "bug-1470-work"),
            "the refused branch should still surface as unshipped, just without a ship hint: {rows:?}"
        );
    }

    // PR #1999 rework: the reviewer's CHANGES REQUESTED finding on BUG-1288
    // was that `active_prefixes` (the bounded-probe candidate gate) was built
    // from only "inprogress"/"in-progress"/"done" statuses, so a branch whose
    // spec sat in NeedsAttention, Approved, or Draft — with genuine unshipped
    // commits and NO lease — was filtered out before the commit/patch probe
    // ever ran, and the detector under-reported. This end-to-end fixture pins
    // exactly that shape for all three previously-invisible statuses.
    // trace:BUG-1288 | ai:claude
    #[test]
    fn detector_lists_unshipped_work_on_leaseless_nonterminal_branches() {
        for status in ["NeedsAttention", "Approved", "Draft"] {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            init_repo(root);

            let branch = format!("bug-9001-{}", status.to_ascii_lowercase());
            branch_with_commit(root, &branch, "BUG-9001");
            // Deliberately no lease and no live session: this is exactly the
            // "shelved but has real unshipped work" shape from the finding.

            let rows = collect_unshipped_work_items(
                root,
                &[summary("BUG-9001", status)],
                true, // no_forge: isolate from gh entirely
                false,
            );

            assert_eq!(
                rows.len(),
                1,
                "status={status}: a lease-less {status} branch with unmerged \
                 commits must be reported as unshipped, got: {rows:?}"
            );
            assert_eq!(rows[0].spec_id, "BUG-9001");
            assert_eq!(rows[0].branch, branch);
        }
    }

    // trace:BUG-1576 | ai:codex
    #[cfg_attr(windows, ignore = "fake-gh harness requires a Unix shebang")]
    #[test]
    fn detector_suppresses_multi_commit_squash_merge_from_recorded_pr_head() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);
        git(root, &["checkout", "-b", "bug-1576-squashed", "main"]);
        commit_file(root, "one.txt", "one\n", "fix: first half (BUG-1576)");
        commit_file(root, "two.txt", "two\n", "fix: second half (BUG-1576)");
        let head = git_output_checked(root, &["rev-parse", "HEAD"]).unwrap();
        git(root, &["checkout", "main"]);
        std::fs::write(root.join("one.txt"), "one\n").unwrap();
        std::fs::write(root.join("two.txt"), "two\n").unwrap();
        git(root, &["add", "one.txt", "two.txt"]);
        git(root, &["commit", "-m", "fix: squash landing (BUG-1576)"]);
        std::fs::write(root.join("one.txt"), "one\nlater main edit\n").unwrap();
        git(root, &["add", "one.txt"]);
        git(root, &["commit", "-m", "chore: advance main"]);

        assert_eq!(
            branch_unshipped_patch_count_default(root, "bug-1576-squashed"),
            Some(2)
        );
        let body = format!(
            r#"#!/usr/bin/env bash
printf '[{{"state":"MERGED","title":"fix (BUG-1576)","headRefName":"bug-1576-squashed","headRefOid":"{}"}}]'
"#,
            head.trim()
        );
        let fake = executable_fake_gh(root, &body);
        let _env =
            crate::test_env::EnvVarsGuard::set(&[("AIDA_TEST_GH_BINARY", fake.to_str().unwrap())]);
        let rows =
            collect_unshipped_work_items(root, &[summary("BUG-1576", "InProgress")], false, false);
        assert!(
            rows.is_empty(),
            "squash-merged work must not produce a ship hint: {rows:?}"
        );
    }

    // trace:BUG-1576 | ai:codex
    #[cfg_attr(windows, ignore = "fake-gh harness requires a Unix shebang")]
    #[test]
    fn detector_handles_regular_merge_and_keeps_open_or_unknown_work_safe() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);
        branch_with_commit(root, "bug-1577-regular", "BUG-1577");
        git(
            root,
            &[
                "merge",
                "--no-ff",
                "bug-1577-regular",
                "-m",
                "merge regular",
            ],
        );
        branch_with_commit(root, "bug-1578-abandoned", "BUG-1578");
        branch_with_commit(root, "bug-1578-real-head", "BUG-1578");
        branch_with_commit(root, "bug-1579-unknown", "BUG-1579");

        let fake = executable_fake_gh(
            root,
            r#"#!/usr/bin/env bash
printf '[{"state":"OPEN","title":"fix (BUG-1578)","headRefName":"bug-1578-real-head","headRefOid":"abc"}]'
"#,
        );
        let _env =
            crate::test_env::EnvVarsGuard::set(&[("AIDA_TEST_GH_BINARY", fake.to_str().unwrap())]);
        let rows = collect_unshipped_work_items(
            root,
            &[
                summary("BUG-1577", "InProgress"),
                summary("BUG-1578", "InProgress"),
                summary("BUG-1579", "InProgress"),
            ],
            false,
            false,
        );
        assert!(
            rows.iter().all(|r| r.spec_id != "BUG-1577"),
            "regular merge is shipped"
        );
        assert!(
            rows.iter().all(|r| r.spec_id != "BUG-1578"),
            "only the open PR head is authoritative and it is already open"
        );
        assert!(
            rows.iter().any(|r| r.spec_id == "BUG-1579"),
            "absent forge evidence must retain genuine work"
        );

        let ambiguous = parse_pr_head_state_snapshot(
            r#"[
          {"state":"OPEN","title":"one (BUG-1580)","headRefName":"bug-1580-one","headRefOid":"1"},
          {"state":"OPEN","title":"two (BUG-1580)","headRefName":"bug-1580-two","headRefOid":"2"}
        ]"#,
        )
        .unwrap();
        assert_eq!(
            ambiguous.open_heads_by_spec["BUG-1580"].len(),
            2,
            "ambiguity must remain explicit rather than selecting a head"
        );
    }

    // trace:BUG-1576 | ai:codex
    #[cfg_attr(windows, ignore = "fake-gh harness requires a Unix shebang")]
    #[test]
    fn merged_same_spec_does_not_hide_an_unrepresented_divergent_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);
        branch_with_commit(root, "bug-1576-divergent", "BUG-1576");
        branch_with_commit(root, "bug-1576-shipped", "BUG-1576");
        let shipped = git_output_checked(root, &["rev-parse", "bug-1576-shipped"]).unwrap();
        let body = format!(
            r#"#!/usr/bin/env bash
if [[ "$*" == *"--state open"* ]]; then printf '[]'; exit 0; fi
if [[ "$*" == *"--head bug-1576-shipped"* ]]; then
  printf '[{{"state":"MERGED","title":"fix (BUG-1576)","headRefName":"bug-1576-shipped","headRefOid":"{}"}}]'
else
  printf '[]'
fi
"#,
            shipped.trim()
        );
        let fake = executable_fake_gh(root, &body);
        let _env =
            crate::test_env::EnvVarsGuard::set(&[("AIDA_TEST_GH_BINARY", fake.to_str().unwrap())]);
        let rows =
            collect_unshipped_work_items(root, &[summary("BUG-1576", "InProgress")], false, false);
        assert!(rows.iter().any(|r| r.branch == "bug-1576-divergent"));
        assert!(rows.iter().all(|r| r.branch != "bug-1576-shipped"));
    }

    // trace:BUG-1576 | ai:codex
    #[cfg_attr(windows, ignore = "fake-gh harness requires a Unix shebang")]
    #[test]
    fn merged_head_lookup_is_targeted_not_limited_by_repository_history_page() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root);
        branch_with_commit(root, "bug-1576-ancient", "BUG-1576");
        let head = git_output_checked(root, &["rev-parse", "bug-1576-ancient"]).unwrap();
        let calls = root.join("gh-calls");
        let body = format!(
            r#"#!/usr/bin/env bash
printf '%s\n' "$*" >> '{}'
if [[ "$*" == *"--state open"* ]]; then printf '[]'; exit 0; fi
if [[ "$*" == *"--head bug-1576-ancient"* ]]; then
  printf '[{{"state":"MERGED","title":"ancient (BUG-1576)","headRefName":"bug-1576-ancient","headRefOid":"{}"}}]'
else
  printf '[]'
fi
"#,
            calls.display(),
            head.trim()
        );
        let fake = executable_fake_gh(root, &body);
        let _env =
            crate::test_env::EnvVarsGuard::set(&[("AIDA_TEST_GH_BINARY", fake.to_str().unwrap())]);
        let rows =
            collect_unshipped_work_items(root, &[summary("BUG-1576", "InProgress")], false, false);
        assert!(rows.is_empty());
        let calls = std::fs::read_to_string(calls).unwrap();
        assert!(
            calls.contains("--head bug-1576-ancient --state all"),
            "{calls}"
        );
        assert!(!calls.contains("--state all --limit 1000"), "{calls}");
    }
}

/// Look up a PR's merge state via `gh pr view <N>`. Returns `Some(true)`
/// when merged, `Some(false)` when open/closed-without-merge, `None` when
/// gh is missing or the call fails.
pub(crate) fn gh_pr_is_merged(project_root: &std::path::Path, n: u64) -> Option<bool> {
    let gh_bin = resolve_gh_binary()?;
    let out = std::process::Command::new(&gh_bin)
        .current_dir(project_root)
        .args([
            "pr",
            "view",
            &n.to_string(),
            "--json",
            "state",
            "-q",
            ".state",
        ])
        .output_retrying_etxtbsy()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout)
        .trim()
        .to_ascii_lowercase();
    Some(s == "merged")
}

/// Scan `~/.claude/projects/` for project dirs whose recorded cwd no
/// longer exists. Re-derived from the same detection logic as
/// `session_prune_orphans` so the cleanup surface stays consistent with
/// the cleanup command.
/// trace:STORY-385 | ai:claude
pub(crate) fn collect_orphan_project_dirs() -> Vec<status_cleanup::OrphanProjectDirItem> {
    let Some(home) = crate::home_dir() else {
        return Vec::new();
    };
    let projects = home.join(".claude/projects");
    if !projects.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let Ok(read) = std::fs::read_dir(&projects) else {
        return out;
    };
    for entry in read.filter_map(|e| e.ok()) {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let mut found_cwd: Option<String> = None;
        let mut jsonl_count = 0;
        if let Ok(read) = std::fs::read_dir(&dir) {
            for jsonl in read.filter_map(|e| e.ok()) {
                if jsonl.path().extension().and_then(|s| s.to_str()) != Some("jsonl") {
                    continue;
                }
                jsonl_count += 1;
                if found_cwd.is_none() {
                    if let Ok(text) = std::fs::read_to_string(jsonl.path()) {
                        for line in text.lines().take(20) {
                            if let Some(idx) = line.find("\"cwd\":\"") {
                                let after = &line[idx + 7..];
                                if let Some(end) = after.find('"') {
                                    found_cwd = Some(after[..end].to_string());
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
        let cwd_str = found_cwd.unwrap_or_else(|| {
            let name = dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            name.replace('-', "/")
        });
        if cwd_str.is_empty() {
            continue;
        }
        if std::path::Path::new(&cwd_str).is_dir() {
            continue;
        }
        out.push(status_cleanup::OrphanProjectDirItem {
            path: dir,
            decoded_cwd: cwd_str,
            jsonl_count,
        });
    }
    out
}

/// Parse a lease scope of the form `PR-<n>` (or `MR-<n>`) into its PR
/// number. Used to cross-reference reviewer leases against `gh pr view`
/// for the stale-on-merged detector.
pub(crate) fn parse_pr_scope(scope: &str) -> Option<u64> {
    let rest = scope
        .strip_prefix("PR-")
        .or_else(|| scope.strip_prefix("MR-"))?;
    rest.parse().ok()
}

/// True when `branch` matches the AIDA *work-branch* naming convention:
/// `<type>-<n>` for spec types that drive an implementer session
/// (`task`, `story`, `bug`, `epic`, `spike`, `spec`, `fr`, `nfr`,
/// `user`, `sr`), optionally with a `-suffix` or `.suffix`. PR/MR
/// branches (reviewer worktrees) are excluded — they surface via the
/// stale-reviewer-on-merged detector.
/// trace:STORY-385 | ai:claude
pub(crate) fn is_work_spec_branch_name(branch: &str) -> bool {
    work_spec_id_from_branch(branch).is_some()
}

// trace:BUG-888 | ai:codex
pub(crate) fn work_spec_id_from_branch(branch: &str) -> Option<String> {
    const PREFIXES: &[&str] = &[
        "task-", "story-", "bug-", "epic-", "spike-", "spec-", "fr-", "nfr-", "user-", "sr-",
    ];
    let lower = branch.to_ascii_lowercase();
    for p in PREFIXES {
        if let Some(rest) = lower.strip_prefix(p) {
            let segments: Vec<&str> = rest.split(['-', '.']).filter(|s| !s.is_empty()).collect();
            if segments.is_empty() {
                continue;
            }
            if !segments
                .iter()
                .all(|s| s.chars().all(|c| c.is_ascii_alphanumeric()))
            {
                continue;
            }
            let spec_segment_count = if segments[0].chars().all(|c| c.is_ascii_digit()) {
                if segments
                    .get(1)
                    .is_some_and(|s| s.chars().all(|c| c.is_ascii_digit()))
                {
                    2
                } else {
                    1
                }
            } else {
                segments
                    .iter()
                    .position(|s| s.chars().all(|c| c.is_ascii_digit()))
                    .map(|idx| idx + 1)
                    .unwrap_or(0)
            };
            if spec_segment_count == 0 {
                continue;
            }
            let kind = p.trim_end_matches('-').to_ascii_uppercase();
            return Some(format!(
                "{}-{}",
                kind,
                segments[..spec_segment_count]
                    .iter()
                    .map(|s| s.to_ascii_uppercase())
                    .collect::<Vec<_>>()
                    .join("-")
            ));
        }
    }
    None
}

#[cfg(test)]
#[path = "tests/is_work_spec_branch_name_tests.rs"]
mod is_work_spec_branch_name_tests;

/// Build the full `CleanupReport` from the live project state. Each
/// detector is independent — when one fails (gh missing, git invocation
/// broken, store unreadable) it returns an empty vec rather than
/// aborting the whole report. trace:STORY-385 | ai:claude
pub(crate) fn collect_cleanup_report(
    project_root: &std::path::Path,
    store: &aida_core::models::RequirementsStore,
) -> status_cleanup::CleanupReport {
    let leases = list_leases(project_root);
    let now = chrono::Utc::now();
    let live_sessions = process_probe::probe_live_claude_sessions();

    // Per-lease state classification — reused by sticky-in-progress,
    // dormant, and stale-reviewer detectors.
    let lease_states: Vec<(SessionLease, LeaseState)> = leases
        .iter()
        // BUG-511: review-verb leases classify by creator PID, not worktree.
        .map(|l| (l.clone(), lease_state_for(l, &live_sessions, now)))
        .collect();

    // TASK-1056: batch the per-branch git probes the detectors below need into
    // two `git for-each-ref` calls — local branch tip-times and the remote
    // branch name set — instead of spawning a `git log -1 --format=%ct` per
    // local branch and a `git rev-parse --verify origin/<branch>` per branch.
    // On a fleet repo (hundreds of work branches) that collapses thousands of
    // git subprocesses into two. The derived values are identical, so the
    // rendered report is unchanged. trace:TASK-1056 | ai:claude
    let local_branch_times = collect_local_branch_commit_times(project_root);
    let remote_branches = collect_remote_branch_name_set(project_root);

    // ── Detector 1: Uncommitted WIP across every worktree ──
    // Skip the orphan-store worktree (`.aida-store`) — it's AIDA-managed,
    // dirty state there is the normal mid-write moment between
    // `aida add`/`aida edit` and the next `aida push`, not lost work.
    // trace:STORY-385 | ai:claude
    let worktrees = list_worktrees(project_root);
    let mut uncommitted_wip = Vec::new();
    for wt in &worktrees {
        if !wt.path.exists() {
            continue;
        }
        if wt.branch.as_deref() == Some("aida-store") {
            continue;
        }
        let dirty = worktree_dirty_entries(&wt.path);
        if dirty.is_empty() {
            continue;
        }
        let canon_path = wt.path.canonicalize().unwrap_or_else(|_| wt.path.clone());
        let scope = lease_states
            .iter()
            .find(|(l, _)| {
                let lp = l
                    .worktree_path
                    .canonicalize()
                    .unwrap_or_else(|_| l.worktree_path.clone());
                lp == canon_path
            })
            .map(|(l, _)| l.scope.clone());
        let branch = wt.branch.clone().unwrap_or_else(|| "(detached)".into());
        let age_hours = lease_states
            .iter()
            .find(|(l, _)| l.worktree_path == canon_path || l.worktree_path == wt.path)
            .map(|(l, _)| now.signed_duration_since(l.started_at).num_hours())
            .unwrap_or(0);
        uncommitted_wip.push(status_cleanup::UncommittedWipItem {
            worktree_path: wt.path.clone(),
            branch,
            scope,
            modified_files: dirty.len(),
            age_hours,
        });
    }
    uncommitted_wip.sort_by(|a, b| b.age_hours.cmp(&a.age_hours));

    // ── Detector 2: Sticky In-Progress specs (no Live/Dormant lease) ──
    let active_scopes: std::collections::HashSet<String> = lease_states
        .iter()
        .filter(|(_, s)| matches!(s, LeaseState::Live | LeaseState::Dormant))
        .map(|(l, _)| l.scope.clone())
        .collect();
    let mut sticky_in_progress = Vec::new();
    for req in &store.requirements {
        if !matches!(req.status, RequirementStatus::InProgress) {
            continue;
        }
        let Some(spec_id) = req.spec_id.as_deref() else {
            continue;
        };
        if active_scopes.contains(spec_id) {
            continue;
        }
        // STORY-385: parent EPICs / Visions / Folders flip to In Progress
        // when their children are working — the parent itself has no
        // branch and isn't "stuck". Skip these so the category surfaces
        // only specs that actually need a recovery action.
        // trace:STORY-385 | ai:claude
        if matches!(
            req.req_type,
            aida_core::RequirementType::Epic
                | aida_core::RequirementType::Vision
                | aida_core::RequirementType::Folder
                | aida_core::RequirementType::Meta
                | aida_core::RequirementType::Sprint
        ) {
            continue;
        }
        // STORY-385: derive the spec branch by convention (kebab-cased
        // spec id) and check whether it carries unpushed work. Branches
        // are typed `<type>-<num>` (e.g. `task-281`). Missing branches
        // are reported with `unpushed_commits == 0`.
        // trace:STORY-385 | ai:claude
        let branch_name = spec_id.to_ascii_lowercase();
        // TASK-1056: batched-map lookups replace per-spec `git rev-parse
        // --verify` probes. trace:TASK-1056 | ai:claude
        let local_exists = local_branch_times.contains_key(&branch_name);
        let (unpushed, pushed) = if local_exists {
            let upstream = format!("origin/{}", branch_name);
            let upstream_exists = remote_branches.contains(&branch_name);
            if upstream_exists {
                let ahead_upstream =
                    branch_ahead_of(project_root, &branch_name, &upstream).unwrap_or(0);
                let pushed_count = std::process::Command::new("git")
                    .arg("-C")
                    .arg(project_root)
                    .args(["rev-list", "--count", &format!("origin/main..{}", upstream)])
                    .output()
                    .ok()
                    .and_then(|o| {
                        if o.status.success() {
                            String::from_utf8_lossy(&o.stdout).trim().parse().ok()
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0);
                (ahead_upstream, pushed_count)
            } else {
                let ahead = branch_ahead_of(project_root, &branch_name, "origin/main")
                    .or_else(|| branch_ahead_of(project_root, &branch_name, "main"))
                    .unwrap_or(0);
                (ahead, 0)
            }
        } else {
            (0, 0)
        };
        sticky_in_progress.push(status_cleanup::StickyInProgressItem {
            spec_id: spec_id.to_string(),
            title: req.title.clone(),
            branch: if local_exists {
                Some(branch_name)
            } else {
                None
            },
            unpushed_commits: unpushed,
            pushed_commits: pushed,
            age_hours: None,
        });
    }

    // ── Detector 3 + 5: Open PRs + branches-ahead-no-PR (gh calls) ──
    let open_prs_snapshot = collect_open_prs(project_root);
    let open_pr_branches: std::collections::HashSet<String> =
        open_prs_snapshot.by_branch.keys().cloned().collect();
    // Every branch that has EVER had a PR — used to filter out
    // squash-merged branches whose local tip looks "ahead" of main but
    // already shipped. trace:STORY-385 | ai:claude
    let ever_pr_branches = collect_all_pr_head_branches(project_root);

    // STORY-385: branches-ahead-no-PR scans all local branches, which
    // becomes noisy in long-lived repos that accumulate hundreds of
    // historical branches. Filter to branches with a commit in the
    // recent window (default 30 days) AND skip the orphan store
    // branch — those carry archived history, not in-flight work.
    // trace:STORY-385 | ai:claude
    let recent_branch_window_secs: i64 = 30 * 24 * 3600;
    let now_secs = chrono::Utc::now().timestamp();
    let mut branches_ahead_no_pr = Vec::new();
    let default_ref = detect_default_branch_ref(project_root);
    if let Some(default_ref) = default_ref.as_deref() {
        // TASK-1056: one batched ahead-count map for every local branch,
        // replacing a `git rev-list --count <default>..<branch>` per surviving
        // branch in the loop below. Falls back per-branch for anything the
        // batch missed. trace:TASK-1056 | ai:claude
        let ahead_by_branch = collect_branch_ahead_of(project_root, default_ref);
        for branch in list_local_branches(project_root) {
            // Skip the default branch itself + the orphan store branch.
            if branch == "main" || branch == "master" || branch == "aida-store" {
                continue;
            }
            // Skip branches already in an open PR.
            if open_pr_branches.contains(&branch) {
                continue;
            }
            // Skip branches that have EVER had a PR (closed or merged).
            // A squash-merged branch's local tip remains "ahead" of main
            // because the squash gave its commits a different SHA; the
            // ever-PR set is the most reliable "this already shipped"
            // signal.
            if ever_pr_branches.contains(&branch) {
                continue;
            }
            // STORY-385: only flag branches whose name matches the
            // SPEC-ID convention for *work* branches — `task-<n>`,
            // `story-<n>`, `bug-<n>`, `epic-<n>`, `spike-<n>`, `spec-<n>`,
            // optionally with a `-suffix` like `-batch7`. The canonical
            // motivating example in the spec is `task-281`; long-lived
            // feature branches and ad-hoc agent-tool branches (e.g.
            // `claude/plan-...`) are not what this category is for.
            // `pr-<n>` / `mr-<n>` branches are reviewer-worktree state
            // and surface elsewhere (stale-reviewer-on-merged), so
            // exclude them here to avoid double-counting.
            // trace:STORY-385 | ai:claude
            if !is_work_spec_branch_name(&branch) {
                continue;
            }
            // Skip branches with no recent activity — the cleanup section
            // is for in-flight work, not historical archaeology.
            // TASK-1056: read the tip-commit time from the batched
            // for-each-ref map instead of a `git log -1 --format=%ct` per
            // branch. trace:TASK-1056 | ai:claude
            let last_commit_ts = local_branch_times.get(&branch).copied().unwrap_or(0);
            if now_secs - last_commit_ts > recent_branch_window_secs {
                continue;
            }
            // TASK-1056: batched-map ahead count, falling back to the
            // per-branch probe when the batch didn't cover this branch.
            // trace:TASK-1056 | ai:claude
            let ahead = match ahead_by_branch
                .get(&branch)
                .copied()
                .or_else(|| branch_ahead_of(project_root, &branch, default_ref))
            {
                Some(n) if n > 0 => n,
                _ => continue,
            };
            // Skip branches already surfaced as sticky-in-progress —
            // those are visible in their own category and double-listing
            // them is noise.
            let already_sticky = sticky_in_progress
                .iter()
                .any(|s| s.branch.as_deref() == Some(branch.as_str()));
            if already_sticky {
                continue;
            }
            // TASK-1056: batched-map membership replaces a per-branch
            // `git rev-parse --verify origin/<branch>`. trace:TASK-1056
            let upstream_exists = remote_branches.contains(&branch);
            branches_ahead_no_pr.push(status_cleanup::BranchAheadItem {
                branch,
                commits_ahead: ahead,
                has_upstream: upstream_exists,
            });
        }
    }
    branches_ahead_no_pr.sort_by(|a, b| b.commits_ahead.cmp(&a.commits_ahead));

    let mut open_prs: Vec<status_cleanup::OpenPrItem> =
        open_prs_snapshot.by_branch.values().cloned().collect();
    open_prs.sort_by_key(|p| p.number);

    // ── Detector 4: Missed auto-bump ──
    let landings = scan_default_branch_for_spec_landings(project_root, 200);
    let mut missed_auto_bump = Vec::new();
    for (spec_id, sha) in landings {
        let Some(req) = store.get_requirement_by_spec_id(&spec_id) else {
            continue;
        };
        if !matches!(req.status, RequirementStatus::Done) {
            continue;
        }
        missed_auto_bump.push(status_cleanup::MissedAutoBumpItem {
            spec_id,
            title: req.title.clone(),
            landing_sha: sha,
        });
    }

    // ── Detector 7: Stale reviewer leases on merged PRs ──
    // (Detected first so we can dedupe these out of the Dormant category
    // below — a reviewer lease on a merged PR is recoverable but the
    // merged-PR signal is more actionable.)
    // trace:STORY-385 | ai:claude
    let mut stale_reviewer_leases = Vec::new();
    let mut stale_reviewer_lease_ids: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for (l, _state) in &lease_states {
        if l.role.as_deref() != Some("reviewer") {
            continue;
        }
        let Some(pr_n) = parse_pr_scope(&l.scope) else {
            continue;
        };
        // Skip if the PR is still open — those show up under "Open PRs".
        if open_prs_snapshot
            .by_branch
            .values()
            .any(|p| p.number == pr_n)
        {
            continue;
        }
        let merged = gh_pr_is_merged(project_root, pr_n).unwrap_or(false);
        if !merged {
            continue;
        }
        let age_hours = now.signed_duration_since(l.started_at).num_hours();
        stale_reviewer_lease_ids.insert(l.id.clone());
        stale_reviewer_leases.push(status_cleanup::StaleReviewerLeaseItem {
            lease_id: l.id.clone(),
            pr_number: pr_n,
            worktree_path: l.worktree_path.clone(),
            age_hours,
        });
    }
    stale_reviewer_leases.sort_by(|a, b| b.age_hours.cmp(&a.age_hours));

    // ── Detector 6: Dormant leases ──
    let mut dormant_leases = Vec::new();
    for (l, state) in &lease_states {
        if !matches!(state, LeaseState::Dormant) {
            continue;
        }
        // STORY-385: stale-reviewer-on-merged is a strict refinement of
        // dormant — show the more actionable signal, suppress here.
        if stale_reviewer_lease_ids.contains(&l.id) {
            continue;
        }
        let age_hours = now.signed_duration_since(l.started_at).num_hours();
        // BUG-376: detect the "lingering implementer with done queue"
        // subcategory by looking up the lease's scope as a spec ID and
        // checking whether it has already reached Done / Completed.
        // Informational annotation only — the recovery verb is the same
        // as a plain dormant lease (`aida session end`). Cap the lookup
        // to implementer-role leases so reviewer / triage / advisor
        // dormancy doesn't get mis-tagged (their lifecycle relative to
        // the spec's status is different — a reviewer can be dormant on
        // a Done spec entirely legitimately while waiting for merge).
        // trace:BUG-376 | ai:claude
        let spec_done = if matches!(l.role.as_deref(), Some("implementer")) {
            store.requirements.iter().any(|req| {
                req.spec_id.as_deref() == Some(l.scope.as_str())
                    && matches!(
                        req.status,
                        RequirementStatus::Done | RequirementStatus::Completed
                    )
            })
        } else {
            false
        };
        dormant_leases.push(status_cleanup::DormantLeaseItem {
            lease_id: l.id.clone(),
            scope: l.scope.clone(),
            role: l.role.clone(),
            worktree_path: l.worktree_path.clone(),
            age_hours,
            spec_done,
        });
    }
    dormant_leases.sort_by(|a, b| b.age_hours.cmp(&a.age_hours));

    // ── Detector 8: Orphan Claude Code project dirs ──
    let orphan_project_dirs = collect_orphan_project_dirs();

    // ── Detector 9 (STORY-469 Guard 3): claimed-Done-vs-substrate divergence ──
    // A spec whose status is Done/Completed but whose local reality contradicts
    // the claim — an agent's "I shipped" the substrate doesn't corroborate. Two
    // signals fire (see `detect_claimed_done_divergence`): (1) an active lease +
    // a dirty worktree (uncommitted work despite Done), (2) no commit references
    // the spec AND no PR exists. We build a filesystem-derived input row per
    // Done/Completed spec, then the pure detector decides. trace:STORY-469
    // BUG-606: full-history, body-aware corroboration set. The prior call here
    // capped at 200 commits AND read subjects only, so every Done/Completed spec
    // older than ~200 commits — or referenced only in a squash-merge BODY — was
    // falsely reported as "no commit references it" (~1464 false positives on
    // this repo). The missed-auto-bump detector above keeps the recent-window
    // landing scan (it needs the landing sha and only cares about recent Done
    // specs). trace:BUG-606 | ai:claude
    let landed_spec_ids = referenced_spec_ids_on_default_branch(project_root);
    // Map a spec's active-lease worktree (Live/Dormant) to its dirty count.
    let mut claimed_done_inputs = Vec::new();
    for req in &store.requirements {
        if !matches!(
            req.status,
            RequirementStatus::Done | RequirementStatus::Completed
        ) {
            continue;
        }
        let Some(spec_id) = req.spec_id.as_deref() else {
            continue;
        };
        let active_lease = lease_states.iter().find(|(l, s)| {
            l.scope == spec_id && matches!(s, LeaseState::Live | LeaseState::Dormant)
        });
        let (has_active_lease, branch, modified_files, age_hours) = match active_lease {
            Some((l, _)) => {
                let dirty = if l.worktree_path.exists() {
                    worktree_dirty_entries(&l.worktree_path).len()
                } else {
                    0
                };
                (
                    true,
                    Some(l.branch.clone()),
                    dirty,
                    now.signed_duration_since(l.started_at).num_hours(),
                )
            }
            None => (false, None, 0, 0),
        };
        let branch_name = spec_id.to_ascii_lowercase();
        // BUG-606: corroborate by spec_id OR agreed_id — a commit may reference
        // the short display id (TASK-216) while the spec is stored under its
        // node-aware id, or vice versa.
        let has_commit = landed_spec_ids.contains(&spec_id.to_ascii_uppercase())
            || req
                .agreed_id
                .as_deref()
                .map(|a| landed_spec_ids.contains(&a.to_ascii_uppercase()))
                .unwrap_or(false);
        let has_pr =
            open_pr_branches.contains(&branch_name) || ever_pr_branches.contains(&branch_name);
        claimed_done_inputs.push(status_cleanup::ClaimedDoneInput {
            spec_id: spec_id.to_string(),
            title: req.title.clone(),
            status: format!("{:?}", req.status),
            has_active_lease,
            branch,
            modified_files,
            age_hours,
            has_commit,
            has_pr,
        });
    }
    let claimed_done_diverged =
        status_cleanup::detect_claimed_done_divergence(&claimed_done_inputs);

    status_cleanup::CleanupReport {
        uncommitted_wip,
        sticky_in_progress,
        branches_ahead_no_pr,
        missed_auto_bump,
        open_prs,
        dormant_leases,
        stale_reviewer_leases,
        orphan_project_dirs,
        claimed_done_diverged,
        // STORY-508/TASK-651: resolve the active forge so the open-change hint
        // names the right CLI (gh/glab) or none (pure-git).
        forge_kind: Some(crate::forge::resolve_forge_kind(project_root)),
    }
}

/// Gather every "human-gate" item — mergeable PRs the operator still
/// needs to merge, briefs filed for the running agent, findings awaiting
/// triage, reviewer-queue verdicts, `NeedsAttention` escalations, unread
/// mail, and pending worker directives —
/// into the structured report rendered as the "Awaiting you" section.
/// Empty (and so hidden) on a quiet day; that absence is the signal.
/// trace:STORY-465 | ai:claude
pub(crate) fn collect_awaiting_report(
    project_root: &std::path::Path,
    backend: &aida_core::CachedGitBackend,
    ctx: &UserStatusContext,
    no_ci: bool,
) -> awaiting_you::AwaitingReport {
    collect_awaiting_report_inner(project_root, backend, ctx, no_ci, false)
}

pub(crate) fn collect_awaiting_report_inner(
    project_root: &std::path::Path,
    backend: &aida_core::CachedGitBackend,
    ctx: &UserStatusContext,
    no_ci: bool,
    notice_fast: bool,
) -> awaiting_you::AwaitingReport {
    // BUG-1288: the nightly channel performs independent forge reads. Start it
    // while the PR/store/local channels are collected so network latency is
    // paid once rather than serially on the machine-readable path.
    // trace:BUG-1288 | ai:codex
    let nightly_handle = if no_ci {
        None
    } else {
        let root = project_root.to_path_buf();
        Some(std::thread::spawn(move || cached_nightly_red_status(&root)))
    };
    // Mergeable PRs — reuse the same `gh pr list` snapshot that the cleanup
    // report consumes, then filter via the awaiting-you classifier.
    // STORY-1397: every active hold is read once, READ-ONLY — this render
    // (and the per-turn `--notice` hook) never rewrites a marker, writes a
    // brief, or probes process liveness; `aida merge-hold list --fix` owns
    // that. A held PR is never offered as ready-to-merge, and every held PR
    // stays visible with its typed hold reason. trace:STORY-1397 | ai:claude
    let hold_records: Vec<merge_hold::MergeHoldRecord> = merge_hold::list_holds(project_root)
        .into_iter()
        .filter_map(|(pr, _)| merge_hold::read_hold_record(project_root, pr))
        .collect();
    let held_numbers: std::collections::HashSet<u64> =
        hold_records.iter().map(|record| record.pr).collect();
    let open_prs: Option<Vec<status_cleanup::OpenPrItem>> = (!no_ci).then(|| {
        collect_open_prs(project_root)
            .by_branch
            .into_values()
            .collect()
    });
    let mergeable_prs = match &open_prs {
        None => Vec::new(),
        Some(all_prs) => {
            let prs: Vec<_> = all_prs
                .iter()
                .filter(|pr| !held_numbers.contains(&pr.number))
                .cloned()
                .collect();
            let local_suppressed = local_suppressed_prs(project_root, &prs);
            let mut items = awaiting_you::classify_open_prs(&prs, &local_suppressed);
            // STORY-1405: a mergeable PR a reviewer is mid-way through says so.
            // Local file reads only; the head comes from the same snapshot.
            // trace:STORY-1405 | ai:claude
            let marker_root = main_worktree_root_from(project_root);
            for item in &mut items {
                let head = prs
                    .iter()
                    .find(|p| p.number == item.number)
                    .and_then(|p| p.head_sha.as_deref());
                item.under_review =
                    review_marker::live_description(&marker_root, item.number, head);
            }
            items
        }
    };
    // Non-recusal holds (supervision / rework / decision / unreadable) on
    // OPEN PRs: shown with their reason and whose action it is. Needs the PR
    // snapshot to tell an open PR from a stale marker, so the notice path
    // (no snapshot) skips it exactly as it skips `mergeable_prs`.
    let mut held_prs: Vec<awaiting_you::HeldPrItem> = match &open_prs {
        None => Vec::new(),
        Some(all_prs) => hold_records
            .iter()
            .filter(|record| record.reason_kind != merge_hold::HoldReasonKind::Recusal)
            .filter_map(|record| {
                all_prs.iter().find(|pr| pr.number == record.pr).map(|pr| {
                    // trace:BUG-1788 | ai:antigravity
                    let approved_at_head = pr.head_sha.as_deref().filter(|head| {
                        crate::review_verdict::verdicts_for_sha(project_root, head)
                            .into_iter()
                            .any(|v| !v.is_closed() && v.kind.approves())
                    });
                    awaiting_you::project_held_pr(record, &pr.title, approved_at_head, pr.is_draft)
                })
            })
            .collect(),
    };

    // STORY-1397: typed recusal holds are local/offline coordination state, so
    // they remain visible even on the fast notice path. A recused principal is
    // explicitly told this is awaiting somebody else, never offered a merge.
    // A moved head is PROJECTED as stale in memory; nothing is written here.
    // The principal comes from the self-declared AIDA_AGENT_ID and only
    // personalizes this read-only view (see `merge_hold::current_principal`).
    let current_principal = merge_hold::current_principal()
        .map(|p| p.key())
        .unwrap_or_default();
    let live_pr_heads: std::collections::HashMap<u64, &str> = open_prs
        .iter()
        .flatten()
        .filter_map(|pr| pr.head_sha.as_deref().map(|sha| (pr.number, sha)))
        .collect();
    let recusal_holds = hold_records
        .iter()
        .filter(|record| record.reason_kind == merge_hold::HoldReasonKind::Recusal)
        .map(|record| merge_hold::project_live_head(record, live_pr_heads.get(&record.pr).copied()))
        .filter_map(|record| awaiting_you::project_recusal_hold(&record, &current_principal))
        .collect();

    // Pending briefs — prefer narrowing to the running agent so we
    // don't spam the operator with hand-offs filed for a different
    // agent. `detect_agent_type` returns "other" when no agent env is
    // present (raw shell), in which case we surface every unacked brief
    // so the operator orchestrating multiple agents still sees the
    // backlog of hand-offs.
    let agent_type = agent_registry::detect_agent_type();
    let brief_filter = match agent_type.as_str() {
        "claude" | "codex" | "antigravity" => Some(agent_type.as_str()),
        _ => None,
    };
    // BUG-569: bare agent-type filter for the status surface — stay silent on
    // type-class ambiguity.
    let pending_briefs = collect_agent_briefs_inner(project_root, brief_filter, false, false)
        .ok()
        .unwrap_or_default()
        .into_iter()
        .map(|e| awaiting_you::PendingBriefItem {
            agent: e.agent,
            spec_id: e.spec_id,
            path: e.path,
        })
        .collect();

    // Escalations need the full summary list; findings need a draft-only view.
    // trace:TASK-1526 | ai:codex
    // Advisory reads use a zero-wait labelled snapshot. Compatible full-rebuild
    // cases defer, preserving the no-full-scan notice contract.
    let summaries = if notice_fast {
        backend.list_summaries_with_budget(
            &aida_core::ListFilter::default(),
            aida_core::db::cache_refresh::ReadBudget(std::time::Duration::ZERO),
        )
    } else {
        backend.list_summaries(&aida_core::ListFilter::default())
    }
    .unwrap_or_default();
    // BUG-1773: a rework hold DERIVED from the verdict corpus, for an open PR
    // with no marker of its own. Only `handle_review_record_at` writes a
    // `.aida/merge-holds/PR-<n>` marker, so a refusal recorded by any other
    // producer (the reviewer skill's direct write, `stamp_pr_review_verdict`,
    // a hand-edited file) was invisible here — BUG-1705 measured exactly that
    // on PR #2242, which sat CLEAN and unlisted with `request-changes` at its
    // exact head. The predicate is the POSITIVE form (an outstanding refusal AT
    // the current head), not `local_verdict_blocks_merge`, which is the
    // fail-closed merge test and would list every PR carrying a stale verdict.
    //
    // STORY-1397's contract is preserved: this reads the corpus and writes
    // nothing — no marker, no label. Arming the gate is BUG-1774.
    // trace:BUG-1773 | ai:claude
    if let Some(all_prs) = open_prs.as_ref() {
        let mut status_by_spec = std::collections::HashMap::new();
        insert_summary_statuses(&mut status_by_spec, &summaries);
        let candidates: Vec<awaiting_you::CorpusHoldCandidate> = all_prs
            .iter()
            .map(|pr| {
                let spec = worktree_lease::spec_id_from_branch(&pr.head_branch);
                let mut keys = vec![format!("PR-{}", pr.number)];
                keys.extend(spec.clone());
                let bodies = keys
                    .iter()
                    .map(|key| review_verdict::verdict_path(project_root, key))
                    .filter_map(|path| std::fs::read_to_string(path).ok())
                    .collect();
                let spec_completed = spec.as_deref().is_some_and(|id| {
                    status_by_spec
                        .get(&id.to_ascii_uppercase())
                        .is_some_and(|status| status == "completed")
                });
                awaiting_you::CorpusHoldCandidate {
                    pr: pr.number,
                    title: pr.title.clone(),
                    head_sha: pr.head_sha.clone(),
                    bodies,
                    spec_completed,
                    has_marker_hold: held_numbers.contains(&pr.number),
                    is_draft: pr.is_draft,
                }
            })
            .collect();
        held_prs.extend(awaiting_you::corpus_held_prs(&candidates));
    }
    // BUG-472: the findings breadcrumb must mirror `aida findings list` — DRAFT
    // specs carrying a from-* tag only. Building it from the unfiltered
    // `summaries` also counts completed/rejected specs that still carry their
    // origin from-review/from-implementer tag, overcounting the count (status
    // said "35" while `aida findings list` showed 0). Filter to draft like
    // print_status_findings_section does. trace:BUG-472 | ai:claude
    let findings_total = {
        let filter = aida_core::ListFilter {
            status: Some("draft".to_string()),
            ..Default::default()
        };
        let draft = if notice_fast {
            backend.list_summaries_with_budget(
                &filter,
                aida_core::db::cache_refresh::ReadBudget(std::time::Duration::ZERO),
            )
        } else {
            backend.list_summaries(&filter)
        }
        .unwrap_or_default();
        findings::count_findings(&findings::build_findings_view(
            &draft,
            &findings::FindingsFilter::default(),
        ))
    };
    // trace:STORY-1023 | ai:codex
    let mut shelved_total = 0usize;
    let mut escalations: Vec<awaiting_you::EscalationItem> = Vec::new();
    for s in summaries
        .iter()
        .filter(|s| s.status.eq_ignore_ascii_case("NeedsAttention"))
    {
        let spec_id = s
            .agreed_id
            .clone()
            .or_else(|| s.spec_id.clone())
            .unwrap_or_else(|| "?".to_string());
        match backend.get_requirement_by_spec_id(&spec_id) {
            Ok(Some(req)) if req.failure_reason.is_some() => {
                shelved_total += 1;
            }
            _ => escalations.push(awaiting_you::EscalationItem {
                spec_id,
                title: s.title.clone(),
            }),
        }
    }

    // Reviewer-queue items — surface queue entries where the verdict is
    // the operator's only when the active role IS reviewer. Otherwise
    // these would just duplicate the Queue section below.
    //
    // BUG-1508 AC1/AC2/AC3/AC8: each routed row is annotated with its
    // actionability, resolved entirely locally (verdict file + git refs
    // via `reviewer_row_actionability` -- the same helper `aida queue
    // list --for reviewer` uses, and no forge call). Rows are never
    // dropped for being already-reviewed (AC2); the depth figure `aida
    // awaiting` leads with ("actionable N of M") is built from these
    // states in `render`/`compact_line`/`to_json` below.
    // trace:BUG-1508 | ai:claude
    let reviewer_queue_items: Vec<awaiting_you::ReviewerQueueItem> =
        if matches!(ctx.role.as_deref(), Some("reviewer")) {
            let leases = list_leases(project_root);
            ctx.queue_head
                .iter()
                .filter(|r| !r.in_progress)
                .map(|r| {
                    let state = backend
                        .get_requirement_by_spec_id(&r.spec_id)
                        .ok()
                        .flatten()
                        .map(|req| {
                            reviewer_row_actionability(project_root, &req, &leases, |uuid| {
                                backend
                                    .get_requirement(&uuid)
                                    .ok()
                                    .flatten()
                                    .and_then(|r| r.agreed_id.or(r.spec_id))
                            })
                        })
                        .unwrap_or(review_verdict::ReviewActionability::NeedsReview);
                    awaiting_you::ReviewerQueueItem {
                        spec_id: r.spec_id.clone(),
                        title: r.title.clone(),
                        state,
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

    // Unread mail — folded into the awaiting-you report so the coordination
    // inbox is ONE surface (STORY-741). Reads only the local + canonical
    // mailbox files (never a network call), so this stays cheap enough to ride
    // the per-turn `aida awaiting --notice` path. Every read degrades to empty
    // on error, never bubbling up.
    //
    // BUG-767: the headline number is the OPERATOR's OWN inbox — the handle
    // this session is (`AIDA_USER`/`USER`), and nothing else. The rest of the
    // ambient identity set (`inbox_identities()` also unions the session ROLE
    // and the AGENT TYPE) are SHARED inboxes carrying agent-to-agent traffic;
    // counting them as "your unread mail" made the `awaiting` number a function
    // of env vars rather than of your inbox, so an operator who emptied their
    // own inbox saw the count jump to the shared backlog instead of 0. They are
    // still surfaced — as a separate, labelled `shared_unread` line — so the
    // fleet-wide view survives without leaking into the operator-gated count.
    // trace:STORY-741 | ai:claude
    // trace:BUG-767 | ai:claude
    let mail = {
        let store_root = project_root.join(".aida-store");
        let local = mailbox_store::read_local_messages(project_root).unwrap_or_default();
        let canonical = mailbox_store::read_canonical_messages(&store_root).unwrap_or_default();
        let merged = aida_core::mailbox::merge_dedup(&local, &canonical);
        let watermarks = mailbox_store::read_all_watermarks(project_root).unwrap_or_default();
        let operator = current_user_id(None);
        let mut shared: Vec<String> = Vec::new();
        if let Some(raw) = ctx
            .role
            .clone()
            .or_else(|| std::env::var("AIDA_SESSION_ROLE").ok())
            .filter(|s| !s.trim().is_empty())
        {
            let (role, _is_default) = resolve_effective_role(Some(raw.as_str()));
            if role != operator {
                shared.push(role);
            }
        }
        awaiting_you::split_mail_scopes(&operator, &shared, &merged, &watermarks)
    };

    // Pending worker directives — the enqueue channel (`aida human audit`,
    // MCP directive posters, hand-written overnight plans) lands in the local
    // directive file, and until now only surfaced via the worker poll view.
    // Folding it in makes the unified inbox the one place the advisor looks.
    // A single local file read (absent file → empty), never a network call,
    // so it rides the per-turn notice path too. trace:TASK-1146 | ai:claude
    let worker_directives = {
        let directives = worker::parse_directives(&worker::worker_cmd_path(project_root));
        awaiting_you::DirectivesChannel {
            pending: directives.len(),
            next: directives.first().map(|d| d.summary()),
        }
    };

    // Due seat jobs from the `[schedule]` registry — the per-seat delivery
    // channel (STORY-1226). Scoped to the session's seat when one is known;
    // every seat otherwise (the operator orchestrating several seats sees
    // them all, like briefs). File-only: a TOML parse plus the store ledger /
    // local state files — never git, never the network — so it rides the
    // per-turn notice. Substrate jobs never surface here; the tick runs them.
    // trace:STORY-1226 | ai:claude
    let cron = {
        let seat = ctx
            .role
            .clone()
            .or_else(|| std::env::var("AIDA_SESSION_ROLE").ok())
            .filter(|s| !s.trim().is_empty())
            .map(|r| canonical_role_name(&r));
        let due = maintenance_schedule::due_seat_jobs(project_root, seat.as_deref());
        awaiting_you::CronChannel {
            due: due.len(),
            next: due.first().map(|j| j.line(chrono::Utc::now())),
        }
    };

    // The per-turn notice has a hard latency contract. Branch divergence walks
    // spawn git processes and can exceed that budget in a busy repository; the
    // full awaiting/status views retain this channel. trace:BUG-1239 | ai:codex
    let (unshipped_work, unshipped_work_scan) = if notice_fast {
        (Vec::new(), None)
    } else {
        // BUG-1288: bound the wall clock this scan may spend, not which
        // branches are eligible for it — the candidate set is exactly the
        // one PR #1999 (STORY-1368) widened it to. A repo whose candidate
        // population has since grown large enough to blow the budget gets a
        // truncated-but-honest scan (`unshipped_work_scan.complete: false`)
        // instead of a multi-minute block; see
        // `collect_unshipped_work_items_bounded`. trace:BUG-1288 | ai:claude
        let deadline = std::time::Instant::now() + unshipped_work_scan_budget();
        let (items, status) = collect_unshipped_work_items_bounded(
            project_root,
            &summaries,
            no_ci,
            !no_ci,
            Some(deadline),
        );
        (items, Some(status))
    };
    // trace:STORY-1043 | ai:codex
    let nightly_red = nightly_handle.and_then(|handle| handle.join().ok().flatten());

    // TASK-192: red PRs must not enter the green orphan-review sweep, but a
    // definitively failing PR with no verdict, hold, reviewer route, or live
    // branch owner still needs an operator-visible repair route. This is a
    // full-report channel only because it depends on the forge snapshot.
    // trace:TASK-192 | ai:codex
    let unowned_failing_prs = if notice_fast || no_ci {
        Vec::new()
    } else {
        let routed_prs = reviewer_queue_story_ids(project_root).map(|queued_story_ids| {
            let queued_story_ids: std::collections::HashSet<String> = queued_story_ids
                .into_iter()
                .map(|id| id.to_ascii_uppercase())
                .collect();
            summaries
                .iter()
                .filter(|summary| {
                    summary
                        .agreed_id
                        .as_deref()
                        .or(summary.spec_id.as_deref())
                        .is_some_and(|id| queued_story_ids.contains(&id.to_ascii_uppercase()))
                })
                .filter_map(|summary| parse_review_story_pr_number(&summary.title))
                .collect::<std::collections::HashSet<u64>>()
        });
        let live_branches = live_owned_branches(project_root);
        // BUG-1514: the general form of the two-way inconsistency (acceptance
        // #1) — a spec already marked Done whose PR title names it is red
        // regardless of who owns/reviews the branch. Built from the spec ids
        // in the PR title (the same trailer convention `aida pr` writes), so
        // it costs no extra `gh` calls beyond the snapshot already fetched.
        // trace:BUG-1514 | ai:claude
        let done_spec_ids: std::collections::HashSet<String> = summaries
            .iter()
            .filter(|s| s.status.eq_ignore_ascii_case("done"))
            .flat_map(|s| [s.spec_id.clone(), s.agreed_id.clone()])
            .flatten()
            .map(|id| id.to_ascii_uppercase())
            .collect();
        let candidates: Vec<awaiting_you::UnownedFailingPrCandidate> =
            collect_open_prs(project_root)
                .by_branch
                .into_values()
                .map(|pr| {
                    let done_spec = pr_ship::extract_spec_ids_from_text(&pr.title)
                        .into_iter()
                        .find(|id| done_spec_ids.contains(id));
                    awaiting_you::UnownedFailingPrCandidate {
                        has_local_verdict: pr_has_local_verdict(project_root, pr.number),
                        held: pr_has_merge_hold(project_root, &pr),
                        route: reviewer_route_for_pr(routed_prs.as_ref(), pr.number),
                        actively_owned: live_branches.contains(&pr.head_branch),
                        done_spec,
                        pr,
                    }
                })
                .collect();
        awaiting_you::classify_unowned_failing_prs(&candidates, chrono::Utc::now())
    };

    // STORY-1419: PRs whose rework has landed on a refusal this seat recorded.
    // Needs the PR snapshot for current heads, so it is skipped on the
    // notice-fast path and when CI/forge lookups are off — the same contract as
    // mergeable_prs. Reuses the MEMOIZED snapshot, so this adds no request.
    // trace:STORY-1419 | ai:claude
    // trace:BUG-1549 | ai:claude — shares the candidate build with rework_ready
    // below (same PR snapshot, same seat scoping); only the direction differs.
    let (rework_ready, stale_approvals, blocked_reviews) = if notice_fast || no_ci {
        (Vec::new(), Vec::new(), Vec::new())
    } else {
        let snapshot = collect_open_prs(project_root);
        let seat = std::env::var("AIDA_USER")
            .ok()
            .filter(|s| !s.trim().is_empty());
        // trace:STORY-1420 | ai:claude — an exited recorder's refusal is
        // shown to every seat instead of routed to nobody. Liveness is
        // registry + pid only.
        let reader = awaiting_you::ReworkReader {
            identity: seat.as_deref(),
        };
        let rows = pr_review_rows_routed(project_root, snapshot.by_branch.values(), reader);
        (
            rows.rework_ready,
            rows.stale_approvals,
            rows.blocked_reviews,
        )
    };

    // TASK-1445 (containment for BUG-1510 AC5): does a live drain's
    // lease-based PR attribution agree with what the PR's own commits
    // credit? Local-only (drain-state file + a git log per in-flight
    // member) — no network — so it's skipped on the notice-fast path for
    // the same latency reason as `unshipped_work` above.
    // trace:TASK-1445 | ai:claude
    let pr_attribution_disagreements = if notice_fast {
        Vec::new()
    } else {
        collect_pr_attribution_disagreements(project_root)
    };

    // BUG-1564: In-Progress specs with no live session/lease/process behind
    // the flag — reuses `gather_running_work`'s orphan pass verbatim (the
    // same verdict `aida ps` computes), so this is not a second
    // implementation of the detection. Needs a lease scan + a live-process
    // probe, the same "heavier local probe" tier as `unshipped_work` /
    // `pr_attribution_disagreements` above, so it is skipped on the
    // notice-fast path to keep the per-turn hook local and fast (no
    // full-store load, no network).
    // trace:BUG-1564 | ai:claude
    let orphaned_in_progress = if notice_fast {
        Vec::new()
    } else {
        let (_rows, orphans) = gather_running_work(project_root);
        orphaned_in_progress_items(orphans, |spec| {
            summaries
                .iter()
                .find(|s| {
                    s.agreed_id.as_deref() == Some(spec) || s.spec_id.as_deref() == Some(spec)
                })
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s.modified_at).ok())
                .map(|t| {
                    let secs = chrono::Utc::now()
                        .signed_duration_since(t.with_timezone(&chrono::Utc))
                        .num_seconds()
                        .max(0) as u64;
                    format!("last touched {} ago", humanize_duration_secs(secs))
                })
                .unwrap_or_else(|| "last-touched time unknown".to_string())
        })
    };

    // TASK-1454: cheap on every path (see doc comment) — computed
    // unconditionally, including on the notice-fast path, unlike
    // `orphaned_in_progress` above.
    let blocked_seats = collect_blocked_seat_items(project_root);

    awaiting_you::AwaitingReport {
        mergeable_prs,
        recusal_holds,
        held_prs,
        unowned_failing_prs,
        rework_ready,
        stale_approvals,
        blocked_reviews,
        pending_briefs,
        findings_total,
        reviewer_queue_items,
        escalations,
        mail,
        worker_directives,
        cron,
        shelved_total,
        unshipped_work,
        unshipped_work_scan,
        nightly_red,
        pr_attribution_disagreements,
        orphaned_in_progress,
        blocked_seats,
        // trace:BUG-1530 | ai:claude — the seat this session reads as, so the
        // headline (render) and per-turn compact line can scope themselves
        // to channels this seat can act on.
        role: ctx.role.clone(),
    }
}

/// STORY-741: render the unified "Awaiting you" report as a first-class
/// command. Two shapes:
///   - `--notice`: the COMPACT one-line per-turn signal the `UserPromptSubmit`
///     hook injects. Silent when nothing awaits. Cache/local-backed and
///     network-free — it forces the `no_ci` path (so the gh-backed PR probe
///     never runs) and builds a lightweight context (role from env, no
///     full-store load), so a hook firing every prompt stays cheap and
///     fail-open.
///   - default: the full multi-channel report (text / `--json`), including the
///     gh-backed PR channel unless `--no-ci`. Mirrors `aida status --awaiting`.
// trace:STORY-741 | ai:claude
/// STORY-769: emit the always-on "Current date/time + Timing" leading line for
/// `aida awaiting --notice`, and stamp the per-session last-human-input oracle.
/// Called EARLY in dispatch (before store init) so the line survives even when
/// the store/backend can't be resolved — the per-turn hook always gets its time
/// context and the escalation cascade always gets a fresh presence stamp.
///
/// The UserPromptSubmit / SessionStart hook relay passes its JSON payload in
/// `AIDA_HOOK_PAYLOAD` (`session_id`, `hook_event_name`). The command itself
/// must never read stdin: scripted callers may attach an open pipe whose writer
/// outlives this process. Missing or malformed hook metadata simply renders the
/// time line with no session key. Fail-open throughout.
pub(crate) fn emit_notice_time_line() {
    // trace:BUG-1239 | ai:codex
    let (session_id, is_session_start) = std::env::var("AIDA_HOOK_PAYLOAD")
        .ok()
        .map(|payload| presence::parse_hook_payload(&payload))
        .unwrap_or((None, false));
    if is_session_start {
        let project_root = std::env::current_dir()
            .ok()
            .and_then(|cwd| find_aida_project_root_from(&cwd).ok())
            .map(|root| main_worktree_root_from(&root));
        if let Some(project_root) = project_root {
            let binary = agent_registry::AgentBinaryIdentity::new(
                env!("CARGO_PKG_VERSION").to_string(),
                env!("AIDA_BUILD_GIT_SHA").to_string(),
            );
            let _ = agent_registry::touch_session_start_agent(
                &project_root,
                session_id.as_deref(),
                &binary,
            );
        }
    }
    let now = chrono::Local::now();
    let label = presence::stamp_turn_clock(
        session_id.as_deref(),
        is_session_start,
        now.with_timezone(&chrono::Utc),
    );
    // Local time, matching the trial hook's `%A %Y-%m-%d %H:%M %Z` shape.
    let when = now.format("%A %Y-%m-%d %H:%M %Z").to_string();
    println!("{}", format_notice_time_line(&when, &label));
    // STORY-1464: seat rotation signal — when this session's latest model call
    // crossed the context ceiling, tell the seat to hand off and restart.
    // Tail-read of the hook's transcript only; silent below the ceiling.
    // trace:STORY-1464 | ai:claude
    if let Ok(payload) = std::env::var("AIDA_HOOK_PAYLOAD") {
        let seat = std::env::var("AIDA_SESSION_ROLE")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(|s| canonical_role_name(&s))
            .unwrap_or_else(|| "<seat>".to_string());
        if let Some(line) = seat_rotation::notice_from_hook_payload(&payload, &seat) {
            println!("{line}");
        }
    }
}

/// Enforce the per-turn notice's fail-open latency contract across the whole
/// dispatch path. Most notice reads are deliberately cheap, but cache refresh,
/// lease discovery, or an unusually large protocol store can still stall after
/// this early dispatch point. A detached watchdog bounds all of those paths and
/// exits successfully because the notice is advisory. Arm this before the
/// time-line/session bookkeeping so that work is covered by the same bound.
// trace:BUG-1239 | ai:codex
// trace:TASK-1274 | ai:claude
pub(crate) fn arm_notice_deadline() {
    let Some(deadline) = notice_deadline() else {
        return;
    };
    std::thread::spawn(move || {
        std::thread::sleep(deadline);
        std::process::exit(0);
    });
}

/// The per-turn notice's fail-open bound. TASK-1274 traced a flaky leased-spec
/// reminder to its full-store protocol lookup racing this watchdog. BUG-1569
/// replaces that scan with targeted/cache-indexed reads, allowing the product
/// bound to remain meaningfully sub-second. The override exists only so
/// black-box tests can assert tighter latency budgets in their subprocesses.
pub(crate) fn notice_deadline() -> Option<std::time::Duration> {
    // Test-only escape hatch; a production caller never sets this. Zero
    // disables the watchdog so lifecycle/content tests can synchronize on the
    // command's completion instead of racing a wall-clock deadline under CI
    // load. Latency behavior remains covered by its dedicated black-box test.
    // trace:BUG-1567 | ai:codex
    if let Ok(ms) = std::env::var("AIDA_TEST_NOTICE_DEADLINE_MS") {
        if let Ok(ms) = ms.parse::<u64>() {
            return (ms != 0).then(|| std::time::Duration::from_millis(ms));
        }
    }
    const PRODUCT_NOTICE_DEADLINE: std::time::Duration = std::time::Duration::from_millis(750);
    Some(PRODUCT_NOTICE_DEADLINE)
}

/// BUG-1288: the wall-clock budget [`collect_unshipped_work_items_bounded`]
/// (and the `gh` probe it drives, [`collect_pr_head_state_snapshot_bounded`])
/// may spend probing candidate branches from `aida awaiting --json` / `aida
/// status --full`. Same escape-hatch shape as [`notice_deadline`]: a
/// production caller never sets the override.
// trace:BUG-1288 | ai:claude
pub(crate) fn unshipped_work_scan_budget() -> std::time::Duration {
    if let Ok(ms) = std::env::var("AIDA_TEST_UNSHIPPED_SCAN_BUDGET_MS") {
        if let Ok(ms) = ms.parse::<u64>() {
            return std::time::Duration::from_millis(ms);
        }
    }
    // BUG-1756: raised from BUG-1288's 2000ms. That number was sized when
    // the scan's dominant cost was one `gh pr list --head` network round
    // trip per candidate, and it protected poll latency by truncating —
    // which on a working repository (24 candidates, ~0.5s per gh call)
    // truncated at 0/N on every single run, leaving the channel effectively
    // dead. With the per-candidate network calls now lazy (survivors only),
    // the budget's job is to cover the CHEAP local pass (~0.2s of git per
    // candidate on this class of host, measured under its normal multi-agent
    // build load) plus a handful of survivor queries; 10s covers the
    // observed 26-candidate population at roughly 2× headroom, while still
    // bounding a pathological repo. The per-turn notice path skips this
    // channel entirely, so this budget never touches per-turn latency, and
    // the full `awaiting` report it rides already spends ~30s on its other
    // forge-backed channels. trace:BUG-1756 | ai:claude
    const PRODUCT_UNSHIPPED_SCAN_BUDGET: std::time::Duration =
        std::time::Duration::from_millis(10_000);
    PRODUCT_UNSHIPPED_SCAN_BUDGET
}

/// PURE: the notice's always-on leading line. Separated so the exact contract
/// shape is unit-testable without a clock or stdin.
pub(crate) fn format_notice_time_line(when: &str, label: &str) -> String {
    format!("Current date/time: {when}. Timing: {label}.")
}

pub(crate) fn handle_awaiting_command(
    notice: bool,
    json: bool,
    verbose: bool,
    no_ci: bool,
    backend: &aida_core::CachedGitBackend,
) -> Result<()> {
    let project_root = std::env::current_dir()?;

    if notice {
        // CHEAP per-turn path: NO full-store load, NO gh/network. Mail, briefs,
        // findings, worker directives and escalations are all cache/local
        // reads; PRs (gh) and the
        // reviewer-verdict channel (which needs the queue snapshot off the
        // loaded store) are deliberately omitted here — run bare `aida awaiting`
        // for those. The lightweight context carries only the role (from env),
        // so `collect_awaiting_report` sees an empty queue head and skips the
        // reviewer channel. Fail-open: a caught-up inbox prints nothing.
        let role = std::env::var("AIDA_SESSION_ROLE")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let ctx = UserStatusContext {
            session: None,
            role,
            branch: None,
            pr: None,
            queue_head: Vec::new(),
            queue_total: 0,
            agents: Vec::new(),
        };
        let report = collect_awaiting_report_inner(&project_root, backend, &ctx, true, true);
        if let Some(line) = report.compact_line() {
            println!("{line}");
        }
        // Re-teach a compacted session its type contract while this worktree
        // still holds the spec lease. `queue done` removes/releases that lease,
        // so the reminder naturally disappears. Fail-open on every read.
        // trace:STORY-1221 | ai:codex
        let canonical_cwd = project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.clone());
        let lease_root = find_main_worktree_root().unwrap_or_else(|_| project_root.clone());
        if let Some(scope) = list_leases(&lease_root)
            .into_iter()
            .find(|lease| canonical_cwd.starts_with(&lease.worktree_path))
            .map(|lease| lease.scope)
        {
            match protocol_cmd::targeted_notice_line_for_scope(backend, &scope) {
                Ok(Some(line)) => {
                    println!("{line}");
                }
                Ok(None) => {}
                Err(err) => {
                    eprintln!("warning: unable to resolve protocol notice for {scope}: {err:#}")
                }
            }
        }
        if let Some(line) = mailbox_latency_warning(&project_root, ctx.role.as_deref()) {
            println!("{line}");
        }
        let store_path = detect_distributed_store_from(&project_root)
            .unwrap_or_else(|| project_root.join(".aida-store"));
        if let Some(line) = status_cmd::absence_notice_line(&project_root, &store_path) {
            println!("{line}");
        }
        return Ok(());
    }

    // Full view: reuse the exact same machinery as `aida status --awaiting`.
    let store = backend.load()?;
    let ctx = collect_user_context(&project_root, &store, backend, no_ci);
    let report = collect_awaiting_report(&project_root, backend, &ctx, no_ci);
    if json {
        println!("{}", crate::cache_output::json_pretty(&report.to_json())?);
    } else if agent_output_mode() {
        // BUG-695: honor AIDA_AGENT_OUTPUT like `aida integrate`/`ps`/`status` —
        // emit token-efficient TOON (flat scalars + uniform tables) instead of the
        // human box-drawing header + status emoji. `--json` above still wins for
        // structured consumers; the `--notice` compact path returned earlier.
        //
        // BUG-1739: the body moved into `AwaitingReport::render_agent_view` so a
        // test can assert what it emits. While it was inline here, nine of the
        // contributors `total()` counts drifted out of it with no test able to
        // see that, and `awaiting: 6` printed above eight empty tables.
        // trace:BUG-695 | ai:claude
        // trace:BUG-1739 | ai:claude
        print!("{}", report.render_agent_view());
    } else if report.is_empty() {
        println!("{}", "─── Awaiting you (0) ───".bold().dimmed());
        println!("  Nothing awaits you right now.");
        println!();
    } else {
        let stdout = std::io::stdout();
        let _ = report.render(verbose, stdout.lock());
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/story_465_awaiting_report_tests.rs"]
mod story_465_awaiting_report_tests;

#[cfg(test)]
#[path = "tests/task_515_status_agent_lease_fallback_tests.rs"]
mod task_515_status_agent_lease_fallback_tests;

pub(crate) fn print_status_hygiene_section(
    project_root: &std::path::Path,
    store: &aida_core::models::RequirementsStore,
    verbose: bool,
    no_hygiene: bool,
) -> Result<()> {
    if no_hygiene {
        return Ok(());
    }

    let mut findings = collect_doctor_findings(project_root, store, None)?;

    // Filter findings
    if !verbose {
        findings.retain(|f| {
            matches!(
                f.category.as_str(),
                "spec-status-drift" | "stale-locks" | "orphan-worktrees" | "orphan-branches"
            )
        });
    }

    // TASK-673: a one-line count of Completed specs git can't corroborate. The
    // full listing + remediation lives in `aida doctor`; here we just surface
    // the integrity tripwire so the substrate polices its own completion claim
    // even on a quick `aida status`. Honors AIDA_DOCTOR_COMPLETED_SINCE for the
    // legacy-history exemption. trace:TASK-673 | ai:claude
    let integrity_since = std::env::var("AIDA_DOCTOR_COMPLETED_SINCE")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let uncorroborated =
        scan_completed_without_commit(project_root, store, integrity_since.as_deref()).len();

    if findings.is_empty() && uncorroborated == 0 {
        return Ok(());
    }

    // Sort findings by priority: spec-status-drift > stale-locks > orphan-worktrees > orphan-branches > others
    fn category_priority(cat: &str) -> usize {
        match cat {
            "spec-status-drift" => 0,
            "stale-locks" => 1,
            "orphan-worktrees" => 2,
            "orphan-branches" => 3,
            _ => 4,
        }
    }

    findings.sort_by(|a, b| {
        category_priority(&a.category)
            .cmp(&category_priority(&b.category))
            .then_with(|| a.category.cmp(&b.category))
            .then_with(|| a.id.cmp(&b.id))
    });

    let total = findings.len();
    let limit = if verbose { total } else { 2 };

    println!("{}", "─── Hygiene ───".bold());

    for finding in findings.iter().take(limit) {
        let heal_mode = if finding.safe_heal { "safe" } else { "manual" };
        println!(
            "  {} {}",
            crate::glyph(crate::glyphs::Glyph::Bullet),
            finding.category.cyan()
        );
        println!("    - {} [{}]", finding.summary, heal_mode.dimmed());
        println!("      {} {}", "→".yellow(), finding.action.dimmed());
    }

    if total > limit {
        let overflow = total - limit;
        println!(
            "    {} +{} more (run aida doctor)",
            crate::glyph(crate::glyphs::Glyph::Bullet),
            overflow
        );
    }

    if uncorroborated > 0 {
        println!(
            "  {} {} Completed spec{} with no corroborating commit (run {})",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            uncorroborated,
            if uncorroborated == 1 { "" } else { "s" },
            "aida doctor --category completed-without-commit".cyan()
        );
    }
    println!();

    Ok(())
}

/// True for summaries that represent real, user-authored requirements — i.e.
/// the rows `aida list` shows by default. META rows (AI-prompt customization
/// seeded by `aida init`, e.g. META-001..006) are plumbing, not work, and are
/// hidden from `aida list`'s default view (BUG-27). The `aida status`
/// Requirements panel must apply the same exclusion so its Total / per-status
/// counts reconcile with `aida list` instead of reporting phantom Draft rows
/// on a fresh store (BUG-415).
///
/// `req_type` is the cache's stored Debug form of `RequirementType`
/// (e.g. "Meta", "Task", "Bug"); the comparison is case-insensitive to be
/// robust against casing drift in the projection.
// trace:BUG-415 | ai:claude
pub(crate) fn is_real_requirement_summary(req_type: &str) -> bool {
    !req_type.eq_ignore_ascii_case("meta")
}

#[cfg(test)]
#[path = "tests/bug415_status_count_tests.rs"]
mod bug415_status_count_tests;

/// Whether a command MUTATES the shared store / queue (so a shared `"default"`
/// identity in a team context is genuinely hazardous — collides queues +
/// attribution). Reads only get a warning; writes can be refused behind
/// `AIDA_TEAM_REQUIRE_USER=1`. Conservative: anything not clearly a read is a
/// write. trace:STORY-640 | ai:claude
pub(crate) fn is_write_command(command: &Command) -> bool {
    matches!(
        command,
        Command::Do { .. }
            | Command::Add { .. }
            | Command::Edit { .. }
            | Command::Comment(_)
            | Command::Queue(_)
            | Command::Session(_)
            | Command::Defer { .. }
            | Command::Undefer { .. }
            | Command::Archive { .. }
            | Command::Unarchive { .. }
            | Command::Brief { .. }
    )
}

/// STORY-640: the team distinct-identity guard + join-the-team onboarding hint.
/// Both key off the shared node roster and are silent outside a team context.
/// Best-effort — never returns an error except the explicit opt-in refusal.
/// trace:STORY-640 | ai:claude
pub(crate) fn maybe_team_identity_guard(
    store_path: &std::path::Path,
    command: &Command,
) -> Result<()> {
    // `aida team` surfaces the same facts; the JSON status path must stay clean
    // for machine consumers. Skip both so the guard never pollutes them.
    if matches!(command, Command::Team { .. }) {
        return Ok(());
    }
    if let Command::Status { json: true, .. } = command {
        return Ok(());
    }
    // STORY-764: `--format json` pins the same machine surface; keep it clean too.
    // trace:STORY-764 | ai:claude
    if matches!(command, Command::Status { .. }) && output_format_is_json() {
        return Ok(());
    }

    let our_clone = team::our_clone_path(store_path);
    let registry = team::load_roster(store_path);
    let team_context = team::is_team_context(&registry, &our_clone);
    if !team_context {
        return Ok(()); // solo / single-own-node — no nag, ever.
    }

    // Onboarding: a fresh clone that joined an existing roster but never
    // acquired its own node id. One-time per command; a light hint, not a
    // wizard. Suppressible idempotently via a marker so we don't nag forever.
    if !team::clone_is_registered(&registry, &our_clone) {
        let project_root = store_path.parent().unwrap_or(store_path);
        let marker = project_root.join(".aida").join("team-onboarded");
        if !marker.exists() {
            let bullet = crate::glyph(crate::glyphs::Glyph::Bullet);
            eprintln!(
                "{} You've joined a shared AIDA store with {} other node(s). To keep your \
                 work attributable + your queue separate:\n  {bullet} set a distinct identity:  {}\n  \
                 {bullet} claim a node id:           {}",
                "Welcome:".cyan().bold(),
                registry.nodes.len(),
                "export AIDA_USER=<your-name>".cyan(),
                "aida node acquire".cyan(),
            );
            eprintln!();
            // Best-effort one-time suppression (dir may not exist yet).
            if let Some(parent) = marker.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&marker, "1\n");
        }
    }

    // Distinct-identity guard (BUG-89 hardening).
    let user_id = current_user_id(None);
    if team::identity_verdict(&user_id, team_context) == team::IdentityVerdict::DefaultInTeam {
        let require = std::env::var("AIDA_TEAM_REQUIRE_USER")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let writes = is_write_command(command);
        eprintln!(
            "{} This is a shared (team) store, but no distinct identity is set — you are \
             the BUG-89 '{}' fallback. A shared '{}' id collides queues + commit \
             attribution across machines.\n  Fix it for this shell:  {}",
            "warning:".yellow().bold(),
            "default".bold(),
            "default".bold(),
            "export AIDA_USER=<your-name>".cyan(),
        );
        if require && writes {
            anyhow::bail!(
                "refusing a write op on a team store with the shared '{}' identity \
                 (AIDA_TEAM_REQUIRE_USER=1). Set a distinct AIDA_USER, or unset \
                 AIDA_TEAM_REQUIRE_USER to downgrade this to a warning.",
                "default"
            );
        }
        eprintln!();
    }
    Ok(())
}

/// `aida identity link <a> <b>` (TASK-845): link two identity strings as one
/// canonical person in `registry/aliases.toml` on the store (CAS push). After
/// this, the queue / team roster / block list all resolve both strings to one
/// person. Idempotent — a redundant link reports "already linked" and commits
/// nothing.
// trace:TASK-845 | ai:claude
pub(crate) fn handle_identity_link(store_path: &std::path::Path, a: &str, b: &str) -> Result<()> {
    let fa = aida_core::node::canonical_user_id(a);
    let fb = aida_core::node::canonical_user_id(b);
    if fa.is_empty() || fb.is_empty() {
        anyhow::bail!("both identity strings must be non-empty");
    }
    if fa == fb {
        println!(
            "{} {} and {} are the same identity (after case-fold) — nothing to link.",
            "No change:".yellow().bold(),
            a.bold(),
            b.bold()
        );
        return Ok(());
    }
    let changed = aida_core::alias::link_cas(store_path, a, b)
        .map_err(|e| anyhow::anyhow!("could not write the identity link: {e}"))?;
    // Resolve to show the canonical person the two now share.
    let registry = aida_core::alias::AliasRegistry::load(store_path);
    let canonical = registry.resolve(a);
    if changed {
        println!(
            "{} {} {} {}  (canonical person: {})",
            "Linked:".green().bold(),
            a.bold(),
            crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
            b.bold(),
            canonical.cyan().bold()
        );
        println!(
            "  {}",
            "The queue, `aida team`, and `aida db block list` now resolve both to one person."
                .dimmed()
        );
    } else {
        println!(
            "{} {} and {} already resolve to one person ({}).",
            "Already linked:".yellow().bold(),
            a.bold(),
            b.bold(),
            canonical.cyan()
        );
    }
    Ok(())
}

/// `aida identity list` (TASK-845): show the recorded person links — one block
/// per canonical person with the aliases that resolve to them.
// trace:TASK-845 | ai:claude
pub(crate) fn handle_identity_list(store_path: &std::path::Path, json: bool) -> Result<()> {
    let registry = aida_core::alias::AliasRegistry::load(store_path);
    let people = registry.people();
    if json {
        let rows: Vec<serde_json::Value> = people
            .iter()
            .map(|(canonical, aliases)| {
                serde_json::json!({ "person": canonical, "aliases": aliases })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!(rows))?
        );
        return Ok(());
    }
    if people.is_empty() {
        println!("{}", "Identity links".bold());
        println!();
        println!(
            "  No links yet. Link a person's identity strings with {}.",
            "aida identity link <a> <b>".cyan()
        );
        return Ok(());
    }
    println!("{}", "Identity links".bold());
    println!();
    for (canonical, aliases) in &people {
        println!("  {}", canonical.cyan().bold());
        for alias in aliases {
            println!(
                "    {} {}",
                crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                alias
            );
        }
    }
    println!();
    println!(
        "  {}",
        "Each block is one person; the listed aliases all resolve to the canonical id.".dimmed()
    );
    Ok(())
}

/// `aida identity show <id>` (TASK-845): show the canonical person an identity
/// resolves to (case-fold then alias-resolve) plus every alias that shares it.
// trace:TASK-845 | ai:claude
pub(crate) fn handle_identity_show(
    store_path: &std::path::Path,
    id: &str,
    json: bool,
) -> Result<()> {
    let registry = aida_core::alias::AliasRegistry::load(store_path);
    let canonical = registry.resolve(id);
    let members = registry.members_of(id);
    if json {
        let out = serde_json::json!({
            "input": id,
            "canonical": canonical,
            "members": members,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    println!(
        "{} {} {} {}",
        format!("{}", id).bold(),
        "→".dimmed(),
        "canonical person:".dimmed(),
        canonical.cyan().bold()
    );
    if members.len() > 1 {
        println!("  {}", "aliases:".dimmed());
        for m in &members {
            let marker = if *m == canonical {
                " (canonical)".dimmed().to_string()
            } else {
                String::new()
            };
            println!(
                "    {} {}{}",
                crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                m,
                marker
            );
        }
    } else {
        println!(
            "  {}",
            "Not linked to any other identity — resolves to itself.".dimmed()
        );
    }
    Ok(())
}

// STORY-673: compose the terse one-line requirement breakdown for default
// `aida status`. Leads with OPEN work (what's actionable — every status that
// isn't a terminal Completed/Rejected), then the closed tallies, then a pointer
// to the per-status detail. Pure (no I/O) so the copy is unit-testable.
// trace:STORY-673 | ai:claude
pub(crate) fn requirement_breakdown_summary_line(
    by_status: &std::collections::BTreeMap<String, usize>,
) -> String {
    let is_open_lens_status = |s: &str| status_is_open_lens_work(s);
    let completed = by_status.get("Completed").copied().unwrap_or(0);
    let rejected = by_status.get("Rejected").copied().unwrap_or(0);

    // Open = the same status set as `aida list open`, with a compact per-status
    // tail in deterministic (BTreeMap) order.
    let open_total: usize = by_status
        .iter()
        .filter(|(s, _)| is_open_lens_status(s))
        .map(|(_, n)| *n)
        .sum();
    let open_parts: Vec<String> = by_status
        .iter()
        .filter(|(s, _)| is_open_lens_status(s))
        .map(|(s, n)| format!("{n} {}", s.to_ascii_lowercase()))
        .collect();

    let mut line = if open_parts.is_empty() {
        "0 open".to_string()
    } else {
        format!("{} open ({})", open_total, open_parts.join(" · "))
    };
    if completed > 0 {
        line.push_str(&format!(" · {completed} completed"));
    }
    if rejected > 0 {
        line.push_str(&format!(" · {rejected} rejected"));
    }
    line.push_str(" · `aida status --full` for the per-status breakdown");
    line
}

/// `aida status` cross-clone coordination view (STORY-640, coordination slice
/// 3): the ACTIVE `coordination/` claims (leases + drain + solo) held across
/// all clones — distinct from the LOCAL leases section. Silent when there are
/// no claims. trace:STORY-640 | ai:claude
///
/// BUG-1764: this section is also reached by `aida doctor` (which forces
/// `show_full`). It applied no expiry predicate and — unlike the lease section
/// — does not filter by clone at all, so it reported EVERY claim file as
/// active: measured on one host, 211 claims, the oldest aged 8_078_324s (93
/// days) against its own 1800s TTL, and the collapsed arm printed "211 active
/// claims" on a host holding none. Stale claims are now excluded from both
/// arms, and the count line says how many were suppressed.
// trace:BUG-1764 | ai:claude
pub(crate) fn print_status_coordination_section(
    store_root: &std::path::Path,
    now: chrono::DateTime<chrono::Utc>,
    show_full: bool,
) {
    let mut all_claims = coordination::list_claims(store_root);
    all_claims.extend(coordination::list_lock_claims(store_root));
    if all_claims.is_empty() {
        return;
    }
    // BUG-1764: "active" is a claim the staleness predicate presumes live. The
    // drain/solo lock claims mixed in here ARE `process_backed`, so they also
    // get the same-host pid evaluation; session leases are not, so the TTL
    // governs them. See `coordination::claim_staleness`.
    let (claims, stale) =
        coordination::partition_claims(all_claims, now, &coordination::hostname());
    let hidden = if stale.is_empty() {
        String::new()
    } else {
        format!(
            " · {} stale hidden (`aida session leases --all`)",
            stale.len()
        )
    };
    if claims.is_empty() {
        // Every claim on the store is stale. Say so — silence here would read
        // as "no coordination state", which is what BUG-1764 made impossible
        // to distinguish from "211 claims, all dead".
        println!("{}", "─── Cross-clone coordination ───".bold());
        // Point at the LISTING, not at `--prune-stale`: that command releases
        // only CROSS-CLONE claims (clause 4's scope), while this count also
        // includes this clone's own expired claim files — whose garbage
        // collection is BUG-1764 clause 5, deliberately out of scope.
        println!(
            "  no active claims · {} stale claim{} on the store · {} to inspect",
            stale.len(),
            if stale.len() == 1 { "" } else { "s" },
            "aida session leases --all".cyan()
        );
        println!();
        return;
    }
    println!("{}", "─── Cross-clone coordination ───".bold());
    // STORY-673: the cross-clone claim table is a fleet long-tail — collapse to
    // a one-line count by default; `--full` / `--all` prints the table.
    // trace:STORY-673 | ai:claude
    if !show_full {
        let scopes: Vec<&str> = claims.iter().take(3).map(|c| c.scope.as_str()).collect();
        let preview = scopes.join(", ");
        let suffix = if claims.len() > scopes.len() {
            " …"
        } else {
            ""
        };
        println!(
            "  {} active claim{}: {}{} · `aida status --full`{}",
            claims.len(),
            if claims.len() == 1 { "" } else { "s" },
            preview,
            suffix,
            hidden
        );
        println!();
        return;
    }
    println!();
    println!(
        "  {:<16} {:<10} {:<14} {:<14} age",
        "scope", "host", "agent", "node"
    );
    for c in &claims {
        let age = chrono::DateTime::parse_from_rfc3339(&c.heartbeat_at)
            .ok()
            .map(|t| {
                let secs = now
                    .signed_duration_since(t.with_timezone(&chrono::Utc))
                    .num_seconds()
                    .max(0);
                format!("{secs}s")
            })
            .unwrap_or_else(|| "?".to_string());
        println!(
            "  {:<16} {:<10} {:<14} {:<14} {}",
            truncate(&c.scope, 16),
            truncate(&c.host, 10),
            truncate(if c.agent.is_empty() { "-" } else { &c.agent }, 14),
            truncate(&c.node_id, 14),
            age,
        );
        if !c.clone_path.is_empty() {
            println!("    {}", c.clone_path.dimmed());
        }
    }
    if !stale.is_empty() {
        println!(
            "  {} stale claim{} hidden · {} to inspect",
            stale.len(),
            if stale.len() == 1 { "" } else { "s" },
            "aida session leases --all".cyan()
        );
    }
    println!();
}

// ─── STORY-707: fast cache-backed `aida status` default ────────────────────
//
// The bare `aida status` (no flags) is an ORIENTATION command — it must be
// instant. The historic heavy path (`handle_status_command_distributed`) loads
// the FULL store, makes `gh` PR/CI NETWORK calls (~16s), and probes live Claude
// sessions before any of the --short/--queue branches, so every form paid that
// cost. The fast path below answers "where am I right now" from cache-cheap
// inputs ONLY: role (env/default), current git branch, queue depth (the queue
// dir scan), and key requirement counts sourced from `.aida/cache.db` — the SAME
// cache that powers sub-ms `aida list`. No `backend.load()`, no `gh`, no
// live-session probe. The rich view is preserved opt-in via `aida status --full`
// / `--ci` (which route through the heavy path); the moved diagnostics (PR/CI,
// liveness, worktrees, roster, coordination, hygiene) now live in `aida doctor`.
// trace:STORY-707 | ai:claude

/// The cache-sourced inputs the fast `aida status` snapshot renders. Kept a
/// plain data struct — assembled by [`collect_fast_status_snapshot`] (which does
/// the cache/git/queue reads) and counted by [`fast_status_counts`] (a pure
/// function) — so the "no network / no full load" contract is unit-testable
/// without spawning git or `gh`.
// trace:STORY-707 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct FastStatusSnapshot {
    pub(crate) role: String,
    pub(crate) role_is_default: bool,
    pub(crate) mcp_authority_lines: Vec<String>,
    pub(crate) branch: Option<String>,
    pub(crate) queue_depth: usize,
    /// STORY-723: of `queue_depth` items routed to this role, how many are
    /// ACTUALLY actionable (live + workable status). The raw depth is padded
    /// with archived/completed/deferred corpses; this is the reconcilable count.
    pub(crate) queue_actionable: usize,
    pub(crate) counts: FastStatusCounts,
    /// Per-status breakdown of the same row set `counts` is tallied from —
    /// the monitor-contract `requirements.by_status` field (BUG-1503).
    pub(crate) by_status: std::collections::BTreeMap<String, usize>,
    pub(crate) cache_present: bool,
}

/// Key requirement tallies for the fast snapshot, derived purely from the
/// cache's `(status, req_type, deferred)` rows (non-archived only). `open` =
/// the same actionable status/view lens as `aida list open`.
// trace:STORY-707 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct FastStatusCounts {
    pub(crate) open: usize,
    pub(crate) in_progress: usize,
    pub(crate) draft: usize,
    pub(crate) total: usize,
}

pub(crate) fn status_is_open_lens_work(status: &str) -> bool {
    let normalized_status = status.trim().replace(['-', '_'], "").to_ascii_lowercase();
    aida_core::RequirementStatus::open_statuses()
        .iter()
        .any(|s| s.cache_key().to_ascii_lowercase() == normalized_status)
}

/// Pure counter over non-archived, non-deferred cache rows: each tuple is
/// `(status, req_type)`. META and other standing-artifact types are excluded
/// the same way `aida list` hides them, so the tallies agree with the
/// list/triage surfaces. Status matching is case-insensitive (the cache stores
/// the Debug form, e.g. "InProgress"/"Draft").
// trace:STORY-707 | ai:claude
#[cfg(test)]
pub(crate) fn fast_status_counts<'a>(
    rows: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> FastStatusCounts {
    fast_status_counts_with_defer(
        rows.into_iter()
            .map(|(status, req_type)| (status, req_type, false, "[]")),
    )
}

/// Pure counter over non-archived cache rows carrying the deferred view flag.
/// This is the exact fast-status analogue of `aida list open`: deferred rows
/// are parked work and must not inflate the headline `open` scalar.
// trace:BUG-1155 | ai:codex
pub(crate) fn fast_status_counts_with_defer<'a>(
    rows: impl IntoIterator<Item = (&'a str, &'a str, bool, &'a str)>,
) -> FastStatusCounts {
    let mut c = FastStatusCounts::default();
    for (status, req_type, deferred, tags_json) in rows {
        if !is_real_requirement_summary(req_type) || is_standing_artifact_type(req_type) {
            continue;
        }
        // STORY-584: match the cache list lens: deferred means the flag is set
        // OR a legacy `deferred:*` parking tag is present in tags_json.
        // trace:BUG-1155 | ai:codex
        if deferred || tags_json.contains("\"deferred:") {
            continue;
        }
        c.total += 1;
        // BUG-781: an accepted decision (a `decision` spec at `Approved`) is
        // that class's TERMINAL state, so it must not inflate the open backlog
        // the same way a not-yet-started task at Approved does — the identical
        // rule the default `aida list` lens applies. trace:BUG-781 | ai:claude
        if status_is_open_lens_work(status)
            && !aida_core::lifecycle::is_accepted_decision(req_type, status)
        {
            c.open += 1;
        }
        if status.eq_ignore_ascii_case("inprogress")
            || status.eq_ignore_ascii_case("in-progress")
            || status.eq_ignore_ascii_case("in_progress")
        {
            c.in_progress += 1;
        }
        if status.eq_ignore_ascii_case("draft") {
            c.draft += 1;
        }
    }
    c
}

/// The BUG-1503 monitor-contract companion to [`fast_status_counts_with_defer`]:
/// the SAME row set, the SAME real/standing-artifact/deferred exclusions (so
/// the values sum to exactly `counts.total`), grouped by the raw cache
/// `status` string instead of tallied into the fixed scalar buckets. Keys use
/// the cache's stored Debug-form casing (e.g. "InProgress", "Draft") — the
/// same strings the heavy text panel's `by_status` breakdown groups on
/// (`status_cmd.rs`'s `s.status.clone()`), so a monitor consumer sees
/// identical keys regardless of which status surface it polls.
// trace:BUG-1503 | ai:claude
pub(crate) fn fast_status_by_status_with_defer<'a>(
    rows: impl IntoIterator<Item = (&'a str, &'a str, bool, &'a str)>,
) -> std::collections::BTreeMap<String, usize> {
    let mut by_status: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    for (status, req_type, deferred, tags_json) in rows {
        if !is_real_requirement_summary(req_type) || is_standing_artifact_type(req_type) {
            continue;
        }
        if deferred || tags_json.contains("\"deferred:") {
            continue;
        }
        *by_status.entry(status.to_string()).or_insert(0) += 1;
    }
    by_status
}

/// Read `(status, req_type)` for every non-archived row straight from the cache
/// DB (read-only sqlite), then count via [`fast_status_counts`]. This is the
/// same read-only cache `read_draft_backlog_depth` uses — NO `backend.load()`, no
/// git spawn. Returns zeroed counts (and an empty `by_status`) when the cache is
/// absent/unreadable (a fresh `aida init` with no reads yet). A single `SELECT`
/// of `requirements_cache` (grouped in Rust rather than SQL, so it can share
/// the exact real/standing-artifact/deferred exclusions the scalar counts
/// already apply) feeds both the scalar counts and the per-status
/// breakdown — one query against the cache, not two.
// trace:STORY-707 trace:BUG-1503 | ai:claude
pub(crate) fn fast_status_counts_from_cache(
    cache_path: &std::path::Path,
) -> (FastStatusCounts, std::collections::BTreeMap<String, usize>) {
    if !cache_path.exists() {
        return (FastStatusCounts::default(), Default::default());
    }
    let conn = match rusqlite::Connection::open_with_flags(
        cache_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(c) => c,
        Err(_) => return (FastStatusCounts::default(), Default::default()),
    };
    let has_deferred_column = conn
        .prepare("SELECT deferred FROM requirements_cache LIMIT 0")
        .is_ok();
    let has_tags_json_column = conn
        .prepare("SELECT tags_json FROM requirements_cache LIMIT 0")
        .is_ok();
    let sql = match (has_deferred_column, has_tags_json_column) {
        (true, true) => {
            "SELECT status, req_type, deferred, tags_json FROM requirements_cache WHERE archived = 0"
        }
        (true, false) => {
            "SELECT status, req_type, deferred, '[]' AS tags_json FROM requirements_cache WHERE archived = 0"
        }
        (false, true) => {
            "SELECT status, req_type, 0 AS deferred, tags_json FROM requirements_cache WHERE archived = 0"
        }
        (false, false) => {
            "SELECT status, req_type, 0 AS deferred, '[]' AS tags_json FROM requirements_cache WHERE archived = 0"
        }
    };
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(_) => return (FastStatusCounts::default(), Default::default()),
    };
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)? != 0,
            row.get::<_, String>(3)?,
        ))
    });
    let Ok(rows) = rows else {
        return (FastStatusCounts::default(), Default::default());
    };
    let collected: Vec<(String, String, bool, String)> = rows.flatten().collect();
    let counts = fast_status_counts_with_defer(
        collected
            .iter()
            .map(|(s, t, d, tags)| (s.as_str(), t.as_str(), *d, tags.as_str())),
    );
    let by_status = fast_status_by_status_with_defer(
        collected
            .iter()
            .map(|(s, t, d, tags)| (s.as_str(), t.as_str(), *d, tags.as_str())),
    );
    (counts, by_status)
}

/// Assemble the fast snapshot from cache-cheap inputs: role from
/// `AIDA_SESSION_ROLE` (implementer default), current git branch (one cheap
/// `git symbolic-ref`), queue depth (the queue dir scan the statusline uses),
/// and key counts from the read-only cache. No `gh`, no full store load, no
/// live-session probe.
// trace:STORY-707 | ai:claude
pub(crate) fn collect_fast_status_snapshot(project_root: &std::path::Path) -> FastStatusSnapshot {
    let (role, role_is_default) = effective_role_resolved();
    let branch = current_branch_at(project_root);
    let queue_depth = read_queue_depth(project_root, Some(role.as_str())).unwrap_or(0);
    // STORY-723: the subset of the role queue that is actually actionable, so
    // the snapshot can reconcile "163 routed" against a far smaller live set.
    let queue_actionable = role_queue_actionable(project_root, &role).len();
    let mcp_authority_lines = agent_registry::mcp_authority_status_lines_for_project(project_root);
    let cache_path = project_root.join(".aida/cache.db");
    let cache_present = cache_path.exists();
    let (counts, by_status) = fast_status_counts_from_cache(&cache_path);
    FastStatusSnapshot {
        role,
        role_is_default,
        mcp_authority_lines,
        branch,
        queue_depth,
        queue_actionable,
        counts,
        by_status,
        cache_present,
    }
}

/// STORY-730: print the "since you were away" morning-after banner ABOVE the
/// human `aida status` snapshot when a recent, un-acknowledged drain outcome is
/// persisted at `.aida/last-drain.json`. The single highest-trust moment is "I
/// walked away — what happened?", and this is the first place that answers it.
/// No-op when there is no recent/non-empty/un-acknowledged outcome. The pause
/// marker routes through the glyph registry so an `ascii` profile re-renders it.
// trace:STORY-730 | ai:claude
pub(crate) fn print_morning_after_banner(project_root: &std::path::Path) {
    let Some(outcome) = last_drain::LastDrainOutcome::read(project_root) else {
        return;
    };
    let pause = crate::glyphs::get(crate::glyphs::Glyph::Pause, Some(project_root));
    if let Some(line) = outcome.banner(chrono::Utc::now(), pause) {
        println!("{}", line.yellow().bold());
        if outcome.findings_to_triage > 0 {
            println!("  {}", "drill down → `aida findings list`".dimmed());
        }
        println!();
    }
}

/// STORY-730: the AGENT/TOON mirror of [`print_morning_after_banner`] — a single
/// compact `last_drain` scalar line (no glyph) above the TOON status snapshot,
/// under the same suppression rules. No-op when nothing recent to report.
// trace:STORY-730 | ai:claude
pub(crate) fn print_morning_after_toon(project_root: &std::path::Path) {
    let Some(outcome) = last_drain::LastDrainOutcome::read(project_root) else {
        return;
    };
    if let Some(compact) = outcome.compact(chrono::Utc::now()) {
        println!("{}", crate::toon::scalar("last_drain", &compact));
    }
}

/// Render the fast `aida status` snapshot. One compact screen: role, branch,
/// queue depth, and the cache-sourced key counts, plus a footer pointing at the
/// rich/diagnostic surfaces (`--full`, `aida doctor`).
// trace:STORY-707 | ai:claude
pub(crate) fn print_fast_status(snap: &FastStatusSnapshot) {
    println!("{}", "─── Status ───".bold());
    let role_cell = if snap.role_is_default {
        format!("{} (default)", snap.role)
    } else {
        snap.role.clone()
    };
    println!("  {:<10} {}", "role:".bold(), role_cell.cyan());
    for line in &snap.mcp_authority_lines {
        println!("{line}");
    }
    match &snap.branch {
        Some(b) => println!("  {:<10} {}", "branch:".bold(), b.cyan()),
        None => println!(
            "  {:<10} {}",
            "branch:".bold(),
            "(not a git branch)".dimmed()
        ),
    }
    // STORY-723: label the queue count unambiguously. `queue_depth` is the
    // SHARED role-routed queue (which can be padded with archived/completed
    // corpses), NOT the caller's personal `aida queue list` — that mismatch
    // ("163 routed" vs "your queue is empty") was a top first-impression
    // confusion. Show how many of the routed items are actually actionable and
    // point at the personal queue. trace:STORY-723
    println!(
        "  {:<10} {} routed to role:{} ({} actionable) — shared role queue, not your personal queue",
        "queue:".bold(),
        snap.queue_depth,
        snap.role,
        snap.queue_actionable,
    );
    println!(
        "  {:<10} {}",
        "",
        "your personal queue: `aida queue list`".dimmed()
    );
    if let Some(line) = crate::intent_capture::status_intent_capture_line(
        &std::env::current_dir().unwrap_or_default(),
    ) {
        println!("{line}");
    }
    println!();

    println!("{}", "─── Requirements (cache) ───".bold());
    if snap.cache_present {
        let c = &snap.counts;
        // STORY-723: name the denominator so it reconciles with `aida list`.
        // `total` here counts every ACTIVE (non-archived) spec across all
        // statuses; bare `aida list` defaults to the OPEN subset of these.
        println!(
            "  {} open · {} in-progress · {} draft · {} total active (non-archived)",
            c.open, c.in_progress, c.draft, c.total
        );
    } else {
        println!(
            "  {}",
            "(no cache yet — run `aida list` to build it)".dimmed()
        );
    }
    println!();

    // Point at the heavy surfaces the fast default deliberately omits. The PR/CI,
    // live-session/lease liveness, worktree, roster, coordination, and hygiene
    // diagnostics moved to `aida doctor`; the full rich snapshot stays opt-in
    // behind `aida status --full`. trace:STORY-707 | ai:claude
    println!(
        "  {}",
        "PR/CI, agents, worktrees, hygiene → `aida doctor` · full snapshot → `aida status --full`"
            .dimmed()
    );
    println!();
}

/// The machine-readable twin of [`print_fast_status`]. Serializes the SAME
/// [`FastStatusSnapshot`] the human bare `aida status` prints — no extra
/// cache/git/`gh` reads — so `aida status --format json` (and `--json`)
/// return in the same order of magnitude as the human form instead of
/// silently falling through to the heavy `--full`-equivalent report. Emits
/// ONLY the JSON document on stdout (no banners/text before it) so the
/// output always parses.
//
// Fix note: the pre-existing bug_1289_format_json.rs contract
// (status_bare_format_json_parses) asserts the document carries either a
// `requirements` or an `agents` key — the shape the heavy
// print_status_json path produced when bare `--format json` used to fall
// through to it. The fast path has no live-agent roster to report (that
// moved to `aida doctor`), so it keeps the contract via `requirements`: the
// SAME cache-sourced `counts` already computed for the human view, at no
// extra cost. Plain `//` keeps the marker out of any doc/help.
// trace:BUG-1503 | ai:claude
//
// The monitor contract (`monitor_contract.rs` / `docs/monitor-contract-
// fixtures/status.json`) promises `requirements.total` (an integer) AND
// `requirements.by_status` (an object) from `aida status --json`. `counts`
// (the internal/duplicate scalar key) stays as-is; `requirements` gets the
// `by_status` breakdown alongside it so the contract holds on the fast path
// too, not just when it used to fall through to the heavy report.
// trace:BUG-1503 | ai:claude
pub(crate) fn print_fast_status_json(snap: &FastStatusSnapshot) -> Result<()> {
    let counts = serde_json::json!({
        "open": snap.counts.open,
        "in_progress": snap.counts.in_progress,
        "draft": snap.counts.draft,
        "total": snap.counts.total,
    });
    let mut requirements = counts.clone();
    requirements["by_status"] = serde_json::json!(snap.by_status);
    let out = serde_json::json!({
        "role": snap.role,
        "role_is_default": snap.role_is_default,
        "branch": snap.branch,
        "queue": {
            "depth": snap.queue_depth,
            "actionable": snap.queue_actionable,
        },
        "cache_present": snap.cache_present,
        "counts": counts,
        "requirements": requirements,
    });
    println!("{}", crate::cache_output::json_pretty(&out)?);
    Ok(())
}

/// Assemble the AGENT-MODE scalar head lines for `aida status` with a single,
/// unambiguous "is there work for me?" signal. `queue_actionable` LEADS: it is
/// the count that actually answers the question (live, workable, role-routed
/// specs) and is consistent with `aida queue list`. The raw `queue_depth` (the
/// role-routed queue-file scan) is the total routed count — larger than
/// actionable when the queue is padded with archived/completed/deferred corpses.
///
/// BUG-675: `queue_depth` now resolves user identity through the SAME two-step
/// path `aida queue list` uses (`current_user_id` + the `resolve_queue_user`
/// case-fold, TASK-951), so it is trustworthy — it no longer collapses to zero on
/// a case-only identity mismatch. That removes the reason for BUG-670's
/// divergence-suppression (which dropped the scalar whenever it disagreed with
/// `queue_actionable`, partly because the count could be a spurious zero). The
/// depth now ALWAYS rides along, AFTER the lead actionable signal, giving the
/// agent both "how many are workable now" and "how many are routed in total".
/// The human TTY path ([`print_fast_status`]) keeps its fully-labelled "N routed
/// (M actionable)" line unchanged. Pure, so the ordering contract is
/// unit-testable without a queue file or a cache DB.
// trace:BUG-675 trace:BUG-670 | ai:claude — plain `//` keeps the marker out of any doc/help.
pub(crate) fn toon_status_scalar_lines(snap: &FastStatusSnapshot) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    lines.push(crate::toon::scalar("role", &snap.role));
    lines.push(crate::toon::scalar(
        "role_default",
        &snap.role_is_default.to_string(),
    ));
    for line in &snap.mcp_authority_lines {
        lines.push(crate::toon::scalar("mcp_authority", line.trim()));
    }
    lines.push(crate::toon::scalar(
        "branch",
        snap.branch.as_deref().unwrap_or(""),
    ));
    // LEAD with the actionable count — the unambiguous work-signal an agent acts
    // on. STORY-723 introduced it as the reconcilable subset of the padded depth.
    lines.push(crate::toon::scalar(
        "queue_actionable",
        &snap.queue_actionable.to_string(),
    ));
    // BUG-675: the raw role-routed depth is now identity-trustworthy (same
    // resolver as `aida queue list`), so it always rides along AFTER the lead
    // actionable signal — no longer suppressed on divergence.
    lines.push(crate::toon::scalar(
        "queue_depth",
        &snap.queue_depth.to_string(),
    ));
    if snap.cache_present {
        lines.push(crate::toon::scalar("open", &snap.counts.open.to_string()));
        lines.push(crate::toon::scalar(
            "in_progress",
            &snap.counts.in_progress.to_string(),
        ));
        lines.push(crate::toon::scalar("draft", &snap.counts.draft.to_string()));
        lines.push(crate::toon::scalar("total", &snap.counts.total.to_string()));
    } else {
        lines.push(crate::toon::scalar("cache", "absent"));
    }
    lines
}

/// AGENT-MODE token-efficient TOON render of the fast status snapshot. Scalar
/// head fields (role / branch / the lead `queue_actionable` signal / the cache
/// counts) then a uniform TOON table of the top queued items for the active
/// role. No emoji, no section rules — the token-efficient analog of
/// [`print_fast_status`], which stays byte-identical for the human TTY path.
// trace:TASK-964 trace:BUG-670
pub(crate) fn print_toon_status(snap: &FastStatusSnapshot, project_root: &std::path::Path) {
    println!("{}", toon_status_scalar_lines(snap).join("\n"));

    // STORY-723: the queued[] table + the `next` head are the ACTIONABLE set
    // (archived/completed/deferred/blocked corpses filtered out), so an agent is
    // never nudged to `aida queue work <archived-spec>`.
    let actionable = role_queue_actionable(project_root, &snap.role);
    let top: Option<(String, String)> = actionable
        .first()
        .map(|(id, _title, status)| (id.clone(), status.clone()));
    let rows: Vec<Vec<String>> = actionable
        .into_iter()
        .take(AGENT_BARE_QUEUE_TOPN)
        .map(|(id, title, _status)| vec![id, title])
        .collect();
    println!(
        "{}",
        crate::toon::table_raw("queued", &["id", "title"], &rows)
    );
    // TASK-974 (AXI #9) / STORY-723: next-step block — lead with the fastest
    // thought-to-merged path for the actionable head (`aida zen` / `aida ship`),
    // else point at the approvable backlog. BUG-670: drive the nudge off the
    // ACTIONABLE count, never the padded/divergent `queue_depth`, so a
    // `fill-queue` hint only ever appears alongside `queue_actionable: 0` (never
    // next to a non-zero depth the agent can't reconcile). trace:TASK-974 trace:STORY-723 trace:BUG-670
    let next = crate::help_next::status_next(
        snap.queue_actionable,
        top.as_ref().map(|(id, st)| (id.as_str(), st.as_str())),
    );
    if let Some(block) = crate::help_next::render(&next) {
        println!("{block}");
    }
}

/// STORY-723: is `status` an ACTIONABLE lifecycle state — one a front-door
/// `next` nudge may safely point at? Live + workable: approved / planned /
/// in-progress / needs-attention. Draft (not yet approved), done (awaiting
/// merge), and the terminal states (completed / rejected / released) are NOT
/// actionable. Case- and spelling-tolerant (the cache stores the Debug form,
/// e.g. "InProgress" / "NeedsAttention").
pub(crate) fn is_actionable_queue_status(status: &str) -> bool {
    let s = status.to_ascii_lowercase().replace(['-', '_', ' '], "");
    matches!(
        s.as_str(),
        "approved" | "planned" | "inprogress" | "needsattention"
    )
}

/// STORY-723: pure predicate — is a queued requirement row (its cache fields)
/// safe to surface as the actionable head of the front-door `next` nudge? A row
/// qualifies only when it is NOT archived, NOT deferred, NOT blocked, and sits
/// in an actionable status. Kept side-effect-free so the "never nudge an
/// archived/completed corpse" invariant is unit-testable without a queue file or
/// a cache DB.
pub(crate) fn queue_row_actionable(
    status: &str,
    archived: i64,
    deferred: i64,
    blocked: i64,
) -> bool {
    archived == 0 && deferred == 0 && blocked == 0 && is_actionable_queue_status(status)
}

/// STORY-723: should bare `aida list` apply the default OPEN/actionable lens?
/// Only when no status filter is already in play and the view wasn't widened to
/// include closed/archived/deferred rows. Pure so the default is unit-testable.
pub(crate) fn list_default_open_lens(
    has_status: bool,
    all: bool,
    archived: bool,
    deferred: bool,
) -> bool {
    !has_status && !all && !archived && !deferred
}

/// BUG-788: is the requested status filter the explicit `open` alias
/// (`aida list open` / `aida list --status open`)? The `open` shortcut is a
/// second spelling of the bare-list default open lens, so it must share that
/// lens's decision-class exclusion (BUG-781: an accepted ADR is terminal). Bare
/// list clears the status axis and takes the default lens; the explicit `open`
/// shortcut instead SETS the status axis (to the open set), which turns the
/// default lens OFF — so without this the two verbs disagreed about whether an
/// accepted ADR is open work. Matches only the lone `open` token
/// (case-insensitive); a mixed spec like `open,closed` is a deliberately wider
/// ask and keeps the terminals visible. Pure so it's unit-testable.
// trace:BUG-788 | ai:claude
pub(crate) fn status_spec_is_open_alias(raw_status: Option<&str>) -> bool {
    match raw_status {
        Some(spec) => {
            let mut tokens = spec.split(',').map(str::trim).filter(|t| !t.is_empty());
            matches!((tokens.next(), tokens.next()),
                (Some(only), None) if only.eq_ignore_ascii_case("open"))
        }
        None => false,
    }
}

/// BUG-1498: only the single explicit `draft` status activates the advisor's
/// human-first draft lens. A mixed status expression is a broader operational
/// query and must not silently lose machine-filed rows.
// trace:BUG-1498 | ai:codex
pub(crate) fn status_spec_is_exact_draft(raw: &str) -> bool {
    raw.trim().eq_ignore_ascii_case("draft")
}

pub(crate) fn is_machine_filed_draft(r: &aida_core::RequirementSummary) -> bool {
    // One definition shared with the capture-coverage report.
    // trace:STORY-1487 | ai:claude
    criteria_coverage::is_auto_drafted(&r.tags, &r.description)
}

/// Partition a draft grooming query by provenance. Returns the number hidden
/// from the human-first view so renderers can advertise the escape hatch.
// trace:BUG-1498 | ai:codex
pub(crate) fn apply_machine_draft_lens(
    reqs: &mut Vec<aida_core::RequirementSummary>,
    exact_draft_view: bool,
    machine_only: bool,
    explicit_machine_tag: bool,
) -> usize {
    if !exact_draft_view {
        return 0;
    }
    if machine_only {
        reqs.retain(is_machine_filed_draft);
        return 0;
    }
    if explicit_machine_tag {
        return 0;
    }
    let before = reqs.len();
    reqs.retain(|r| !is_machine_filed_draft(r));
    before - reqs.len()
}

#[cfg(test)]
#[path = "tests/bug_1498_machine_draft_lens_tests.rs"]
mod bug_1498_machine_draft_lens_tests;

/// BUG-788: should the open-work accepted-decision lens apply? True under the
/// bare-list default open lens (STORY-723) OR the explicit `open` shortcut, so
/// `aida list` and `aida list open` hide accepted ADRs identically. The explicit
/// shortcut still yields to any view-widening flag (`--all` / `--archived` /
/// `--deferred`) — BUG-781's guarantee that `--all` shows accepted ADRs holds
/// for `aida list open --all` too (`default_open_lens` already bakes in that
/// yield for the bare-list arm). Pure so the agreement between the two verbs is
/// unit-testable without a cache DB.
// trace:BUG-788 | ai:claude
pub(crate) fn list_applies_open_work_lens(
    default_open_lens: bool,
    explicit_open_alias: bool,
    all: bool,
    archived: bool,
    deferred: bool,
) -> bool {
    default_open_lens || (explicit_open_alias && !all && !archived && !deferred)
}

/// BUG-781: drop the ACCEPTED (terminal) decision rows from a listing under the
/// default open lens, returning how many were hidden.
///
/// For a `decision` spec (an ADR) the stored `Approved` means ACCEPTED — the
/// end of that class's lifecycle (draft = proposed, approved = accepted) — not
/// the "cleared to start" that `Approved` means for a task. Left in the open
/// lens, every ratified ADR reads as work that never finishes, and the only way
/// out was a manual `aida archive`. This is the sibling of the TASK-773
/// standing-artifact pass: same lens, same "an explicit ask overrides it" rule.
///
/// Escapes, all preserved by the caller's two gates: an explicit `--status`,
/// `--all`, `--archived` or `--deferred` clears `default_open_lens`, and
/// `--type decision` sets `asked_for_decision_type`. Nothing is written — the
/// stored status and the archive flag are untouched.
///
/// Pure (takes the row vector, not a backend) so the lens is unit-testable
/// without a cache DB.
// trace:BUG-781 | ai:claude
pub(crate) fn hide_accepted_decisions(
    reqs: &mut Vec<aida_core::RequirementSummary>,
    default_open_lens: bool,
    asked_for_decision_type: bool,
) -> usize {
    if !default_open_lens || asked_for_decision_type {
        return 0;
    }
    let before = reqs.len();
    reqs.retain(|r| !aida_core::lifecycle::is_accepted_decision(&r.req_type, &r.status));
    before - reqs.len()
}

#[cfg(test)]
#[path = "tests/bug_781_accepted_decision_lens_tests.rs"]
mod bug_781_accepted_decision_lens_tests;

// trace:BUG-788 | ai:claude
#[cfg(test)]
#[path = "tests/bug_788_open_shortcut_accepted_decision_tests.rs"]
mod bug_788_open_shortcut_accepted_decision_tests;

// trace:TASK-1176 | ai:claude
#[cfg(test)]
#[path = "tests/task_1176_superseded_state_tests.rs"]
mod task_1176_superseded_state_tests;

/// STORY-723: the denominator label for the agent `aida list` `count: N of M`
/// header — `open` under the default actionable lens (so it reconciles with the
/// `aida status` active total, which counts all statuses) else plain `matched`.
/// BUG-1737: the one sentence the default `aida list` view owes the reader —
/// that it is also withholding the archived and deferred tiers.
///
/// The lens already discloses every other class it drops (closed rows via
/// STORY-723, accepted ADRs via BUG-781, machine-filed drafts via BUG-1498),
/// each with a count and a flag. Archived and deferred were the only two
/// omitted silently, which let the disclosed list read as exhaustive: an
/// operator seeing `count: 30 of 53 open` plus three `note:` lines concluded
/// the 53 was the project's open backlog. It is not — it is the live slice of
/// it, and on this repository 1548 archived and 119 deferred rows sat outside
/// it unmentioned.
///
/// This is deliberately NOT the counted STORY-441 / STORY-584 nudge. BUG-783
/// turned those off by default on purpose ("an explicit open-work request
/// doesn't need reminding on every invocation that the other two tiers
/// exist"), and each costs an extra `list_summaries` query that the hot path
/// skips. That decision stands and `[list] show_hidden_hints` keeps its
/// meaning. What was missing is scope disclosure for the *count*, which needs
/// no count — hence a `bool` in and a fixed string out, so wiring it up cannot
/// add a backend query to the default path.
///
/// Gated on the open-WORK lens rather than the bare-list lens so `aida list`
/// and `aida list open` disclose identically — BUG-788 established that the
/// explicit `open` alias is a second spelling of the same view and filed their
/// disagreement as a defect. Every other explicit `--status`, and any
/// `--all` / `--archived` / `--deferred`, returns `None`: those views are not
/// hiding these tiers behind the reader's back.
// trace:BUG-1737 | ai:claude
pub(crate) fn list_lens_scope_disclosure(open_work_lens: bool) -> Option<&'static str> {
    open_work_lens.then_some("open lens excludes archived and deferred specs")
}

#[cfg(test)]
#[path = "tests/bug_1737_lens_scope_disclosure_tests.rs"]
mod bug_1737_lens_scope_disclosure_tests;

pub(crate) fn list_count_denom_label(default_open_lens: bool) -> &'static str {
    if default_open_lens {
        "open"
    } else {
        "matched"
    }
}

/// Render the human `aida list` footer without hiding an explicit `--limit`
/// slice. Untrimmed output remains byte-for-byte compatible with the historic
/// footer; a trimmed slice shares the TOON denominator vocabulary.
// trace:BUG-1209 | ai:codex
pub(crate) fn list_human_count_footer(
    visible: usize,
    total_after_filters: usize,
    limit: Option<usize>,
    default_open_lens: bool,
) -> String {
    if total_after_filters > visible {
        if let Some(limit) = limit {
            return format!(
                "{visible} of {total_after_filters} {} requirements (--limit {limit}; drop it or raise N to see the rest)",
                list_count_denom_label(default_open_lens)
            );
        }
    }
    format!("{visible} requirements")
}

/// Print the count footer for an empty human list only when an explicit limit
/// hid matching rows. A genuinely empty filtered result keeps the existing
/// concise `No requirements found.` presentation.
// trace:BUG-1209 | ai:codex
pub(crate) fn print_empty_human_list_truncation_footer(
    total_after_filters: usize,
    limit: Option<usize>,
    default_open_lens: bool,
) {
    if total_after_filters > 0 && limit.is_some() {
        println!(
            "\n{}",
            list_human_count_footer(0, total_after_filters, limit, default_open_lens)
        );
    }
}

#[cfg(test)]
#[path = "tests/bug_1209_list_limit_footer_tests.rs"]
mod bug_1209_list_limit_footer_tests;

/// STORY-723: the role-routed queue entries that are ACTUALLY actionable, in
/// queue-position order. The raw role queue is padded with archived / completed
/// / deferred corpses that were never dequeued (BUG: the front-door `next` once
/// nudged `aida queue work STORY-248` for an archived spec); this is the set a
/// nudge may safely point at. Each tuple is `(display_id, title, status)`. Live
/// = not archived, not deferred, not blocked, and in a workable status. Reads
/// the queue YAML + read-only cache only — NO full store load. (Supersedes
/// TASK-970's `agent_bare_top_queued`, which did no liveness filtering.)
// trace:STORY-723 trace:TASK-970
pub(crate) fn role_queue_actionable(
    project_root: &std::path::Path,
    role: &str,
) -> Vec<(String, String, String)> {
    // BUG-675: fold identity the SAME way read_queue_depth / `aida queue list`
    // do, so the actionable set is read from the case-correct queue file (a shell
    // reporting `Joe` finds `joe.yaml`) rather than silently reading zero.
    // trace:BUG-675 | ai:claude
    let store_path = project_root.join(".aida-store");
    let user = aida_core::db::resolve_queue_user(&store_path, &current_user_id(None));
    let queue_path = store_path
        .join("registry/queues")
        .join(format!("{}.yaml", user));
    // Own file first (its ordering is this shell's own intent), then every
    // sibling file — a `--for <role>` routing written by a peer lands in the
    // PEER's queue file, and reading only our own hid it. Same role predicate
    // either way; nothing is written back. trace:BUG-774 | ai:claude
    let mut queue_files: Vec<std::path::PathBuf> = vec![queue_path.clone()];
    if let Ok(read) = std::fs::read_dir(store_path.join("registry/queues")) {
        let me = aida_core::node::canonical_user_id(&user);
        let mut siblings: Vec<std::path::PathBuf> = read
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("yaml"))
            .filter(|p| {
                p.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|stem| aida_core::node::canonical_user_id(stem) != me)
                    .unwrap_or(false)
            })
            .collect();
        siblings.sort();
        queue_files.extend(siblings);
    }
    // Mirror read_queue_depth's role filter, then order by queue position.
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut role_entries: Vec<(i64, String)> = Vec::new();
    for path in &queue_files {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(entries) = serde_yaml::from_str::<Vec<serde_yaml::Value>>(&content) else {
            continue;
        };
        let mut from_file: Vec<(i64, String)> = entries
            .iter()
            .filter(|e| {
                e.get("for_role")
                    .and_then(serde_yaml::Value::as_str)
                    .map(|r| r == role)
                    .unwrap_or(false)
            })
            .filter_map(|e| {
                let pos = e
                    .get("position")
                    .and_then(serde_yaml::Value::as_i64)
                    .unwrap_or(0);
                let id = e
                    .get("requirement_id")
                    .and_then(serde_yaml::Value::as_str)?
                    .to_string();
                Some((pos, id))
            })
            .collect();
        from_file.sort_by_key(|(pos, _)| *pos);
        for (pos, id) in from_file {
            if seen_ids.insert(id.clone()) {
                role_entries.push((pos, id));
            }
        }
    }
    // Resolve id + title + liveness fields from the read-only cache (no full
    // store load), keyed by the requirement UUID the queue entry carries, and
    // KEEP only the actionable ones.
    let cache_path = project_root.join(".aida/cache.db");
    let conn = rusqlite::Connection::open_with_flags(
        &cache_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok();
    role_entries
        .into_iter()
        .filter_map(|(_, uuid)| {
            let row = conn.as_ref().and_then(|c| {
                c.query_row(
                    "SELECT COALESCE(agreed_id, spec_id, ''), title, status, \
                     archived, deferred, blocked \
                     FROM requirements_cache WHERE id = ?1",
                    [&uuid],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, i64>(5)?,
                        ))
                    },
                )
                .ok()
            })?;
            let (id, title, status, archived, deferred, blocked) = row;
            // Drop archived / deferred / blocked / non-workable-status corpses —
            // never surface one as the actionable head. trace:STORY-723
            if !queue_row_actionable(&status, archived, deferred, blocked) {
                return None;
            }
            let display_id = if id.is_empty() { uuid } else { id };
            Some((display_id, title, status))
        })
        .collect()
}

/// STORY-723: the top-N ACTIONABLE queued items for the active role as
/// `(display_id, title)` — the head of [`role_queue_actionable`].
pub(crate) fn agent_bare_top_queued(
    project_root: &std::path::Path,
    role: &str,
    n: usize,
) -> Vec<(String, String)> {
    role_queue_actionable(project_root, role)
        .into_iter()
        .take(n)
        .map(|(id, title, _status)| (id, title))
        .collect()
}

/// TASK-970: content-first bare `aida` for AGENT MODE. Prints the same fast
/// `aida status` snapshot a human gets from `aida status` (role / branch /
/// queue depth / counts), then lists the top queued items for the active role.
/// Self-resolves the project root and reads the cache read-only — no full store
/// load — mirroring the STORY-707 fast path. Falls back to the getting-started
/// menu when not inside an AIDA project (where the snapshot would be empty).
// trace:TASK-970
pub(crate) fn handle_bare_agent_status() -> Result<()> {
    let Ok(project_root) = find_project_root() else {
        print_tiered_help();
        return Ok(());
    };
    let snap = collect_fast_status_snapshot(&project_root);
    // TASK-964: in TOON agent mode, emit the token-efficient snapshot (this
    // handler is already agent-only, but AIDA_AGENT_OUTPUT=0 force-selects the
    // human shape for parity with the explicit `aida status`). STORY-730: lead
    // with the morning-after drain banner. trace:TASK-964 trace:STORY-730
    let drain_root = find_main_worktree_root().unwrap_or_else(|_| project_root.clone());
    if agent_output_mode() {
        print_live_drain_status_line(&drain_root);
        print_morning_after_toon(&project_root);
        print_toon_status(&snap, &project_root);
        return Ok(());
    }
    print_live_drain_status_line(&drain_root);
    print_morning_after_banner(&project_root);
    print_fast_status(&snap);

    let queued = agent_bare_top_queued(&project_root, &snap.role, AGENT_BARE_QUEUE_TOPN);
    println!("{}", "─── Queued (top) ───".bold());
    if queued.is_empty() {
        println!("  {}", "(nothing queued for this role)".dimmed());
    } else {
        for (id, title) in &queued {
            if title.is_empty() {
                println!("  {id}");
            } else {
                println!("  {}  {}", id.bold(), title);
            }
        }
        if snap.queue_depth > queued.len() {
            println!(
                "  {}",
                format!(
                    "(+{} more — `aida queue list`)",
                    snap.queue_depth - queued.len()
                )
                .dimmed()
            );
        }
    }
    println!();
    Ok(())
}

/// Surface an in-flight drain at the top of `aida status` using the same
/// local-only liveness probe as `statusline` and `statusbar`. Stale/no drain
/// renders nothing, preserving existing bytes on quiet projects.
// trace:TASK-1194 | ai:codex
pub(crate) fn print_live_drain_status_line(project_root: &std::path::Path) {
    if let Some(probe) = crate::drain_state::drain_liveness_probe(project_root) {
        println!("{}", probe.status_line().cyan().bold());
        println!();
    }
}

#[cfg(test)]
#[path = "tests/story723_front_door_tests.rs"]
mod story723_front_door_tests;

#[cfg(test)]
#[path = "tests/task1456_rework_visibility_tests.rs"]
mod task1456_rework_visibility_tests;

#[cfg(test)]
#[path = "tests/story707_fast_status_tests.rs"]
mod story707_fast_status_tests;

// BUG-670 → BUG-675: the AGENT-MODE `aida status` projection must give an
// unambiguous "is there work for me?" signal — lead with `queue_actionable`. With
// BUG-675 making `queue_depth` identity-trustworthy (same resolver as
// `aida queue list`), the raw depth now ALWAYS rides along after the lead signal
// instead of being suppressed on divergence.
#[cfg(test)]
#[path = "tests/bug670_agent_status_tests.rs"]
mod bug670_agent_status_tests;

/// TASK-1055: warm the independent gh-backed status probes concurrently so a
/// `--full` render pays the latency of the SLOWEST probe, not the SUM. Each
/// target is behind a process-lifetime memo (open-PR snapshot via BUG-613,
/// PR/CI facts + recently-merged via TASK-1055), so calling them here only
/// pre-populates the cache the sequential render reads next — output is
/// unchanged. The current-branch facts need the branch name, derived from a
/// cheap local `git` call. Worktree rows read `collect_open_prs(main_root)`,
/// so both the cwd root and the main-worktree root are warmed (deduped to one
/// fetch when they're the same canonical path).
// trace:TASK-1055
pub(crate) fn warm_status_network_probes(project_root: &std::path::Path) {
    let main_root = main_worktree_root_from(project_root);
    let branch_name = collect_branch_facts(project_root).map(|b| b.name);

    // Distinct canonical roots whose open-PR snapshot the render will read.
    let proj_canon = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let main_canon = main_root
        .canonicalize()
        .unwrap_or_else(|_| main_root.clone());
    let mut pr_roots: Vec<std::path::PathBuf> = vec![project_root.to_path_buf()];
    if main_canon != proj_canon {
        pr_roots.push(main_root.clone());
    }

    std::thread::scope(|s| {
        for root in &pr_roots {
            s.spawn(move || {
                let _ = collect_open_prs(root);
            });
        }
        s.spawn(|| {
            // Matches the limit the recently-merged render uses below.
            let _ = collect_recently_merged_prs(project_root, 5);
        });
        if let Some(branch) = branch_name.as_deref() {
            s.spawn(move || {
                let _ = collect_pr_facts(project_root, branch);
            });
        }
    });
}

/// TASK-1065: assemble the `RequirementsStore` the rich `aida status` sections
/// consume from the CACHE read-projection instead of a full `backend.load()`.
///
/// The store's metadata (name / title / description / features / id-config) comes
/// from one cheap `metadata.yaml` read; its `requirements` are reconstructed from
/// `list_summaries` (archive + defer = Both, so the projection mirrors the full
/// on-disk set `backend.load()` returned). Each lightweight `Requirement` carries
/// exactly the fields the `aida status --full` hygiene/cleanup doctor scans, the
/// queue snapshot, and the Project/Requirements sections read: id, spec_id,
/// agreed_id, title, status, req_type, modified_at, archived. Fields not projected
/// into the cache (description body, comments, relationships, the decision-request
/// payload, …) are left at their `Requirement::new` defaults — the status sections
/// never read them (the decision-inbox count reads the `has_pending_decision`
/// cache column via `backend.pending_decision_count()` instead).
///
/// Fidelity notes (same cache-vs-load divergences `aida list` already carries):
/// an EPIC's status is the cache's derived rollup rather than its stored value,
/// and a spec with a *custom* status string maps to `Draft` + `custom_status`
/// since it isn't one of the eight canonical variants. Both are exotic and do not
/// affect the doctor scans (which exclude epics and only match canonical
/// statuses).
// trace:TASK-1065 | ai:claude
pub(crate) fn build_status_store_from_cache(
    backend: &aida_core::CachedGitBackend,
) -> Result<aida_core::models::RequirementsStore> {
    use aida_core::{RequirementStatus, RequirementType};

    let mut store = backend.load_metadata_only()?;

    let summaries = backend.list_summaries(&aida_core::ListFilter {
        archive: aida_core::ArchiveFilter::Both,
        defer: aida_core::db::DeferFilter::Both,
        ..Default::default()
    })?;

    store.requirements = summaries
        .into_iter()
        .map(|s| {
            let mut req = aida_core::Requirement::new(s.title, String::new());
            req.id = s.id;
            req.spec_id = s.spec_id;
            req.agreed_id = s.agreed_id;
            req.owner = s.owner;
            req.assignee = s.assignee;
            req.feature = s.feature;
            req.archived = s.archived;
            req.deferred = s.deferred;
            // The cache stores RequirementType's Debug form; map it back. An
            // unrecognized (custom) type falls back to Task — it won't match any
            // status-scan type predicate, which is the safe default.
            req.req_type =
                RequirementType::from_cache_str(&s.req_type).unwrap_or(RequirementType::Task);
            // Canonical status → the typed enum. A custom status string isn't one
            // of the eight variants, so preserve it verbatim in `custom_status`
            // (and leave the enum at Draft) rather than mis-map it.
            match RequirementStatus::from_filter_str(&s.status) {
                Some(st) => req.status = st,
                None => {
                    req.status = RequirementStatus::Draft;
                    req.custom_status = Some(s.status);
                }
            }
            req.modified_at = chrono::DateTime::parse_from_rfc3339(&s.modified_at)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or(req.modified_at);
            req
        })
        .collect();

    Ok(store)
}

// TASK-1065: the rich `aida status --full` store is sourced from the cache
// read-projection, never a full `backend.load()` over the object YAMLs, and the
// decision-inbox count reads the `has_pending_decision` cache column. These tests
// lock both contracts. trace:TASK-1065 | ai:claude
#[cfg(test)]
#[path = "tests/task_1065_status_cacheback_tests.rs"]
mod task_1065_status_cacheback_tests;

/// Detect whether `project_root` is the aida repo itself — used to opt into
/// the developer-context section. We check the workspace root Cargo.toml for
/// the joemooney/aida repository URL.
pub(crate) fn is_aida_repo(project_root: &std::path::Path) -> bool {
    let cargo_toml = project_root.join("Cargo.toml");
    if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
        // Match the workspace.package repository field exactly so this is
        // robust against forks: only the canonical repo gets dev context.
        return content.contains("repository = \"https://github.com/joemooney/aida\"")
            && content.contains("[workspace]");
    }
    false
}

pub(crate) fn print_aida_dev_context(project_root: &std::path::Path) {
    println!("{}", "─── AIDA development context ───".bold());

    // Workspace version vs latest tag.
    let workspace_version = read_workspace_version(project_root).unwrap_or_else(|| "?".into());
    let latest_tag = git_describe_latest_tag(project_root).unwrap_or_else(|| "(none)".into());
    let commits_since_tag = git_commits_since_tag(project_root, &latest_tag).unwrap_or(0);
    let cross_platform_ci = if commits_since_tag > 0 {
        Some(cross_platform_ci_status(project_root))
    } else {
        None
    };
    println!("  Running binary:     {}", build_banner());
    println!("  Workspace version:  v{}", workspace_version);
    println!("  Latest release tag: {}", latest_tag);
    if commits_since_tag > 0 {
        let readiness = match cross_platform_ci.as_ref().map(|ci| ci.release_gate) {
            Some(CrossPlatformReleaseGate::Ready) => "ready to cut a release".green().to_string(),
            Some(CrossPlatformReleaseGate::Blocked) => "BLOCKED".red().bold().to_string(),
            Some(CrossPlatformReleaseGate::Unknown) | None => "unknown".yellow().to_string(),
        };
        println!(
            "  {} commits ahead of {} — release-readiness: {}",
            commits_since_tag, latest_tag, readiness
        );
        if let Some(ci) = &cross_platform_ci {
            println!("  Cross-platform CI:  {}", ci.summary);
            if let Some(detail) = &ci.detail {
                println!("    {}", detail);
            }
        }
    } else {
        println!("  Tree matches latest tag — no pending release");
    }

    // Template-symlink integrity.
    let symlink_status = check_template_symlinks(project_root);
    println!("  Template symlinks:  {}", symlink_status);

    // Quick build sanity (just check `target/` exists and Cargo.lock is in sync).
    let cargo_lock_synced = project_root.join("Cargo.lock").exists();
    println!(
        "  Cargo.lock present: {}",
        if cargo_lock_synced {
            "yes".green().to_string()
        } else {
            "NO".red().to_string()
        }
    );

    println!();
    println!("  Helpful:");
    println!(
        "    {}    — bump version + tag + push",
        "scripts/release.sh".cyan()
    );
    println!(
        "    {}    — cargo publish to crates.io",
        "scripts/publish.sh".cyan()
    );
    println!();
}

/// STORY-673: terse one-line AIDA-dev-context for the default `aida status`.
/// Shows the running binary + workspace version + commits-since-tag, and
/// crucially SKIPS the cross-platform-CI network probe and template/lockfile
/// checks the full block runs — so the default path stays cheap. `--full` /
/// `--all` calls `print_aida_dev_context` for the complete picture.
/// trace:STORY-673 | ai:claude
pub(crate) fn print_aida_dev_context_summary(project_root: &std::path::Path) {
    let workspace_version = read_workspace_version(project_root).unwrap_or_else(|| "?".into());
    let latest_tag = git_describe_latest_tag(project_root).unwrap_or_else(|| "(none)".into());
    let commits_since_tag = git_commits_since_tag(project_root, &latest_tag).unwrap_or(0);
    let ahead = if commits_since_tag > 0 {
        format!(", {commits_since_tag} commits ahead of {latest_tag}")
    } else {
        ", matches latest tag".to_string()
    };
    println!("{}", "─── AIDA development context ───".bold());
    println!(
        "  {}  (v{}{}) · `aida status --full`",
        build_banner(),
        workspace_version,
        ahead
    );
    println!();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CrossPlatformReleaseGate {
    Ready,
    Blocked,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CrossPlatformCiSummary {
    pub(crate) release_gate: CrossPlatformReleaseGate,
    pub(crate) summary: String,
    pub(crate) detail: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct CrossPlatformCiCache {
    pub(crate) fetched_at: chrono::DateTime<chrono::Utc>,
    pub(crate) runs: Vec<GhWorkflowRun>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct GhWorkflowRun {
    pub(crate) status: Option<String>,
    pub(crate) conclusion: Option<String>,
    #[serde(rename = "createdAt")]
    pub(crate) created_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(rename = "databaseId")]
    pub(crate) database_id: Option<u64>,
    #[serde(rename = "headSha")]
    pub(crate) head_sha: Option<String>,
    pub(crate) url: Option<String>,
}

pub(crate) fn cross_platform_ci_status(project_root: &std::path::Path) -> CrossPlatformCiSummary {
    const CACHE_TTL_SECONDS: i64 = 300;
    let now = chrono::Utc::now();

    if let Some(cache) = read_cross_platform_ci_cache(project_root) {
        if now.signed_duration_since(cache.fetched_at).num_seconds() < CACHE_TTL_SECONDS {
            return summarize_cross_platform_ci_runs(now, Ok(cache.runs));
        }
    }

    let runs = fetch_cross_platform_ci_runs(project_root);
    if let Ok(runs) = &runs {
        let cache = CrossPlatformCiCache {
            fetched_at: now,
            runs: runs.clone(),
        };
        let _ = write_cross_platform_ci_cache(project_root, &cache);
    }
    summarize_cross_platform_ci_runs(now, runs)
}

pub(crate) fn fetch_cross_platform_ci_runs(
    project_root: &std::path::Path,
) -> std::result::Result<Vec<GhWorkflowRun>, String> {
    fetch_cross_platform_ci_runs_with_args(project_root, &[])
}

// trace:STORY-1043 | ai:codex
pub(crate) fn fetch_scheduled_cross_platform_ci_runs(
    project_root: &std::path::Path,
) -> std::result::Result<Vec<GhWorkflowRun>, String> {
    fetch_cross_platform_ci_runs_with_args(project_root, &["--event", "schedule"])
}

// trace:STORY-1043 | ai:codex
pub(crate) fn fetch_cross_platform_ci_runs_with_args(
    project_root: &std::path::Path,
    extra_args: &[&str],
) -> std::result::Result<Vec<GhWorkflowRun>, String> {
    let mut args = vec![
        "run",
        "list",
        "--workflow",
        "cross-platform.yml",
        "--branch",
        "main",
        "--limit",
        "20",
        "--json",
        "status,conclusion,createdAt,databaseId,headSha,url",
    ];
    args.extend_from_slice(extra_args);
    let output = std::process::Command::new("gh")
        .args(args)
        .current_dir(project_root)
        .output()
        .map_err(|err| err.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            "gh run list failed".to_string()
        } else {
            stderr
        });
    }
    serde_json::from_slice(&output.stdout).map_err(|err| err.to_string())
}

pub(crate) fn read_cross_platform_ci_cache(
    project_root: &std::path::Path,
) -> Option<CrossPlatformCiCache> {
    let path = cross_platform_ci_cache_path(project_root);
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

pub(crate) fn write_cross_platform_ci_cache(
    project_root: &std::path::Path,
    cache: &CrossPlatformCiCache,
) -> Result<()> {
    let path = cross_platform_ci_cache_path(project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_atomic(&path, serde_json::to_string_pretty(cache)?.as_bytes())?;
    Ok(())
}

pub(crate) fn cross_platform_ci_cache_path(project_root: &std::path::Path) -> std::path::PathBuf {
    project_root
        .join(".aida")
        .join("cache")
        .join("cross-platform-ci-status.json")
}

pub(crate) fn summarize_cross_platform_ci_runs(
    now: chrono::DateTime<chrono::Utc>,
    runs: std::result::Result<Vec<GhWorkflowRun>, String>,
) -> CrossPlatformCiSummary {
    let runs = match runs {
        Ok(runs) => runs,
        Err(_) => {
            return CrossPlatformCiSummary {
                release_gate: CrossPlatformReleaseGate::Unknown,
                summary: "unknown (gh unreachable)".yellow().to_string(),
                detail: Some(
                    "Release readiness cannot be confirmed without GitHub CI status.".to_string(),
                ),
            };
        }
    };

    let Some(latest) = runs.first() else {
        return CrossPlatformCiSummary {
            release_gate: CrossPlatformReleaseGate::Unknown,
            summary: "unknown (no cross-platform run found)".yellow().to_string(),
            detail: Some(
                "Run `gh workflow run cross-platform.yml --ref main` before release prep."
                    .to_string(),
            ),
        };
    };

    let latest_age = latest
        .created_at
        .map(|created| format_ci_age(now, created))
        .unwrap_or_else(|| "unknown age".to_string());
    let latest_run = latest
        .database_id
        .map(|id| format!(", run {}", id))
        .unwrap_or_default();
    let latest_completed_success = latest.status.as_deref() == Some("completed")
        && latest.conclusion.as_deref() == Some("success");
    let latest_fresh = latest
        .created_at
        .map(|created| now.signed_duration_since(created).num_hours() < 24)
        .unwrap_or(false);

    if latest_completed_success && latest_fresh {
        return CrossPlatformCiSummary {
            release_gate: CrossPlatformReleaseGate::Ready,
            summary: format!(
                "{} green ({}{})",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                latest_age,
                latest_run
            ),
            detail: None,
        };
    }

    let last_green = runs.iter().find(|run| {
        run.status.as_deref() == Some("completed") && run.conclusion.as_deref() == Some("success")
    });
    let last_green_detail = last_green.and_then(|run| {
        run.created_at.map(|created| {
            format!(
                "Last green: {}. Releases require <24h green.",
                format_ci_age(now, created)
            )
        })
    });

    let reason = if latest_completed_success {
        "stale"
    } else {
        latest.conclusion.as_deref().unwrap_or("not green")
    };
    CrossPlatformCiSummary {
        release_gate: CrossPlatformReleaseGate::Blocked,
        summary: format!(
            "{} {} ({}{})",
            crate::glyph(crate::glyphs::Glyph::Cross).red(),
            reason,
            latest_age,
            latest_run
        ),
        detail: last_green_detail.or_else(|| {
            Some(
                "No previous green cross-platform run found. Releases require <24h green."
                    .to_string(),
            )
        }),
    }
}

// trace:STORY-1043 | ai:codex
pub(crate) fn nightly_red_status(
    project_root: &std::path::Path,
) -> Option<awaiting_you::NightlyRedItem> {
    let now = chrono::Utc::now();
    let latest_completed_main_run =
        fetch_cross_platform_ci_runs_with_args(project_root, &["--status", "completed"])
            .ok()
            .and_then(|runs| runs.into_iter().next());
    summarize_nightly_red_runs(
        now,
        fetch_scheduled_cross_platform_ci_runs(project_root).ok(),
        latest_completed_main_run.as_ref(),
        |ancestor, descendant| {
            is_ancestor_commit(project_root, ancestor, descendant).unwrap_or(false)
        },
    )
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct NightlyRedCache {
    pub(crate) fetched_at: chrono::DateTime<chrono::Utc>,
    pub(crate) item: Option<awaiting_you::NightlyRedItem>,
}

// BUG-1288: machine consumers poll this surface. Bound the two workflow API
// calls with a documented 30-second freshness window; a miss refreshes the
// exact same report and subsequent polls remain network-free.
// trace:BUG-1288 | ai:codex
pub(crate) fn cached_nightly_red_status(
    project_root: &std::path::Path,
) -> Option<awaiting_you::NightlyRedItem> {
    let path = project_root
        .join(".aida")
        .join("cache")
        .join("awaiting-nightly-red.json");
    let now = chrono::Utc::now();
    if let Ok(raw) = std::fs::read_to_string(&path) {
        if let Ok(cache) = serde_json::from_str::<NightlyRedCache>(&raw) {
            if now.signed_duration_since(cache.fetched_at).num_seconds() < 30 {
                return cache.item;
            }
        }
    }
    let item = nightly_red_status(project_root);
    let cache = NightlyRedCache {
        fetched_at: now,
        item: item.clone(),
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(raw) = serde_json::to_vec(&cache) {
        let _ = write_atomic(&path, &raw);
    }
    item
}

// trace:STORY-1043 | ai:codex
pub(crate) fn summarize_nightly_red_runs(
    now: chrono::DateTime<chrono::Utc>,
    runs: Option<Vec<GhWorkflowRun>>,
    latest_completed_main_run: Option<&GhWorkflowRun>,
    head_descends_from_latest_scheduled_failure: impl Fn(&str, &str) -> bool,
) -> Option<awaiting_you::NightlyRedItem> {
    let runs = runs?;
    let latest = runs.first()?;
    if latest.status.as_deref() != Some("completed")
        || latest.conclusion.as_deref() == Some("success")
    {
        return None;
    }
    if latest_completed_main_run_clears_nightly_red(
        latest,
        latest_completed_main_run,
        head_descends_from_latest_scheduled_failure,
    ) {
        return None;
    }
    let streak: Vec<&GhWorkflowRun> = runs
        .iter()
        .take_while(|run| {
            run.status.as_deref() == Some("completed")
                && run.conclusion.as_deref().is_some_and(|c| c != "success")
        })
        .collect();
    let nights = streak.len().max(1);
    let since_run = streak.last().copied().unwrap_or(latest);
    let since = since_run
        .created_at
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_else(|| "unknown date".to_string());
    let run = latest.database_id;
    let run_text = run
        .map(|id| format!("run {id}"))
        .unwrap_or_else(|| "run unknown".to_string());
    let summary = format!(
        "cross-platform nightly red since {since} ({run_text}, {nights} night{})",
        if nights == 1 { "" } else { "s" }
    );
    let _ = now;
    Some(awaiting_you::NightlyRedItem {
        summary,
        run_id: run,
        nights,
    })
}

// trace:BUG-1148 | ai:codex
pub(crate) fn latest_completed_main_run_clears_nightly_red(
    latest_scheduled_failure: &GhWorkflowRun,
    latest_completed_main_run: Option<&GhWorkflowRun>,
    head_descends_from_latest_scheduled_failure: impl Fn(&str, &str) -> bool,
) -> bool {
    let Some(latest_completed_main_run) = latest_completed_main_run else {
        return false;
    };
    if latest_completed_main_run.status.as_deref() != Some("completed")
        || latest_completed_main_run.conclusion.as_deref() != Some("success")
    {
        return false;
    }
    let Some(failure_created_at) = latest_scheduled_failure.created_at else {
        return false;
    };
    let Some(clearing_created_at) = latest_completed_main_run.created_at else {
        return false;
    };
    if clearing_created_at <= failure_created_at {
        return false;
    }
    let Some(failure_sha) = latest_scheduled_failure
        .head_sha
        .as_deref()
        .filter(|sha| !sha.trim().is_empty())
    else {
        return false;
    };
    let Some(clearing_sha) = latest_completed_main_run
        .head_sha
        .as_deref()
        .filter(|sha| !sha.trim().is_empty())
    else {
        return false;
    };
    head_descends_from_latest_scheduled_failure(failure_sha, clearing_sha)
}

pub(crate) fn format_ci_age(
    now: chrono::DateTime<chrono::Utc>,
    then: chrono::DateTime<chrono::Utc>,
) -> String {
    let seconds = now.signed_duration_since(then).num_seconds().max(0);
    if seconds < 3600 {
        let minutes = (seconds / 60).max(1);
        format!("{}m ago", minutes)
    } else if seconds < 48 * 3600 {
        format!("{}h ago", seconds / 3600)
    } else {
        format!("{} days ago", seconds / 86_400)
    }
}

#[cfg(test)]
#[path = "tests/task_486_cross_platform_ci_status_tests.rs"]
mod task_486_cross_platform_ci_status_tests;

pub(crate) fn read_workspace_version(root: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let mut in_workspace_package = false;
    for line in content.lines() {
        let line = line.trim();
        if line == "[workspace.package]" {
            in_workspace_package = true;
            continue;
        }
        if in_workspace_package {
            if line.starts_with('[') {
                break;
            }
            if let Some(rest) = line.strip_prefix("version") {
                let v = rest
                    .trim_start_matches(|c: char| c.is_whitespace() || c == '=')
                    .trim_matches('"');
                return Some(v.to_string());
            }
        }
    }
    None
}

pub(crate) fn git_describe_latest_tag(root: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["describe", "--tags", "--abbrev=0"])
        .current_dir(root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub(crate) fn git_commits_since_tag(root: &std::path::Path, tag: &str) -> Option<usize> {
    if tag == "(none)" {
        return None;
    }
    let out = std::process::Command::new("git")
        .args(["rev-list", "--count", &format!("{}..HEAD", tag)])
        .current_dir(root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

pub(crate) fn check_template_symlinks(root: &std::path::Path) -> String {
    let claude_skills = root.join(".claude/skills");
    if !claude_skills.is_dir() {
        return "no .claude/skills/ directory".to_string();
    }
    let mut total = 0;
    let mut broken = 0;
    if let Ok(entries) = std::fs::read_dir(&claude_skills) {
        for entry in entries.flatten() {
            let path = entry.path();
            // Only count the actual symlinks (skip non-symlink files).
            if let Ok(meta) = std::fs::symlink_metadata(&path) {
                if meta.file_type().is_symlink() {
                    total += 1;
                    if !path.exists() {
                        broken += 1;
                    }
                }
            }
        }
    }
    if total == 0 {
        return "no symlinks (templates copied, not symlinked)".to_string();
    }
    if broken == 0 {
        format!("{}/{} OK", total, total).green().to_string()
    } else {
        format!("{} broken / {}", broken, total).red().to_string()
    }
}

/// Returns (ahead, behind) of `aida-store` branch vs `origin/aida-store`.
pub(crate) fn orphan_branch_sync_state(store_path: &std::path::Path) -> Option<(usize, usize)> {
    // Run inside the store worktree so we see the right branch.
    let ahead = std::process::Command::new("git")
        .args(["rev-list", "--count", "origin/aida-store..HEAD"])
        .current_dir(store_path)
        .output()
        .ok()?;
    let behind = std::process::Command::new("git")
        .args(["rev-list", "--count", "HEAD..origin/aida-store"])
        .current_dir(store_path)
        .output()
        .ok()?;
    if !ahead.status.success() || !behind.status.success() {
        return None;
    }
    let a: usize = String::from_utf8_lossy(&ahead.stdout).trim().parse().ok()?;
    let b: usize = String::from_utf8_lossy(&behind.stdout)
        .trim()
        .parse()
        .ok()?;
    Some((a, b))
}

// ----------------------------------------------------------------------------
// `aida upgrade` — fetch latest release and replace the running binary.
// trace:EPIC-1-001 | ai:claude
// ----------------------------------------------------------------------------

/// How aida was installed on this machine. Determines the upgrade strategy.
pub(crate) enum InstallMethod {
    /// Found under `~/.cargo/bin/` — installed via `cargo install`.
    /// Upgrade by re-running `cargo install --git`.
    Cargo(std::path::PathBuf),
    /// Found in a system bin dir (`/usr/local/bin`, `/opt/...`, etc.) —
    /// installed via release tarball. Upgrade by downloading the matching
    /// release artifact and replacing the binary in place.
    Binary(std::path::PathBuf),
    /// Found inside a `target/debug` or `target/release` directory — the
    /// running binary is a developer build. Refuse to upgrade.
    DeveloperBuild(std::path::PathBuf),
}

pub(crate) fn detect_install_method() -> Result<InstallMethod> {
    let exe = aida_exe_path();
    let exe_str = exe.to_string_lossy();

    if exe_str.contains("/target/debug/") || exe_str.contains("/target/release/") {
        return Ok(InstallMethod::DeveloperBuild(exe));
    }

    // Cargo install puts binaries in $CARGO_HOME/bin (default ~/.cargo/bin).
    let cargo_home = std::env::var("CARGO_HOME").ok();
    let cargo_bin = cargo_home
        .map(|h| std::path::PathBuf::from(h).join("bin"))
        .or_else(|| crate::home_dir().map(|h| h.join(".cargo/bin")));
    if let Some(bin) = cargo_bin {
        if exe.starts_with(&bin) {
            return Ok(InstallMethod::Cargo(exe));
        }
    }

    Ok(InstallMethod::Binary(exe))
}

pub(crate) fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Build git SHA stamped at compile time (or "unknown" if git wasn't
/// available in the build env). Set by build.rs.
pub(crate) fn build_git_sha() -> &'static str {
    env!("AIDA_BUILD_GIT_SHA")
}

/// Whether the working tree had uncommitted changes at build time.
pub(crate) fn build_git_dirty() -> bool {
    env!("AIDA_BUILD_GIT_DIRTY") == "1"
}

/// Build time formatted in the user's local timezone — the banner is
/// human-facing so we render it where the user reads it, not in UTC.
/// On-disk fields (oplog, YAML created_at, etc.) stay UTC.
/// trace:feedback_local_time | ai:claude
pub(crate) fn build_time_iso() -> String {
    let secs: i64 = env!("AIDA_BUILD_UNIX_TIME").parse().unwrap_or(0);
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S %Z")
                .to_string()
        })
        .unwrap_or_else(|| "(unknown)".to_string())
}

/// One-line build banner for use in --version and status output:
///   "0.4.0 (built 2026-05-03 01:23:45 PDT, sha 866b050[+dirty])"
pub(crate) fn build_banner() -> String {
    format!(
        "{} (built {}, sha {}{})",
        current_version(),
        build_time_iso(),
        build_git_sha(),
        if build_git_dirty() { "+dirty" } else { "" }
    )
}

/// Compile-time-resolved release-artifact target name (matches the matrix in
/// `.github/workflows/release.yml`). Returns None on unsupported platforms.
pub(crate) fn release_target() -> Option<&'static str> {
    use std::env::consts::{ARCH, OS};
    match (OS, ARCH) {
        ("linux", "x86_64") => Some("linux-x86_64"),
        ("linux", "aarch64") => Some("linux-arm64"),
        ("macos", "x86_64") => Some("darwin-x86_64"),
        ("macos", "aarch64") => Some("darwin-arm64"),
        _ => None,
    }
}

/// Strip a leading `v` from a tag string (`v0.4.0` -> `0.4.0`).
pub(crate) fn strip_v(s: &str) -> &str {
    s.strip_prefix('v').unwrap_or(s)
}

// ---- shared helpers -------------------------------------------------------

// ?-exempt: bare helper; each consequential CALLER carries its own card or exemption. trace:STORY-809
pub(crate) fn confirm(prompt: &str) -> bool {
    print!("{}", prompt);
    std::io::Write::flush(&mut std::io::stdout()).ok();
    let mut answer = String::new();
    if std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Common locations where users typically have aida installed. Order matters
/// for display; we use it as-is for the scan-and-report output.
/// Walk up from `start` (a binary path or directory) looking for the aida
/// repo root. Used to discover the dev binary's source repo so we can ask
/// "is this build ahead of the latest release tag".
pub(crate) fn find_aida_repo_above(start: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut probe = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };
    for _ in 0..6 {
        if is_aida_repo(&probe) {
            return Some(probe);
        }
        probe = match probe.parent() {
            Some(p) => p.to_path_buf(),
            None => return None,
        };
    }
    None
}

/// Bulk-import helper for git-canonical stores (FR-1-002): drains an iterator
/// of new requirements through a single `GitBackend::bulk_writer()` session
/// so one git commit covers the whole batch. Falls back to the legacy
/// `update_atomically` path for non-git backends (SQLite / YAML), since the
/// bulk-writer's optimization only applies to the git path.
/// trace:FR-1-002 | ai:claude
pub(crate) fn bulk_import_via_writer<I>(
    storage: &Storage,
    commit_subject: &str,
    reqs: I,
) -> Result<usize>
where
    I: IntoIterator<Item = Requirement>,
{
    let path = storage.path();
    if path.is_dir() {
        // Attach the same dispenser `storage` was built with, when it has
        // one — otherwise a fresh `GitBackend` defaults to node_id "0" for
        // every oplog entry this writer commits. trace:TASK-1487 | ai:claude
        let mut backend = aida_core::GitBackend::new(path)?;
        if let Some(dispenser) = storage.dispenser() {
            backend = backend.with_dispenser(dispenser.clone());
        }
        let mut writer = backend.bulk_writer()?;
        for req in reqs {
            writer.add(req)?;
        }
        let n = writer.finish(commit_subject)?;
        return Ok(n);
    }

    // Non-git backend: legacy path (single update_atomically commit).
    let mut count = 0;
    let reqs: Vec<Requirement> = reqs.into_iter().collect();
    storage.update_atomically(|store| {
        for req in reqs {
            let type_prefix = store.get_type_prefix(&req.req_type);
            store.add_requirement_with_id(req, None, type_prefix.as_deref());
            count += 1;
        }
    })?;
    Ok(count)
}

/// TASK-679: a parent/child edge is canonically BIDIRECTIONAL so both ends
/// reflect the link — matching `aida add --parent`, which writes the parent's
/// `Parent --> child` edge and the child's reciprocal `Child --> parent` edge.
/// `aida rel add` previously wrote only the source-side edge for these types.
/// Any other edge type keeps the explicit `--bidirectional` opt-in. Pure so
/// the canonical-edge decision is unit-testable. trace:TASK-679 | ai:claude
pub(crate) fn rel_should_write_inverse(
    rel_type: &RelationshipType,
    bidirectional_flag: bool,
) -> bool {
    bidirectional_flag || matches!(rel_type, RelationshipType::Parent | RelationshipType::Child)
}

/// Parse the relationship vocabulary accepted by `aida rel add` / `aida rel
/// remove`.
///
/// `related` is the natural spelling for a general link, but storing it as a
/// custom edge makes the link invisible to standard graph traversal; this
/// normalizes it (and the other input-only aliases like `depends-on` /
/// `verified_by` / `replaced_by`) to the matching standard taxonomy member.
/// Thin wrapper around the shared parser in aida-core
/// (`RelationshipType::parse_relationship_type`) so `rel add`, `rel remove`
/// and the MCP `relationship_type` param all resolve the same spelling the
/// same way — see that function's doc comment for why it's kept separate
/// from `RelationshipType::from_str`, the core/Deserialize parser.
// trace:BUG-1471 | ai:codex trace:BUG-1602 | ai:claude
pub(crate) fn cli_relationship_type(input: &str) -> RelationshipType {
    RelationshipType::parse_relationship_type(input)
}

/// Does a stored edge match the `--type` given to `rel remove`?
///
/// Exact type match, plus: asking to remove `references` (or its `related`
/// alias) also matches a stored legacy custom `related` / `related-to` /
/// `relates-to` edge, and asking for one of those legacy spellings matches
/// the whole family, so the obvious hand repair does not leave a stale edge
/// behind. Before this, the git-store `rel remove` ignored `--type` and
/// deleted every edge to the target.
// trace:TASK-1426 | ai:claude
pub(crate) fn rel_remove_matches(stored: &RelationshipType, requested: &RelationshipType) -> bool {
    use crate::related_edge_migration::is_legacy_related;
    if stored == requested {
        return true;
    }
    let requested_is_references_family =
        *requested == RelationshipType::References || is_legacy_related(requested);
    requested_is_references_family
        && (is_legacy_related(stored)
            || (is_legacy_related(requested) && *stored == RelationshipType::References))
}

pub(crate) fn looks_like_cross_store_spec_ref(s: &str) -> bool {
    let Some((project, spec)) = s.split_once('#') else {
        return false;
    };
    !project.trim().is_empty()
        && !spec.trim().is_empty()
        && spec
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')
}

pub(crate) fn reject_cross_store_spec_ref(raw: &str) -> Result<()> {
    if looks_like_cross_store_spec_ref(raw) {
        anyhow::bail!(
            "cross-store requirement reference `{raw}` cannot be resolved as a local graph edge. \
             Record it in a spec comment as `{raw}` until AIDA has first-class cross-store links."
        );
    }
    Ok(())
}

pub(crate) fn relationship_terminal_ambiguity_warning(
    rel_type: &RelationshipType,
    source: &Requirement,
    target: &Requirement,
) -> Option<String> {
    if matches!(rel_type, RelationshipType::Parent | RelationshipType::Child) {
        return None;
    }
    if is_terminal_status(&source.status) || !is_terminal_status(&target.status) {
        return None;
    }
    Some(format!(
        "target {} is {} while source {} is {}; if this was meant for another store, do not use this local edge. Record the cross-store reference in a comment instead.",
        target.spec_id.as_deref().unwrap_or("?"),
        target.status,
        source.spec_id.as_deref().unwrap_or("?"),
        source.status,
    ))
}

// TASK-928 (SPIKE-71 source-side fix): a `parent:<SPEC-ID>` tag must
// materialize the REAL bidirectional parent/child edge, not just sit on the
// spec as an opaque string. The canonical `--parent` flag already writes the
// edge; a spec filed with only `--tags "parent:EPIC"` used to be orphaned
// from the graph — the exact trap behind SPIKE-71 (`aida graph tree` and the
// TUI focus-lens never saw it). This closes that gap for `aida add` and the
// tag-changing `aida edit` path. (Plain `//`, not `///`: keeps the SPEC-ID
// breadcrumb out of any --help surface — substrate-as-bouncer pre-commit gate.)
//
// Contract:
//   - Additive — the `parent:` tag is left in place (the edge is *added*).
//   - Lenient — an unresolvable / self / empty target is a silent no-op
//     (tag kept, NO error), per the spec.
//   - Idempotent — an already-present Child->parent edge is skipped, and the
//     reciprocal Parent->child edge is deduped independently.
//   - Bidirectional — writes BOTH ends (same shape as `aida add --parent`),
//     so the cache write-through on each `update_requirement` keeps the graph
//     immediately consistent without a manual `aida cache rebuild`.
//
// Returns the linked parent's display id when a new edge was written (for
// caller messaging), `None` when nothing changed. trace:TASK-928 | ai:claude
pub(crate) fn ensure_parent_edge_from_tag(
    backend: &aida_core::CachedGitBackend,
    child_spec_id: &str,
) -> Result<Option<String>> {
    use aida_core::models::{Relationship, RelationshipType};

    // trace:TASK-1468 | ai:claude
    let Some(child) = backend.get_requirement_unambiguous(child_spec_id)? else {
        return Ok(None);
    };
    // First `parent:<ID>` tag, if any (a spec carrying several is degenerate;
    // honor the first deterministically).
    let Some(target) = child
        .tags
        .iter()
        .filter_map(|t| t.strip_prefix("parent:").map(|s| s.trim().to_string()))
        .find(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    // Lenient resolve — an unknown target leaves the tag untouched, no error.
    // trace:TASK-1468 | ai:claude
    let Some(parent) = backend.get_requirement_unambiguous(&target)? else {
        return Ok(None);
    };
    if parent.id == child.id {
        return Ok(None); // a spec cannot be its own parent
    }
    // Idempotent: skip when the child already records the Child→parent edge.
    let child_linked = child
        .relationships
        .iter()
        .any(|r| r.target_id == parent.id && r.rel_type == RelationshipType::Child);
    let parent_linked = parent
        .relationships
        .iter()
        .any(|r| r.target_id == child.id && r.rel_type == RelationshipType::Parent);
    if child_linked && parent_linked {
        return Ok(None);
    }

    let now = chrono::Utc::now();
    if !child_linked {
        let mut child_mut = child.clone();
        child_mut.relationships.push(Relationship {
            target_id: parent.id,
            rel_type: RelationshipType::Child,
            created_at: Some(now),
            created_by: None,
        });
        child_mut.modified_at = now;
        backend.update_requirement(&child_mut)?;
    }
    if !parent_linked {
        let mut parent_mut = parent.clone();
        parent_mut.relationships.push(Relationship {
            target_id: child.id,
            rel_type: RelationshipType::Parent,
            created_at: Some(now),
            created_by: None,
        });
        parent_mut.modified_at = now;
        backend.update_requirement(&parent_mut)?;
    }
    Ok(Some(parent.spec_id.clone().unwrap_or(target)))
}

/// The canonical user-facing spellings of the STANDARD relationship types — the
/// ones `RelationshipType::from_str` recognizes and the graph queries
/// (`--blocked-by`, `--blocks`, `--tree`, `--impact`) actually traverse. The
/// did-you-mean lens for `aida rel add --type` is computed against this set.
/// trace:TASK-887 | ai:claude
pub(crate) const STANDARD_REL_TYPES: &[&str] = &[
    "parent",
    "child",
    "duplicate",
    "verifies",
    "verified-by",
    "references",
    "related",
    "blocked-by",
    "blocks",
    // trace:TASK-1176 | ai:claude — the supersede lineage pair.
    "superseded-by",
    "supersedes",
];

/// Plain Levenshtein edit distance between two strings (case folding is the
/// caller's job). Small inputs only — relationship-type names are a handful of
/// chars — so the O(n*m) DP is fine. trace:TASK-887 | ai:claude
pub(crate) fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// TASK-887: when `aida rel add --type <x>` gets a value that ISN'T a known
/// standard relationship type, it's stored as a `Custom` edge — which the graph
/// traversals silently won't follow. Returns the nearest standard type when the
/// input is a plausible typo of one (edit distance small relative to its
/// length), for a "did you mean" hint. Returns `None` for an already-standard
/// type or a value too far from any standard to be a typo (a genuine custom
/// type). Pure → unit-testable. trace:TASK-887 | ai:claude
pub(crate) fn nearest_standard_rel_type(input: &str) -> Option<&'static str> {
    let needle = input.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }
    // Already standard (or an accepted alias normalizing to one) → no hint.
    if !matches!(cli_relationship_type(&needle), RelationshipType::Custom(_)) {
        return None;
    }
    let mut best: Option<(&'static str, usize)> = None;
    for &std_ty in STANDARD_REL_TYPES {
        let d = levenshtein(&needle, std_ty);
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((std_ty, d));
        }
    }
    match best {
        // Only call it a typo when it's close: ≤2 edits. Empirically the
        // standard-type typos sit at d≤2 while deliberate custom types
        // ("implements", "relates-to") sit at d≥6, so this cleanly
        // separates the two without nagging genuine custom edges.
        Some((ty, d)) if d <= 2 => Some(ty),
        _ => None,
    }
}

// TASK-1082: did-you-mean for a mistyped spec id. When `aida show`/`edit`/
// `queue work` (and every other spec-resolution surface) hits "Requirement not
// found: <id>", compare the requested id against the existing id set and — if a
// close match exists — append `did you mean <ID>?`, mirroring the affordance
// clap gives for mistyped subcommands and `nearest_standard_rel_type`
// (TASK-887) gives for rel-type typos. Scoped to the not-found branch; the
// found path is untouched and the exit code stays non-zero.

/// Find the nearest existing spec id to a requested one, for the not-found
/// did-you-mean hint. Case-insensitive Levenshtein with a length-aware budget:
/// short ids (≤3 chars) tolerate 1 edit, longer ids up to 2 — so a typo like
/// `TASK-11` → `TASK-1` is caught while an unrelated id is left alone. An exact
/// (case-insensitive) match returns None because that would have resolved on
/// the found path. Deterministic: lowest edit distance wins, ties prefer an id
/// that is a prefix of / prefixed by the request (the extra-digit / dropped-
/// suffix typo), then the lexicographically-smallest candidate. Pure over the
/// caller-supplied id set → unit-testable.
// trace:TASK-1082 | ai:claude
pub(crate) fn nearest_spec_id(requested: &str, known_ids: &[String]) -> Option<String> {
    let needle = requested.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }
    // If the requested id exactly (case-insensitively) exists, it's not a typo
    // — suggest nothing. In practice this fn only runs on the not-found branch,
    // but the guard keeps it honest if invoked with a resolvable id.
    if known_ids
        .iter()
        .any(|k| k.trim().eq_ignore_ascii_case(&needle))
    {
        return None;
    }
    let max_dist = if needle.len() <= 3 { 1 } else { 2 };
    // Sort key: (distance, not-prefix-related, candidate). Lower is better, so
    // `!prefix_related` sorts prefix matches first among equal-distance ties.
    let mut best: Option<(usize, bool, String)> = None;
    for known in known_ids {
        let cand = known.trim();
        if cand.is_empty() {
            continue;
        }
        let cand_lc = cand.to_lowercase();
        let d = levenshtein(&needle, &cand_lc);
        // d == 0 is the found path — never suggest the request back to itself.
        if d == 0 || d > max_dist {
            continue;
        }
        let prefix_related = needle.starts_with(&cand_lc) || cand_lc.starts_with(&needle);
        let key = (d, !prefix_related, cand.to_string());
        if best.as_ref().is_none_or(|b| key < *b) {
            best = Some(key);
        }
    }
    best.map(|(_, _, id)| id)
}

/// Extract the requested id from a "Requirement not found: <id>" error's first
/// line, so the top-level handler can compute a did-you-mean suggestion.
/// Returns None for not-found messages that carry no id (the legacy
/// `.context("Requirement not found")` chains, whose first line has no `: <id>`).
/// `invalid_spec_id_format` appends " (not a valid spec ID)" — the trailing
/// parenthetical is stripped so the bare id is returned. Pure → unit-testable.
// trace:TASK-1082 | ai:claude
pub(crate) fn not_found_requested_id(msg: &str) -> Option<String> {
    let first = msg.lines().next()?.trim();
    let rest = first.strip_prefix("Requirement not found: ")?;
    let id = rest.split(" (").next().unwrap_or(rest).trim();
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

/// Best-effort gather of every known spec id (plus agreed id) from the cache,
/// for the not-found did-you-mean lens. Cache-backed and read-only; any failure
/// (no store attached, cache locked, wrong directory) yields an empty set so
/// the caller simply omits the suggestion. Runs only on the not-found error
/// path, so the one-shot cache read is off the hot path.
// trace:TASK-1082 | ai:claude
pub(crate) fn known_spec_ids_for_suggestion(project_root: &std::path::Path) -> Vec<String> {
    let store_path = project_root.join(".aida-store");
    let Ok(dispenser) = load_dispenser(&store_path) else {
        return Vec::new();
    };
    let Ok(inner) = aida_core::GitBackend::new(&store_path).map(|b| b.with_dispenser(dispenser))
    else {
        return Vec::new();
    };
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let Ok(backend) = aida_core::CachedGitBackend::with_inner(inner, &cache_path) else {
        return Vec::new();
    };
    // Suggest against EVERY id, including archived/deferred rows — a typo should
    // still resolve to a real (if hidden) spec.
    let filter = aida_core::ListFilter {
        archive: aida_core::ArchiveFilter::Both,
        defer: aida_core::DeferFilter::Both,
        ..Default::default()
    };
    let Ok(rows) = backend.list_summaries(&filter) else {
        return Vec::new();
    };
    let mut ids = Vec::with_capacity(rows.len());
    for r in rows {
        if let Some(s) = r.spec_id {
            ids.push(s);
        }
        if let Some(a) = r.agreed_id {
            ids.push(a);
        }
    }
    ids
}

/// Glue for the top-level error handler: if `msg` is a "Requirement not found"
/// error carrying an id, resolve the nearest existing spec id and return a
/// ready-to-print `did you mean <ID>?` string. None when the message isn't a
/// not-found-with-id, or nothing is close enough to suggest.
// trace:TASK-1082 | ai:claude
pub(crate) fn did_you_mean_for_not_found(msg: &str) -> Option<String> {
    let requested = not_found_requested_id(msg)?;
    let project_root = find_main_worktree_root().ok()?;
    let known = known_spec_ids_for_suggestion(&project_root);
    let suggestion = nearest_spec_id(&requested, &known)?;
    Some(format!("did you mean {suggestion}?"))
}

// TASK-679: fully-isolated unit tests for the canonical parent/child edge
// behaviour of `aida rel add`. These exercise the model layer directly (no
// filesystem, no git) plus the pure `rel_should_write_inverse` decision.
// trace:TASK-679 | ai:claude
#[cfg(test)]
#[path = "tests/task_679_canonical_rel_tests.rs"]
mod task_679_canonical_rel_tests;

// TASK-928 (SPIKE-71 source-side fix): end-to-end tests over a real
// CachedGitBackend (temp git store + cache), proving:
//   * a `parent:<ID>` tag materializes the real bidirectional edge (part B);
//   * the edge is visible to a fresh full-store load + graph walk WITHOUT a
//     manual `aida cache rebuild` (part D — the cache write-through restamps
//     HEAD, and graph reads the worktree YAML via inner.load());
//   * the helper is lenient (bad target = no-op) and idempotent.
// trace:TASK-928 | ai:claude
#[cfg(test)]
#[path = "tests/task_928_parent_tag_edge_tests.rs"]
mod task_928_parent_tag_edge_tests;

// TASK-1426: fixture tests for `aida db migrate-related-edges` and for
// `rel remove` honoring `--type` against stored custom related edges.
// trace:TASK-1426 | ai:claude
#[cfg(test)]
#[path = "tests/task_1426_related_edge_migration_tests.rs"]
mod task_1426_related_edge_migration_tests;

// TASK-1488: fixture tests for the widened `aida db migrate-related-edges`
// selection predicate — any Custom edge whose spelling parses to a standard
// type via `RelationshipType::parse_relationship_type`, not just the
// `related` family. trace:TASK-1488 | ai:claude
#[cfg(test)]
#[path = "tests/task_1488_related_edge_migration_widen_tests.rs"]
mod task_1488_related_edge_migration_widen_tests;

// BUG-1602: handler-level tests of `aida rel remove` itself (typed removal,
// --bidirectional, the parent/child pair, the legacy Custom-related family)
// through the real Command::Rel dispatch path — task_1426's tests above only
// cover the pure `rel_remove_matches` helper. trace:BUG-1602 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1602_rel_remove_handler_tests.rs"]
mod bug_1602_rel_remove_handler_tests;

// BUG-1611: lifecycle authority guard at queue done, the forced reopen, and
// zen auto-approve. trace:BUG-1611 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1611_lifecycle_authority_tests.rs"]
mod bug_1611_lifecycle_authority_tests;

// BUG-1732: `load_store_for_lookup` is project-root-scoped, so no
// project-scoped read can surface another project's requirements.
// trace:BUG-1732 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1732_store_lookup_scope_tests.rs"]
mod bug_1732_store_lookup_scope_tests;

// trace:TASK-1468 | ai:claude
#[cfg(test)]
#[path = "tests/task_1468_ambiguous_write_ids_tests.rs"]
mod task_1468_ambiguous_write_ids_tests;

#[cfg(test)]
#[path = "tests/task_887_888_input_validation_tests.rs"]
mod task_887_888_input_validation_tests;

#[cfg(test)]
#[path = "tests/task_1082_did_you_mean_tests.rs"]
mod task_1082_did_you_mean_tests;

/// Modern `aida rel list` over the git-canonical backend. Supports three
/// modes (global / outgoing / incoming) plus `--type`, `--dangling`, and
/// `--all` filters. Output uses a uniform `FROM → TO   TITLE` row format
/// across all modes for grep-friendly behavior. trace:TASK-65 | ai:claude
pub(crate) fn handle_rel_list_modern(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
    source: Option<&str>,
    target: Option<&str>,
    type_filter: Option<&str>,
    dangling_only: bool,
    include_all: bool,
    limit: Option<usize>,
) -> Result<()> {
    use aida_core::DatabaseBackend;

    // Default cap for the unfiltered global firehose so `aida rel list` with
    // no args doesn't dump every edge on a large store. Any filter (source /
    // target / type / dangling) or an explicit --limit overrides it.
    // trace:TASK-778
    const GLOBAL_AUTO_CAP: usize = 50;

    if source.is_some() && target.is_some() {
        anyhow::bail!(
            "pass either a source (positional or --source) or --target, not both — \
             they pick different edge sets"
        );
    }

    // Normalize the type filter against the canonical names. We let custom
    // types through as exact string compare so user-defined relationship
    // names still filter correctly.
    let type_filter_norm: Option<RelationshipType> = type_filter.map(RelationshipType::from_str);

    // Build the universe we need to read. Outgoing/global: walk every
    // requirement. Incoming: same — there's no inverse index, so a target
    // query is "find every edge whose target_id resolves to this UUID".
    let all_reqs = backend.list_requirements(false)?;

    // BUG-573: `aida rel list` is a READ-ONLY query. When the named source /
    // target spec doesn't resolve there are simply no edges to list — that's
    // an empty result, not a failure. Emitting a non-zero exit here was the
    // single biggest distortion in `aida usage errors` (loops/agents query
    // relationships for specs that may be archived/removed) and papercut every
    // hook that called it. Surface the "not found" message to stderr (still
    // visible) and exit 0 — consistent with the existing empty-result branch
    // below, which also prints a "(no … match)" line and returns Ok. Genuine
    // bad-arguments (source AND target both passed) still bail above with a
    // non-zero exit. trace:BUG-573 | ai:claude
    //
    // For target mode we need the target's UUID up front to compare against
    // each edge's target_id field.
    let target_uuid: Option<uuid::Uuid> = if let Some(t) = target {
        match backend.get_requirement_by_spec_id(t)? {
            Some(req) => Some(req.id),
            None => {
                eprintln!("{}", not_found::requirement_not_found(t, Some(store_path)));
                return Ok(());
            }
        }
    } else {
        None
    };

    // For source mode the legacy behavior is "list relationships for one
    // requirement"; we keep that shape, just routed through the same
    // emitter. trace:TASK-65 | ai:claude
    let source_uuid: Option<uuid::Uuid> = if let Some(s) = source {
        match backend.get_requirement_by_spec_id(s)? {
            Some(req) => Some(req.id),
            None => {
                eprintln!("{}", not_found::requirement_not_found(s, Some(store_path)));
                return Ok(());
            }
        }
    } else {
        None
    };

    // Index reqs by uuid for fast target-resolution during printing.
    use std::collections::HashMap;
    let req_by_uuid: HashMap<uuid::Uuid, &aida_core::Requirement> =
        all_reqs.iter().map(|r| (r.id, r)).collect();

    // Collect matching edges as (from_req, &Relationship, target_req_or_none).
    let mut rows: Vec<RelRow> = Vec::new();
    let mut hidden_terminal = 0usize;
    for req in &all_reqs {
        if let Some(s) = source_uuid {
            if req.id != s {
                continue;
            }
        }
        for rel in &req.relationships {
            if let Some(t) = target_uuid {
                if rel.target_id != t {
                    continue;
                }
            }
            if let Some(ref tf) = type_filter_norm {
                if !rel_type_matches(&rel.rel_type, tf) {
                    continue;
                }
            }
            let target_req = req_by_uuid.get(&rel.target_id).copied();
            if dangling_only && target_req.is_some() {
                continue;
            }
            // Hide edges where both endpoints are terminal in global mode
            // (no specific source/target asked). When the user explicitly
            // names a source or target they get the full picture for that
            // node regardless of status. trace:TASK-65 | ai:claude
            let global_mode = source_uuid.is_none() && target_uuid.is_none();
            if global_mode && !include_all {
                let from_terminal = is_terminal_status(&req.status);
                let to_terminal = target_req
                    .map(|t| is_terminal_status(&t.status))
                    .unwrap_or(false);
                if from_terminal && to_terminal {
                    hidden_terminal += 1;
                    continue;
                }
            }
            rows.push(RelRow {
                from_spec: req.spec_id.clone().unwrap_or_else(|| "?".into()),
                from_status: req.status.to_string(),
                rel_type: rel.rel_type.clone(),
                target_id_uuid: rel.target_id,
                target_spec: target_req
                    .and_then(|t| t.spec_id.clone())
                    .unwrap_or_else(|| {
                        // BUG-53: tombstone form for deleted targets.
                        let s = rel.target_id.to_string();
                        format!("{}…", &s[..s.len().min(8)])
                    }),
                target_status: target_req.map(|t| t.status.to_string()),
                target_title: target_req
                    .map(|t| t.title.clone())
                    .unwrap_or_else(|| "(removed — see `aida doctor verify-relationships`)".into()),
                target_resolved: target_req.is_some(),
            });
        }
    }

    if rows.is_empty() {
        let mode = if target_uuid.is_some() {
            "incoming"
        } else if source_uuid.is_some() {
            "outgoing"
        } else {
            "global"
        };
        println!(
            "{}",
            format!("(no {mode} relationships match the filter)").yellow()
        );
        if hidden_terminal > 0 {
            println!(
                "{}",
                format!(
                    "  ({hidden_terminal} hidden between Completed/Rejected reqs — pass --all to see them)"
                )
                .dimmed()
            );
        }
        return Ok(());
    }

    // Apply the row cap. An explicit --limit (when non-zero) always wins;
    // otherwise the unfiltered global listing auto-caps to avoid a firehose.
    // A source/target/type/dangling filter lifts the auto-cap. trace:TASK-778
    let total_rows = rows.len();
    let is_filtered =
        source.is_some() || target.is_some() || type_filter.is_some() || dangling_only;
    let effective_cap: Option<usize> = match limit {
        Some(0) => None,                               // explicit "no cap"
        Some(n) => Some(n),                            // explicit cap
        None if !is_filtered => Some(GLOBAL_AUTO_CAP), // unfiltered global firehose
        None => None,                                  // filtered: show all matches
    };
    let mut truncated_by: Option<(usize, bool)> = None; // (shown, was_auto_cap)
    if let Some(cap) = effective_cap {
        if rows.len() > cap {
            rows.truncate(cap);
            truncated_by = Some((cap, limit.is_none()));
        }
    }

    // Column widths sized to data.
    let from_w = rows
        .iter()
        .map(|r| r.from_spec.len())
        .max()
        .unwrap_or(8)
        .max(4);
    let type_w = rows
        .iter()
        .map(|r| rel_type_label(&r.rel_type).len())
        .max()
        .unwrap_or(8)
        .max(4);
    let to_w = rows
        .iter()
        .map(|r| r.target_spec.len())
        .max()
        .unwrap_or(10)
        .max(2);

    println!(
        "{}",
        format!(
            "{:<from_w$}  {:<type_w$}  {:<to_w$}  TITLE",
            "FROM",
            "TYPE",
            "TO",
            from_w = from_w,
            type_w = type_w,
            to_w = to_w,
        )
        .dimmed()
    );

    for r in &rows {
        let from = format!("{:<from_w$}", r.from_spec, from_w = from_w);
        let typ = rel_type_label(&r.rel_type);
        let typ_cell = format!("{:<type_w$}", typ, type_w = type_w);
        let to = format!("{:<to_w$}", r.target_spec, to_w = to_w);
        let title = shorten_title(&r.target_title, 60);
        let marker = if !r.target_resolved {
            format!("{} ", crate::glyph(crate::glyphs::Glyph::Warning))
                .red()
                .to_string()
        } else if r
            .target_status
            .as_deref()
            .map(is_terminal_status_str)
            .unwrap_or(false)
        {
            "· ".dimmed().to_string()
        } else {
            "  ".into()
        };
        println!(
            "{marker}{from}  {typ_cell}  {to}  {title}",
            title = title.dimmed()
        );
    }

    if let Some((shown, was_auto_cap)) = truncated_by {
        let hint = if was_auto_cap {
            " — pass --limit 0 for all, or a filter (--source/--target/--type)"
        } else {
            " — raise or drop --limit (0 = all) to see more"
        };
        println!("\n{} of {} edges shown{}", shown, total_rows, hint.dimmed());
    } else {
        println!("\n{} edges", rows.len());
    }
    if hidden_terminal > 0 {
        println!(
            "{}",
            format!(
                "  ({hidden_terminal} hidden between Completed/Rejected reqs — pass --all to see them)"
            )
            .dimmed()
        );
    }
    let _ = store_path; // suppress unused — already used for error context above
    Ok(())
}

/// One row emitted by `aida rel list`. Decoupling the struct from the
/// rendering loop keeps the column widths calculable in one pass.
pub(crate) struct RelRow {
    pub(crate) from_spec: String,
    #[allow(dead_code)] // surfaced through future --show-status etc.
    pub(crate) from_status: String,
    pub(crate) rel_type: RelationshipType,
    #[allow(dead_code)] // kept for future --uuid / --json renderers
    pub(crate) target_id_uuid: uuid::Uuid,
    pub(crate) target_spec: String,
    pub(crate) target_status: Option<String>,
    pub(crate) target_title: String,
    pub(crate) target_resolved: bool,
}

/// Compare a relationship-type from storage to a user-supplied filter.
/// Canonical types match by Display name (case-insensitive); custom types
/// match exact string. trace:TASK-65 | ai:claude
pub(crate) fn rel_type_matches(actual: &RelationshipType, want: &RelationshipType) -> bool {
    match (actual, want) {
        (RelationshipType::Custom(a), RelationshipType::Custom(b)) => a.eq_ignore_ascii_case(b),
        (RelationshipType::Custom(_), _) | (_, RelationshipType::Custom(_)) => false,
        (a, b) => std::mem::discriminant(a) == std::mem::discriminant(b),
    }
}

/// Short label for a RelationshipType in tabular output.
pub(crate) fn rel_type_label(rt: &RelationshipType) -> String {
    match rt {
        RelationshipType::Parent => "parent".to_string(),
        RelationshipType::Child => "child".to_string(),
        RelationshipType::Duplicate => "duplicate".to_string(),
        RelationshipType::Verifies => "verifies".to_string(),
        RelationshipType::VerifiedBy => "verified-by".to_string(),
        RelationshipType::References => "references".to_string(),
        // trace:STORY-333 | ai:claude
        RelationshipType::BlockedBy => "blocked-by".to_string(),
        RelationshipType::Blocks => "blocks".to_string(),
        // trace:TASK-1176 | ai:claude
        RelationshipType::SupersededBy => "superseded-by".to_string(),
        RelationshipType::Supersedes => "supersedes".to_string(),
        RelationshipType::Custom(s) => s.clone(),
    }
}

pub(crate) fn shorten_title(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Result of the STORY-63 scope-fallback resolver.
/// trace:STORY-63 | ai:claude
pub(crate) struct ScopeFallback<'a> {
    /// Total number of Approved children of the scope (informational —
    /// we display this count in the rendered message).
    pub(crate) approved_count: usize,
    /// The chosen pick after priority + created_at sort.
    pub(crate) pick: &'a Requirement,
}

/// Find the highest-priority approved child of `lease.scope` that no
/// session is already mid-work on. Returns `None` when:
///   - the scope is a path-glob / free-form string we can't resolve to
///     a spec (no ancestry to walk)
///   - any child of the scope is already InProgress (don't pick a
///     parallel within the same EPIC)
///   - no child satisfies status=Approved + the role scope filter
///
/// Priority order: High > Medium > Low, then created_at ascending so
/// the oldest approved item wins ties (closest to the project's
/// implicit work order).
/// trace:STORY-63 | ai:claude
pub(crate) fn scope_fallback_pick<'a>(
    store: &'a RequirementsStore,
    lease: &SessionLease,
    role_scope: Option<&(Vec<String>, Option<String>)>,
) -> Option<ScopeFallback<'a>> {
    let scope_lc = lease.scope.to_ascii_lowercase();
    let scope_req = store.requirements.iter().find(|r| {
        let spec_match = r
            .spec_id
            .as_deref()
            .map(|s| s.to_ascii_lowercase() == scope_lc)
            .unwrap_or(false);
        let agreed_match = r
            .agreed_id
            .as_deref()
            .map(|s| s.to_ascii_lowercase() == scope_lc)
            .unwrap_or(false);
        spec_match || agreed_match
    })?;

    let child_ids: HashSet<Uuid> = scope_req
        .relationships
        .iter()
        .filter(|r| r.rel_type == RelationshipType::Parent)
        .map(|r| r.target_id)
        .collect();
    if child_ids.is_empty() {
        return None;
    }
    let children: Vec<&Requirement> = store
        .requirements
        .iter()
        .filter(|r| child_ids.contains(&r.id))
        .collect();

    // If anything under this scope is already in flight, the session's
    // attention belongs to that — don't suggest a second item.
    if children
        .iter()
        .any(|r| r.status == RequirementStatus::InProgress)
    {
        return None;
    }

    let approved_count = children
        .iter()
        .filter(|r| r.status == RequirementStatus::Approved)
        .count();

    let mut candidates: Vec<&Requirement> = children
        .iter()
        .copied()
        .filter(|r| r.status == RequirementStatus::Approved)
        .filter(|r| {
            // Apply the active role's tag/status scope filter if present
            // (mirrors the existing queue-next post-filter so the two
            // entry points behave consistently).
            let Some((scope_tags, scope_status)) = role_scope else {
                return true;
            };
            if let Some(want) = scope_status {
                if !format!("{}", r.status).eq_ignore_ascii_case(want)
                    && !format!("{:?}", r.status).eq_ignore_ascii_case(want)
                {
                    return false;
                }
            }
            for tag in scope_tags {
                if !r.tags.iter().any(|t| t == tag) {
                    return false;
                }
            }
            true
        })
        .collect();
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by(|a, b| {
        priority_rank(&a.priority)
            .cmp(&priority_rank(&b.priority))
            .then_with(|| a.created_at.cmp(&b.created_at))
    });
    Some(ScopeFallback {
        approved_count,
        pick: candidates[0],
    })
}

/// Sort key for priority — lower rank wins (High = 0, Medium = 1,
/// Low = 2) so an ascending sort puts High first.
/// trace:STORY-63 | ai:claude
pub(crate) fn priority_rank(p: &RequirementPriority) -> u8 {
    match p {
        RequirementPriority::High => 0,
        RequirementPriority::Medium => 1,
        RequirementPriority::Low => 2,
    }
}

/// Render `root` and its descendants as an indented tree, two spaces per
/// level. Each node prints as `<status-glyph> <ID>  <Status>  <title>`.
/// Children are walked via rel_type:Parent edges (AIDA's
/// parent-points-at-child storage convention). Recursion stops at
/// `max_depth` (the root is depth 0). Cycles are guarded by a visited
/// set — defensive only; the data model shouldn't allow them.
/// trace:STORY-62 | ai:claude
pub(crate) fn render_tree(
    backend: &aida_core::CachedGitBackend,
    root: &Requirement,
    max_depth: usize,
) -> Result<()> {
    let mut visited: HashSet<Uuid> = HashSet::new();
    render_tree_node(backend, root, 0, max_depth, &mut visited)
}

pub(crate) fn render_tree_node(
    backend: &aida_core::CachedGitBackend,
    req: &Requirement,
    depth: usize,
    max_depth: usize,
    visited: &mut HashSet<Uuid>,
) -> Result<()> {
    if !visited.insert(req.id) {
        return Ok(()); // already rendered — treat as cycle, skip silently
    }
    let indent = "  ".repeat(depth);
    let status = req.effective_status().to_string();
    // Glyph hint without emoji (CLAUDE.md house rule): two-state mark on
    // the most useful axis — completed vs everything else.
    let glyph = if status.eq_ignore_ascii_case("completed") {
        crate::glyph(crate::glyphs::Glyph::Check)
            .green()
            .to_string()
    } else {
        "○".dimmed().to_string()
    };
    let id_label = req.display_id();
    println!(
        "{}{} {:<14} {:<12} {}",
        indent, glyph, id_label, status, req.title,
    );
    if depth >= max_depth {
        return Ok(());
    }
    // Collect direct children (Parent edges on this req point to children).
    let mut children: Vec<Requirement> = Vec::new();
    for rel in &req.relationships {
        if rel.rel_type == RelationshipType::Parent {
            if let Some(child) = backend.get_requirement(&rel.target_id)? {
                children.push(child);
            }
        }
    }
    // Stable order: by display_id ascending so the same tree prints the
    // same way across runs.
    children.sort_by_key(|a| a.display_id());
    for child in &children {
        render_tree_node(backend, child, depth + 1, max_depth, visited)?;
    }
    Ok(())
}

pub(crate) fn print_comment(comment: &Comment, indent: usize) {
    let indent_str = "  ".repeat(indent);
    println!();
    println!("{}{}:", indent_str, comment.id.to_string().yellow());
    let edited_marker = if comment.modified_at > comment.created_at {
        format!(
            " (edited {})",
            comment
                .modified_at
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
        )
        .dimmed()
        .to_string()
    } else {
        String::new()
    };
    println!(
        "{}  {} {} at {}{}",
        indent_str,
        "By:".dimmed(),
        comment.author.cyan(),
        comment
            .created_at
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string()
            .dimmed(),
        edited_marker,
    );
    // trace:TASK-330 | ai:claude — surface the producing session so a reader
    // can correlate the comment back to a session (`aida session list`).
    if let Some(short) = comment.short_session_id() {
        println!("{}  {} {}", indent_str, "Session:".dimmed(), short.dimmed());
    }
    // trace:BUG-1534 | ai:claude — a relayed claim names its original seat.
    if let Some(orig) = comment.relayed_from.as_deref() {
        println!(
            "{}  {} {}",
            indent_str,
            "Relayed:".dimmed(),
            aida_core::mailbox::provenance_label(&comment.author, Some(orig)).magenta()
        );
    }
    println!("{}  {}", indent_str, comment.content);

    if !comment.replies.is_empty() {
        for reply in &comment.replies {
            print_comment(reply, indent + 1);
        }
    }
}

/// Resolve a user-supplied comment identifier into a concrete Uuid by
/// matching against the requirement's comment tree. Accepts:
/// - a full UUID string (e.g., `019df478-7a34-7f92-8d46-b00e0d1eeda7`)
/// - a UUID prefix (e.g., `019df478`) — must uniquely match one comment
///   Returns an error on no-match or ambiguous-prefix.
///   trace:SPIKE-2 | ai:claude
pub(crate) fn resolve_comment_uuid(req: &aida_core::Requirement, query: &str) -> Result<Uuid> {
    if let Ok(parsed) = Uuid::parse_str(query) {
        // Verify the exact UUID exists in the tree, even if parse succeeded.
        if collect_comment_ids(&req.comments)
            .into_iter()
            .any(|id| id == parsed)
        {
            return Ok(parsed);
        }
        anyhow::bail!("No comment with id {} on this requirement", parsed);
    }

    let q = query.to_lowercase();
    let matches: Vec<Uuid> = collect_comment_ids(&req.comments)
        .into_iter()
        .filter(|id| id.to_string().to_lowercase().starts_with(&q))
        .collect();
    match matches.len() {
        0 => anyhow::bail!(
            "No comment matches '{}' on this requirement (use `aida comment list <REQ>` to see ids)",
            query
        ),
        1 => Ok(matches[0]),
        n => anyhow::bail!(
            "Ambiguous comment prefix '{}' — matches {} comments. Use a longer prefix.",
            query,
            n
        ),
    }
}

/// Walk the (potentially nested) comment tree and collect every comment id.
pub(crate) fn collect_comment_ids(comments: &[Comment]) -> Vec<Uuid> {
    let mut out = Vec::new();
    for c in comments {
        out.push(c.id);
        out.extend(collect_comment_ids(&c.replies));
    }
    out
}

pub(crate) fn open_user_guide(dark_mode: bool) -> Result<()> {
    // Get the path to the docs directory relative to the executable
    let exe_path = aida_exe_path();

    // Try multiple possible locations for the docs
    let possible_paths = [
        // Relative to executable (for installed binaries)
        exe_path.parent().unwrap().join("../docs"),
        exe_path.parent().unwrap().join("../../docs"),
        // Development paths
        exe_path.parent().unwrap().join("../../../docs"),
        exe_path.parent().unwrap().join("../../../../docs"),
        // Current directory
        std::env::current_dir().unwrap_or_default().join("docs"),
        // Project root (when running from project directory)
        std::path::PathBuf::from("docs"),
    ];

    let filename = if dark_mode {
        "user-guide-dark.html"
    } else {
        "user-guide.html"
    };

    // Find the first path that exists
    let doc_path = possible_paths
        .iter()
        .map(|p| p.join(filename))
        .find(|p| p.exists());

    match doc_path {
        Some(path) => {
            let path_str = path
                .canonicalize()
                .unwrap_or(path.clone())
                .to_string_lossy()
                .to_string();

            // Convert to file:// URL
            let url = format!("file://{}", path_str);

            println!(
                "Opening user guide{}...",
                if dark_mode { " (dark mode)" } else { "" }
            );

            // Try to open in browser using platform-specific commands
            #[cfg(target_os = "linux")]
            {
                std::process::Command::new("xdg-open")
                    .arg(&url)
                    .spawn()
                    .context("Failed to open browser. Try opening manually: {}")?;
            }

            #[cfg(target_os = "macos")]
            {
                std::process::Command::new("open")
                    .arg(&url)
                    .spawn()
                    .context("Failed to open browser")?;
            }

            #[cfg(target_os = "windows")]
            {
                std::process::Command::new("cmd")
                    .args(["/C", "start", &url])
                    .spawn()
                    .context("Failed to open browser")?;
            }

            println!("{}", "User guide opened in browser".green());
            Ok(())
        }
        None => {
            println!("{}", "User guide not found.".yellow());
            println!("Expected location: docs/{}", filename);
            println!("\nTo generate the documentation, run:");
            println!("  ./helper/generate-docs.sh");
            anyhow::bail!("User guide not found")
        }
    }
}

// ============================================================================
// Trace Command Handlers
// ============================================================================

// ---------------------------------------------------------------------------
// `aida review` — review-workflow helpers (STORY-67)
// ---------------------------------------------------------------------------

/// Section headings the prompt-generator looks for in a requirement's
/// description. The first match (case-insensitive) wins; everything from
/// that heading until the next `## ` heading or end-of-string is the
/// extracted body.
/// trace:STORY-67 | ai:claude
pub(crate) const ACCEPTANCE_SECTION_HEADINGS: &[&str] = &[
    "Acceptance",
    "Verify",
    "Test cases",
    "Tests",
    "Verification",
];

/// Density levels for the `aida show --card` spec card. trace:TASK-265
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CardDensity {
    /// One-line id/type/priority/status/title summary, no box. For
    /// autonomous / scripted flows that just need the spec named.
    Brief,
    /// Boxed card: a truncated lead-prose description + the acceptance
    /// criteria. The default.
    Balanced,
    /// Boxed card with the full description, no truncation. For deep
    /// dives.
    Full,
}

/// The headings the spec card renders relationships under, in render order.
///
/// This list is the card's completeness contract: every label
/// [`card_rel_label`] can return MUST appear here. A label with no matching
/// heading does not render under a rough heading — it does not render at all.
/// That is how relabelling `Custom` edges without adding this third bucket
/// made every stored `Custom` edge (≈ 1480 of them store-wide, including this
/// spec's own `implemented-by`) vanish from the card: a wrong label traded for
/// no label, which is BUG-1471's own defect class.
// trace:BUG-1471 | ai:claude
pub(crate) const CARD_REL_BUCKETS: &[&str] = &["Parent", "Related", "Custom"];

/// Map a relationship type to the field label the spec card buckets it
/// under. AIDA's `RelationshipType` reads as "I am X to the target", so a
/// `Child` edge means the target is this spec's parent.
///
/// Every value returned here must be listed in [`CARD_REL_BUCKETS`].
// trace:TASK-265 | ai:claude
pub(crate) fn card_rel_label(rt: &RelationshipType) -> &'static str {
    match rt {
        // This spec is a child of the target → target is the parent.
        RelationshipType::Child => "Parent",
        // Custom edges must not be presented with a label that looks like a
        // standard type: that taught users to type `--type related`, creating
        // graph-inert edges. They get their own bucket instead, with the
        // edge's real name shown inline by `CardRel::render`.
        // trace:BUG-1471 | ai:codex
        RelationshipType::Custom(_) => "Custom",
        // Standard non-parent edges share the general Related bucket.
        _ => "Related",
    }
}

/// The edge's own name, when the bucket heading does not already carry it.
///
/// `Child` is the sole exception because its `Parent` heading already names
/// the target's role. Custom and standard edges under neutral headings print
/// their canonical names, so blocking, verification, reference, duplicate,
/// and supersession edges cannot collapse into indistinguishable target ids.
// trace:BUG-1471 trace:BUG-1584 | ai:codex
pub(crate) fn card_rel_edge_name(rt: &RelationshipType) -> Option<String> {
    match rt {
        RelationshipType::Child => None,
        _ => Some(rt.to_string()),
    }
}

/// One relationship as the spec card renders it: the heading it falls under,
/// the edge's own name when that heading does not carry it, and the resolved
/// target.
// trace:BUG-1471 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CardRel {
    pub(crate) bucket: &'static str,
    pub(crate) edge: Option<String>,
    pub(crate) id: String,
    pub(crate) title: String,
}

impl CardRel {
    /// Build a card row from a relationship type and its already-resolved
    /// target id/title.
    // trace:BUG-1471 | ai:claude
    pub(crate) fn new(rt: &RelationshipType, id: String, title: String) -> Self {
        CardRel {
            bucket: card_rel_label(rt),
            edge: card_rel_edge_name(rt),
            id,
            title,
        }
    }

    /// One rendered entry: `[edge-name] ID — title`, with the bracketed name
    /// present only for an edge whose heading does not name it.
    // trace:BUG-1471 | ai:claude
    pub(crate) fn render(&self) -> String {
        let target = if self.title.is_empty() {
            self.id.clone()
        } else {
            format!("{} — {}", self.id, self.title)
        };
        match &self.edge {
            Some(name) => format!("[{name}] {target}"),
            None => target,
        }
    }
}

/// Group a card's relationships into the headed sections it prints, in
/// [`CARD_REL_BUCKETS`] order, skipping empty ones.
///
/// Rendering through this one function is what keeps the card total. The
/// previous shape hand-rolled one filter per heading at the call site, so
/// adding a label without adding its heading dropped those edges on the floor
/// with nothing to notice it.
// trace:BUG-1471 | ai:claude
pub(crate) fn card_rel_sections(rels: &[CardRel]) -> Vec<(&'static str, String)> {
    let mut sections = Vec::new();
    for bucket in CARD_REL_BUCKETS {
        let joined = rels
            .iter()
            .filter(|r| r.bucket == *bucket)
            .map(CardRel::render)
            .collect::<Vec<_>>()
            .join(", ");
        if !joined.is_empty() {
            sections.push((*bucket, joined));
        }
    }
    sections
}

/// The lead prose of a requirement description — everything before the
/// first `## ` section heading. AIDA descriptions front-load a plain
/// summary, then break into `## Acceptance`, `## Origin`, etc.; the
/// balanced card shows just that summary. trace:TASK-265 | ai:claude
pub(crate) fn card_lead_prose(description: &str) -> &str {
    let mut from = 0usize;
    while let Some(rel) = description[from..].find("## ") {
        let abs = from + rel;
        // Only treat it as a heading when it starts a line.
        if abs == 0 || description.as_bytes()[abs - 1] == b'\n' {
            return description[..abs].trim_end();
        }
        from = abs + 3;
    }
    description.trim_end()
}

/// Truncate text to at most `max_paragraphs` blank-line-separated
/// paragraphs and roughly `max_chars` characters, never cutting inside a
/// paragraph (so the result never ends mid-sentence). The first paragraph
/// is always kept even if it alone exceeds the char budget. Returns the
/// kept text and whether anything was dropped. trace:TASK-265 | ai:claude
pub(crate) fn card_truncate_paragraphs(
    text: &str,
    max_paragraphs: usize,
    max_chars: usize,
) -> (String, bool) {
    let paragraphs: Vec<&str> = text
        .split("\n\n")
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut total = 0usize;
    for (i, para) in paragraphs.iter().enumerate() {
        if i >= max_paragraphs {
            break;
        }
        if i > 0 && total + para.chars().count() > max_chars {
            break;
        }
        kept.push(para);
        total += para.chars().count();
    }
    let truncated = kept.len() < paragraphs.len();
    (kept.join("\n\n"), truncated)
}

/// Count `- [ ]` / `* [ ]` checklist items in an acceptance-section body.
/// trace:TASK-265 | ai:claude
pub(crate) fn card_count_acceptance(body: &str) -> usize {
    body.lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with("- [") || t.starts_with("* [")
        })
        .count()
}

/// Lead the spec card's brief with the spec's cached `aida intent`
/// comprehension (TASK-838). When a FRESH comprehension exists, render its
/// `llm` register at the top of the brief, clearly labeled AI-generated, ahead
/// of the description and plan brief. When absent or stale, emit a one-line
/// nudge and continue — NEVER block pickup on an AI pass, never generate inline.
///
/// Computes the fresh neighborhood hash exactly as `aida intent` does (reusing
/// [`build_intent_neighborhood`] + [`intent::is_stale`] via
/// [`intent::decide_pickup_intent`]), so the brief and `aida intent` agree on
/// drift. Feature-detects the `Option<SpecIntent>`: a store predating STORY-631
/// (or a spec never run through `aida intent`) takes the absent-note path with
/// no other behavior change. Store-load failure is non-fatal — the card is a
/// convenience surface, so we silently skip the intent block rather than abort
/// the whole card. trace:TASK-838 | ai:claude
pub(crate) fn render_card_intent(req: &aida_core::Requirement, store_path: &std::path::Path) {
    let project_root = store_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    // The full requirement set lets build_intent_neighborhood resolve the
    // spec's immediate neighbors for the same drift hash `aida intent` computes.
    let Some(store) = load_store_for_lookup(&project_root) else {
        return;
    };
    let inputs = build_intent_neighborhood(req, &store.requirements);
    let fresh_hash = inputs.source_hash();
    let disp = req.display_id();

    match intent::decide_pickup_intent(req.intent.as_ref(), &fresh_hash) {
        intent::PickupIntent::Render {
            llm,
            model,
            generated_at,
        } => {
            // Lead the brief. Label it AI-generated so no reader mistakes the
            // prose for hand-authored ground truth.
            println!(
                "  {}",
                format!(
                    "{} Intent (AI-generated):",
                    crate::glyph(crate::glyphs::Glyph::Arrow)
                )
                .bold()
            );
            println!(
                "    {}",
                format!("model={model} · generated {generated_at}").dimmed()
            );
            for line in llm.trim().lines() {
                println!("    {line}");
            }
            println!();
        }
        intent::PickupIntent::Note { stale } => {
            println!(
                "  {} {}",
                format!("{} Intent:", crate::glyph(crate::glyphs::Glyph::Arrow)).bold(),
                intent::pickup_intent_note(&disp, stale).dimmed()
            );
            println!();
        }
    }
}

/// Render a requirement as a compact, boxed "spec card" — the rendering
/// behind `aida show --card`. The /aida-pickup skill calls this at session
/// start so the spec's contract stays in terminal scrollback for the whole
/// working session, with no separate `aida show`. The plain `aida show`
/// view remains the canonical detail surface.
///
/// `rels` is the requirement's relationships already resolved by the
/// caller to [`CardRel`] rows.
// trace:TASK-265 | ai:claude
pub(crate) fn render_spec_card(
    req: &aida_core::Requirement,
    rels: &[CardRel],
    store_path: &std::path::Path,
    density: CardDensity,
    no_git: bool,
    verbose: bool,
) {
    let id = req.display_id();
    let req_type = req.req_type.to_string();
    let priority = req.effective_priority();

    // Brief: a single line, no box — for autonomous / scripted flows.
    // TASK-269: badge the status (glyph + colour) here too. trace:TASK-269
    if density == CardDensity::Brief {
        println!(
            "{} · {} · {} · {} · {}",
            id,
            req_type,
            priority,
            status_display::parked_status_badge(req),
            req.title
        );
        return;
    }

    const WIDTH: usize = 72;
    let rule = "═".repeat(WIDTH);

    // Header: a flow-active marker then `<ID> — <title>`, title trimmed to keep the banner tidy.
    let prefix = format!(
        "{} {} — ",
        crate::glyph(crate::glyphs::Glyph::FlowActive),
        id
    );
    let budget = WIDTH.saturating_sub(prefix.chars().count());
    let title = if req.title.chars().count() > budget {
        let t: String = req.title.chars().take(budget.saturating_sub(1)).collect();
        format!("{}…", t)
    } else {
        req.title.clone()
    };
    println!("{}", rule.dimmed());
    println!("{}{}", prefix.cyan().bold(), title.cyan().bold());
    println!("{}", rule.dimmed());
    println!();

    // One-liner: ID · type · priority · status (badged — TASK-269).
    // STORY-333: surface `[human-only]` chip when the marker is set, so a
    // reader scanning the spec card can see at a glance why the orchestrator
    // skips it. trace:STORY-333 | ai:claude
    let human_only_chip = if req.human_only {
        format!(" · {}", "[human-only]".magenta().bold())
    } else {
        String::new()
    };
    println!(
        "  {} · {} · {} · {}{}",
        id.bold(),
        req_type,
        priority,
        status_display::parked_status_badge(req),
        human_only_chip,
    );
    println!();

    // Key fields. "Uncategorized" is the default feature — suppress it
    // so the card only shows a feature the user actually set.
    let mut printed_field = false;
    if !req.feature.is_empty() && req.feature != "Uncategorized" {
        println!(
            "  {} {}",
            format!("{} Feature:", crate::glyph(crate::glyphs::Glyph::Arrow)).bold(),
            req.feature
        );
        printed_field = true;
    }
    if !req.tags.is_empty() {
        let tags: Vec<String> = req.tags.iter().cloned().collect();
        println!(
            "  {} {}",
            format!("{} Tags:", crate::glyph(crate::glyphs::Glyph::Arrow)).bold(),
            tags.join(", ")
        );
        printed_field = true;
    }
    // BUG-1471: render EVERY heading the card defines, from one place. A
    // reader treats this block as the spec's relationship list, so an edge
    // that lands in no bucket is worse than one under a rough heading — it
    // is silently absent. trace:BUG-1471 | ai:claude
    for (heading, joined) in card_rel_sections(rels) {
        println!(
            "  {} {}",
            format!("{} {}:", crate::glyph(crate::glyphs::Glyph::Arrow), heading).bold(),
            joined
        );
        printed_field = true;
    }
    if printed_field {
        println!();
    }

    // TASK-838: lead the implementer brief with the spec's cached `aida intent`
    // comprehension — the distilled plain-terms WHY-this-exists, ahead of the
    // raw spec body and the plan brief (Critical Files / Followups). This is
    // THIN linking work: surface the already-cached `llm`-register prose, NEVER
    // generate inline (pickup must stay fast and non-AI-blocking). A spec with
    // no cached comprehension, or a STALE one (neighborhood hash drifted),
    // gets a single-line nudge toward `aida intent <spec>` and we continue.
    // Feature-detects the `Option<SpecIntent>`, so a store without STORY-631
    // intent sees only the absent note. trace:TASK-838 | ai:claude
    render_card_intent(req, store_path);

    // STORY-332: a punted spec carries its fork reason — surface it on the
    // card so a triager sees the contract and the obstacle together.
    if let Some(reason) = req.attention_reason.as_ref() {
        println!(
            "  {} {}",
            format!("{} Punted:", crate::glyph(crate::glyphs::Glyph::Warning))
                .magenta()
                .bold(),
            reason.category
        );
        println!("    {}", reason.detail);
        if let Some(lean) = &reason.lean {
            println!(
                "    {}",
                format!(
                    "{} lean: {lean}",
                    crate::glyph(crate::glyphs::Glyph::SubArrow)
                )
                .dimmed()
            );
        }
        println!();
    }

    // Description: full text for --full, otherwise the lead prose
    // truncated to ~3 paragraphs / ~500 chars on a paragraph boundary.
    // Brief already returned above; only Balanced and Full reach here.
    let (desc_body, desc_label) = if density == CardDensity::Full {
        (
            req.description.trim().to_string(),
            format!("{} Description:", crate::glyph(crate::glyphs::Glyph::Arrow)),
        )
    } else {
        let (body, truncated) = card_truncate_paragraphs(card_lead_prose(&req.description), 3, 500);
        let body = if truncated && !body.is_empty() {
            format!("{}\n\n…", body)
        } else {
            body
        };
        (
            body,
            format!(
                "{} Description (summary):",
                crate::glyph(crate::glyphs::Glyph::Arrow)
            ),
        )
    };
    if !desc_body.is_empty() {
        println!("  {}", desc_label.bold());
        for line in desc_body.lines() {
            println!("    {}", line);
        }
        println!();
    }

    // TASK-1148: the three optional narrative fields — the genuinely-new
    // metadata not derivable from git/status/trace. Each renders its own
    // labelled block only when set; absent fields print nothing.
    // trace:TASK-1148 | ai:claude
    let narrative_blocks: [(&str, &Option<String>); 3] = [
        ("Implementation summary", &req.implementation_summary),
        ("Risk notes", &req.risk_notes),
        ("Test coverage notes", &req.test_coverage_notes),
    ];
    for (label, value) in narrative_blocks {
        if let Some(text) = value {
            let text = text.trim();
            if !text.is_empty() {
                println!(
                    "  {}",
                    format!("{} {label}:", crate::glyph(crate::glyphs::Glyph::Arrow)).bold()
                );
                for line in text.lines() {
                    println!("    {}", line);
                }
                println!();
            }
        }
    }

    // Acceptance criteria as their own block (the --full description
    // already carries the section verbatim, so skip it there).
    if density == CardDensity::Balanced {
        if let Some(acc) = extract_acceptance_section(&req.description) {
            let n = card_count_acceptance(&acc);
            let label = if n == 1 {
                "1 item".to_string()
            } else {
                format!("{} items", n)
            };
            println!(
                "  {}",
                format!(
                    "{} Acceptance ({}):",
                    crate::glyph(crate::glyphs::Glyph::Arrow),
                    label
                )
                .bold()
            );
            for line in acc.lines() {
                let t = line.trim_end();
                if !t.is_empty() {
                    println!("    {}", t);
                }
            }
            println!();
        }
    }

    // TASK-313: the owning plan's brief (Critical Files / Followups /
    // Verification) inside the card box — the same brief /aida-pickup surfaces,
    // reusing `aida queue work`'s plan discovery (discover_plan_context) rather
    // than duplicating it. Renders only when a docs/plans/ file owns the spec.
    // trace:TASK-313 | ai:claude
    {
        let project_root = store_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let plan = discover_plan_context(&project_root, &req.display_id()).or_else(|| {
            req.spec_id
                .as_deref()
                .and_then(|s| discover_plan_context(&project_root, s))
        });
        if let Some(ctx) = plan {
            println!(
                "  {} {}",
                format!("{} Plan brief:", crate::glyph(crate::glyphs::Glyph::Arrow)).bold(),
                ctx.plan_file.cyan()
            );
            if !ctx.critical_files.is_empty() {
                println!(
                    "    {} ({})",
                    "Critical files".dimmed(),
                    ctx.critical_files.len()
                );
                for f in &ctx.critical_files {
                    println!("      {}", f);
                }
            }
            if !ctx.followups.is_empty() {
                println!("    {} ({})", "Followups".dimmed(), ctx.followups.len());
                for f in &ctx.followups {
                    println!("      - {}", f);
                }
            }
            if let Some(v) = &ctx.verification {
                println!("    {}", "Definition of done".dimmed());
                for l in v.lines() {
                    println!("      {}", l.dimmed());
                }
            }
            println!();
        }
    }

    // Git linkage — reuse the same section `aida show` appends, grepped
    // against every id form the spec has worn. trace:TASK-241
    if !no_git {
        let mut ids: Vec<String> = vec![req.display_id()];
        if let Some(ref a) = req.agreed_id {
            if !ids.contains(&a.to_string()) {
                ids.push(a.to_string());
            }
        }
        if let Some(ref o) = req.spec_id {
            if !ids.contains(o) {
                ids.push(o.clone());
            }
        }
        let project_root = store_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        print_git_linkage(&project_root, &ids, verbose, req.filed_at.as_ref());
    }
    println!("{}", rule.dimmed());
}

/// Extract the acceptance-criteria body from a requirement description.
/// Returns the body text (without the heading line) when one of the
/// recognized headings is found; None otherwise so the caller can render
/// a "no acceptance criteria documented" placeholder instead of silently
/// emitting an empty section. trace:STORY-67 | ai:claude
pub(crate) fn extract_acceptance_section(description: &str) -> Option<String> {
    let lines: Vec<&str> = description.lines().collect();
    let mut start: Option<usize> = None;
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i].trim();
        if let Some(rest) = line.strip_prefix("## ") {
            let lower = rest.trim().to_ascii_lowercase();
            if ACCEPTANCE_SECTION_HEADINGS
                .iter()
                .any(|h| lower.starts_with(&h.to_ascii_lowercase()))
            {
                start = Some(i + 1);
                break;
            }
        }
        i += 1;
    }
    let start = start?;
    let mut end = lines.len();
    for (j, line) in lines.iter().enumerate().skip(start) {
        if line.trim_start().starts_with("## ") {
            end = j;
            break;
        }
    }
    let body = lines[start..end].join("\n").trim().to_string();
    if body.is_empty() {
        None
    } else {
        Some(body)
    }
}

/// The description prose that precedes the `## Acceptance` section — the lead
/// paragraphs to show under the spec's title. For a fallback (no-AI) draft there
/// is no acceptance heading, so the whole body is the lead. Sibling of
/// [`extract_acceptance_section`].
// trace:STORY-736 | ai:claude
pub(crate) fn drafted_lead_prose(description: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for line in description.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("## ") {
            let lower = rest.trim().to_ascii_lowercase();
            if ACCEPTANCE_SECTION_HEADINGS
                .iter()
                .any(|h| lower.starts_with(&h.to_ascii_lowercase()))
            {
                break;
            }
        }
        out.push(line);
    }
    out.join("\n").trim().to_string()
}

/// The acceptance bullets of a drafted body, as clean strings (leading `- `/`* `
/// markers stripped). Reuses [`extract_acceptance_section`], then keeps only the
/// genuine bullet lines — so the provenance footer line (`_Drafted by …_`) that
/// follows the bullets in a composed body is filtered out.
// trace:STORY-736 | ai:claude
pub(crate) fn drafted_acceptance_bullets(description: &str) -> Vec<String> {
    let Some(section) = extract_acceptance_section(description) else {
        return Vec::new();
    };
    section
        .lines()
        .map(str::trim)
        .filter_map(|l| l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Render the SAFE `aida zen "<thought>" --dry-run` preview: compose the SAME
/// draft the real run would file (genuine AI draft, with the offline fallback)
/// and pretty-print it in the `aida show` idiom — title + description +
/// acceptance — so the operator sees the structured spec a sentence becomes,
/// not a flat echo. Composes + renders only; nothing is persisted.
// trace:STORY-736 | ai:claude
pub(crate) fn render_zen_dry_run_draft(thought: &str) -> String {
    let ai = zen_try_ai_draft(thought);
    let drafted = zen_drive::compose_draft_from_thought(thought, ai);
    render_drafted_thought_preview(&drafted)
}

/// Pure renderer for a composed [`zen_drive::DraftedThought`] — split from the AI
/// wiring above so the layout is unit-testable without the transport. Mirrors the
/// `aida show` card: a clean rule, the title under a status glyph, the lead
/// description prose, an indented acceptance block, a source note, and the
/// lifecycle line. All glyphs route through the registry (ascii-safe).
// trace:STORY-736 | ai:claude
pub(crate) fn render_drafted_thought_preview(drafted: &zen_drive::DraftedThought) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let rule = "─".repeat(60);
    let arrow = crate::glyph(crate::glyphs::Glyph::Arrow);
    let pending = crate::glyph(crate::glyphs::Glyph::Pending);
    let bullet = crate::glyph(crate::glyphs::Glyph::Bullet);
    let check = crate::glyph(crate::glyphs::Glyph::Check);

    let _ = writeln!(out, "{}", rule.dimmed());
    let _ = writeln!(
        out,
        "{} would draft + file + drive a new {} from your thought:",
        arrow.cyan(),
        "Draft".yellow()
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "  {} {}", pending.yellow(), drafted.title.bold());
    let _ = writeln!(out);

    let lead = drafted_lead_prose(&drafted.description);
    if !lead.is_empty() {
        let _ = writeln!(out, "  {}", format!("{arrow} Description:").bold());
        for line in lead.lines() {
            let _ = writeln!(out, "    {line}");
        }
        let _ = writeln!(out);
    }

    let criteria = drafted_acceptance_bullets(&drafted.description);
    if !criteria.is_empty() {
        let label = if criteria.len() == 1 {
            "1 item".to_string()
        } else {
            format!("{} items", criteria.len())
        };
        let _ = writeln!(out, "  {}", format!("{arrow} Acceptance ({label}):").bold());
        for c in &criteria {
            let _ = writeln!(out, "    {bullet} {c}");
        }
        let _ = writeln!(out);
    }

    let note = match drafted.source {
        zen_drive::DraftSource::Ai => "AI-drafted from your thought.".to_string(),
        zen_drive::DraftSource::Fallback => {
            "no AI was reachable — showing the offline-fallback draft (refine the \
             acceptance criteria before it ships)."
                .to_string()
        }
    };
    let _ = writeln!(out, "  {} {}", check.green(), note.dimmed());
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  …then: approve {arrow} implement {arrow} CI {arrow} review {arrow} merge, \
         fully headless. Run without --dry-run to drive it."
    );
    let _ = writeln!(out, "{}", rule.dimmed());
    out
}

/// Pull `(REQ-ID)` trailers from a commit message **subject** only. AIDA's
/// commit format wraps the requirement id in parens at end-of-subject:
/// `[AI:tool] feat(scope): description (REQ-ID)`. Also tolerates the
/// shorter form without `[AI:tool]` (chores/docs).
///
/// Body content is ignored — those IDs are "referenced" (trace comments,
/// prose, sub-commit lines in squash bodies) and are returned by
/// [`extract_referenced_spec_ids_from_commit`] instead. trace:BUG-85 | ai:claude
/// trace:STORY-67 | ai:claude
/// True when a commit subject is an AIDA *plan* commit — `docs(plans): …`
/// or `docs(plan): …`, optionally behind an `[AI:tool]` authorship prefix
/// (`[AI:claude] docs(plans): …`). Plan commits are pre-implementation
/// artifacts (the plan template leaves the planned specs at Approved), so
/// their trailing `(SPEC-ID …)` group names what the plan is FOR, not what
/// shipped. The auto-bump completion-candidate scan skips them so a plan-only
/// commit never false-completes the specs it plans. trace:BUG-426 | ai:claude
pub(crate) fn is_plan_commit_subject(subject: &str) -> bool {
    let s = subject.trim();
    // Strip an optional leading `[AI:tool]` authorship tag.
    let s = s
        .strip_prefix('[')
        .and_then(|rest| rest.split_once(']'))
        .map(|(_, after)| after.trim_start())
        .unwrap_or(s);
    let lower = s.to_ascii_lowercase();
    lower.starts_with("docs(plans)") || lower.starts_with("docs(plan)")
}

pub(crate) fn extract_spec_ids_from_commit(message: &str) -> Vec<String> {
    let subject = message.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let mut out = Vec::new();
    push_paren_spec_ids_from_line(subject, &mut out);
    // BUG-270: also recognize a leading `SPEC-ID:` prefix, e.g.
    // "STORY-439: three-way complexity calibration substrate (#270)". Some
    // merge commits (external planner output, hand-authored squashes) put the
    // spec id at the FRONT with a colon instead of the AIDA-convention
    // trailing `(REQ-ID)`. Without this, such a spec is never auto-bumped /
    // reconciled and strands at its pre-merge status. The `looks_like_spec_id`
    // guard keeps conventional-commit heads (`fix:`, `feat(scope):`,
    // `[AI:claude] ...`) from false-matching — none of them parse as a bare
    // `<ALPHA>-<DIGITS...>` token. trace:BUG-270 | ai:claude
    if let Some((head, _)) = subject.split_once(':') {
        let head = head.trim();
        if looks_like_spec_id(head) && !out.iter().any(|x| x.eq_ignore_ascii_case(head)) {
            out.push(head.to_string());
        }
    }
    out
}

// ============================================================================
// STORY-498: CI spec-id validity gate (MVP slice of EPIC-34)
//
// Walk a commit range, resolve every `(SPEC-ID)` subject trailer against the
// live requirement graph, and FAIL when a trailer references a SPEC-ID that
// does not exist or is rejected — a dead/dangling provenance link. Promotes
// AIDA's headline guarantee (code traces to a *live* requirement) from a
// client-side message-grammar check into a server-side validity gate that is
// non-bypassable from a dev machine (`git commit --no-verify` cannot dodge a
// CI step). Reuses the existing `(SPEC-ID)` trailer parser and honours the
// TASK-488 mechanical-commit exemption (a commit with no trailer carries
// nothing to validate, matching the "no REQ-ID needed" rule for mechanical /
// release / docs commits).
// ============================================================================

/// Why a trailer reference fails the validity gate. trace:STORY-498 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TrailerVerdict {
    /// The id does not resolve to any spec in the requirement graph (a
    /// hallucinated or typo'd id, or a since-deleted spec).
    Nonexistent,
    /// The id resolves but the spec is rejected — a dead provenance link.
    Rejected,
}

impl TrailerVerdict {
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            TrailerVerdict::Nonexistent => "does not exist in the requirement graph",
            TrailerVerdict::Rejected => "resolves to a rejected spec",
        }
    }
}

/// One offending `(SPEC-ID)` reference found by the gate. trace:STORY-498 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct TrailerViolation {
    /// Short SHA of the offending commit.
    pub(crate) sha: String,
    /// The commit subject (for the human-readable failure message).
    pub(crate) subject: String,
    /// The id that failed to resolve / resolved to a dead spec.
    pub(crate) spec_id: String,
    pub(crate) verdict: TrailerVerdict,
}

/// How a SPEC-ID resolves against the live requirement graph — the result the
/// pure validation core consumes so it stays independent of the store/git.
/// trace:STORY-498 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpecResolution {
    /// Resolves to a live (non-rejected) spec — passes.
    Live,
    /// Resolves to a rejected spec — a dead reference.
    Rejected,
    /// Resolves to nothing.
    Missing,
}

/// Pure, testable core of the validity gate: given commits as
/// `(short_sha, subject)` pairs and a resolver that classifies each SPEC-ID
/// against the graph, return every offending reference.
///
/// Exemptions baked in here (matching the auto-bump scan + TASK-488):
/// - Plan commits (`docs(plans): …`) are skipped — their trailer names what is
///   planned, not what shipped, so a planned-but-not-yet-real id is legitimate.
/// - A commit with no `(SPEC-ID)` trailer is skipped — there is nothing to
///   validate (mechanical / release / docs commits with "no REQ-ID needed").
///
/// trace:STORY-498 | ai:claude
pub(crate) fn validate_trailer_references<F>(
    commits: &[(String, String)],
    mut resolve: F,
) -> Vec<TrailerViolation>
where
    F: FnMut(&str) -> SpecResolution,
{
    let mut violations = Vec::new();
    for (sha, subject) in commits {
        if is_plan_commit_subject(subject) {
            continue;
        }
        for id in extract_spec_ids_from_commit(subject) {
            let verdict = match resolve(&id) {
                SpecResolution::Live => continue,
                SpecResolution::Rejected => TrailerVerdict::Rejected,
                SpecResolution::Missing => TrailerVerdict::Nonexistent,
            };
            violations.push(TrailerViolation {
                sha: sha.clone(),
                subject: subject.clone(),
                spec_id: id,
                verdict,
            });
        }
    }
    violations
}

/// Resolve the commit range to scan. Explicit `--range` wins; otherwise scan
/// the commits this branch adds over the default branch
/// (`<default-branch>..HEAD`), falling back to `HEAD~20..HEAD` when no default
/// branch resolves (e.g. a shallow CI checkout or a repo with no main).
/// trace:STORY-498 | ai:claude
pub(crate) fn resolve_gate_range(project_root: &std::path::Path, range: Option<&str>) -> String {
    if let Some(r) = range {
        return r.to_string();
    }
    if let Some(default_ref) = resolve_default_branch_ref(project_root) {
        // `<base>..HEAD`: the commits HEAD has that the default branch doesn't.
        return format!("{default_ref}..HEAD");
    }
    "HEAD~20..HEAD".to_string()
}

/// TASK-1444: resolve the commit range for reviewer-verdict shelve
/// attribution against the **PR's own branch**, not the drain's main
/// checkout `HEAD`. `resolve_gate_range(.., None)` scans
/// `<default>..HEAD`, which is the drain's own worktree/checkout — for the
/// orchestrator's phase driver that is NOT necessarily the branch the PR
/// under review is on. Prefers `origin/<branch>` (what CI and the reviewer
/// actually saw) and falls back to the local `<branch>` ref when the origin
/// ref hasn't been fetched; returns `None` when neither resolves (or the
/// default branch itself can't be resolved) so the caller can treat
/// attribution as `Uncertain` instead of silently reading the wrong range.
// trace:TASK-1444 | ai:claude
pub(crate) fn resolve_shelve_gate_range(
    project_root: &std::path::Path,
    branch: Option<&str>,
) -> Option<String> {
    let branch = branch?;
    let default_ref = resolve_default_branch_ref(project_root)?;
    use std::process::Command as PCmd;
    let ref_exists = |r: &str| -> bool {
        PCmd::new("git")
            .arg("-C")
            .arg(project_root)
            .args(["rev-parse", "--verify", "--quiet", r])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    let origin_branch = format!("origin/{branch}");
    let branch_ref = if ref_exists(&origin_branch) {
        origin_branch
    } else if ref_exists(branch) {
        branch.to_string()
    } else {
        return None;
    };
    Some(format!("{default_ref}..{branch_ref}"))
}

/// Resolve a single SPEC-ID against a loaded store, mirroring the trace-gate
/// resolver. When `store` is `None` (no requirement store reachable) every id
/// resolves `Live` — failing every id would block legitimate ships on a
/// checkout without store access, so the safe default is to surface the
/// missing-store condition separately (callers warn) rather than refuse.
/// Shared by `aida trace gate` (STORY-498) and the client-side ship/done guard.
/// trace:STORY-498 trace:STORY-469 | ai:claude
pub(crate) fn resolve_spec_in_store(
    store: Option<&aida_core::RequirementsStore>,
    id: &str,
) -> SpecResolution {
    let Some(store) = store else {
        return SpecResolution::Live;
    };
    let want = id.to_ascii_uppercase();
    let found = store.requirements.iter().find(|r| {
        r.spec_id
            .as_deref()
            .map(|s| s.eq_ignore_ascii_case(&want))
            .unwrap_or(false)
            || r.agreed_id
                .as_deref()
                .map(|s| s.eq_ignore_ascii_case(&want))
                .unwrap_or(false)
    });
    match found {
        None => SpecResolution::Missing,
        Some(r) if matches!(r.status, RequirementStatus::Rejected) => SpecResolution::Rejected,
        Some(_) => SpecResolution::Live,
    }
}

// ============================================================================
// TASK-868: trace-ROT detector (`aida trace check`) — move #1 of EPIC-50.
//
// `gate` validates commit `(SPEC-ID)` trailers; `coverage` checks the changed
// CODE carries provenance. `check` closes the third corner: it walks the inline
// `// trace:SPEC-ID` markers *already in the source* and resolves each against
// the live graph, flagging the ones that have ROTTED — the target was deleted,
// renumbered, or rejected. This neutralizes the sharpest self-undercut of the
// separate-model approach ("trace comments rot invisibly"): a stale trace now
// goes red like a failing type. Report-only by default; `--block` for CI.
//
// Reuses `walk_source_for_traces` (the same inline scanner `aida doctor
// validate-trace-comments` uses) and `resolve_spec_in_store` / `load_store_for_lookup`
// (the same resolution `aida show` and `trace gate` use). trace:TASK-868
// ============================================================================

/// How a single inline `// trace:SPEC-ID` marker resolves. A superset of
/// `SpecResolution` that also splits out the archived-but-live case so the
/// report can call it cheaply (an archived target is a softer rot signal than a
/// deleted one — the spec still exists, just hidden). trace:TASK-868
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TraceRotVerdict {
    /// Resolves to a live, non-archived, non-rejected spec — healthy.
    Live,
    /// Resolves to an archived spec — exists but hidden; soft rot.
    Archived,
    /// Resolves to a rejected spec — a dead provenance link.
    Rejected,
    /// Resolves to nothing — deleted / renumbered / typo'd target; hard rot.
    Unknown,
}

impl TraceRotVerdict {
    /// Whether this verdict counts as HARD trace-rot — a genuinely dead
    /// provenance link (deleted/renumbered target or a rejected spec). This is
    /// what `--block` fails on. An archived target is NOT hard rot: the spec
    /// still exists in the graph, it's just hidden from default views, so its
    /// trace still resolves — it's reported as a soft signal, not a failure.
    pub(crate) fn is_rot(self) -> bool {
        matches!(self, TraceRotVerdict::Rejected | TraceRotVerdict::Unknown)
    }

    /// Whether this verdict is worth surfacing at all (anything but healthy).
    pub(crate) fn is_flagged(self) -> bool {
        !matches!(self, TraceRotVerdict::Live)
    }

    pub(crate) fn reason(self) -> &'static str {
        match self {
            TraceRotVerdict::Live => "resolves to a live spec",
            TraceRotVerdict::Archived => "target is archived",
            TraceRotVerdict::Rejected => "target is rejected",
            TraceRotVerdict::Unknown => "target does not exist (deleted/renumbered)",
        }
    }
}

/// Resolve a SPEC-ID against the store for the rot check, distinguishing
/// archived from rejected from missing. When the store is unreachable, every id
/// resolves `Live` (callers warn separately) so a checkout without store access
/// doesn't flag the whole tree. trace:TASK-868
pub(crate) fn resolve_trace_rot(
    store: Option<&aida_core::RequirementsStore>,
    id: &str,
) -> TraceRotVerdict {
    let Some(store) = store else {
        return TraceRotVerdict::Live;
    };
    let want = id.to_ascii_uppercase();
    let found = store.requirements.iter().find(|r| {
        r.spec_id
            .as_deref()
            .map(|s| s.eq_ignore_ascii_case(&want))
            .unwrap_or(false)
            || r.agreed_id
                .as_deref()
                .map(|s| s.eq_ignore_ascii_case(&want))
                .unwrap_or(false)
    });
    match found {
        None => TraceRotVerdict::Unknown,
        Some(r) if matches!(r.status, RequirementStatus::Rejected) => TraceRotVerdict::Rejected,
        Some(r) if r.archived => TraceRotVerdict::Archived,
        Some(_) => TraceRotVerdict::Live,
    }
}

/// One dangling (rotted) inline trace marker. trace:TASK-868
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct TraceRot {
    /// The referenced SPEC-ID that rotted.
    pub(crate) spec_id: String,
    /// Project-relative file path.
    pub(crate) file: String,
    /// 1-based line number.
    pub(crate) line: usize,
    pub(crate) verdict: TraceRotVerdict,
}

/// CLI handler for `aida trace check`. Scans inline `// trace:SPEC-ID` markers,
/// resolves each against the live graph, reports the rot, and exits non-zero
/// (code 1) under `--block` when any dangling trace exists. trace:TASK-868
pub(crate) fn handle_trace_check(path: Option<&str>, json: bool, block: bool) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let scan_root = match path {
        Some(p) => std::path::PathBuf::from(p),
        None => project_root.clone(),
    };

    let store = load_store_for_lookup(&project_root);
    if store.is_none() && !json {
        eprintln!(
            "{} trace check: no requirement store reachable from {} — cannot resolve trace \
             targets, so nothing can be flagged as rotted. Run where the store is attached \
             (`aida cache rebuild` / fresh-clone auto-attach).",
            crate::glyph(crate::glyphs::Glyph::Warning),
            project_root.display()
        );
    }

    // Same marker grammar + scanner the doctor trace check uses.
    let trace_re = regex::Regex::new(r"trace:([A-Z]+(?:-[A-Z0-9]+)?-[0-9]+(?:-[0-9]+)?)").unwrap();
    let mut by_spec: std::collections::HashMap<String, Vec<(std::path::PathBuf, usize)>> =
        std::collections::HashMap::new();
    walk_source_for_traces(&scan_root, &trace_re, &mut by_spec);

    // Resolve each unique id once; expand to per-location rows for the flagged ones.
    let mut total_markers = 0usize;
    let mut unique_ids = 0usize;
    let mut resolved_markers = 0usize;
    let mut flagged: Vec<TraceRot> = Vec::new();
    let mut ids: Vec<&String> = by_spec.keys().collect();
    ids.sort();
    for id in ids {
        unique_ids += 1;
        let locations = &by_spec[id];
        total_markers += locations.len();
        let verdict = resolve_trace_rot(store.as_ref(), id);
        if verdict.is_flagged() {
            for (p, line) in locations {
                let rel = p.strip_prefix(&project_root).unwrap_or(p);
                flagged.push(TraceRot {
                    spec_id: id.clone(),
                    file: rel.display().to_string(),
                    line: *line,
                    verdict,
                });
            }
        } else {
            resolved_markers += locations.len();
        }
    }
    flagged.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));

    // Hard rot (dead links) is what `--block` fails on; archived is a soft signal.
    let hard: Vec<&TraceRot> = flagged.iter().filter(|r| r.verdict.is_rot()).collect();
    let archived = flagged.len() - hard.len();
    let dangling = hard.len();
    let unknown = hard
        .iter()
        .filter(|r| matches!(r.verdict, TraceRotVerdict::Unknown))
        .count();
    let rejected = dangling - unknown;
    let rot_rate = if total_markers == 0 {
        0.0
    } else {
        (dangling as f64) * 100.0 / (total_markers as f64)
    };

    if json {
        let payload = serde_json::json!({
            "scan_root": scan_root.display().to_string(),
            "store_reachable": store.is_some(),
            "total_traces": total_markers,
            "unique_spec_ids": unique_ids,
            "resolved": resolved_markers,
            "dangling": dangling,
            "dangling_unknown": unknown,
            "dangling_rejected": rejected,
            "archived": archived,
            "rot_rate_pct": (rot_rate * 100.0).round() / 100.0,
            "flagged": flagged,
            "ok": dangling == 0,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if dangling == 0 && archived == 0 {
        println!(
            "{} trace check: {} inline trace(s) across {} spec-id(s) — every marker resolves to a \
             live spec (0 rotted)",
            crate::glyph(crate::glyphs::Glyph::Check),
            total_markers,
            unique_ids
        );
    } else {
        if dangling > 0 {
            eprintln!(
                "{} trace check: {} dangling/rotted trace marker(s) ({:.1}% rot rate — {} unknown, \
                 {} rejected; {} of {} resolve):",
                crate::glyph(crate::glyphs::Glyph::Cross),
                dangling,
                rot_rate,
                unknown,
                rejected,
                resolved_markers,
                total_markers
            );
            for r in &hard {
                eprintln!(
                    "  {}:{} → {} ({})",
                    r.file,
                    r.line,
                    r.spec_id,
                    r.verdict.reason()
                );
            }
        } else {
            println!(
                "{} trace check: 0 dead trace links ({} of {} resolve)",
                crate::glyph(crate::glyphs::Glyph::Check),
                resolved_markers,
                total_markers
            );
        }
        if archived > 0 {
            println!(
                "\n{} {} trace(s) point at archived specs (still resolve; soft signal, not \
                 blocking). Re-target or accept.",
                crate::glyph(crate::glyphs::Glyph::Warning),
                archived
            );
        }
        if dangling > 0 {
            eprintln!(
                "\nA `// trace:SPEC-ID` marker must name a live requirement. Update the marker to \
                 the spec's current id (after a merge-gate renumber the id changes), point it at a \
                 real spec, or delete the stale comment. `aida doctor validate-trace-comments \
                 --strip-dangling` can sweep the unknown ones in bulk."
            );
        }
    }

    if block && dangling > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// CLI handler for `aida trace gate`. Reads the commit range from git, runs the
/// pure validator against the live store, prints the result, and exits non-zero
/// (code 1) when any commit references a dead/dangling SPEC-ID.
/// trace:STORY-498 | ai:claude
/// Read `(short_sha, subject)` rows for a git range. A unit separator keeps
/// subjects with arbitrary punctuation intact. Returns an error only when git
/// itself fails (bad range, no git). Shared by the trace gate and the
/// client-side ship/done guard. trace:STORY-498 trace:STORY-469 | ai:claude
pub(crate) fn read_commits_in_range(
    project_root: &std::path::Path,
    range: &str,
) -> Result<Vec<(String, String)>> {
    use std::process::Command as PCmd;
    let output = PCmd::new("git")
        .arg("-C")
        .arg(project_root)
        // trace:BUG-1622 | ai:claude
        .args([
            "log",
            "--no-merges",
            "--pretty=format:%h\x1f%s",
            git_arg_guard::END_OF_OPTIONS,
            range,
            "--",
        ])
        .output();
    match output {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter_map(|line| {
                let (sha, subject) = line.split_once('\x1f')?;
                let subject = subject.trim();
                if subject.is_empty() {
                    return None;
                }
                Some((sha.to_string(), subject.to_string()))
            })
            .collect()),
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            anyhow::bail!("git log failed for range `{range}`: {}", err.trim());
        }
        Err(e) => anyhow::bail!("could not run git: {e}"),
    }
}

// ============================================================================
// STORY-469 Guard 1: client-side trailer spec-ID validation at ship/done.
//
// STORY-498 shipped a SERVER-side (CI) version of this check. Guard 1 is its
// client-side twin: it runs in `aida pr ship` and `aida queue done` so a
// hallucinated/typo'd/dead spec-ID is caught BEFORE the commit reaches the PR
// or the spec is flipped to Done — never lands in shared git history. It reuses
// the same pure validator (`validate_trailer_references`) + resolver
// (`resolve_spec_in_store`) as the gate; only the call site differs.
// ============================================================================

/// Format the human-readable refusal lines for client-side trailer violations.
/// Pure (takes pre-computed violations) so it's unit-testable in isolation.
/// `surface` names the command for the message ("pr ship" / "queue done").
/// trace:STORY-469 | ai:claude
pub(crate) fn format_trailer_guard_refusal(
    surface: &str,
    violations: &[TrailerViolation],
) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!(
        "{} {surface}: {} commit trailer reference(s) name a spec that does not resolve:",
        crate::glyph(crate::glyphs::Glyph::Cross),
        violations.len()
    ));
    for v in violations {
        lines.push(format!(
            "  {} ({}) {} — {}",
            v.sha,
            v.spec_id,
            v.verdict.reason(),
            v.subject
        ));
    }
    lines.push(String::new());
    lines.push(
        "A commit's `(SPEC-ID)` trailer must name a live requirement — typo? hallucination? \
         since-rejected? Check `aida show <SPEC-ID>` and fix the trailer (`git commit --amend` / \
         interactive rebase) before retrying, or re-run with --force to bypass."
            .to_string(),
    );
    lines
}

/// Guard 1: validate that every `(SPEC-ID)` trailer on the commits this branch
/// adds over the default branch resolves to a live (non-rejected) spec. Refuses
/// (exits 1) on any dead/dangling reference unless `force` is set. A no-op when
/// the branch is the default branch (nothing to ship) or no store is reachable
/// (cannot corroborate — surfaced as a soft warning, never a hard refusal, so a
/// store-less checkout can still ship). `surface` names the calling command.
/// trace:STORY-469 | ai:claude
pub(crate) fn run_client_trailer_guard(project_root: &std::path::Path, surface: &str, force: bool) {
    if force {
        return;
    }
    let range = resolve_gate_range(project_root, None);
    let commits = match read_commits_in_range(project_root, &range) {
        Ok(c) => c,
        Err(e) => {
            // Git read failed (e.g. shallow checkout, unresolved range). The
            // guard cannot corroborate; surface softly and proceed — failing
            // the ship/done on a git hiccup would be worse than the miss.
            eprintln!(
                "{} {surface}: trailer spec-ID check skipped — {}",
                "warning:".yellow().bold(),
                e
            );
            return;
        }
    };
    if commits.is_empty() {
        return;
    }
    let store = load_store_for_lookup(project_root);
    if store.is_none() {
        eprintln!(
            "{} {surface}: trailer spec-ID check skipped — no requirement store reachable. \
             Run where the store is attached (`aida cache rebuild`) to enforce.",
            "warning:".yellow().bold()
        );
        return;
    }
    let resolve = |id: &str| -> SpecResolution { resolve_spec_in_store(store.as_ref(), id) };
    let violations = validate_trailer_references(&commits, resolve);
    if violations.is_empty() {
        return;
    }
    for line in format_trailer_guard_refusal(surface, &violations) {
        eprintln!("{line}");
    }
    std::process::exit(1);
}

// ============================================================================
// TASK-1442: PR-open spec-attribution guard (containment for BUG-1510).
//
// STORY-469's `run_client_trailer_guard` above checks that every `(SPEC-ID)`
// trailer on the branch resolves to a LIVE spec — it never checks that a
// trailer names the SPEC THE BRANCH IS FOR. BUG-1510's incident: STORY-1391's
// drain opened PR #2043 whose commits were all trailered BUG-1420 — every
// trailer was live, so Guard 1 passed, but the PR was misattributed. This
// guard closes that gap: before a NEW PR is opened, at least one commit the
// branch adds over the default branch must carry a trailer naming the spec
// the branch is leased for.
// ============================================================================

/// Pure, testable core: given commits as `(sha, subject)` pairs and the spec
/// the branch is leased for, return `None` when some commit's `(SPEC-ID)`
/// trailer names `expected_spec` (attribution OK), or `Some(other_ids)` — the
/// distinct spec ids the trailers DO name — when none does (refuse). Reuses
/// the same trailer extractor + plan-commit exemption as Guard 1
/// (`validate_trailer_references`) so the two guards agree on what a
/// "trailer" is.
// trace:TASK-1442 | ai:claude
pub(crate) fn pr_open_spec_guard_violation(
    commits: &[(String, String)],
    expected_spec: &str,
) -> Option<Vec<String>> {
    let mut other_ids: Vec<String> = Vec::new();
    for (_, subject) in commits {
        if is_plan_commit_subject(subject) {
            continue;
        }
        for id in extract_spec_ids_from_commit(subject) {
            if id.eq_ignore_ascii_case(expected_spec) {
                return None; // found a matching trailer — attributed correctly
            }
            if !other_ids
                .iter()
                .any(|s: &String| s.eq_ignore_ascii_case(&id))
            {
                other_ids.push(id);
            }
        }
    }
    Some(other_ids)
}

// ============================================================================
// TASK-1444 (containment for BUG-1510 AC4): reviewer-verdict shelve
// attribution.
//
// The incident: STORY-1391's drain got a RequestChanges verdict whose
// findings were about BUG-1420 (the PR's commits were all trailered
// BUG-1420, per the TASK-1442 guard above), and the orchestrator shelved it
// onto STORY-1391 — the lease's spec — silently. A shelve must record
// against the spec the VERDICT is about, or say the attribution is
// uncertain; it must never read as a confirmed attribution to the lease
// when that was never checked.
// ============================================================================

/// Which spec a reviewer-verdict shelve should be recorded against.
// trace:TASK-1444 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShelveAttribution {
    /// A commit trailer confirms the verdict is about the lease's own spec.
    Confirmed(String),
    /// Every non-plan commit trailer names exactly one spec, and it is NOT
    /// the lease's — the shelve should target THAT spec, not the lease.
    Reattributed(String),
    /// The commits don't confirm a single spec (none named, or more than
    /// one) — the attribution can't be safely resolved either way.
    Uncertain(String),
}

/// Pure, testable core: decide which spec a reviewer-verdict shelve is
/// actually about, from what the PR's own commits credit — never silently
/// `lease_spec`. Reuses `pr_open_spec_guard_violation`'s trailer extraction
/// (TASK-1442) so the two guards agree on what a trailer is: `None` there
/// means some commit trailers `lease_spec` itself (`Confirmed`); `Some(ids)`
/// means none does, and `ids` is what the non-plan commits DO name — exactly
/// one other id means the verdict is confidently about that spec instead
/// (`Reattributed`), while zero or several distinct ids means the commits
/// don't settle it (`Uncertain`).
// trace:TASK-1444 | ai:claude
pub(crate) fn decide_shelve_attribution(
    commits: &[(String, String)],
    lease_spec: &str,
) -> ShelveAttribution {
    match pr_open_spec_guard_violation(commits, lease_spec) {
        None => ShelveAttribution::Confirmed(lease_spec.to_string()),
        Some(other_ids) => match other_ids.as_slice() {
            [only] => ShelveAttribution::Reattributed(only.clone()),
            [] => ShelveAttribution::Uncertain(
                "no commit on this PR carries a spec-ID trailer".to_string(),
            ),
            many => ShelveAttribution::Uncertain(format!(
                "commits name multiple specs: {}",
                many.join(", ")
            )),
        },
    }
}

// ============================================================================
// TASK-1445 (containment for BUG-1510 AC5): surface a PR attribution split
// between `aida drain status` (attributes a PR by the lease it ran under)
// and the trailer/title-based detectors (`awaiting_you.rs` unshipped-work
// code) — instead of letting the two sources silently disagree, as happened
// for 52 seconds in the BUG-1510 incident before a reviewer verdict landed
// on the wrong spec.
// ============================================================================

/// Pure, testable core: given the spec a PR's lease attributes it to and
/// that PR's own commits, decide whether the two attribution sources agree.
/// Reuses `decide_shelve_attribution` (TASK-1444) so this and the
/// reviewer-verdict shelve guard agree on what "trailer evidence" means:
/// `Confirmed` (a trailer names the lease spec) and `Uncertain` (the
/// trailers don't settle it — zero or several distinct ids) are both
/// non-disagreements; only a confident `Reattributed` to a DIFFERENT spec is
/// a disagreement worth surfacing. Never silently prefers one source over
/// the other — both claimed owners are returned.
// trace:TASK-1445 | ai:claude
pub(crate) fn pr_attribution_disagreement(
    pr: u64,
    commits: &[(String, String)],
    lease_spec: &str,
) -> Option<awaiting_you::PrAttributionDisagreementItem> {
    match decide_shelve_attribution(commits, lease_spec) {
        ShelveAttribution::Reattributed(trailer_spec) => {
            Some(awaiting_you::PrAttributionDisagreementItem {
                pr,
                lease_spec: lease_spec.to_string(),
                trailer_spec,
            })
        }
        ShelveAttribution::Confirmed(_) | ShelveAttribution::Uncertain(_) => None,
    }
}

/// Collect every attribution split for the currently live drain's in-flight
/// members. Local-only: a `.aida/drain-state.json` read, the local lease
/// list, and one `git log` per in-flight member with a recorded PR — no
/// network call. Returns nothing when no drain is active, a member has no
/// PR yet, or its lease's branch can't be resolved/read (fails open — a git
/// hiccup here must not manufacture a false disagreement).
// trace:TASK-1445 | ai:claude
pub(crate) fn collect_pr_attribution_disagreements(
    project_root: &std::path::Path,
) -> Vec<awaiting_you::PrAttributionDisagreementItem> {
    let state = match drain_state::probe(project_root) {
        drain_state::DrainStatus::Active(state) => state,
        drain_state::DrainStatus::None
        | drain_state::DrainStatus::Stale(_)
        | drain_state::DrainStatus::Stopped(_) => return Vec::new(),
    };
    let Some(default_ref) = resolve_default_branch_ref(project_root) else {
        return Vec::new();
    };
    let leases = list_leases(project_root);
    let mut out = Vec::new();
    for member in state.members.iter().filter(|m| m.is_running()) {
        let Some(pr) = member.pr else { continue };
        let Some(lease) = leases
            .iter()
            .find(|l| l.scope.eq_ignore_ascii_case(&member.spec))
        else {
            continue;
        };
        let range = format!("{default_ref}..{}", lease.branch);
        let Ok(commits) = read_commits_in_range(project_root, &range) else {
            continue;
        };
        if let Some(disagreement) = pr_attribution_disagreement(pr as u64, &commits, &member.spec) {
            out.push(disagreement);
        }
    }
    out
}

/// Refuse (exit 1) to open a PR when no commit the branch adds over the
/// default branch carries a `(SPEC-ID)` trailer naming `expected_spec`. A
/// no-op when the commit range can't be read (soft warning — a git hiccup
/// shouldn't block shipping) or when the branch has no commits to ship.
// trace:TASK-1442 | ai:claude
pub(crate) fn run_pr_open_spec_guard(
    project_root: &std::path::Path,
    branch: &str,
    expected_spec: &str,
) {
    let range = resolve_gate_range(project_root, None);
    let commits = match read_commits_in_range(project_root, &range) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "{} pr ship: PR-spec attribution check skipped — {}",
                "warning:".yellow().bold(),
                e
            );
            return;
        }
    };
    if commits.is_empty() {
        return;
    }

    let other_ids = match pr_open_spec_guard_violation(&commits, expected_spec) {
        None => return,
        Some(ids) => ids,
    };

    eprintln!(
        "{} pr ship: refusing to open a PR — branch `{}` is leased for {} but no commit on \
         it carries a `({})` trailer.",
        crate::glyph(crate::glyphs::Glyph::Cross),
        branch,
        expected_spec,
        expected_spec
    );
    if other_ids.is_empty() {
        eprintln!("  no commit on this branch carries a (SPEC-ID) trailer at all.");
    } else {
        eprintln!(
            "  commit trailer(s) instead name: {} — a mismatch against the leased spec {}.",
            other_ids.join(", "),
            expected_spec
        );
    }
    eprintln!(
        "  Fix the trailer(s) (`git commit --amend` / interactive rebase) to reference {}, or \
         end this lease and open the PR from the branch that actually owns {}.",
        expected_spec, expected_spec
    );
    std::process::exit(1);
}

/// TASK-1457 (BUG-1527 follow-up): the same three-way classification
/// `decide_shelve_attribution` (TASK-1444) gives a reviewer-verdict shelve,
/// but for the branch-swap seam `RealPhaseDriver::run_implementer` gates on
/// before it will accept a mid-phase branch change. `ensure_pr_open_spec_attribution`
/// collapses "a trailer names a different spec" and "no commit carries any
/// spec-ID trailer" into the same `Err` — which is why a same-spec rename
/// (BUG-223) whose commits simply have not been trailered yet used to read
/// as a confidently-worded "swap". PRIN-5 requires these stay distinct
/// outcomes: `Reattributed` is a spec-ID trailer actively pointing somewhere
/// else (the confident swap case); `Uncertain` is absent evidence, not
/// contrary evidence, and must never be reported the same way. Fails open
/// (`Confirmed`) exactly where `ensure_pr_open_spec_attribution` does — an
/// unresolvable default branch, an unreadable range, or no commits at all —
/// so a git hiccup here can't manufacture a false swap report either.
// trace:BUG-1527 trace:TASK-1457 | ai:claude
pub(crate) fn classify_branch_swap_attribution(
    repo: &std::path::Path,
    branch_ref: &str,
    expected_spec: &str,
) -> ShelveAttribution {
    let Some(default_ref) = resolve_default_branch_ref(repo) else {
        return ShelveAttribution::Confirmed(expected_spec.to_string());
    };
    let range = format!("{default_ref}..{branch_ref}");
    let commits = match read_commits_in_range(repo, &range) {
        Ok(c) => c,
        Err(_) => return ShelveAttribution::Confirmed(expected_spec.to_string()),
    };
    if commits.is_empty() {
        return ShelveAttribution::Confirmed(expected_spec.to_string());
    }
    decide_shelve_attribution(&commits, expected_spec)
}

/// TASK-1442 follow-up: the same PR-spec attribution check as
/// `run_pr_open_spec_guard`, but as a `Result` instead of an exiting side
/// effect. `run_pr_open_spec_guard` is only safe at `aida pr ship`'s own
/// top level; the autonomous drain's PR-open recovery paths
/// (`open_orchestrator_pr_for_implementer_worktree`,
/// `open_orchestrator_pr_for_pushed_branch`) run INSIDE the orchestrator
/// process, where `std::process::exit` would kill the whole drain instead of
/// failing just the one phase. `repo` is the directory to run git in (the
/// implementer worktree, or the main project root for a pushed branch);
/// `branch_ref` is whatever ref names the branch's commits from there (a
/// local branch name or `origin/<branch>`). A no-op when the default branch
/// or commit range can't be resolved (soft — a git hiccup shouldn't block
/// recovery) or when there are no commits to check.
///
/// TASK-1457: this collapses `Reattributed` and `Uncertain` (see
/// `classify_branch_swap_attribution` above) into the same `Err` — a
/// deliberate, kept decision for THIS gate. `ensure_spec_done_after_pr`
/// (its only status-writing caller) must not flip a spec to Done on
/// EITHER absent or contrary trailer evidence; per PRIN-5, "cannot
/// determine" is not license to proceed on a Done write any more than
/// "determined otherwise" is. The two PR-open recovery callers
/// (`open_orchestrator_pr_for_implementer_worktree`,
/// `open_orchestrator_pr_for_pushed_branch`) inherit the same fail-safe
/// for the same reason: opening a PR under an unconfirmed identity is a
/// second write worth refusing on absent evidence too. Only the
/// branch-swap seam needs the finer three-way split, because only it has
/// to tell an operator whether to look for a swap or a missing trailer.
// trace:TASK-1442 trace:TASK-1457 | ai:claude
pub(crate) fn ensure_pr_open_spec_attribution(
    repo: &std::path::Path,
    branch_ref: &str,
    expected_spec: &str,
) -> Result<()> {
    let Some(default_ref) = resolve_default_branch_ref(repo) else {
        return Ok(());
    };
    let range = format!("{default_ref}..{branch_ref}");
    let commits = match read_commits_in_range(repo, &range) {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };
    if commits.is_empty() {
        return Ok(());
    }
    if let Some(other_ids) = pr_open_spec_guard_violation(&commits, expected_spec) {
        let named = if other_ids.is_empty() {
            "no commit carries a (SPEC-ID) trailer at all".to_string()
        } else {
            format!("commit trailer(s) instead name: {}", other_ids.join(", "))
        };
        anyhow::bail!(
            "refusing to open a PR for {expected_spec} on `{branch_ref}` — no commit on it \
             carries a `({expected_spec})` trailer; {named}"
        );
    }
    Ok(())
}

/// CLI handler for `aida trace gate`. Reads the commit range from git, runs the
/// pure validator against the live store, prints the result, and exits non-zero
/// (code 1) when any commit references a dead/dangling SPEC-ID.
/// trace:STORY-498 | ai:claude
pub(crate) fn handle_trace_gate(range: Option<&str>, json: bool) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    // trace:BUG-1622 | ai:claude
    if let Some(r) = range {
        git_arg_guard::reject_option_like("--range", r)?;
    }
    let range = resolve_gate_range(&project_root, range);

    let commits = read_commits_in_range(&project_root, &range)?;

    // Load the live requirement graph once, then resolve each id against it.
    let store = load_store_for_trace_gate(&project_root)?;
    let resolve = |id: &str| -> SpecResolution { resolve_spec_in_store(Some(&store), id) };

    let violations = validate_trailer_references(&commits, resolve);

    if json {
        let payload = serde_json::json!({
            "range": range,
            "commits_scanned": commits.len(),
            "violations": violations,
            "ok": violations.is_empty(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if violations.is_empty() {
        println!(
            "{} trace gate: {} commit(s) in `{}` — every (SPEC-ID) trailer resolves to a live spec",
            crate::glyph(crate::glyphs::Glyph::Check),
            commits.len(),
            range
        );
    } else {
        eprintln!(
            "{} trace gate: {} dead/dangling SPEC-ID reference(s) in `{}`:",
            crate::glyph(crate::glyphs::Glyph::Cross),
            violations.len(),
            range
        );
        for v in &violations {
            eprintln!(
                "  {} ({}) {} — {}",
                v.sha,
                v.spec_id,
                v.verdict.reason(),
                v.subject
            );
        }
        eprintln!(
            "\nA commit's `(SPEC-ID)` trailer must name a live requirement. Fix the trailer to a \
             real, non-rejected id (or file the missing spec), then amend/re-commit."
        );
    }

    if !violations.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

pub(crate) fn load_store_for_trace_gate(
    project_root: &std::path::Path,
) -> Result<aida_core::RequirementsStore> {
    // trace:TASK-1206 | ai:codex
    if let Some(store_path) = detect_distributed_store_from(project_root) {
        if let Ok(backend) = aida_core::GitBackend::new(&store_path) {
            if let Ok(store) = aida_core::DatabaseBackend::load(&backend) {
                return Ok(store);
            }
        }
    }

    for legacy_name in ["requirements.db", "requirements.yaml"] {
        let legacy_path = project_root.join(legacy_name);
        if legacy_path.exists() {
            return Storage::new(legacy_path).load();
        }
    }

    anyhow::bail!(
        "{} trace gate: no requirement store reachable from {} — cannot validate references. \
         Ensure the gate runs where the store is attached (`aida cache rebuild` / fresh-clone \
         auto-attach).",
        crate::glyph(crate::glyphs::Glyph::Cross),
        project_root.display()
    )
}

// ============================================================================
// STORY-499: diff-level trace-COVERAGE check (gap #1 of EPIC-34).
//
// STORY-498 shut gaps #2 + #3: `aida trace gate` validates that every commit
// `(SPEC-ID)` *trailer* resolves to a live spec. STORY-499 closes gap #1 —
// COVERAGE: does the changed CODE in a diff actually carry the required
// provenance (`// trace:` anchor OR a live commit trailer), per the SPIKE-47
// definition? It is report-only by default (SPIKE-47 §5/§6: a coverage gate's
// failure mode is adoption death; ship it report-first, flip to block after a
// calibration period).
//
// The hard part is precision without false positives. SPIKE-47 settled the
// definition deterministically:
//   - UNIT: the changed source hunk (NOT line, NOT function — §2).
//   - EXEMPTIONS: a two-layer, explicit, audited list (§3) — commit-level
//     (reusing STORY-498's plan/no-trailer machinery) + file/path-level
//     (tests, generated, docs, config, vendored) + hunk-level (pure deletion,
//     whitespace/fmt-only, comment-only, trivial one-liner).
//   - ATTRIBUTION (§4.2): a coverable hunk is COVERED if ANY of — an in-hunk
//     `// trace:<live-id>`; a `// trace:` within N lines ABOVE the hunk in the
//     post-change file; a module-level `//! trace:`; OR the commit carries a
//     live `(SPEC-ID)` trailer (the floor — STORY-498 already enforces this for
//     validity, so the default world is fully covered and the gate stays
//     silent).
//
// The CORE (`compute_diff_coverage`) is a PURE, fully-isolated-tested fn: it
// takes parsed hunks + per-file post-change content + a commit-trailer-covered
// flag + an anchor resolver, and returns the per-hunk classification + ratio.
// No git, no store, no I/O. The CLI handler gathers those inputs from git and
// the live graph and calls it. trace:STORY-499 | ai:claude
// ============================================================================

/// Why a coverable hunk is exempt from the coverage denominator. Every
/// exemption is reported (never silently dropped) per SPIKE-47 §3.
/// trace:STORY-499 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CoverageExemption {
    /// File path matched a test glob (SPIKE-47 F1).
    TestFile,
    /// File path / generated-marker matched (SPIKE-47 F2).
    Generated,
    /// Non-source / docs / prose extension (SPIKE-47 F3).
    DocOrProse,
    /// Config / data / lockfile (SPIKE-47 F4).
    ConfigData,
    /// Vendored / third-party path (SPIKE-47 F5).
    Vendored,
    /// Hunk with only deletions (SPIKE-47 H1).
    PureDeletion,
    /// Hunk whose added lines are only comments / blank (SPIKE-47 H2/H4).
    CommentOrBlankOnly,
    /// Hunk net change ≤ trivial threshold (SPIKE-47 H4).
    Trivial,
}

impl CoverageExemption {
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            CoverageExemption::TestFile => "test file",
            CoverageExemption::Generated => "generated code",
            CoverageExemption::DocOrProse => "docs / non-source",
            CoverageExemption::ConfigData => "config / data / lockfile",
            CoverageExemption::Vendored => "vendored / third-party",
            CoverageExemption::PureDeletion => "pure deletion",
            CoverageExemption::CommentOrBlankOnly => "comment / blank only",
            CoverageExemption::Trivial => "trivial change",
        }
    }
}

/// How a coverable hunk earned its coverage (SPIKE-47 §4.2). Cheapest-first.
/// trace:STORY-499 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CoverageSource {
    /// A `// trace:<live-id>` appears in the hunk's added lines (§4.2.1).
    InHunkAnchor,
    /// A `// trace:<live-id>` sits within N lines above the hunk (§4.2.2).
    ProximityAnchor,
    /// A module-level `//! trace:<live-id>` covers the whole file (§4.2.3).
    FileAnchor,
    /// The hunk's commit carries a live `(SPEC-ID)` trailer (§4.2.4 — the floor).
    CommitTrailer,
}

/// The classification of one changed hunk. trace:STORY-499 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct HunkVerdict {
    /// File the hunk lives in (post-change path).
    pub(crate) file: String,
    /// 1-based start line of the hunk in the post-change file.
    pub(crate) new_start: usize,
    /// Number of added/modified lines in the hunk.
    pub(crate) added_lines: usize,
    /// `Some(reason)` when the hunk is exempt from the denominator.
    pub(crate) exempt: Option<CoverageExemption>,
    /// `Some(source)` when the hunk is covered. `None` + not-exempt = a
    /// reported uncovered coverable hunk.
    pub(crate) covered: Option<CoverageSource>,
}

impl HunkVerdict {
    /// A coverable hunk that is NOT covered — the thing the gate reports.
    pub(crate) fn is_uncovered_coverable(&self) -> bool {
        self.exempt.is_none() && self.covered.is_none()
    }
    pub(crate) fn is_coverable(&self) -> bool {
        self.exempt.is_none()
    }
}

/// One parsed changed hunk fed into the pure core. trace:STORY-499 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedHunk {
    /// Post-change file path.
    pub(crate) file: String,
    /// 1-based start line of the hunk in the post-change file.
    pub(crate) new_start: usize,
    /// The `+` (added) line bodies, in order, with the leading `+` stripped.
    pub(crate) added: Vec<String>,
    /// The `-` (removed) line bodies, with the leading `-` stripped.
    pub(crate) removed: Vec<String>,
}

/// Aggregate coverage result returned by the pure core. trace:STORY-499 | ai:claude
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub(crate) struct DiffCoverage {
    pub(crate) hunks: Vec<HunkVerdict>,
}

impl DiffCoverage {
    pub(crate) fn coverable(&self) -> usize {
        self.hunks.iter().filter(|h| h.is_coverable()).count()
    }
    pub(crate) fn covered(&self) -> usize {
        self.hunks
            .iter()
            .filter(|h| h.is_coverable() && h.covered.is_some())
            .count()
    }
    pub(crate) fn exempt(&self) -> usize {
        self.hunks.iter().filter(|h| h.exempt.is_some()).count()
    }
    pub(crate) fn uncovered(&self) -> Vec<&HunkVerdict> {
        self.hunks
            .iter()
            .filter(|h| h.is_uncovered_coverable())
            .collect()
    }
    /// `covered / coverable`. 1.0 when there is nothing coverable (vacuously
    /// fully covered — an all-exempt diff passes). trace:STORY-499 | ai:claude
    pub(crate) fn ratio(&self) -> f64 {
        let coverable = self.coverable();
        if coverable == 0 {
            return 1.0;
        }
        self.covered() as f64 / coverable as f64
    }
}

/// Default source extensions that REQUIRE coverage. Mirrors `aida trace scan
/// --extensions` default (`rs`); extensible per-repo (SPIKE-47 §2.1.2).
/// trace:STORY-499 | ai:claude
pub(crate) const COVERAGE_SOURCE_EXTENSIONS: &[&str] =
    &["rs", "py", "ts", "tsx", "js", "jsx", "go", "java"];

/// SPIKE-47 §3.2 file/path-level exemption classifier. Returns `Some(reason)`
/// when the whole file is exempt, `None` when its changed source hunks are
/// coverable. Deterministic — globs/extensions only, no parsing. PURE.
/// trace:STORY-499 | ai:claude
pub(crate) fn classify_coverage_file(
    path: &str,
    generated_header: Option<&str>,
) -> Option<CoverageExemption> {
    let lower = path.to_ascii_lowercase();
    let segments: Vec<&str> = lower.split('/').collect();

    // F5: vendored / third-party.
    if segments
        .iter()
        .any(|s| matches!(*s, "vendor" | "node_modules" | "third_party" | "target"))
    {
        return Some(CoverageExemption::Vendored);
    }

    // F2: generated — path globs OR an in-file generated marker.
    if segments.contains(&"generated") || lower.contains(".gen.") || lower.ends_with(".gen.rs") {
        return Some(CoverageExemption::Generated);
    }
    if let Some(header) = generated_header {
        let h = header.to_ascii_lowercase();
        if h.contains("@generated") || h.contains("do not edit") || h.contains("auto-generated") {
            return Some(CoverageExemption::Generated);
        }
    }

    // F1: tests — `tests/` segment, or a test-named file.
    let file_name = segments.last().copied().unwrap_or("");
    if segments.contains(&"tests")
        || file_name.starts_with("test_")
        || file_name.contains("_test.")
        || file_name.contains(".test.")
        || file_name.ends_with("_test")
    {
        return Some(CoverageExemption::TestFile);
    }

    // Extension-based classification.
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");

    // F3: docs / prose — non-source extensions, or anything under `docs/`.
    if segments.contains(&"docs")
        || matches!(ext, "md" | "txt" | "rst" | "adoc" | "html" | "css" | "svg")
    {
        return Some(CoverageExemption::DocOrProse);
    }

    // F4: config / data / lockfiles.
    if lower.ends_with(".lock")
        || file_name == ".gitignore"
        || matches!(
            ext,
            "toml" | "yaml" | "yml" | "json" | "ini" | "cfg" | "lock"
        )
    {
        return Some(CoverageExemption::ConfigData);
    }

    // Coverable only when the extension is in the source set.
    if COVERAGE_SOURCE_EXTENSIONS.contains(&ext) {
        return None;
    }
    // Unknown / non-source extension → not coverable (treated as prose).
    Some(CoverageExemption::DocOrProse)
}

/// True when a single source line is comment-only or blank, for the common
/// comment leaders (`//`, `///`, `//!`, `#`, `*`, `/*`, `*/`, `<!--`). PURE.
/// Used by the comment-only hunk exemption (SPIKE-47 H4). trace:STORY-499 | ai:claude
pub(crate) fn coverage_line_is_comment_or_blank(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return true;
    }
    t.starts_with("//")
        || t.starts_with('#')
        || t.starts_with('*')
        || t.starts_with("/*")
        || t.starts_with("*/")
        || t.starts_with("<!--")
        || t.starts_with("-->")
}

/// SPIKE-47 §3.3 hunk-level exemption classifier. Returns `Some(reason)` when
/// a hunk (in an already-coverable file) is exempt. `trivial_max_lines` is the
/// configurable trivial-change floor (default 1). PURE. trace:STORY-499 | ai:claude
pub(crate) fn classify_coverage_hunk(
    hunk: &ParsedHunk,
    trivial_max_lines: usize,
) -> Option<CoverageExemption> {
    // H1: pure deletion — removed lines, no additions.
    if hunk.added.is_empty() {
        return Some(CoverageExemption::PureDeletion);
    }

    // H2 (fmt/whitespace) collapses into H1 once whitespace is normalized: a
    // whitespace-only reflow has `+`/`-` lines equal after trimming, so it
    // carries no net coverable addition. Detect by comparing the whitespace-
    // normalized added vs removed multisets.
    let norm = |lines: &[String]| -> Vec<String> {
        lines
            .iter()
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|l| !l.is_empty())
            .collect()
    };
    let mut added_norm = norm(&hunk.added);
    let mut removed_norm = norm(&hunk.removed);
    added_norm.sort();
    removed_norm.sort();
    if !added_norm.is_empty() && added_norm == removed_norm {
        // Whitespace-only reflow (no net content change) → fmt-only.
        return Some(CoverageExemption::CommentOrBlankOnly);
    }

    // H4 (comment-only): every added line is a comment or blank.
    if hunk
        .added
        .iter()
        .all(|l| coverage_line_is_comment_or_blank(l))
    {
        return Some(CoverageExemption::CommentOrBlankOnly);
    }

    // H4 (trivial): net effective added lines (non-blank, non-comment) at or
    // below the threshold.
    let effective_added = hunk
        .added
        .iter()
        .filter(|l| !coverage_line_is_comment_or_blank(l))
        .count();
    if effective_added <= trivial_max_lines {
        return Some(CoverageExemption::Trivial);
    }

    None
}

/// SPIKE-47 trace-anchor grammar (reused verbatim from `aida trace gate`):
/// matches `trace:<SPEC-ID>` regardless of comment leader. PURE compile.
/// trace:STORY-499 | ai:claude
pub(crate) fn coverage_anchor_re() -> regex::Regex {
    regex::Regex::new(r"trace:([A-Z]+(?:-[A-Z0-9]+)?-[0-9]+(?:-[0-9]+)?)").unwrap()
}

/// True when `line` carries a `// trace:<id>` whose id resolves Live via
/// `resolve`. Module-level `//!` anchors and `///` doc anchors all match (the
/// regex keys off the `trace:` token, not the leader). PURE (resolver injected).
/// trace:STORY-499 | ai:claude
pub(crate) fn coverage_line_has_live_anchor<F>(
    line: &str,
    re: &regex::Regex,
    resolve: &mut F,
) -> bool
where
    F: FnMut(&str) -> SpecResolution,
{
    re.captures_iter(line).any(|cap| {
        let id = &cap[1];
        matches!(resolve(id), SpecResolution::Live)
    })
}

/// Attribute one hunk to a coverage source, per SPIKE-47 §4.2 (cheapest-first):
///   1. in-hunk anchor (added lines)
///   2. proximity anchor (≤ `proximity_lines` ABOVE the hunk in the post-change file)
///   3. file-level `//!` anchor anywhere in the file
///   4. commit-trailer fallback (the floor)
///      `post_change_lines` is the full post-change file (1-based logical, passed as a
///      slice). PURE — resolver + content injected, no I/O. trace:STORY-499 | ai:claude
pub(crate) fn attribute_hunk_coverage<F>(
    hunk: &ParsedHunk,
    post_change_lines: &[String],
    commit_trailer_covered: bool,
    proximity_lines: usize,
    re: &regex::Regex,
    resolve: &mut F,
) -> Option<CoverageSource>
where
    F: FnMut(&str) -> SpecResolution,
{
    // 1. In-hunk anchor.
    if hunk
        .added
        .iter()
        .any(|l| coverage_line_has_live_anchor(l, re, resolve))
    {
        return Some(CoverageSource::InHunkAnchor);
    }

    // 2. Proximity anchor — scan ABOVE-only (§4.2 rule 2 + risk #2). The hunk
    // starts at 1-based `new_start`; lines above are indices [start-1-N, start-1)
    // in the 0-based file vector.
    if hunk.new_start >= 1 {
        let start_idx = hunk.new_start.saturating_sub(1); // 0-based first hunk line
        let from = start_idx.saturating_sub(proximity_lines);
        for line in post_change_lines.iter().take(start_idx).skip(from) {
            if coverage_line_has_live_anchor(line, re, resolve) {
                return Some(CoverageSource::ProximityAnchor);
            }
        }
    }

    // 3. File-level `//!` module anchor anywhere in the file.
    if post_change_lines
        .iter()
        .any(|l| l.trim_start().starts_with("//!") && coverage_line_has_live_anchor(l, re, resolve))
    {
        return Some(CoverageSource::FileAnchor);
    }

    // 4. Commit-trailer fallback — the floor.
    if commit_trailer_covered {
        return Some(CoverageSource::CommitTrailer);
    }

    None
}

/// The PURE, fully-isolated core of the coverage gate. Given parsed hunks, the
/// post-change content of each touched file, a per-file generated-header probe,
/// a per-hunk commit-trailer-covered flag, and an anchor resolver, classify
/// every hunk (exempt / covered / uncovered-coverable) per the SPIKE-47
/// definition. NO git, NO store, NO I/O — every dependency is an argument.
///
/// `files`: post-change line content keyed by path (the file the hunk lives in).
/// `generated_headers`: first-lines blob per path (for the §3.2 generated-marker
///   probe); absent ⇒ probe skipped.
/// `commit_trailer_covered`: per-file flag — does the file's commit carry a live
///   `(SPEC-ID)` trailer? (the §4.2.4 floor).
/// trace:STORY-499 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn compute_diff_coverage<F>(
    hunks: &[ParsedHunk],
    files: &std::collections::HashMap<String, Vec<String>>,
    generated_headers: &std::collections::HashMap<String, String>,
    commit_trailer_covered: &std::collections::HashMap<String, bool>,
    proximity_lines: usize,
    trivial_max_lines: usize,
    mut resolve: F,
) -> DiffCoverage
where
    F: FnMut(&str) -> SpecResolution,
{
    let re = coverage_anchor_re();
    let mut out = Vec::with_capacity(hunks.len());
    for hunk in hunks {
        let added_lines = hunk.added.len();

        // File-level exemption first (cheapest; whole file dropped).
        if let Some(reason) = classify_coverage_file(
            &hunk.file,
            generated_headers.get(&hunk.file).map(|s| s.as_str()),
        ) {
            out.push(HunkVerdict {
                file: hunk.file.clone(),
                new_start: hunk.new_start,
                added_lines,
                exempt: Some(reason),
                covered: None,
            });
            continue;
        }

        // Hunk-level exemption.
        if let Some(reason) = classify_coverage_hunk(hunk, trivial_max_lines) {
            out.push(HunkVerdict {
                file: hunk.file.clone(),
                new_start: hunk.new_start,
                added_lines,
                exempt: Some(reason),
                covered: None,
            });
            continue;
        }

        // Coverable — attribute coverage.
        let empty = Vec::new();
        let post = files.get(&hunk.file).unwrap_or(&empty);
        let trailer_covered = commit_trailer_covered
            .get(&hunk.file)
            .copied()
            .unwrap_or(false);
        let covered = attribute_hunk_coverage(
            hunk,
            post,
            trailer_covered,
            proximity_lines,
            &re,
            &mut resolve,
        );
        out.push(HunkVerdict {
            file: hunk.file.clone(),
            new_start: hunk.new_start,
            added_lines,
            exempt: None,
            covered,
        });
    }
    DiffCoverage { hunks: out }
}

/// Parse `git diff --unified=0` output into `ParsedHunk`s. PURE (takes the raw
/// diff text). Tracks the current `+++ b/<path>` file and each `@@ -a,b +c,d @@`
/// header to assign `new_start` and split `+`/`-` line bodies. Skips
/// `/dev/null` (deleted-file) targets. trace:STORY-499 | ai:claude
pub(crate) fn parse_unified_diff_hunks(diff: &str) -> Vec<ParsedHunk> {
    let hunk_re = regex::Regex::new(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@").unwrap();
    let mut hunks: Vec<ParsedHunk> = Vec::new();
    let mut current_file: Option<String> = None;
    let mut cur: Option<ParsedHunk> = None;

    let flush = |cur: &mut Option<ParsedHunk>, hunks: &mut Vec<ParsedHunk>| {
        if let Some(h) = cur.take() {
            if !h.added.is_empty() || !h.removed.is_empty() {
                hunks.push(h);
            }
        }
    };

    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            flush(&mut cur, &mut hunks);
            let path = rest.trim();
            current_file = if path == "/dev/null" {
                None
            } else {
                // Strip the `b/` prefix git prepends.
                Some(path.strip_prefix("b/").unwrap_or(path).to_string())
            };
            continue;
        }
        if line.starts_with("--- ") || line.starts_with("diff --git") {
            continue;
        }
        if let Some(cap) = hunk_re.captures(line) {
            flush(&mut cur, &mut hunks);
            if let Some(file) = &current_file {
                let new_start: usize = cap[1].parse().unwrap_or(1);
                cur = Some(ParsedHunk {
                    file: file.clone(),
                    new_start,
                    added: Vec::new(),
                    removed: Vec::new(),
                });
            }
            continue;
        }
        if let Some(h) = cur.as_mut() {
            if let Some(body) = line.strip_prefix('+') {
                h.added.push(body.to_string());
            } else if let Some(body) = line.strip_prefix('-') {
                h.removed.push(body.to_string());
            }
        }
    }
    flush(&mut cur, &mut hunks);
    hunks
}

/// Read the unified diff for a range, `git diff -M -w --unified=0 <base>..HEAD`.
/// `-M` enables rename detection (renamed files emit no content hunks → §3.3 H3),
/// `-w` ignores whitespace so fmt-only reflows produce no hunks (§3.3 H2).
/// trace:STORY-499 | ai:claude
pub(crate) fn read_diff_for_range(project_root: &std::path::Path, range: &str) -> Result<String> {
    use std::process::Command as PCmd;
    let output = PCmd::new("git")
        .arg("-C")
        .arg(project_root)
        // trace:BUG-1622 | ai:claude
        .args([
            "diff",
            "-M",
            "-w",
            "--unified=0",
            git_arg_guard::END_OF_OPTIONS,
            range,
            "--",
        ])
        .output();
    match output {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).to_string()),
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            anyhow::bail!("git diff failed for range `{range}`: {}", err.trim());
        }
        Err(e) => anyhow::bail!("could not run git: {e}"),
    }
}

/// Read the post-change content of a file at HEAD (or the working tree) as a
/// line vector, for the proximity / file-level anchor scan (§4.2). Tries the
/// on-disk file first (the merged tree in CI checkout), falling back to
/// `git show HEAD:<path>`. Returns an empty vec when neither resolves (the hunk
/// can still be covered by an in-hunk anchor or the commit trailer).
/// trace:STORY-499 | ai:claude
pub(crate) fn read_post_change_file(project_root: &std::path::Path, path: &str) -> Vec<String> {
    let on_disk = project_root.join(path);
    if let Ok(content) = std::fs::read_to_string(&on_disk) {
        return content.lines().map(|l| l.to_string()).collect();
    }
    use std::process::Command as PCmd;
    if let Ok(o) = PCmd::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["show", &format!("HEAD:{path}")])
        .output()
    {
        if o.status.success() {
            return String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.to_string())
                .collect();
        }
    }
    Vec::new()
}

/// For each touched file, determine whether ANY commit in `range` that touches
/// it carries a live `(SPEC-ID)` trailer (the §4.2.4 floor). Conservative and
/// cheap: if any non-plan commit referencing a live spec touched the file, its
/// hunks get the trailer-floor. Returns a per-file flag map. trace:STORY-499 | ai:claude
pub(crate) fn compute_trailer_floor<F>(
    project_root: &std::path::Path,
    range: &str,
    files: &[String],
    resolve: &mut F,
) -> std::collections::HashMap<String, bool>
where
    F: FnMut(&str) -> SpecResolution,
{
    use std::process::Command as PCmd;
    let mut out = std::collections::HashMap::new();
    for file in files {
        // Commits in range that touched this file, with subjects.
        let covered = PCmd::new("git")
            .arg("-C")
            .arg(project_root)
            // trace:BUG-1622 | ai:claude
            .args([
                "log",
                "--no-merges",
                "--pretty=format:%s",
                git_arg_guard::END_OF_OPTIONS,
                range,
                "--",
                file,
            ])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                String::from_utf8_lossy(&o.stdout).lines().any(|subject| {
                    if is_plan_commit_subject(subject) {
                        return false;
                    }
                    extract_spec_ids_from_commit(subject)
                        .iter()
                        .any(|id| matches!(resolve(id), SpecResolution::Live))
                })
            })
            .unwrap_or(false);
        out.insert(file.clone(), covered);
    }
    out
}

/// CLI handler for `aida trace coverage`. Gathers the diff + post-change files +
/// commit-trailer floor + live-graph resolver from git/the store, runs the pure
/// `compute_diff_coverage` core, prints a report, and exits non-zero ONLY when
/// `block` posture is active and a coverable hunk is uncovered. Default posture
/// is report-only (CI-success regardless) per SPIKE-47 §5/§6. trace:STORY-499 | ai:claude
pub(crate) fn handle_trace_coverage(range: Option<&str>, json: bool, block: bool) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    // trace:BUG-1622 | ai:claude
    if let Some(r) = range {
        git_arg_guard::reject_option_like("--range", r)?;
    }
    let range = resolve_gate_range(&project_root, range);

    let diff = read_diff_for_range(&project_root, &range)?;
    let hunks = parse_unified_diff_hunks(&diff);

    // The live requirement graph (for anchor/trailer resolution).
    let store = load_store_for_lookup(&project_root);
    if store.is_none() {
        eprintln!(
            "{} trace coverage: no requirement store reachable from {} — anchor/trailer ids cannot \
             be confirmed live (every id is treated as live; coverage may be over-counted). Run \
             where the store is attached (`aida cache rebuild`).",
            crate::glyph(crate::glyphs::Glyph::Warning),
            project_root.display()
        );
    }

    // Distinct touched files (post-change paths).
    let mut touched: Vec<String> = hunks.iter().map(|h| h.file.clone()).collect();
    touched.sort();
    touched.dedup();

    // Post-change content + generated-marker headers per file.
    let mut files = std::collections::HashMap::new();
    let mut generated_headers = std::collections::HashMap::new();
    for path in &touched {
        let lines = read_post_change_file(&project_root, path);
        let header: String = lines.iter().take(5).cloned().collect::<Vec<_>>().join("\n");
        generated_headers.insert(path.clone(), header);
        files.insert(path.clone(), lines);
    }

    // Commit-trailer floor per file.
    let mut floor_resolve =
        |id: &str| -> SpecResolution { resolve_spec_in_store(store.as_ref(), id) };
    let trailer_floor = compute_trailer_floor(&project_root, &range, &touched, &mut floor_resolve);

    let resolve = |id: &str| -> SpecResolution { resolve_spec_in_store(store.as_ref(), id) };
    let coverage = compute_diff_coverage(
        &hunks,
        &files,
        &generated_headers,
        &trailer_floor,
        5, // proximity_lines (SPIKE-47 §4.2 default N=5)
        1, // trivial_max_lines (SPIKE-47 §3.3 H4 default)
        resolve,
    );

    let uncovered = coverage.uncovered();
    let posture = if block { "block" } else { "report" };

    // Audited exemption breakdown — SPIKE-47 §3 requires every exemption to be
    // reported (never silently dropped). trace:STORY-499 | ai:claude
    let mut exempt_breakdown: std::collections::BTreeMap<&'static str, usize> =
        std::collections::BTreeMap::new();
    for h in &coverage.hunks {
        if let Some(reason) = h.exempt {
            *exempt_breakdown.entry(reason.reason()).or_insert(0) += 1;
        }
    }

    if json {
        let payload = serde_json::json!({
            "range": range,
            "posture": posture,
            "hunks_total": coverage.hunks.len(),
            "coverable": coverage.coverable(),
            "covered": coverage.covered(),
            "exempt": coverage.exempt(),
            "exempt_breakdown": exempt_breakdown,
            "uncovered": uncovered.len(),
            "ratio": coverage.ratio(),
            "uncovered_hunks": uncovered,
            "ok": uncovered.is_empty(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!(
            "trace coverage [{posture}]: {} hunk(s) in `{}` — {} coverable, {} covered, {} exempt \
             (ratio {:.0}%)",
            coverage.hunks.len(),
            range,
            coverage.coverable(),
            coverage.covered(),
            coverage.exempt(),
            coverage.ratio() * 100.0,
        );
        if !exempt_breakdown.is_empty() {
            let summary = exempt_breakdown
                .iter()
                .map(|(reason, n)| format!("{n} {reason}"))
                .collect::<Vec<_>>()
                .join(", ");
            println!("  exempt: {summary}");
        }
        if uncovered.is_empty() {
            println!(
                "{} every coverable changed hunk carries trace coverage",
                crate::glyph(crate::glyphs::Glyph::Check)
            );
        } else {
            eprintln!(
                "{} {} uncovered coverable hunk(s):",
                crate::glyph(crate::glyphs::Glyph::Warning),
                uncovered.len()
            );
            for h in &uncovered {
                eprintln!(
                    "  {}:{} (+{} line(s)) — no // trace: anchor and no live commit trailer",
                    h.file, h.new_start, h.added_lines
                );
            }
            eprintln!(
                "\nAdd a `// trace:<SPEC-ID>` near the change (or ensure the commit carries a live \
                 `(SPEC-ID)` trailer). Tests / generated / docs / config / trivial changes are \
                 exempt automatically."
            );
        }
    }

    // Report-only by default (CI succeeds); only `block` posture exits non-zero.
    if block && !uncovered.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

// ============================================================================
// TASK-939: diff-driven doc nudge at PR-open — `aida doc suggest`.
//
// `aida doc coverage` is the release-time backstop (every Completed spec should
// have a Doc). This is the front-of-the-pipeline nudge: at PR-open, when the
// branch ADDS new public surface (a CLI flag, a named CLI subcommand, or an MCP
// tool) and the spec that surface traces to has no Doc entry about it, remind
// the author to capture one while the "why" is fresh. Warn-only — `/aida-pr`
// calls it and it never blocks the PR (exit 0 always).
//
// Reuses the STORY-499 diff machinery (`parse_unified_diff_hunks`,
// `read_diff_for_range`, `resolve_gate_range`, `coverage_anchor_re`) and the
// same `Doc References` documented-set logic as `find_uncovered_completed_specs`
// (TASK-680). The detection core is pure and unit-tested. trace:TASK-939
// ============================================================================

/// A kind of newly-added public CLI/MCP surface detected in a diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PublicSurfaceKind {
    /// A new clap flag (an `#[arg(long…)]` / `#[clap(long…)]` attribute).
    CliFlag,
    /// A new named clap subcommand (an `#[command(name = …)]` attribute).
    CliSubcommand,
    /// A new MCP tool descriptor (`"name": "<snake_case>"` in `mcp.rs`).
    McpTool,
}

impl PublicSurfaceKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            PublicSurfaceKind::CliFlag => "CLI flag",
            PublicSurfaceKind::CliSubcommand => "CLI subcommand",
            PublicSurfaceKind::McpTool => "MCP tool",
        }
    }
}

/// One added line the classifier flagged as new public surface.
// trace:TASK-939 | ai:claude
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub(crate) struct SurfaceHit {
    pub(crate) file: String,
    /// 1-based start line of the containing hunk in the post-change file.
    pub(crate) new_start: usize,
    pub(crate) kind: PublicSurfaceKind,
    /// The trimmed added line that matched (for the human report).
    pub(crate) snippet: String,
}

/// True when `word` occurs in `hay` bounded by non-identifier chars on both
/// sides (so `long` matches in `#[clap(long)]` but NOT inside `long_help`).
// trace:TASK-939 | ai:claude
pub(crate) fn contains_ident_word(hay: &str, word: &str) -> bool {
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let bytes = hay.as_bytes();
    let mut from = 0;
    while let Some(pos) = hay[from..].find(word) {
        let start = from + pos;
        let end = start + word.len();
        let before_ok = start == 0 || !is_ident(bytes[start - 1]);
        let after_ok = end >= bytes.len() || !is_ident(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

/// An added line that declares a new clap long flag: an `#[arg(…)]` /
/// `#[clap(…)]` attribute carrying a word-boundaried `long`. Excludes
/// `long_help` / `long_about` (not a flag) and `#[clap(subcommand)]` (no
/// `long`). PURE.
// trace:TASK-939 | ai:claude
pub(crate) fn is_clap_long_flag(line: &str) -> bool {
    let t = line.trim_start();
    if !t.starts_with("#[") {
        return false;
    }
    if !(t.contains("clap(") || t.contains("arg(")) {
        return false;
    }
    contains_ident_word(t, "long")
}

/// An added line that declares a *named* clap subcommand: an `#[command(…)]` /
/// `#[clap(…)]` attribute carrying `name = "…"`. (Unit-variant subcommands
/// that derive their name from the variant carry no attribute and are not
/// detected here — flags added alongside them usually are.) PURE.
// trace:TASK-939 | ai:claude
pub(crate) fn is_clap_named_subcommand(line: &str) -> bool {
    let t = line.trim_start();
    if !t.starts_with("#[") {
        return false;
    }
    if !(t.contains("clap(") || t.contains("command(")) {
        return false;
    }
    contains_ident_word(t, "name") && t.contains('=')
}

/// Extract the tool name from an MCP tool descriptor line — `"name": "<value>"`
/// where `<value>` is a snake_case identifier. Returns `None` for Title-case
/// resource titles (`"Project Summary"`) or non-descriptor lines. PURE.
// trace:TASK-939 | ai:claude
pub(crate) fn mcp_tool_name_in_line(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix("\"name\"")?;
    let rest = rest.trim_start().strip_prefix(':')?;
    let rest = rest.trim_start().strip_prefix('"')?;
    let end = rest.find('"')?;
    let val = &rest[..end];
    let ident = !val.is_empty()
        && val.starts_with(|c: char| c.is_ascii_lowercase())
        && val
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    ident.then_some(val)
}

/// Classify a single ADDED diff line as new public surface, or `None`. PURE —
/// the `file` path only selects the MCP-tool probe (limited to `mcp.rs`).
// trace:TASK-939 | ai:claude
pub(crate) fn classify_surface_line(file: &str, line: &str) -> Option<PublicSurfaceKind> {
    // MCP tool descriptor — only in the MCP surface file. The server-name line
    // (`"name": "aida"`) is excluded so it never reads as a new tool.
    if file.ends_with("mcp.rs") {
        if let Some(name) = mcp_tool_name_in_line(line) {
            if name != "aida" {
                return Some(PublicSurfaceKind::McpTool);
            }
        }
    }
    if is_clap_named_subcommand(line) {
        return Some(PublicSurfaceKind::CliSubcommand);
    }
    if is_clap_long_flag(line) {
        return Some(PublicSurfaceKind::CliFlag);
    }
    None
}

/// Scan parsed diff hunks for newly-added public surface. PURE — the testable
/// core of `aida doc suggest`.
// trace:TASK-939 | ai:claude
pub(crate) fn detect_new_public_surface(hunks: &[ParsedHunk]) -> Vec<SurfaceHit> {
    let mut hits = Vec::new();
    for h in hunks {
        for line in &h.added {
            if let Some(kind) = classify_surface_line(&h.file, line) {
                hits.push(SurfaceHit {
                    file: h.file.clone(),
                    new_start: h.new_start,
                    kind,
                    snippet: line.trim().to_string(),
                });
            }
        }
    }
    hits
}

/// Collect the spec ids this PR references — commit-trailer ids across `range`
/// (plan commits skipped) plus inline `trace:` anchors in the added lines.
/// These are the ids a Doc entry would be `--about`.
// trace:TASK-939 | ai:claude
pub(crate) fn specs_referenced_in_range(
    project_root: &std::path::Path,
    range: &str,
    hunks: &[ParsedHunk],
) -> Vec<String> {
    use std::process::Command as PCmd;
    let mut ids: Vec<String> = Vec::new();

    if let Ok(out) = PCmd::new("git")
        .arg("-C")
        .arg(project_root)
        // trace:BUG-1622 | ai:claude
        .args([
            "log",
            "--no-merges",
            "--pretty=format:%s",
            git_arg_guard::END_OF_OPTIONS,
            range,
            "--",
        ])
        .output()
    {
        if out.status.success() {
            for subject in String::from_utf8_lossy(&out.stdout).lines() {
                if is_plan_commit_subject(subject) {
                    continue;
                }
                ids.extend(extract_spec_ids_from_commit(subject));
            }
        }
    }

    let re = coverage_anchor_re();
    for h in hunks {
        for line in &h.added {
            for cap in re.captures_iter(line) {
                ids.push(cap[1].to_string());
            }
        }
    }

    ids.sort();
    ids.dedup();
    ids
}

/// CLI handler for `aida doc suggest`. Reads the branch diff, detects new public
/// surface, resolves the referenced specs against the live graph, and nudges
/// `aida doc add --about <ID>` for any that carry no Doc entry. Warn-only —
/// always exits 0.
// trace:TASK-939 | ai:claude
pub(crate) fn handle_doc_suggest(range: Option<&str>, json: bool) -> Result<()> {
    use aida_core::models::{RelationshipType, RequirementStatus, RequirementType};

    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    // trace:BUG-1622 | ai:claude
    if let Some(r) = range {
        git_arg_guard::reject_option_like("--range", r)?;
    }
    let range = resolve_gate_range(&project_root, range);

    let diff = read_diff_for_range(&project_root, &range)?;
    let hunks = parse_unified_diff_hunks(&diff);
    let surface = detect_new_public_surface(&hunks);
    let referenced = specs_referenced_in_range(&project_root, &range, &hunks);

    // The live requirement graph — for the documented-set check.
    let store = load_store_for_lookup(&project_root);

    // Set of spec uuids referenced by at least one Doc.
    let mut documented: std::collections::HashSet<uuid::Uuid> = std::collections::HashSet::new();
    if let Some(s) = store.as_ref() {
        for doc in s
            .requirements
            .iter()
            .filter(|r| r.req_type == RequirementType::Doc)
        {
            for rel in &doc.relationships {
                if rel.rel_type == RelationshipType::References {
                    documented.insert(rel.target_id);
                }
            }
        }
    }

    // Partition the referenced specs into gaps (undocumented, live, non-Doc)
    // and already-documented.
    let mut gaps: Vec<(String, String)> = Vec::new();
    let mut documented_ids: Vec<String> = Vec::new();
    if let Some(s) = store.as_ref() {
        for cid in &referenced {
            let want = cid.to_ascii_uppercase();
            let found = s.requirements.iter().find(|r| {
                r.spec_id
                    .as_deref()
                    .is_some_and(|x| x.eq_ignore_ascii_case(&want))
                    || r.agreed_id
                        .as_deref()
                        .is_some_and(|x| x.eq_ignore_ascii_case(&want))
            });
            match found {
                // A Doc, a rejected spec, or an unresolvable id is not a target.
                Some(r) if r.req_type == RequirementType::Doc => {}
                Some(r) if matches!(r.status, RequirementStatus::Rejected) => {}
                Some(r) if r.archived => {}
                Some(r) => {
                    if documented.contains(&r.id) {
                        documented_ids.push(r.display_id());
                    } else {
                        gaps.push((r.display_id(), r.title.clone()));
                    }
                }
                None => {}
            }
        }
    }

    let has_surface = !surface.is_empty();
    // "ok" = nothing to nudge: either no new surface, or every referenced spec
    // already carries a doc.
    let ok = !has_surface || gaps.is_empty();

    if json {
        let surface_rows: Vec<serde_json::Value> = surface
            .iter()
            .map(|h| {
                serde_json::json!({
                    "file": h.file,
                    "line": h.new_start,
                    "kind": h.kind,
                    "snippet": h.snippet,
                })
            })
            .collect();
        let gap_rows: Vec<serde_json::Value> = gaps
            .iter()
            .map(|(id, title)| serde_json::json!({ "id": id, "title": title }))
            .collect();
        let payload = serde_json::json!({
            "range": range,
            "surface_count": surface.len(),
            "surface": surface_rows,
            "referenced_specs": referenced,
            "documented_specs": documented_ids,
            "gaps": gap_rows,
            "ok": ok,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    if !has_surface {
        println!(
            "{} No new CLI/MCP surface added in `{}` — nothing to capture.",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            range
        );
        return Ok(());
    }

    println!(
        "{} New public surface added in `{}`:",
        crate::glyph(crate::glyphs::Glyph::Info).cyan(),
        range
    );
    for h in &surface {
        println!(
            "  {} · {}:{} · {}",
            h.kind.label().cyan(),
            h.file,
            h.new_start,
            h.snippet
        );
    }
    println!();

    if !gaps.is_empty() {
        println!(
            "{} Referenced spec(s) with no doc entry:",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        );
        for (id, title) in &gaps {
            println!("  {} · {}", id.cyan(), title);
        }
        println!();
        println!("Capture the \"why\" while it's fresh:");
        for (id, _) in &gaps {
            println!("  aida doc add --title \"…\" --about {}", id);
        }
        println!("(warn-only — this nudge does not block the PR)");
    } else if !documented_ids.is_empty() {
        println!(
            "{} Referenced spec(s) already have doc entries — nothing to capture.",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
    } else {
        println!(
            "{} Couldn't resolve which spec this surface belongs to (no live commit trailer or \
             `trace:` anchor). Capture docs with:",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        );
        println!("  aida doc add --title \"…\" --about <SPEC-ID>");
        println!("(warn-only — this nudge does not block the PR)");
    }

    Ok(())
}

/// Extract a trailing `(#N)` PR-number suffix from a commit subject (the
/// shape `gh` writes when squash-merging a PR). Returns None when the
/// subject doesn't end with `(#<digits>)`. trace:BUG-102 | ai:claude
pub(crate) fn extract_pr_number_from_commit_subject(message: &str) -> Option<u64> {
    let subject = message.lines().find(|l| !l.trim().is_empty())?;
    let trimmed = subject.trim();
    if !trimmed.ends_with(')') {
        return None;
    }
    let open_at = trimmed.rfind('(')?;
    let inner = &trimmed[open_at + 1..trimmed.len() - 1];
    let digits = inner.strip_prefix('#')?;
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u64>().ok()
}

/// True when `subject` references any of `ids` as a real spec-completion
/// reference — i.e. the id appears in the commit's trailing `(REQ-ID)` group
/// or leading `SPEC-ID:` prefix, the same shapes the auto-bump scan honours
/// (`extract_spec_ids_from_commit`). Plan commits (`docs(plans): …`) are NOT a
/// completion signal, so they never count even when they name the id.
///
/// This is the testable core of TASK-579: when a finding is promoted, its
/// origin-ID may already have a merged fix referencing it (the fix shipped
/// against the id the finding carried *before* it became real work). Matching
/// is case-insensitive so `task-1-097` in a subject matches `TASK-1-097`.
/// trace:TASK-579 | ai:claude
pub(crate) fn commit_subject_references_id(subject: &str, ids: &[String]) -> bool {
    if is_plan_commit_subject(subject) {
        return false;
    }
    let referenced = extract_spec_ids_from_commit(subject);
    ids.iter().any(|wanted| {
        referenced
            .iter()
            .any(|got| got.eq_ignore_ascii_case(wanted))
    })
}

/// Scan the default branch's recent git log for a merged commit that references
/// any of `ids` (a finding's spec-id + agreed-id). Returns the first matching
/// `(short_sha, subject)` — first as in newest-first git-log order, so the most
/// recent landing wins. Returns `None` when no merged commit references the ids,
/// when there's no resolvable default branch, or when git is unavailable.
///
/// Used by `aida findings promote` so a finding whose underlying fix already
/// shipped against its origin-ID can warn / auto-complete instead of being
/// queued as fresh (no-op) work. trace:TASK-579 | ai:claude
pub(crate) fn find_merged_commit_referencing_ids(
    project_root: &std::path::Path,
    ids: &[String],
) -> Option<(String, String)> {
    use std::process::Command as ProcessCommand;

    if ids.is_empty() {
        return None;
    }

    // Resolve a default-branch ref to scan. Reuse the shared resolver so this
    // matches `aida db reconcile-status` / auto-bump branch semantics: prefer
    // origin/HEAD, fall back to origin/main|master then local main|master.
    let branch_ref = resolve_default_branch_ref(project_root)?;

    let log_out = ProcessCommand::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "log",
            "--max-count=500",
            "--pretty=format:%h%x09%s",
            &branch_ref,
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let log_str = String::from_utf8_lossy(&log_out.stdout);

    for line in log_str.lines() {
        let mut parts = line.splitn(2, '\t');
        let sha = parts.next().unwrap_or("").trim();
        let subject = parts.next().unwrap_or("").trim();
        if sha.is_empty() || subject.is_empty() {
            continue;
        }
        if commit_subject_references_id(subject, ids) {
            return Some((sha.to_string(), subject.to_string()));
        }
    }
    None
}

/// Parse the PR number out of a review-story title. Titles are filed by
/// /aida-pr's auto-queue as `Review PR-<n>: <pr-title>` (see
/// `aida_subcmd_add_review_story` callsite); anything not matching that
/// shape returns None. trace:BUG-102 | ai:claude
pub(crate) fn parse_review_story_pr_number(title: &str) -> Option<u64> {
    let rest = title.strip_prefix("Review PR-")?;
    let (num, _) = rest.split_once(':')?;
    num.trim().parse::<u64>().ok()
}

/// Pull spec-id-shaped `(REQ-ID)` parens from BODY lines of a commit message
/// (i.e. everything after the subject). These are informational
/// "referenced" specs — the commit's code touches their areas but doesn't
/// deliver them per AIDA's "one (REQ-ID) trailer in the subject" convention.
/// IDs already present in the subject are excluded so the two lists are
/// disjoint. trace:BUG-85 | ai:claude
/// BUG-412: heuristic — does a commit-body line look like CODE rather than a
/// prose/trace reference? Used to skip pasted snippets so `(PREFIX-NNN)` literals
/// inside them aren't mined as bogus "referenced" specs. Conservative: only the
/// markers that strongly imply code and don't appear in genuine reference lines
/// (`trace:SPEC-ID`, `(SPEC-ID)` in prose, `- (SPEC-ID) …` bullets).
///
/// BUG-1590: a bare `t.contains(';')` was WAY too broad — AIDA's own
/// integration-batch commit convention writes each folded spec as
/// `- SPEC-ID: <clause>; <clause> (SPEC-ID)`, a prose sentence with a
/// mid-line semicolon joining two clauses, terminated by the completion
/// trailer. That line contains a `;` but is not code, and the old check
/// silently discarded it from the squash-body scan — most lines of a
/// multi-spec squash body were dropped, exactly the shape the integration
/// workflow produces (observed: only 1-2 of 6-7 trailers survived per
/// batch). Real pasted-code semicolons are STATEMENT TERMINATORS — the
/// line ends in `;` (e.g. `let x = compute(CODE-42);`) — so the signal is
/// narrowed to that shape: `ends_with(';')`, not `contains(';')`. A line
/// that merely mentions a semicolon mid-sentence before its trailing
/// `(SPEC-ID)` still counts as a trailer.
// trace:BUG-1590 | ai:claude
pub(crate) fn body_line_is_code_like(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    // Structural code punctuation that prose references don't use. `;` only
    // counts when it TERMINATES the line (a real code statement), not when
    // it merely appears mid-sentence ahead of a trailing `(SPEC-ID)`.
    if t.contains('{')
        || t.contains('}')
        || t.ends_with(';')
        || t.contains("=>")
        || t.contains("::")
    {
        return true;
    }
    // An assignment `=` (but not `==`/`!=`/`<=`/`>=` comparisons or `(SPEC-ID)`
    // prose). A lone `=` surrounded by spaces, or `x =`, signals code.
    if t.contains(" = ") || t.contains("=(") || t.contains(")=") {
        return true;
    }
    // Leading code keywords.
    const KW: [&str; 9] = [
        "fn ", "pub ", "const ", "let ", "enum ", "struct ", "impl ", "use ", "static ",
    ];
    KW.iter().any(|k| t.starts_with(k))
}

/// BUG-546: harvest every spec-id named in a `trace:SPEC-ID` token anywhere in
/// a commit message — the authoritative provenance marker AIDA writes in code
/// comments and commit bodies. The `(SPEC-ID)` paren parser misses these; when
/// a PR's subject paren is non-standard (`(A / B slice 1a)`) the body's
/// `trace:` lines are the reliable spec↔PR link. Handles `trace:` and
/// `trace: ` (with/without a space), comma/whitespace-separated runs
/// (`trace:TASK-800 trace:STORY-610` or `trace:TASK-800,STORY-610`), and the
/// `relates:`/`blocks:` qualifier forms (`trace:relates:STORY-611`) — the
/// trailing `<ALPHA>-<DIGITS>` token is taken in each case. trace:BUG-546
pub(crate) fn extract_trace_line_spec_ids(message: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in message.split("trace:").skip(1) {
        // Consume consecutive spec-id-shaped tokens after this `trace:`,
        // tolerating `relates:`/`blocks:`/etc. qualifier prefixes whose final
        // colon-segment is the id.
        for tok in raw.split(|c: char| c.is_whitespace() || c == ',') {
            let tok = tok.trim().trim_end_matches([')', '.', ';', ':']);
            // A qualifier like `relates:STORY-611` → take the last segment.
            let candidate = tok.rsplit(':').next().unwrap_or(tok);
            if looks_like_spec_id(candidate) {
                if !out.iter().any(|x| x.eq_ignore_ascii_case(candidate)) {
                    out.push(candidate.to_string());
                }
            } else if tok.is_empty() {
                continue;
            } else {
                // First non-id token after a `trace:` ends this run — the rest
                // is prose (`trace:BUG-546 | ai:claude` stops at `|`).
                break;
            }
        }
    }
    out
}

/// Which commit-body SHAPES count as a completion trailer:
/// every non-blank, non-subject body line whose TRAILING paren group
/// starts with a spec-id-shaped token — `- SPEC-ID: <prose>; <more prose>
/// (SPEC-ID)`, `* [AI:tool] fix(scope): thing (SPEC-ID)`, or bare
/// `<prose> (SPEC-ID)` — counts, one trailer per line, regardless of
/// interior punctuation (a mid-sentence `;` does NOT disqualify a line;
/// only a line that structurally looks like pasted code does — see
/// `body_line_is_code_like`). A line whose trailing group does not START
/// with a spec-id (release-note prose like `(scope)`, `(1.2.3)`) or a
/// code-like line (`{`/`}`/line-terminating `;`/`=>`/`::`/assignment `=`)
/// contributes nothing. Ids already delivered by the commit's SUBJECT
/// trailer are excluded (BUG-85) so a lead spec named in both places is
/// not double-reported.
// trace:BUG-1590 | ai:claude
pub(crate) fn extract_referenced_spec_ids_from_commit(message: &str) -> Vec<String> {
    let delivered = extract_spec_ids_from_commit(message);
    let mut lines = message.lines();
    // Skip blank prefix + subject line.
    let _subject = lines.by_ref().find(|l| !l.trim().is_empty());
    let mut raw: Vec<String> = Vec::new();
    for line in lines {
        // BUG-412: skip code-like body lines so a `(PREFIX-NNN)` literal inside a
        // pasted code snippet (e.g. `enum Status { OK = (STATUS-200) }`) isn't
        // mined as a bogus "referenced" spec on the auto-filed review story.
        // Genuine reference lines (trace:, prose, bullet lists) don't carry these
        // code markers. trace:BUG-412 | ai:claude
        if body_line_is_code_like(line) {
            continue;
        }
        push_paren_spec_ids_from_line(line, &mut raw);
    }
    let mut out: Vec<String> = Vec::new();
    for id in raw {
        if delivered.iter().any(|d| d.eq_ignore_ascii_case(&id)) {
            continue;
        }
        if out.iter().any(|x| x.eq_ignore_ascii_case(&id)) {
            continue;
        }
        out.push(id);
    }
    out
}

/// Walk one commit-message line for trailing `(REQ-ID[, REQ-ID...])`
/// groups and push the spec-id-shaped tokens into `out`. Skips `(#N)`
/// PR-number groups (squash commits). A group whose tokens aren't ALL
/// spec-id-shaped contributes nothing and stops the walk, so prose-y
/// parens like `(1.2.3)`, `(foo BUG-23)`, or a conventional-commit
/// `(scope)` don't false-match.
///
/// Collects EVERY consecutive trailing spec-id group, not just the last
/// one — a cluster-PR squash subject like `fix: x (BUG-503) (BUG-504)
/// (#794)` names one group per shipped spec, and both the `aida pull`
/// auto-bump scan and `aida db reconcile-status` must graduate each.
/// IDs are pushed in left-to-right subject order.
/// trace:BUG-78 BUG-85 | ai:claude
// trace:BUG-506 | ai:claude
pub(crate) fn push_paren_spec_ids_from_line(line: &str, out: &mut Vec<String>) {
    let mut tail: &str = line.trim();
    let mut collected: Vec<String> = Vec::new();
    while tail.ends_with(')') {
        let Some(open_at) = tail.rfind('(') else {
            break;
        };
        let inner = &tail[open_at + 1..tail.len() - 1];
        // `(#N)` PR-number group (squash commits): skip it and keep walking.
        if inner.starts_with('#')
            && inner.len() > 1
            && inner[1..].chars().all(|c| c.is_ascii_digit())
        {
            tail = tail[..open_at].trim_end();
            continue;
        }
        // Parse this group. BUG-546: a real trailer may carry MULTIPLE specs
        // plus prose — `(TASK-800 / STORY-610 slice 1a)` — and the old
        // all-tokens-must-be-ids rule extracted NEITHER spec, stranding the
        // auto-bump / reconcile / review surfaces that share this parser. The
        // discriminator that still rejects release-note prose like
        // `(foo BUG-23)` or `(scope)`: the group must START with a spec-id
        // token (the `(SPEC-ID …)` trailer convention). When it does, harvest
        // EVERY spec-id-shaped token and skip the separators / prose
        // (`/`, `slice`, `1a`, …); when it doesn't, the group contributes
        // nothing and the walk stops, exactly as before. trace:BUG-546
        let mut group: Vec<String> = Vec::new();
        let mut first_token_is_id: Option<bool> = None;
        for tok in inner.split(|c: char| c == ',' || c == '/' || c.is_whitespace()) {
            let tok = tok.trim();
            if tok.is_empty() {
                continue;
            }
            let is_id = looks_like_spec_id(tok);
            if first_token_is_id.is_none() {
                first_token_is_id = Some(is_id);
            }
            if is_id {
                group.push(tok.to_string());
            }
        }
        // Reject the group unless it led with a spec-id (so `(foo BUG-23)`,
        // `(scope)`, `(1.2.3)` still contribute nothing and stop the walk).
        if first_token_is_id != Some(true) || group.is_empty() {
            break;
        }
        // Prepend: we walk right-to-left, but `out` keeps subject order.
        collected.splice(0..0, group);
        tail = tail[..open_at].trim_end();
    }
    out.extend(collected);
}

pub(crate) fn looks_like_spec_id(s: &str) -> bool {
    let s = s.trim();
    if s.len() < 3 || s.len() > 40 {
        return false;
    }
    let mut parts = s.split('-');
    let Some(prefix) = parts.next() else {
        return false;
    };
    if prefix.len() < 2 || !prefix.chars().all(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    let tail: Vec<&str> = parts.collect();
    if tail.is_empty() {
        return false;
    }
    if !tail
        .iter()
        .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric()))
    {
        return false;
    }
    tail.last()
        .is_some_and(|part| part.chars().all(|c| c.is_ascii_digit()))
}

pub fn parse_pr_arg(arg: &str) -> Option<u32> {
    let s = arg.trim();
    if s.is_empty() {
        return None;
    }
    let s_lower = s.to_lowercase();
    let digits = if let Some(rest) = s_lower.strip_prefix("pr-") {
        rest
    } else if let Some(rest) = s_lower.strip_prefix("pr") {
        rest
    } else if let Some(rest) = s_lower.strip_prefix('#') {
        rest
    } else {
        s
    };
    if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
        digits.parse::<u32>().ok()
    } else {
        None
    }
}

pub(crate) fn extract_all_spec_ids(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let split_chars = |c: char| {
        c.is_whitespace()
            || c == '('
            || c == ')'
            || c == '['
            || c == ']'
            || c == '{'
            || c == '}'
            || c == ','
            || c == '.'
            || c == ':'
            || c == ';'
            || c == '/'
            || c == '\\'
            || c == '"'
            || c == '\''
    };
    for word in text.split(split_chars) {
        let trimmed = word.trim();
        if looks_like_spec_id(trimmed) {
            let canon = trimmed.to_uppercase();
            if !canon.starts_with("PR-") && !out.contains(&canon) {
                out.push(canon);
            }
        }
    }
    for id in extract_spec_ids_from_commit(text) {
        let canon = id.to_uppercase();
        if !canon.starts_with("PR-") && !out.contains(&canon) {
            out.push(canon);
        }
    }
    out
}

/// Is `ancestor` a transitive parent of `node`, per a `child → parent uuids`
/// map? trace:BUG-431 | ai:claude
pub(crate) fn is_transitive_ancestor(
    parents: &std::collections::HashMap<uuid::Uuid, Vec<uuid::Uuid>>,
    ancestor: uuid::Uuid,
    node: uuid::Uuid,
) -> bool {
    let mut stack: Vec<uuid::Uuid> = parents.get(&node).cloned().unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    while let Some(p) = stack.pop() {
        if p == ancestor {
            return true;
        }
        if seen.insert(p) {
            if let Some(gps) = parents.get(&p) {
                stack.extend(gps.iter().copied());
            }
        }
    }
    false
}

/// BUG-431 #2: when a PR backs several specs that form a parent chain (an epic
/// and its child story, say), the most-specific one IS the answer — the
/// reviewer must not bail "multiple backing specs." Drop any spec in the set
/// that is a proven transitive ANCESTOR of another spec in the set; keep the
/// rest. Specs absent from the store, or genuinely unrelated to the others,
/// are left intact, so the caller still bails on real ambiguity. Order-
/// preserving. trace:BUG-431 | ai:claude
pub(crate) fn reduce_to_most_specific_specs(
    store: &RequirementsStore,
    specs: &[String],
) -> Vec<String> {
    if specs.len() < 2 {
        return specs.to_vec();
    }
    let uuid_of = |s: &str| -> Option<uuid::Uuid> {
        store
            .requirements
            .iter()
            .find(|r| {
                r.spec_id
                    .as_deref()
                    .is_some_and(|x| x.eq_ignore_ascii_case(s))
                    || r.agreed_id
                        .as_deref()
                        .is_some_and(|x| x.eq_ignore_ascii_case(s))
            })
            .map(|r| r.id)
    };
    // child uuid → its parent uuids. RelationshipType::Parent means "this is
    // parent of target" (→ target's parent is this req); Child means "this is
    // child of target" (→ this req's parent is target). trace:BUG-431
    let mut parents: std::collections::HashMap<uuid::Uuid, Vec<uuid::Uuid>> =
        std::collections::HashMap::new();
    for req in &store.requirements {
        for rel in &req.relationships {
            match rel.rel_type {
                aida_core::RelationshipType::Parent => {
                    parents.entry(rel.target_id).or_default().push(req.id);
                }
                aida_core::RelationshipType::Child => {
                    parents.entry(req.id).or_default().push(rel.target_id);
                }
                _ => {}
            }
        }
    }
    let resolved: Vec<(String, Option<uuid::Uuid>)> =
        specs.iter().map(|s| (s.clone(), uuid_of(s))).collect();
    let mut out = Vec::new();
    for s in specs {
        let keep = match uuid_of(s) {
            None => true, // unresolved → can't prove ancestry → keep
            Some(a) => !resolved.iter().any(|(other, ou)| {
                other != s && ou.is_some_and(|ou| is_transitive_ancestor(&parents, a, ou))
            }),
        };
        if keep {
            out.push(s.clone());
        }
    }
    out
}

/// STORY-501 / BUG-440: is a non-terminal "Review PR-N" story currently queued
/// for this user? When one is, `aida queue work PR-N` must NOT be resolved to
/// the PR's backing spec at the dispatch (TASK-518) — it must reach
/// `resolve_queue_work_plan`'s review-story pickup (TASK-85) so it routes to the
/// reviewer (`/aida-review` on a PR-scoped lease) instead of an implementer
/// pickup that re-implements the spec. trace:STORY-501 | ai:claude
// BUG-1195: what looking up a pickable review story for one PR/MR found.
/// The variants are the diagnosis: a caller can tell "no story" (a genuine
/// gap) from "the lookup itself failed" (a transient store/queue read), and
/// the bail message names which check fell through.
// trace:BUG-1195 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReviewStoryLookup {
    /// A queued, pickable `Review PR-n:` story (its display id).
    Found(String),
    /// Nothing queued for this user or the reviewer role at all.
    NoEntries,
    /// Entries exist but none is a review story for this PR.
    NoTitleMatch { entries: usize },
    /// The review story exists but is not pickable (Done / terminal / blocked).
    NotPickable { story: String, reason: String },
    /// The queue or store could not be read — NOT evidence of a missing story.
    Failed(String),
}

impl ReviewStoryLookup {
    pub(crate) fn is_found(&self) -> bool {
        matches!(self, Self::Found(_))
    }
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Found(id) => format!("review story {id} is queued and pickable"),
            Self::NoEntries => "no queue entries for this user or the reviewer role".to_string(),
            Self::NoTitleMatch { entries } => format!(
                "{entries} queue entr{} but none is a review story for this change",
                if *entries == 1 { "y" } else { "ies" }
            ),
            Self::NotPickable { story, reason } => {
                format!("review story {story} is queued but not pickable: {reason}")
            }
            Self::Failed(cause) => format!("the review-story lookup failed: {cause}"),
        }
    }
}

// BUG-1195: pure classification over the entries the caller could see.
// trace:BUG-1195 | ai:claude
pub(crate) fn classify_review_story_lookup(
    entries: &[aida_core::QueueEntry],
    store: &RequirementsStore,
    forge: ReviewForge,
    n: u64,
) -> ReviewStoryLookup {
    if entries.is_empty() {
        return ReviewStoryLookup::NoEntries;
    }
    let mut not_pickable: Option<(String, String)> = None;
    for e in entries {
        let Some(req) = store.requirements.iter().find(|r| r.id == e.requirement_id) else {
            continue;
        };
        if !review_title_matches(&req.title, forge, n) {
            continue;
        }
        // trace:BUG-1515 | ai:claude
        // No filesystem access here — this classifier stays PURE (BUG-1195),
        // so the Done/AwaitingRework distinction (which reads a verdict file)
        // is unavailable; it degrades to the pre-BUG-1515 AwaitingMerge
        // reading rather than doing I/O from a pure function.
        let policy = queue_cmd::queue_fresh_pickup_policy(req, store, false, None);
        if matches!(policy, queue_cmd::QueueFreshPickup::Pickable) {
            return ReviewStoryLookup::Found(req.display_id());
        }
        if not_pickable.is_none() {
            not_pickable = Some((
                req.display_id(),
                queue_cmd::queue_fresh_pickup_reason_label(&policy)
                    .unwrap_or_else(|| format!("{policy:?}")),
            ));
        }
    }
    match not_pickable {
        Some((story, reason)) => ReviewStoryLookup::NotPickable { story, reason },
        None => ReviewStoryLookup::NoTitleMatch {
            entries: entries.len(),
        },
    }
}

/// Is a pickable `Review PR-n:` story queued for this user OR routed to the
// reviewer role by any user? BUG-1195: reads the queue through the same
/// role-fallback the plan-builder uses (`queue_list_with_role_fallback`), so
/// a story filed by a sibling worktree session under a different user id —
/// the BUG-1193 shape — is found here exactly as `queue work <STORY>` finds
/// it. Read failures surface as `Failed`, never as "no story".
// trace:BUG-1195 | ai:claude
pub(crate) fn queued_review_story_for_pr(
    storage: &Storage,
    user_id: &str,
    forge: ReviewForge,
    n: u64,
) -> ReviewStoryLookup {
    let entries = match queue_role_fallback::queue_list_with_role_fallback(
        storage,
        user_id,
        Some("reviewer"),
        /* include_completed */ false,
    ) {
        Ok(e) => e,
        Err(e) => return ReviewStoryLookup::Failed(format!("queue read: {e:#}")),
    };
    let store = match storage.load() {
        Ok(s) => s,
        Err(e) => return ReviewStoryLookup::Failed(format!("store load: {e:#}")),
    };
    classify_review_story_lookup(&entries, &store, forge, n)
}

/// BUG-440: outcome of choosing a single review target from a PR's spec sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrSpecChoice {
    One(String),
    None,
    Ambiguous(Vec<String>),
}

/// BUG-440: pick the single spec a PR should be reviewed against, given the
/// DELIVERED specs (subject `(SPEC-ID)` trailers + review-story covers
/// relationships) and the broader REFERENCED specs (anything spec-id-shaped in
/// titles/bodies/trace lines). Delivered wins outright when non-empty, so a PR
/// that delivers one spec while merely *tracing* others resolves cleanly
/// instead of bailing on false ambiguity (the STORY-501 / TASK-642 case). Only
/// a genuinely ambiguous *delivered* set — or, absent any delivery signal, an
/// ambiguous referenced set — is reported as ambiguous. Both inputs are assumed
/// already reduced to most-specific. Pure, so the precedence is unit-testable
/// without `gh`. trace:BUG-440 | ai:claude
pub(crate) fn pick_pr_spec(delivered: &[String], referenced: &[String]) -> PrSpecChoice {
    let pool = if !delivered.is_empty() {
        delivered
    } else {
        referenced
    };
    match pool.len() {
        0 => PrSpecChoice::None,
        1 => PrSpecChoice::One(pool[0].clone()),
        _ => PrSpecChoice::Ambiguous(pool.to_vec()),
    }
}

pub fn resolve_pr_to_spec(
    project_root: &std::path::Path,
    pr: u32,
    store: &RequirementsStore,
) -> Result<String> {
    // BUG-440: keep DELIVERED specs (the authoritative "this PR ships X" signal:
    // a review story's covers/implements relationships + the `(SPEC-ID)` subject
    // trailers) separate from REFERENCED specs (the broad net: any spec-id-shaped
    // token in titles/descriptions/trace lines). A PR can legitimately deliver
    // one spec while tracing others; preferring delivered avoids a false
    // "multiple backing specs" bail. trace:BUG-440 | ai:claude
    let mut delivered: Vec<String> = Vec::new();
    let mut referenced: Vec<String> = Vec::new();
    let push_unique = |v: &mut Vec<String>, id: String| {
        if !v.contains(&id) {
            v.push(id);
        }
    };

    for req in &store.requirements {
        if let Some(num) = parse_review_story_pr_number(&req.title) {
            if num == pr as u64 {
                // covers/implements relationships = authoritative delivery link.
                for rel in &req.relationships {
                    if let Some(target) = store.requirements.iter().find(|r| r.id == rel.target_id)
                    {
                        if let Some(spec_id) = &target.spec_id {
                            let canon = spec_id.to_uppercase();
                            if !canon.starts_with("PR-") {
                                push_unique(&mut delivered, canon);
                            }
                        }
                    }
                }
                // The title carries the PR's `(SPEC-ID)` delivery trailer; the
                // free-text description is referenced-only (stray trace refs).
                for id in extract_spec_ids_from_commit(&req.title) {
                    push_unique(&mut delivered, id);
                }
                for id in extract_all_spec_ids(&req.title) {
                    push_unique(&mut referenced, id);
                }
                for id in extract_all_spec_ids(&req.description) {
                    push_unique(&mut referenced, id);
                }
            }
        }
    }

    if let Some(gh) = resolve_gh_binary() {
        let pr_str = pr.to_string();
        let mut c = std::process::Command::new(&gh);
        c.current_dir(project_root).args([
            "pr",
            "view",
            &pr_str,
            "--json",
            "title,commits",
            "-q",
            "[.title, (.commits[].messageHeadline)] | @tsv",
        ]);
        if let Ok(out) = c.output_retrying_etxtbsy() {
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let fields = stdout.trim_end().split('\t');
                for field in fields {
                    // Delivered = the trailing `(SPEC-ID)` / leading `SPEC-ID:`
                    // group only; everything spec-id-shaped goes to referenced.
                    for id in extract_spec_ids_from_commit(field) {
                        push_unique(&mut delivered, id);
                    }
                    for id in extract_all_spec_ids(field) {
                        push_unique(&mut referenced, id);
                    }
                }
            }
        }
    }

    // Referenced is the fallback pool only — drop anything already delivered.
    referenced.retain(|r| !delivered.contains(r));

    // BUG-431 #2: a PR can legitimately back an epic + its child story (the
    // session was epic-scoped). Reduce each pool to the most-specific before
    // deciding — drop proven ancestors so epic+child collapses to the child.
    // Genuinely unrelated specs survive and still bail below.
    // trace:BUG-431 | ai:claude
    let delivered = reduce_to_most_specific_specs(store, &delivered);
    let referenced = reduce_to_most_specific_specs(store, &referenced);

    match pick_pr_spec(&delivered, &referenced) {
        PrSpecChoice::One(s) => Ok(s),
        PrSpecChoice::None => anyhow::bail!(
            "PR-{} has no backing specs (could not find any associated spec IDs in review stories or PR metadata)",
            pr
        ),
        PrSpecChoice::Ambiguous(specs) => anyhow::bail!(
            "PR-{} has multiple backing specs: {}. Use a single SPEC-ID instead.",
            pr,
            specs.join(", ")
        ),
    }
}

#[cfg(test)]
#[path = "tests/task_518_pr_to_spec_tests.rs"]
mod task_518_pr_to_spec_tests;

/// trace:STORY-67 | ai:claude
/// The review surface a held spec is sitting on — what `aida review <SPEC>`
/// resolves the spec to before reviewing. NEVER asserts a closed/absent PR
/// (the TASK-715 / BUG-493 failure mode); each variant is a verified state.
/// trace:STORY-553 | ai:claude
#[derive(Debug)]
pub(crate) enum ReviewSurface {
    /// An OPEN change (PR/MR) exists for the spec's branch.
    OpenChange {
        branch: String,
        number: u64,
        url: String,
    },
    /// Work is on a feature branch with commits, but no open change.
    /// (Either never pushed a PR, or the held draft PR was closed.)
    BranchNoChange { branch: String, commits: usize },
    /// Commits reference the spec but already landed on the default branch.
    Shipped { number: Option<u64> },
    /// No commits and no trace comments reference the spec — built locally
    /// and never pushed, or not started.
    Local,
}

pub(crate) fn review_branch_no_change_context_card(
    spec_id: &str,
    branch: &str,
    commits: usize,
    change_noun: &str,
    open_change_cmd: &str,
) -> context_prompt::ContextCard {
    context_prompt::ContextCard {
        decision: format!(
            "whether to open a {change_noun} from `{branch}` before running `aida review {spec_id}`"
        ),
        provenance: vec![
            format!(
                "branch `{branch}` has {commits} commit{} linked to {spec_id}",
                if commits == 1 { "" } else { "s" }
            ),
            format!("no open {change_noun} was found for that branch"),
        ],
        answers: vec![
            format!(
                "y: print `{open_change_cmd}`; reversible by closing the {change_noun}"
            ),
            format!(
                "n: leave the work held without opening a {change_noun}; reversible by rerunning `aida review {spec_id}`"
            ),
        ],
        recommended_default: format!(
            "n - avoid creating a {change_noun} until you decide this branch is the review surface"
        ),
    }
}

// trace:BUG-816 | ai:codex
// trace:BUG-1610 | ai:claude — `base` is now always explicit (see
// `ForgeKind::create_cmd_for_branch`), so this hint never lets the forge
// infer (and possibly mis-infer) the target branch.
pub(crate) fn review_open_change_hint(
    forge: crate::forge::ForgeKind,
    branch: &str,
    base: &str,
) -> String {
    let push = format!("git push -u origin {}", shell_quote(branch));
    match forge.create_cmd_for_branch(branch, base) {
        Some(create) => format!("{push} && {create}"),
        None => push,
    }
}

pub(crate) fn review_rebase_context_card(
    change_noun: &str,
    number: u64,
    behind: u32,
    overlap: &[String],
) -> context_prompt::ContextCard {
    let mut provenance = vec![format!(
        "{change_noun}-{number} is {behind} commit{} behind its base branch",
        if behind == 1 { "" } else { "s" }
    )];
    if overlap.is_empty() {
        provenance.push("no changed file overlap was detected".to_string());
    } else {
        provenance.push(format!(
            "changed-file overlap detected: {}",
            overlap.join(", ")
        ));
    }
    let default = !overlap.is_empty();
    context_prompt::ContextCard {
        decision: format!("whether to rebase {change_noun}-{number} before review"),
        provenance,
        answers: vec![
            format!(
                "y: run `aida pr rebase {number}` now; reversible through normal git/forge recovery if the rebase fails"
            ),
            format!(
                "n: continue reviewing the existing {change_noun} diff against its stale base; reversible by rebasing later"
            ),
        ],
        recommended_default: if default {
            "y - overlapping files make the stale-base review less reliable".to_string()
        } else {
            "n - no file overlap was detected, so the review can proceed".to_string()
        },
    }
}

pub(crate) fn review_diffstat(project_root: &std::path::Path, branch: &str) -> String {
    let base =
        resolve_default_branch_ref(project_root).unwrap_or_else(|| "origin/main".to_string());
    let range = format!("{base}...{branch}");
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["diff", "--stat", &range])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let stat = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if stat.is_empty() {
                format!("No diffstat for `{range}`.")
            } else {
                stat
            }
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            format!("Diffstat unavailable for `{range}`: {err}")
        }
        Err(e) => format!("Diffstat unavailable for `{range}`: {e}"),
    }
}

pub(crate) fn ask_ai_review_prompt(
    req: &aida_core::Requirement,
    spec_id: &str,
    card: &context_prompt::ContextCard,
    diffstat: &str,
) -> String {
    format!(
        "You are AIDA's one-shot Ask-AI reviewer at an interactive CLI fork.\n\n\
         Return a concise recommendation for the operator. Start with exactly one line:\n\
         RECOMMEND: y|n - <short reason>\n\n\
         Then add at most three bullets naming the decisive facts. Do not run tools. \
         Judge only from the context below.\n\n\
         ## Prompt context\n\
         {}\n\
         ## Spec\n\
         ID: {spec_id}\n\
         Title: {}\n\
         Status: {}\n\n\
         Description and acceptance:\n{}\n\n\
         ## Diffstat\n{}\n",
        card.render(),
        req.title,
        req.status,
        req.description.trim(),
        diffstat.trim(),
    )
}

pub(crate) fn ask_ai_review_once<W: std::io::Write + ?Sized>(
    project_root: &std::path::Path,
    req: &aida_core::Requirement,
    spec_id: &str,
    card: &context_prompt::ContextCard,
    diffstat: &str,
    output: &mut W,
) -> Result<()> {
    let vendor = session::resolve_headless_vendor(project_root);
    let adapter = match compete::vendor_adapter(vendor.as_str()) {
        Some(adapter @ compete::VendorAdapter::Headless { .. }) => adapter,
        Some(compete::VendorAdapter::HumanBriefed) | None => {
            writeln!(
                output,
                "\nAsk-AI unavailable: `{}` is not a one-shot headless review vendor here.",
                vendor.as_str()
            )?;
            return Ok(());
        }
    };
    let prompt = ask_ai_review_prompt(req, spec_id, card, diffstat);
    let args = compete::ask_ai_argv(&adapter, &prompt).unwrap_or_default();
    let command = match &adapter {
        compete::VendorAdapter::Headless { command, .. } => *command,
        compete::VendorAdapter::HumanBriefed => unreachable!("human-briefed returned above"),
    };
    writeln!(
        output,
        "\nAsk-AI via {}…",
        format!(
            "{} {}",
            command,
            args.first().map(String::as_str).unwrap_or("")
        )
        .trim()
    )?;
    let result = std::process::Command::new(command)
        .current_dir(project_root)
        .args(&args)
        .output_retrying_etxtbsy();
    match result {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if !stdout.is_empty() {
                writeln!(output, "{stdout}")?;
            }
            if !out.status.success() {
                writeln!(output, "Ask-AI exited with {status}.", status = out.status)?;
                if !stderr.is_empty() {
                    writeln!(output, "{stderr}")?;
                }
            }
        }
        Err(e) => {
            writeln!(
                output,
                "Ask-AI unavailable: could not spawn `{}` ({e}).",
                command
            )?;
        }
    }
    Ok(())
}

/// Classify a spec's review surface from its git linkage + (optional) forge
/// change-lookup. Pure: the side-effecting `collect_git_linkage` /
/// `change_lookup_for_branch` run at the call site; this is the decision so
/// it's unit-testable. Order matters — shipped wins over branch, and only a
/// `ChangeLookup::Found` becomes `OpenChange` (a closed/absent/unreachable
/// PR must NOT be asserted as open — the TASK-715 / BUG-493 failure mode).
/// trace:STORY-553 trace:BUG-493 | ai:claude
pub(crate) fn classify_review_surface(
    linkage: &GitLinkage,
    change: Option<crate::forge::ChangeLookup>,
) -> ReviewSurface {
    classify_review_surface_forge_first(linkage, None, change)
}

/// BUG-876: resolve review surfaces forge-first. A lease/local branch is only
/// an accelerator; an open PR whose head branch or commit trailers reference
/// the spec remains the review surface after the lease is released.
// trace:BUG-876 | ai:codex
pub(crate) fn classify_review_surface_forge_first(
    linkage: &GitLinkage,
    spec_change: Option<crate::forge::ChangeLookup>,
    branch_change: Option<crate::forge::ChangeLookup>,
) -> ReviewSurface {
    if linkage.shipped {
        return ReviewSurface::Shipped {
            number: linkage.shipped_pr,
        };
    }

    if let Some(crate::forge::ChangeLookup::Found(c)) = spec_change {
        let branch = if c.branch.is_empty() {
            linkage
                .branch
                .clone()
                .unwrap_or_else(|| "unknown".to_string())
        } else {
            c.branch
        };
        return ReviewSurface::OpenChange {
            branch,
            number: c.id,
            url: c.url,
        };
    }

    match (linkage.branch.clone(), branch_change) {
        (Some(branch), Some(crate::forge::ChangeLookup::Found(c))) => ReviewSurface::OpenChange {
            branch,
            number: c.id,
            url: c.url,
        },
        (Some(branch), _) => ReviewSurface::BranchNoChange {
            branch,
            commits: linkage.commits.len(),
        },
        (None, _) => ReviewSurface::Local,
    }
}

/// BUG-881: shared forge-first review-surface resolver for human review and
/// PR-only drains. A lease/local branch is only an accelerator; a standalone
/// `queue work --from-pr` must see the same open PR that `aida review` sees
/// when the PR is discoverable by spec trailer or spec-named head branch.
// trace:BUG-881 | ai:codex
pub(crate) fn resolve_review_surface_forge_first(
    project_root: &std::path::Path,
    spec_id: &str,
    linkage: &GitLinkage,
) -> ReviewSurface {
    let spec_change = change_lookup_for_spec(project_root, spec_id);
    let branch_change = linkage
        .branch
        .as_deref()
        .map(|b| change_lookup_for_branch(project_root, b));
    classify_review_surface_forge_first(linkage, Some(spec_change), branch_change)
}

/// BUG-582: the robust invariant guarding the `aida human` reviews-awaiting
/// bucket — a finished spec can NEVER be resurrected onto the operator's seat
/// by a lingering local branch / stale review surface.
///
/// Two stale-data leaks made BUG-581 (already `Completed`, PR-1048 already
/// MERGED) surface as "needs review": (1) a spec whose status is terminal must
/// never be a review candidate regardless of any branch the Agent-tool
/// worktrees left lying around (`worktree-agent-*` branches accumulate), and
/// (2) only a genuinely-open surface (`OpenChange` / `BranchNoChange`) is a
/// review; a merged (`Shipped`) or never-pushed (`Local`) surface is not.
///
/// Centralizing both checks in one pure predicate keeps the guarantee from
/// drifting: the candidate pre-filter in `reviews_awaiting_human` is a cost
/// bound (it reads the cache, which can be stale), while THIS is the
/// load-bearing invariant — even a stale cache row that slipped through the
/// pre-filter is dropped here. Pure ⇒ unit-testable without a repo/forge.
/// trace:BUG-582 | ai:claude
pub(crate) fn spec_eligible_for_review_awaiting(
    status: aida_core::RequirementStatus,
    surface: &ReviewSurface,
) -> bool {
    // A finished (merged) or abandoned spec is never review work — a lingering
    // local branch must not put it back on the operator's seat.
    if matches!(
        status,
        aida_core::RequirementStatus::Completed | aida_core::RequirementStatus::Rejected
    ) {
        return false;
    }
    // Only a genuinely-open review surface counts. `Shipped` = the PR already
    // merged (the merged-PR signal); `Local` = never pushed. Neither is a
    // review awaiting a human. The LEGITIMATE case — Done-on-a-branch with an
    // OPEN PR not yet merged — is `OpenChange` and stays included.
    matches!(
        surface,
        ReviewSurface::OpenChange { .. } | ReviewSurface::BranchNoChange { .. }
    )
}

/// BUG-722: the requirements-store orphan branch is never a code-review target.
/// Commits that reference a spec but landed on `aida-store` during a store
/// reconcile must not register as "a branch awaiting review" — the STORY-760
/// false positive (`[branch aida-store]`). The store branch name is the
/// project-wide constant the rest of the CLI hard-codes (see the
/// `branch_exists_anywhere(.., "aida-store")` probe). Any configured store/
/// mirror ref would also belong here, but a mirror is a REMOTE, not a branch —
/// the branch name is uniformly `aida-store`.
// trace:BUG-722 | ai:claude
pub(crate) fn is_store_branch(branch: &str) -> bool {
    branch == "aida-store"
}

/// BUG-722: the branch a review surface sits on, when it has one. `Shipped`
/// (merged) and `Local` (never pushed) carry no live branch. Lets the
/// classifier apply the store-branch exclusion without re-plumbing git.
// trace:BUG-722 | ai:claude
pub(crate) fn review_surface_branch(surface: &ReviewSurface) -> Option<&str> {
    match surface {
        ReviewSurface::OpenChange { branch, .. } | ReviewSurface::BranchNoChange { branch, .. } => {
            Some(branch.as_str())
        }
        ReviewSurface::Shipped { .. } | ReviewSurface::Local => None,
    }
}

/// BUG-722: which `aida human` bucket a candidate spec lands in once its
/// status, view-state, and review surface are known. See
/// [`classify_human_review_bucket`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HumanReviewBucket {
    /// A genuine code-review gate — an open PR, or a pushed branch on active/
    /// Done work — the same surface `aida review <SPEC>` locates.
    ReviewsAwaiting,
    /// Loose work-in-progress: a `Draft` spec on a pushed branch with commits
    /// but no open PR. Visible under `wip-branches`, but NOT a review gate.
    WipBranch,
    /// Not the human's review work at all (finished/abandoned/deferred/archived
    /// spec, the store branch, or a merged/never-pushed surface).
    Excluded,
}

/// BUG-722: classify what `aida human` bucket a candidate spec belongs to.
/// Pure ⇒ every false-positive class is unit-testable without a repo/forge.
/// Layers three gates on top of the BUG-582 status/surface invariant, in
/// precedence order:
///  1. **view-state** — a deferred or archived spec is hidden from the human's
///     review seat exactly as `aida list` hides it from the default view
///     (STORY-441 archived, STORY-584 deferred);
///  2. **status/surface** — the BUG-582 invariant: a Completed/Rejected spec or
///     a merged (`Shipped`) / never-pushed (`Local`) surface is never review
///     work, even with a lingering local branch;
///  3. **branch-type** — the `aida-store` requirements-store orphan branch is
///     never a code-review target (the STORY-760 false positive);
/// then splits the survivors: a `Draft` spec whose only surface is a pushed
/// branch with no PR is loose WIP (`wip-branches`), not a review gate — nothing
/// awaits a human until there's a PR or a Done claim (operator decision,
/// 2026-07-12). Everything else is a genuine review.
// trace:BUG-722 | ai:claude
pub(crate) fn classify_human_review_bucket(
    status: aida_core::RequirementStatus,
    archived: bool,
    deferred: bool,
    surface: &ReviewSurface,
) -> HumanReviewBucket {
    // Gate 1 — view-state: a filed-away or primed-conditional spec is not the
    // human's review work, even with a live branch (mirrors `aida list`).
    if archived || deferred {
        return HumanReviewBucket::Excluded;
    }
    // Gate 2 — the BUG-582 status/surface invariant.
    if !spec_eligible_for_review_awaiting(status.clone(), surface) {
        return HumanReviewBucket::Excluded;
    }
    // Gate 3 — branch-type: the requirements-store orphan branch is never a
    // code-review target.
    if review_surface_branch(surface).is_some_and(is_store_branch) {
        return HumanReviewBucket::Excluded;
    }
    // Draft + pushed branch + no open PR ⇒ loose WIP, not a review gate.
    if matches!(status, aida_core::RequirementStatus::Draft)
        && matches!(surface, ReviewSurface::BranchNoChange { .. })
    {
        return HumanReviewBucket::WipBranch;
    }
    HumanReviewBucket::ReviewsAwaiting
}

/// BUG-722: mirror the deferred view-state predicate `aida list` applies — the
/// `deferred` flag OR any legacy `deferred:*` parking tag (STORY-584's
/// honor-both rule). Keeps the `aida human` review seat hidden for exactly the
/// specs the default list hides.
// trace:BUG-722 | ai:claude
pub(crate) fn requirement_is_deferred(req: &aida_core::Requirement) -> bool {
    req.deferred || req.tags.iter().any(|t| t.starts_with("deferred:"))
}

/// BUG-511: RAII release for the review-verb lease — removing the lease
/// file on drop covers every exit path of [`handle_review_spec`] (verdict
/// presented, surface bailed early, reviewer launch failed, `?` errors).
/// A SIGKILL'd review leaks the file; the dead-PID reaping in
/// [`acquire_review_lease_with_mode`] / [`auto_release_decision_for_lease`] cleans
/// that up on the next coordination touch. trace:BUG-511 | ai:claude
#[derive(Debug)]
pub(crate) struct ReviewLeaseGuard {
    pub(crate) path: std::path::PathBuf,
}

impl Drop for ReviewLeaseGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewLeaseConflictMode {
    AutoReleaseStale,
    PromptBeforeRelease,
    RefuseStaleNonInteractive,
}

pub(crate) fn review_stale_lease_refusal(spec_id: &str, conflict: &SessionLease) -> String {
    format!(
        "`{spec_id}` is already in flight behind stale lease {} (owner {}, since {}). \
         Headless review cannot ask whether to release it. Re-run from a TTY to release-and-proceed, \
         or end it explicitly with `aida session end {} --yes` and then re-run `aida review {spec_id}`.",
        &conflict.id[..conflict.id.len().min(8)],
        conflict.owner,
        conflict.started_at.format("%Y-%m-%d %H:%M UTC"),
        &conflict.id[..conflict.id.len().min(8)],
    )
}

pub(crate) fn confirm_release_stale_review_lease(
    spec_id: &str,
    conflict: &SessionLease,
) -> Result<bool> {
    use std::io::Write;
    eprintln!(
        "  {} stale lease {} is blocking review of `{}` (owner {}, since {}).",
        crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
        (&conflict.id[..conflict.id.len().min(8)]).yellow(),
        spec_id,
        conflict.owner,
        conflict.started_at.format("%Y-%m-%d %H:%M UTC"),
    );
    eprintln!(
        "  {} release it via session cleanup and continue? [y/N] ",
        crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed()
    );
    std::io::stderr().flush()?;
    let mut ans = String::new();
    std::io::stdin().read_line(&mut ans)?;
    Ok(matches!(
        ans.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// BUG-511: take a session lease scoped to the spec for the duration of an
/// `aida review <spec>` run — the same coordination substrate `aida queue
/// work` uses (`.aida/sessions/`, [`find_scope_lease_conflict`], the
/// BUG-307 auto-release sweep), so the open-spec classifier sees the spec
/// in flight ("being reviewed") and a second `aida review` / `queue work`
/// on it refuses instead of double-starting. The lease is an advisory lock
/// (empty `worktree_path`, the TASK-474 convention); its liveness signal
/// is this process's PID. trace:BUG-511 | ai:claude
pub(crate) fn acquire_review_lease_with_mode(
    project_root: &std::path::Path,
    spec_id: &str,
    branch: &str,
    conflict_mode: ReviewLeaseConflictMode,
) -> Result<ReviewLeaseGuard> {
    let cfg = orchestrator::OrchestratorConfig::load(project_root);
    let mut remaining = 16usize; // defense-in-depth bound, mirrors queue work's sweep
    while let Some(conflict) = find_scope_lease_conflict(&list_leases(project_root), spec_id) {
        let recovery = stale_lease_recovery_for_lease(&conflict);
        if matches!(
            recovery.verdict,
            StaleLeaseRecovery::ReclaimableClean { .. }
        ) && conflict_mode != ReviewLeaseConflictMode::AutoReleaseStale
        {
            match conflict_mode {
                ReviewLeaseConflictMode::PromptBeforeRelease => {
                    // BUG-890: human review is interactive, so a stale
                    // clean/advisory lease should be an explicit
                    // release-and-proceed choice rather than a hard stop.
                    // trace:BUG-890 | ai:codex
                    if !confirm_release_stale_review_lease(spec_id, &conflict)? {
                        anyhow::bail!(
                            "review aborted — stale lease {} on `{spec_id}` is still held.",
                            &conflict.id[..conflict.id.len().min(8)]
                        );
                    }
                }
                ReviewLeaseConflictMode::RefuseStaleNonInteractive => {
                    anyhow::bail!(review_stale_lease_refusal(spec_id, &conflict));
                }
                ReviewLeaseConflictMode::AutoReleaseStale => unreachable!(),
            }
            eprintln!(
                "  {} released stale lease {} on {} (process dead)",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                (&conflict.id[..conflict.id.len().min(8)]).yellow(),
                spec_id,
            );
            let _ = force_cleanup_lease(project_root, &conflict);
            remaining -= 1;
            if remaining == 0 {
                anyhow::bail!(
                    "stale-lease release gave up after 16 iterations on `{spec_id}` — \
                     the lease store may be corrupt; inspect `.aida/sessions/`"
                );
            }
            continue;
        }
        match auto_release_decision_for_lease(project_root, &conflict, &cfg) {
            orchestrator::AutoReleaseDecision::SafelyDormant { process_dead, .. } => {
                match conflict_mode {
                    ReviewLeaseConflictMode::AutoReleaseStale => {}
                    ReviewLeaseConflictMode::PromptBeforeRelease => {
                        // BUG-890: human review is interactive, so a stale
                        // clean/advisory lease should be an explicit
                        // release-and-proceed choice rather than a hard stop.
                        // trace:BUG-890 | ai:codex
                        if !confirm_release_stale_review_lease(spec_id, &conflict)? {
                            anyhow::bail!(
                                "review aborted — stale lease {} on `{spec_id}` is still held.",
                                &conflict.id[..conflict.id.len().min(8)]
                            );
                        }
                    }
                    ReviewLeaseConflictMode::RefuseStaleNonInteractive => {
                        anyhow::bail!(review_stale_lease_refusal(spec_id, &conflict));
                    }
                }
                eprintln!(
                    "  {} released stale lease {} on {} ({})",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    (&conflict.id[..conflict.id.len().min(8)]).yellow(),
                    spec_id,
                    if process_dead {
                        "process dead"
                    } else {
                        "dormant"
                    }
                );
                let _ = force_cleanup_lease(project_root, &conflict);
                remaining -= 1;
                if remaining == 0 {
                    anyhow::bail!(
                        "auto-release sweep gave up after 16 iterations on `{spec_id}` — \
                         the lease store may be corrupt; inspect `.aida/sessions/`"
                    );
                }
            }
            orchestrator::AutoReleaseDecision::DormantDirty { dirty_entries } => {
                anyhow::bail!(
                    "lease {} on `{spec_id}` looks orphaned but its worktree at {} has \
                     {dirty_entries} uncommitted change(s) — resolve that session first \
                     (`aida queue work {spec_id} --resume` keeps the work; \
                     `aida session end {} --force` discards it).",
                    &conflict.id[..conflict.id.len().min(8)],
                    conflict.worktree_path.display(),
                    &conflict.id[..conflict.id.len().min(8)],
                );
            }
            orchestrator::AutoReleaseDecision::Live => {
                let holder = if conflict.review_verb {
                    "another review of it is already running".to_string()
                } else {
                    format!(
                        "a live {} session holds it",
                        conflict.role.as_deref().unwrap_or("work")
                    )
                };
                anyhow::bail!(
                    "`{spec_id}` is already in flight — {holder} (lease {}, owner {}, \
                     since {}). Reviewing it now would double-start; wait for that \
                     session to finish, or release it with `aida session end {}`.",
                    &conflict.id[..conflict.id.len().min(8)],
                    conflict.owner,
                    conflict.started_at.format("%Y-%m-%d %H:%M UTC"),
                    &conflict.id[..conflict.id.len().min(8)],
                );
            }
        }
    }

    let id_long = uuid::Uuid::now_v7().to_string();
    let id = id_long.replace('-', "")[..12].to_string();
    let owner = aida_core::git_ops::git_config_get("user.email")
        .ok()
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "unknown".to_string());
    let lease = SessionLease {
        id: id.clone(),
        scope: spec_id.to_string(),
        slug: slugify(spec_id),
        owner,
        worktree_path: std::path::PathBuf::new(),
        branch: branch.to_string(),
        started_at: chrono::Utc::now(),
        hostname: hostname(),
        role: Some("reviewer".to_string()),
        creator_pid: Some(std::process::id()),
        creator_pid_start_time: process_probe::process_start_identity(std::process::id()),
        active_pid: None,
        active_pid_start_time: None,
        cargo_target_dir: None,
        parent_project_root: None,
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: true,
        claim_verb: false,
        manual_enter_at: None,
        interrupted_at: None,
        interrupted_reason: None,
    };
    std::fs::create_dir_all(leases_dir(project_root))?;
    let path = lease_path(project_root, &id);
    // STORY-1429: atomic, so a reader never sees a half-written lease.
    // trace:STORY-1429 | ai:claude
    aida_core::write_atomic(&path, toml::to_string_pretty(&lease)?)?;
    Ok(ReviewLeaseGuard { path })
}

/// BUG-539: a spec already in a terminal/merged state has no live review
/// surface — running a full review + the Approve / Request-changes menu over
/// it is at best confusing and at worst lets Request-changes → `aida queue
/// rework` disrupt already-merged work. `Completed` (merged to the default
/// branch) is the authoritative signal; `Rejected` is likewise terminal and
/// has nothing to review. Keep this a plain `//`-doc on a non-clap fn so the
/// trace marker can't leak into `--help`.
// trace:BUG-539 | ai:claude
pub(crate) fn review_is_terminal_noop(status: &RequirementStatus) -> bool {
    matches!(
        status,
        RequirementStatus::Completed | RequirementStatus::Rejected
    )
}

/// BUG-1607: resolve (and preflight) the vendor `handle_review_spec` will
/// launch the interactive reviewer with — the SAME shared contract
/// `aida queue work` resolves for the implementer/reviewer launch (flag >
/// `[agents]` project/user config > default, filtered to an enabled AND
/// installed profile). This verb has no `--vendor` flag of its own, so a
/// Codex-only project (no Claude installed) must be picked up from config
/// alone.
///
/// `None` only for `no_agent` (no launch, so no vendor is needed) — every
/// other refusal (disabled profile, unreachable binary, unsupported
/// interactive vendor) propagates as `Err` from here, BEFORE
/// `handle_review_spec` acquires the review lease below it. Previously
/// there was no resolution here at all: the launch always hardcoded
/// `claude`, so the command reached "running reviewer" holding a live
/// lease and then failed with a raw ENOENT.
///
/// Split out from `handle_review_spec` (which gates on an interactive TTY
/// and therefore cannot run inside `cargo test`) so an automated test can
/// exercise this exact resolution-and-preflight code directly.
// trace:BUG-1607 | ai:claude
pub(crate) fn review_spec_resolve_vendor(
    project_root: &std::path::Path,
    no_agent: bool,
) -> Result<Option<session::HeadlessVendor>> {
    if no_agent {
        return Ok(None);
    }
    let vendor = session::resolve_enabled_headless_vendor(project_root)?;
    session::preflight_launch_vendor(vendor, true)?;
    Ok(Some(vendor))
}

/// BUG-1607: build + spawn the interactive reviewer [`session::ReviewerLaunchPlan`]
/// for `vendor` — the exact launch `handle_review_spec` runs once a human is
/// confirmed at an interactive terminal. Split out for the same testability
/// reason as [`review_spec_resolve_vendor`]: `handle_review_spec` itself
/// gates on a TTY and cannot run under `cargo test`, but this is the real
/// launch code, not a reimplemented stand-in for it.
// trace:BUG-1607 | ai:claude
pub(crate) fn review_spec_launch_reviewer(
    vendor: session::HeadlessVendor,
    session_name: &str,
    prompt: &str,
    session_id: &str,
) -> Result<std::process::ExitStatus> {
    let plan = session::interactive_reviewer_launch_plan(
        vendor,
        None,
        Some(session_name),
        prompt,
        session_id,
        false,
    )
    .map_err(|e| anyhow::anyhow!("aida review: {e}"))?;
    // ADR-66: the reviewer child grant validates against the PROJECT root,
    // resolved the same way every other authority check resolves it — not
    // whatever directory the process happens to sit in.
    // trace:STORY-1473 | ai:claude
    session::spawn_reviewer_launch_plan(&plan, &find_project_root()?)
        .context("failed to launch the reviewer")
}

/// trace:STORY-553 | ai:claude — `aida review <SPEC>`: the human-review
/// counterpart to `aida queue work`. Resolves the spec's review surface,
/// runs the existing headless reviewer tier (`/aida-review`) over the diff
/// against the spec's `## Acceptance` criteria, then presents the verdict
/// and lets the human decide (approve / request changes / open the diff /
/// defer). NEVER auto-merges — that would defeat the review:draft-only gate
/// (cf. STORY-529 self-merge bug-class).
pub(crate) fn handle_review_spec(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
    spec: &str,
    no_agent: bool,
    allow_stale_base: bool,
    target_branch: Option<&str>,
) -> Result<()> {
    let project_root = store_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cannot derive project root from store path"))?;

    // ---- Resolve the spec ----
    let req = backend
        .get_requirement_by_spec_id(spec)
        .with_context(|| format!("failed to load {spec}"))?
        .ok_or_else(|| not_found::requirement_not_found(spec, Some(store_path)))?;
    let spec_id = req.spec_id.clone().unwrap_or_else(|| spec.to_string());
    record_role_activity(&spec_id, "review");

    println!(
        "{} {} — {}",
        "Reviewing".green().bold(),
        spec_id.cyan(),
        req.title
    );

    // ---- Locate the review surface (gh-free linkage, then forge lookup) ----
    let ids = vec![spec_id.clone()];
    let linkage = collect_git_linkage(project_root, &ids);
    let forge = crate::forge::resolve_forge_kind(project_root);

    // BUG-539: short-circuit a spec already in a terminal/merged state. When
    // the spec is Completed (work merged to the default branch) — or Rejected
    // — a full review + the Approve / Request-changes menu is at best
    // confusing and at worst lets the Request-changes → `aida queue rework`
    // path disrupt already-merged work. Detect it up front and report instead
    // of reviewing. We key off status as the authoritative signal — git-linkage
    // squash-PR parsing can miss the merge — and enrich the message with the
    // merge commit / PR number from linkage when available. At most we offer a
    // read-only `git show` of the merged diff. trace:BUG-539 | ai:claude
    if review_is_terminal_noop(&req.status) {
        let change_noun = forge.change_noun();
        let merged = matches!(req.status, RequirementStatus::Completed);
        let mut details: Vec<String> = Vec::new();
        if let Some((_, short, _)) = linkage.commits.first() {
            details.push(format!("commit {short}"));
        }
        if let Some(pr) = linkage.shipped_pr {
            details.push(format!("{change_noun}-{pr}"));
        }
        let detail_suffix = if details.is_empty() {
            String::new()
        } else {
            format!(" ({})", details.join(", "))
        };
        let headline = if merged {
            "already merged/completed — nothing to review"
        } else {
            "rejected — nothing to review"
        };
        println!(
            "  {} {}{}",
            "Surface".bold(),
            headline.yellow(),
            detail_suffix.dimmed()
        );
        if merged {
            if let Some((full, _, _)) = linkage.commits.first() {
                println!(
                    "  {} to see the merged diff: {}",
                    crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                    format!("git show {full}").cyan()
                );
            }
            println!(
                "  {} if it genuinely needs another look, {}.",
                crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                format!("aida queue rework {spec_id}").cyan()
            );
        }
        return Ok(());
    }

    // BUG-876 / BUG-881: use the shared forge-first review-surface resolver so
    // `aida review` and `queue work --from-pr` agree after the implementer
    // lease is gone. trace:BUG-881 | ai:codex
    let surface = resolve_review_surface_forge_first(project_root, &spec_id, &linkage);

    let change_noun = forge.change_noun();

    // Report the surface honestly, then either review or guide.
    match &surface {
        ReviewSurface::OpenChange {
            branch,
            number,
            url,
        } => {
            println!(
                "  {} open {}-{} on branch {} {}",
                "Surface".bold(),
                change_noun,
                number,
                branch.cyan(),
                url.dimmed()
            );
        }
        ReviewSurface::BranchNoChange {
            branch, commits, ..
        } => {
            println!(
                "  {} branch {} ({} commit{}) — {}",
                "Surface".bold(),
                branch.cyan(),
                commits,
                if *commits == 1 { "" } else { "s" },
                format!("no open {change_noun}").yellow(),
            );
            println!(
                "  {} the held draft {change_noun} is closed or was never opened.",
                crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed()
            );
            // BUG-1610: resolve the intended base BEFORE offering to open a
            // change, and verify it — a bare `--source-branch` create with
            // no explicit base can silently target the source branch itself
            // when it became the project default (remote main never
            // pushed). An explicit `--target-branch` always wins over
            // whatever a previous attempt saved; the saved state itself
            // never has source == target (`write_mr_recovery_state` refuses
            // that pair). trace:BUG-1610 | ai:claude
            let recovery_path = crate::pr_cmd::mr_recovery_path(project_root, &spec_id);
            let saved_recovery = crate::pr_cmd::read_mr_recovery_state(&recovery_path);
            let default_base = crate::forge::default_branch_of(project_root);
            let base = crate::pr_cmd::resolve_mr_target_branch(
                target_branch,
                saved_recovery.as_ref(),
                &default_base,
            );
            let base_check = crate::pr_cmd::preflight_mr_base(project_root, branch, &base);
            if base_check != crate::pr_cmd::MrBasePreflight::Ok {
                println!(
                    "  {} {}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                    crate::pr_cmd::mr_base_diagnosis_message(forge, base_check, branch, &base)
                        .yellow()
                );
                return Ok(());
            }
            // AC-5: offer to (re)open a PR before review.
            let open_change_cmd = review_open_change_hint(forge, branch, &base);
            if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
                let card = review_branch_no_change_context_card(
                    &spec_id,
                    branch,
                    *commits,
                    change_noun,
                    &open_change_cmd,
                );
                let diffstat = review_diffstat(project_root, branch);
                let reopen = context_prompt::confirm_with_context_and_ai(
                    &format!("Open a {change_noun} from `{branch}` first?"),
                    false,
                    &card,
                    |out| ask_ai_review_once(project_root, &req, &spec_id, &card, &diffstat, out),
                )
                .unwrap_or(false);
                if reopen {
                    println!("  {} run: {}", "→".green(), open_change_cmd.cyan());
                    println!(
                        "  {} then re-run {} once the {change_noun} is open.",
                        crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                        format!("aida review {spec_id}").cyan()
                    );
                    let _ = crate::pr_cmd::write_mr_recovery_state(
                        &recovery_path,
                        &crate::pr_cmd::MrRecoveryState {
                            spec: spec_id.clone(),
                            source_branch: branch.clone(),
                            target_branch: base.clone(),
                        },
                    );
                    return Ok(());
                }
            } else {
                println!(
                    "  {} to open one: {}",
                    crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                    open_change_cmd.cyan()
                );
            }
        }
        ReviewSurface::Shipped { number } => {
            match number {
                Some(n) => println!(
                    "  {} already merged to the default branch (via {}-{}).",
                    "Surface".bold(),
                    change_noun,
                    n
                ),
                None => println!(
                    "  {} already merged to the default branch.",
                    "Surface".bold()
                ),
            }
            println!(
                "  {} nothing to review — the work has landed. \
                 If it needs another look, {}.",
                crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                format!("aida queue rework {spec_id}").cyan()
            );
            return Ok(());
        }
        ReviewSurface::Local => {
            println!(
                "  {} {}",
                "Surface".bold(),
                "built locally, never pushed (no commits reference this spec)".yellow()
            );
            println!(
                "  {} commit your work with a {} trailer, push, and open a {change_noun}; \
                 then re-run {}.",
                crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                format!("({spec_id})").cyan(),
                format!("aida review {spec_id}").cyan()
            );
            return Ok(());
        }
    }

    // ---- Run the reviewer over the diff (AC-2 / AC-4: reuse /aida-review) ----
    let interactive = review_may_launch_reviewer(
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
    );
    let (pr_number, surface_branch) = match &surface {
        ReviewSurface::OpenChange { branch, number, .. } => (Some(*number), branch.clone()),
        ReviewSurface::BranchNoChange { branch, .. } => (None, branch.clone()),
        _ => unreachable!("shipped/local surfaces returned above"),
    };

    // BUG-721: `aida review <spec>` is the human-driven review verb. When stdin
    // isn't a terminal (piped/redirected/CI/agent), do NOT silently launch a
    // blind headless reviewer subprocess the caller can neither see nor
    // interrupt — the surface report above is already printed, so a
    // non-interactive caller gets the read-only surface only. Refuse loudly and
    // exit non-zero. `--no-agent` (the explicit read-only opt-out) still degrades
    // honestly below. This gate is scoped to THIS verb; the orchestrator's drain
    // review phase (`run_reviewer`) never reaches here — it launches its headless
    // reviewer via `aida queue work PR-N` on purpose — so drains are untouched.
    if !no_agent && !interactive {
        anyhow::bail!(
            "aida review needs an interactive terminal to run the reviewer.\n  \
             In a drain the orchestrator runs the reviewer for you; to see the \
             read-only surface without launching one, add --no-agent."
        );
    }

    // BUG-1607: resolve (and preflight) the launch vendor — see
    // `review_spec_resolve_vendor`'s doc for why this is BEFORE the lease
    // below is acquired, and why it is split out as its own function.
    let review_vendor = review_spec_resolve_vendor(project_root, no_agent)?;

    // BUG-511: hold a session lease scoped to the spec while the review
    // runs — same substrate as `aida queue work`, so the footer / `aida
    // why` / burndown-explain see the spec in flight and a concurrent
    // review or pickup refuses instead of double-starting. Released on
    // every exit path via the guard's Drop. trace:BUG-511 | ai:claude
    let review_lease_mode = if interactive {
        ReviewLeaseConflictMode::PromptBeforeRelease
    } else {
        ReviewLeaseConflictMode::RefuseStaleNonInteractive
    };
    let _review_lease =
        acquire_review_lease_with_mode(project_root, &spec_id, &surface_branch, review_lease_mode)?;
    // STORY-1405: while the reviewer runs, PR-N is visibly under review to
    // every merge surface. Released with the lease when this verb exits.
    // trace:STORY-1405 | ai:claude
    let _review_marker = match pr_number.filter(|_| !no_agent) {
        Some(n) => review_marker::hold(
            &main_worktree_root_from(project_root),
            review_marker::Marker::for_this_process(
                n,
                review_marker_head_best_effort(project_root, n, &surface_branch).as_deref(),
                Some(&spec_id),
                &format!("`aida review {spec_id}`"),
            ),
        )
        .map_err(|e| {
            eprintln!(
                "  {} could not mark PR-{n} as under review ({e}) — continuing",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            )
        })
        .ok(),
        None => None,
    };

    // BUG-510: stale-base pre-flight — same predicate as the reviewer-role
    // path (`aida queue work <PR-N> --for reviewer` / orchestrator phase 3:
    // preflight_stale_base_check → classify_stale_base), different posture.
    // A human is driving this verb and the verdict on the code is valid
    // either way, so even the overlap case warns-and-proceeds instead of
    // refusing. `--allow-stale-base` mirrors the reviewer-role opt-out so a
    // deliberate stale review isn't nagged. Fails open on infra errors
    // (gh missing, fetch failure) like both existing call sites.
    // trace:BUG-510 | ai:claude
    if let Some(n) = pr_number {
        if !allow_stale_base && matches!(forge, crate::forge::ForgeKind::GitHub) {
            let stale = match preflight_stale_base_check(project_root, n) {
                Ok(pr_rebase::StaleBaseOutcome::Current) => None,
                Ok(pr_rebase::StaleBaseOutcome::StaleNoOverlap { behind }) => {
                    Some((behind, Vec::new()))
                }
                Ok(pr_rebase::StaleBaseOutcome::StaleOverlap {
                    behind,
                    overlap_files,
                    ..
                }) => Some((behind, overlap_files)),
                Err(e) => {
                    eprintln!(
                        "  {} stale-base check for {change_noun}-{n} failed ({e}); \
                         proceeding with review",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                    );
                    None
                }
            };
            if let Some((behind, overlap)) = stale {
                eprintln!(
                    "  {} {}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                    pr_rebase::stale_base_review_warn_message(n, behind, &overlap).yellow()
                );
                // Offer the rebase inline while a human is at the prompt —
                // the review then runs against current code. Defaults to yes
                // when a file the PR touches has also moved on the base.
                if interactive {
                    let card = review_rebase_context_card(change_noun, n, behind, &overlap);
                    let diffstat = review_diffstat(project_root, &surface_branch);
                    let rebase_now = context_prompt::confirm_with_context_and_ai(
                        &format!("Rebase {change_noun}-{n} now?"),
                        !overlap.is_empty(),
                        &card,
                        |out| {
                            ask_ai_review_once(project_root, &req, &spec_id, &card, &diffstat, out)
                        },
                    )
                    .unwrap_or(false);
                    if rebase_now {
                        if let Err(e) = pr_rebase_handler(n, false, false, false, None, None) {
                            eprintln!(
                                "  {} rebase did not complete ({e}); the review \
                                 will run against the stale base",
                                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                            );
                        }
                    }
                }
            }
        }
    }

    if no_agent {
        // AC: degrade-honest path — surface + recommended command, no agent.
        match pr_number {
            Some(n) => {
                let diff_cmd = forge
                    .change_cmd_hint("diff", &n.to_string())
                    .unwrap_or_else(|| format!("inspect change {n}"));
                println!(
                    "\n  {} review the diff yourself: {}",
                    "→".green(),
                    diff_cmd.cyan()
                );
            }
            None => println!(
                "\n  {} no open {change_noun} to diff.",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
            ),
        }
        return Ok(());
    }

    // The verdict file the `/aida-review` skill writes (it keys off this env
    // var — same handshake the standalone reviewer uses). trace:BUG-226
    // TASK-1460: the SAME path `review_verdict::record_verdict` writes, so
    // the skill's direct write and the record below are one file, one round.
    // trace:TASK-1460 | ai:claude
    let verdict_path = review_verdict::verdict_path(project_root, &spec_id);
    if let Some(dir) = verdict_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::remove_file(&verdict_path);
    std::env::set_var("AIDA_REVIEW_VERDICT_FILE", &verdict_path);

    // The reviewer reads the actual DIFF against `## Acceptance` (AC-2).
    let prompt = match pr_number {
        Some(n) => format!("/aida-review --pr {n}"),
        None => "/aida-review".to_string(),
    };
    let session_id = uuid::Uuid::new_v4().to_string();

    println!(
        "\n  {} running reviewer over the diff against {}'s acceptance criteria…",
        crate::glyph(crate::glyphs::Glyph::FlowActive)
            .green()
            .bold(),
        spec_id.cyan()
    );

    // BUG-721: a non-interactive `aida review` was refused above, so a human is
    // at the terminal here — spawn the INTERACTIVE reviewer only, and let them
    // watch it under the native permission posture. This verb never launches a
    // blind headless reviewer; the orchestrator owns that (see `run_reviewer`).
    debug_assert!(
        interactive,
        "BUG-721: non-interactive review must be refused before the reviewer launch"
    );
    let name = format!("review-{}", spec_id.to_ascii_lowercase());
    // BUG-1607: launch through the SAME shared plan resolver
    // `run_standalone_reviewer` uses, instead of hardcoding `claude` — see
    // `review_spec_launch_reviewer`'s doc. `review_vendor` is always `Some`
    // here — the only `None` arm (`no_agent`) already returned above.
    // trace:BUG-1607 | ai:claude
    let vendor = review_vendor
        .expect("review_vendor is Some whenever no_agent is false (already returned above)");
    let status: std::process::ExitStatus =
        review_spec_launch_reviewer(vendor, &name, &prompt, &session_id)?;
    if !status.success() {
        eprintln!(
            "  {} the reviewer exited non-zero ({})",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            status
        );
    }

    // ---- Read + present the verdict (AGENT ANALYZES + RECOMMENDS) ----
    // TASK-1460: the skill wrote this file directly; archive its round
    // (when it names a commit) before the record below stamps provenance.
    // trace:TASK-1460 | ai:claude
    let _ = review_verdict::adopt_direct_write(&verdict_path);
    let verdict = std::fs::read_to_string(&verdict_path)
        .ok()
        .and_then(|body| reviewer_summary::parse_verdict_file(&body));

    // BUG-775: stamp WHICH COMMIT this review looked at (and when) onto the
    // verdict record. The reviewer skill writes the verdict word; only AIDA
    // knows the branch tip it was pointed at. Without that sha "has the branch
    // moved since the review?" is unanswerable, and the `queue done` gate has
    // to fail closed. Best-effort — never blocks presenting the verdict.
    // trace:BUG-775 | ai:claude
    if let Some(v) = verdict.as_ref() {
        let reviewed_branch = linkage.branch.clone();
        let reviewed_sha =
            resolve_commit_sha(project_root, reviewed_branch.as_deref().unwrap_or("HEAD"))
                .or_else(|| resolve_commit_sha(project_root, "HEAD"));
        let _ = review_verdict::record_verdict(
            project_root,
            &spec_id,
            Some(&v.verdict),
            reviewed_sha.as_deref(),
            reviewed_branch.as_deref(),
            v.summary.as_deref(),
            &[],
            "aida review",
        );
    }

    println!();
    let recommended = match &verdict {
        Some(v) => {
            // trace:BUG-1505 | ai:claude — the one canonical parser.
            let kind = review_verdict::VerdictKind::parse(&v.verdict);
            let label = match kind {
                review_verdict::VerdictKind::Approved => "APPROVE".green().bold().to_string(),
                review_verdict::VerdictKind::RequestChanges => {
                    "REQUEST CHANGES".yellow().bold().to_string()
                }
                review_verdict::VerdictKind::Rejected => "REJECT".red().bold().to_string(),
                review_verdict::VerdictKind::Unknown => v.verdict.clone(),
            };
            println!("  {} {}", "Reviewer verdict:".bold(), label);
            if let Some(s) = v.summary.as_deref().filter(|s| !s.trim().is_empty()) {
                println!("  {} {}", "Summary:".bold(), s);
            }
            if let Some(url) = v.comment_url.as_deref().filter(|s| !s.is_empty()) {
                println!("  {} {}", "Review comment:".bold(), url.dimmed());
            }
            review_verdict::canonical_verdict_word(&v.verdict)
        }
        None => {
            println!(
                "  {} the reviewer produced no verdict file — review against {} yourself.",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                spec_id.cyan()
            );
            String::new()
        }
    };

    // ---- HUMAN DECIDES + ACTS (AC-3). NEVER auto-merge (AC-3 / STORY-529) ----
    if !interactive {
        // No human to prompt — surface the recommended next command honestly.
        let recommend = match recommended.as_str() {
            // No --delete-branch: the implementer's worktree may still hold
            // the PR branch, so the delete's local-cleanup step would fail;
            // `;` keeps the auto-bump pull from being dropped by a broken
            // chain. Worktree cleanup owns branch deletion.
            // trace:BUG-758 | ai:claude
            "approved" => match pr_number {
                Some(n) => format!("gh pr merge {n} --squash; aida pull"),
                None => format!("open a {change_noun}, then merge"),
            },
            _ => format!("aida queue rework {spec_id}"),
        };
        println!("\n  {} recommended next: {}", "→".green(), recommend.cyan());
        return Ok(());
    }

    let merge_label = "Approve → print the merge command";
    let rework_label = "Request changes → aida queue rework";
    let diff_label = "Open the diff myself";
    let defer_label = "Defer (leave it held)";
    let options = vec![merge_label, rework_label, diff_label, defer_label];
    let choice = match inquire::Select::new("What now?", options).prompt() {
        Ok(c) => c,
        Err(_) => {
            println!(
                "  {} deferred — the spec stays held.",
                crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed()
            );
            return Ok(());
        }
    };

    if choice == merge_label {
        // NEVER auto-merge — print the paste-ready command for the human to
        // run. The review:draft-only gate exists precisely so a person, not
        // this verb, performs the merge. trace:STORY-553 | ai:claude
        match pr_number {
            Some(n) => {
                // No --delete-branch: the implementer's worktree may still
                // hold the PR branch (local-cleanup refusal); `;` so the
                // auto-bump pull always runs. trace:BUG-758 | ai:claude
                println!(
                    "\n  {} run to merge: {}",
                    "→".green(),
                    format!("gh pr merge {n} --squash; aida pull").cyan()
                );
                println!(
                    "  {} `aida pull` auto-bumps the spec Done → Completed once the merge lands; branch cleanup is deferred to worktree cleanup.",
                    crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed()
                );
            }
            None => println!(
                "\n  {} open a {change_noun} first, then merge it.",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
            ),
        }
    } else if choice == rework_label {
        // Route to rework in-process (reuse the existing handler).
        let storage = Storage::new(store_path);
        handle_queue_rework(
            &storage, &spec_id, false, None, false, None, None, false, false, false, None, false,
            None,
        )?;
    } else if choice == diff_label {
        match pr_number {
            Some(n) => {
                let diff_cmd = forge
                    .change_cmd_hint("diff", &n.to_string())
                    .unwrap_or_else(|| format!("inspect change {n}"));
                println!("  {} {}", "→".green(), diff_cmd.cyan());
                // Route the interactive diff opener through the Forge trait so
                // GitLab review surfaces run `glab mr diff`, not `gh pr diff`.
                // trace:STORY-1164 | ai:codex
                if let Err(e) = crate::forge::forge_for_kind(project_root, forge).diff_change(n) {
                    eprintln!(
                        "  {} could not open {change_noun}-{n} diff ({e:#})",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                    );
                }
            }
            None => println!(
                "  {} no open {change_noun} to diff.",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
            ),
        }
    } else {
        println!(
            "  {} deferred — the spec stays held.",
            crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed()
        );
    }

    Ok(())
}

pub(crate) fn handle_review_command(cmd: &ReviewCommand, storage: &Storage) -> Result<()> {
    match cmd {
        ReviewCommand::Prompt {
            specs,
            pr,
            forge,
            write,
        } => generate_review_prompt(
            storage,
            specs.as_deref(),
            *pr,
            forge.as_deref(),
            write.as_deref(),
        ),
        ReviewCommand::Assemble { output } => {
            let project_root = find_project_root()?;
            let out_path = rules_sync::assemble_review_md(&project_root, output.as_deref())?;
            println!(
                "{} Assembled root REVIEW.md at {}",
                crate::glyph(crate::glyphs::Glyph::Check),
                out_path.display()
            );
            Ok(())
        }
        // trace:BUG-775 | ai:claude
        ReviewCommand::Record {
            spec,
            verdict,
            sha,
            branch,
            summary,
            finding,
            finding_class,
            pr,
        } => handle_review_record(
            spec,
            verdict,
            sha.as_deref(),
            branch.as_deref(),
            summary.as_deref(),
            finding,
            finding_class,
            *pr,
        ),
        // trace:STORY-1417 | ai:claude
        ReviewCommand::Classes { since, json } => handle_review_classes(since.as_deref(), *json),
        // trace:STORY-1405 | ai:claude
        ReviewCommand::Claim {
            pr,
            sha,
            spec,
            ttl_mins,
            release,
        } => handle_review_claim(*pr, sha.as_deref(), spec.as_deref(), *ttl_mins, *release),
        // trace:BUG-775 | ai:claude
        ReviewCommand::Verdict { spec, json } => handle_review_verdict_show(spec, *json),
        ReviewCommand::List { json } => handle_review_list(*json),
        // trace:BUG-1516 | ai:claude
        ReviewCommand::NormalizeShas { dry_run } => handle_review_normalize_shas(*dry_run),
        // trace:TASK-1307 | ai:claude
        // trace:TASK-1423 | ai:claude
        ReviewCommand::Stranded { json, fix, age } => handle_review_stranded(*json, *fix, *age),
        // trace:STORY-1415 | ai:claude
        ReviewCommand::Mode { mode } => match mode {
            cli::ReviewModeCommand::MassChange { action, days, json } => {
                mass_change::handle_mass_change_command(action.as_str(), *days, *json)
            }
        },
    }
}

/// `aida review normalize-shas` — BUG-1516 criterion 3's repair verb: expand
/// every abbreviated `reviewed_sha` on disk to its full commit sha where this
/// repo can still resolve it, and mark the rest unresolvable rather than
/// guessing. Deliberately never runs on its own — `.aida/review-verdicts/` is
/// live coordination state other seats may be reading right now, so touching
/// it is always an explicit, operator-invoked action.
// trace:BUG-1516 | ai:claude
pub(crate) fn handle_review_normalize_shas(dry_run: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let report = review_verdict::backfill_abbreviated_shas(&project_root, dry_run)
        .with_context(|| "could not sweep .aida/review-verdicts for abbreviated shas")?;
    let verb = if dry_run { "would resolve" } else { "resolved" };
    println!(
        "{} {} {} abbreviated sha(s), {} unresolvable, {} already full, {} with no sha",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        verb,
        report.resolved.len(),
        report.unresolvable.len(),
        report.already_full,
        report.skipped_no_sha
    );
    for name in &report.resolved {
        println!("  {} {name}", "→".green());
    }
    for name in &report.unresolvable {
        println!(
            "  {} {name} (kept verbatim, marked reviewed_sha_unresolvable)",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        );
    }
    Ok(())
}

/// TASK-1307: read-only sweep for specs stranded by a refusal recorded
/// before BUG-1452 started protecting new ones — refused at the PR's
/// CURRENT head, spec still Done, no merge hold, no queue entry to re-drive
/// it. Walks the bounded `.aida/review-verdicts/` directory (never a full
/// store scan), asks the forge for each candidate's LIVE PR head sha so a
/// refusal against a superseded head is never mistaken for one against the
/// current head, and reports without mutating anything.
// trace:TASK-1307 | ai:claude
pub(crate) fn run_stranded_sweep(
    project_root: &std::path::Path,
) -> Result<Vec<stranded_sweep::StrandedRow>> {
    let dir = project_root.join(".aida").join("review-verdicts");
    let mut spec_ids: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !stranded_sweep::is_spec_keyed_verdict_filename(stem) {
                continue;
            }
            spec_ids.push(stem.to_string());
        }
    }
    spec_ids.sort();
    spec_ids.dedup();
    if spec_ids.is_empty() {
        return Ok(Vec::new());
    }

    let Some(store_path) = detect_distributed_store_from(project_root) else {
        return Ok(Vec::new());
    };
    let dispenser = load_dispenser(&store_path)?;
    let inner = aida_core::GitBackend::new(&store_path)?.with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path)?;

    // Condition 3 ("no queue entry exists to re-drive it") reads every
    // user's queue. Queue files are small and few — this is not the
    // full-store scan the storage-model convention warns against.
    let mut queued_ids: std::collections::HashSet<uuid::Uuid> = std::collections::HashSet::new();
    if let Ok(users) = backend.queue_users() {
        for user in users {
            if let Ok(entries) = backend.queue_list(&user, true) {
                for e in entries {
                    queued_ids.insert(e.requirement_id);
                }
            }
        }
    }

    let mut rows = Vec::new();
    for spec_id in spec_ids {
        let Some(verdict) = review_verdict::read_recorded_verdict(project_root, &spec_id) else {
            continue;
        };
        if !verdict.kind.blocks_done() {
            continue;
        }
        let Ok(Some(req)) = backend.get_requirement_by_spec_id(&spec_id) else {
            continue;
        };
        let status_is_done = matches!(req.status, aida_core::RequirementStatus::Done);

        // BUG-1454's search-by-spec-id open-PR lookup. `None` means the
        // forge lookup itself failed; `Some(..)` without this spec means no
        // open PR was found. Either way there is nothing to strand without
        // a live open PR, so skip rather than guess one.
        let Some(open_prs) = specs_with_open_prs(project_root, [spec_id.clone()]) else {
            continue;
        };
        let Some(&pr) = open_prs.get(&spec_id) else {
            continue;
        };

        let mut sink = crate::network_retry::NoopSink;
        let Ok(meta) = forge::forge_for(project_root).change_metadata(pr, &mut sink) else {
            continue;
        };
        if meta.state != forge::ChangeState::Open {
            continue;
        }

        // No local ancestry probe: comparing the verdict's `reviewed_sha`
        // directly against the forge-reported live head is sufficient to
        // tell "refused at the current head" (exact match) from "refused at
        // a superseded head" (any mismatch reads as `Unknown` here, which
        // `classify_stranded` treats identically to a confirmed rewrite —
        // never flagged) — acceptance criterion 2, with no need to fetch
        // the branch locally.
        let relation = review_verdict::classify_tip_relation(
            verdict.reviewed_sha.as_deref(),
            Some(meta.head_sha.as_str()),
            None,
        );
        let hold_present = merge_hold::read_hold(project_root, pr).is_some();
        let queue_entry_present = queued_ids.contains(&req.id);

        let conditions = stranded_sweep::classify_stranded(
            status_is_done,
            hold_present,
            queue_entry_present,
            Some(&verdict),
            relation,
        );
        if conditions.is_stranded() {
            rows.push(stranded_sweep::StrandedRow {
                spec_id: req.display_id(),
                pr,
                conditions,
                verdict_summary: verdict.summary.clone(),
                reviewed_sha: verdict.reviewed_sha.clone(),
                current_head: Some(meta.head_sha.clone()),
            });
        }
    }
    Ok(rows)
}

/// TASK-1423: resolve a branch's CURRENT tip commit from LOCAL git objects
/// only — no `git fetch`, no forge call. Tries the local branch first (a
/// still-live worktree may have it), then the `origin` remote-tracking ref
/// (what a fetched-but-locally-deleted branch leaves behind — the normal
/// shape after a session that pushed and exited without merging). `None`
/// when neither resolves: offline and honest about what it doesn't know,
/// never a guess.
// trace:TASK-1423 | ai:claude
pub(crate) fn local_branch_tip(project_root: &std::path::Path, branch: &str) -> Option<String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return None;
    }
    for refname in [
        format!("refs/heads/{branch}"),
        format!("refs/remotes/origin/{branch}"),
    ] {
        let Ok(out) = std::process::Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(["rev-parse", "--verify", "--quiet", &refname])
            .output()
        else {
            continue;
        };
        if !out.status.success() {
            continue;
        }
        let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !sha.is_empty() {
            return Some(sha);
        }
    }
    None
}

/// TASK-1423: the offline counterpart to [`run_stranded_sweep`] — every spec
/// whose last recorded verdict still blocks done, with no forge call: the
/// "did anything land since the review" check comes from `local_branch_tip`
/// (LOCAL git only) instead of asking the forge for the PR's live head, so
/// this stays fast and answerable with no network and no open-PR
/// requirement. Same file-discovery pass as `run_stranded_sweep` (every
/// spec-keyed file under `.aida/review-verdicts/`), so the two reports can
/// never disagree about which files exist.
// trace:TASK-1423 | ai:claude
pub(crate) fn run_stalled_sweep(
    project_root: &std::path::Path,
) -> Result<Vec<stranded_sweep::StalledRow>> {
    let dir = project_root.join(".aida").join("review-verdicts");
    let mut spec_ids: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !stranded_sweep::is_spec_keyed_verdict_filename(stem) {
                continue;
            }
            spec_ids.push(stem.to_string());
        }
    }
    spec_ids.sort();
    spec_ids.dedup();
    if spec_ids.is_empty() {
        return Ok(Vec::new());
    }

    let Some(store_path) = detect_distributed_store_from(project_root) else {
        return Ok(Vec::new());
    };
    let dispenser = load_dispenser(&store_path)?;
    let inner = aida_core::GitBackend::new(&store_path)?.with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path)?;

    let now = chrono::Utc::now();
    let mut rows = Vec::new();
    for spec_id in spec_ids {
        let Some(verdict) = review_verdict::read_recorded_verdict(project_root, &spec_id) else {
            continue;
        };
        if !verdict.kind.blocks_done() {
            continue;
        }
        let Ok(Some(req)) = backend.get_requirement_by_spec_id(&spec_id) else {
            continue;
        };
        let spec_completed = matches!(req.status, aida_core::RequirementStatus::Completed);
        if !review_verdict::is_outstanding_refusal(&verdict, spec_completed) {
            continue;
        }

        // No forge, no gh: the branch tip comes from whatever local git
        // already has on disk. `verdict.reviewed_branch` is what
        // `aida review record` stamps (defaulting to the checked-out branch
        // at record time), so it names the branch to look up.
        let tip = verdict
            .reviewed_branch
            .as_deref()
            .and_then(|b| local_branch_tip(project_root, b));
        let relation = review_verdict::classify_tip_relation(
            verdict.reviewed_sha.as_deref(),
            tip.as_deref(),
            None,
        );
        if !stranded_sweep::is_stalled(&verdict, spec_completed, relation) {
            continue;
        }

        let age = awaiting_you::blocked_age(verdict.recorded_at.as_deref(), now);
        rows.push(stranded_sweep::StalledRow {
            spec_id: req.display_id(),
            verdict_kind: verdict.kind,
            reviewed_sha: verdict.reviewed_sha.clone(),
            reviewed_branch: verdict.reviewed_branch.clone(),
            recorded_at: verdict.recorded_at.clone(),
            summary: verdict.summary.clone(),
            stall_secs: age.map(|a| a.secs),
            overdue: age.is_some_and(|a| a.overdue),
        });
    }
    stranded_sweep::sort_by_staleness(&mut rows);
    Ok(rows)
}

/// `aida review stranded --age` — TASK-1423. Answers "which specs have a
/// review verdict with no subsequent run, and for how long" entirely
/// offline: the per-spec verdict files plus local git, no forge call, no
/// open-PR requirement. Split out of BUG-1470 acceptance criterion 3 — the
/// session that filed it diagnosed a throughput stall as ten already-refused
/// PRs with zero head movement and no owner, invisible because nothing
/// reported "verdict, no subsequent run".
// trace:TASK-1423 | ai:claude
pub(crate) fn handle_review_stalled(project_root: &std::path::Path, json: bool) -> Result<()> {
    let rows = run_stalled_sweep(project_root)?;

    if json {
        let arr: Vec<_> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "spec": r.spec_id,
                    "verdict": r.verdict_kind.label(),
                    "reviewed_sha": r.reviewed_sha,
                    "reviewed_branch": r.reviewed_branch,
                    "recorded_at": r.recorded_at,
                    "summary": r.summary,
                    "stall_seconds": r.stall_secs,
                    "stall": r
                        .stall_secs
                        .map(crate::last_drain::format_age)
                        .unwrap_or_else(|| "unknown".to_string()),
                    "overdue": r.overdue,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "count": rows.len(),
                "stalled": arr,
            }))?
        );
        return Ok(());
    }

    if rows.is_empty() {
        println!(
            "{} no stalled review verdicts found",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
        return Ok(());
    }
    println!(
        "{} {} spec(s) with a review verdict and no subsequent run — oldest first:",
        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
        rows.len()
    );
    for row in &rows {
        let stall = row
            .stall_secs
            .map(crate::last_drain::format_age)
            .unwrap_or_else(|| "unknown".to_string());
        let overdue = if row.overdue { " — overdue" } else { "" };
        println!(
            "  {} {} at {} — {stall}{overdue}",
            row.spec_id.cyan(),
            row.verdict_kind.label(),
            row.reviewed_sha
                .as_deref()
                .map(review_verdict::short_sha)
                .unwrap_or("?"),
        );
        if let Some(s) = &row.summary {
            println!("      {}", s.dimmed());
        }
    }
    Ok(())
}

/// `aida review stranded` — TASK-1307. Default is a read-only report;
/// `--fix` is the separate explicit remediation pass (acceptance 3),
/// applying the same protection BUG-1452 now gives a fresh refusal: a merge
/// hold plus parking the spec in Needs Attention. Idempotent (acceptance
/// 4) because it re-derives the stranded set from live state each call — a
/// spec the previous `--fix` already held/parked no longer classifies as
/// stranded, so a second run changes nothing.
///
/// `--age` (TASK-1423) is a different report over the same directory of
/// verdict files: offline (local git only, no forge lookup, so it never
/// requires an open PR or `gh`), answering "which specs have a verdict with
/// no subsequent run, and for how long" — see `handle_review_stalled`.
// trace:TASK-1307 | ai:claude
pub(crate) fn handle_review_stranded(json: bool, fix: bool, age: bool) -> Result<()> {
    let project_root = find_project_root()?;
    if age {
        return handle_review_stalled(&project_root, json);
    }
    let rows = run_stranded_sweep(&project_root)?;

    if fix {
        let mut fixed_specs = Vec::new();
        for row in &rows {
            let reason = format!(
                "stranded refusal recovered for {} at {}",
                row.spec_id,
                row.reviewed_sha
                    .as_deref()
                    .map(review_verdict::short_sha)
                    .unwrap_or("unknown"),
            );
            // trace:STORY-1397 | ai:claude — a stranded refusal is rework.
            // BUG-1532 criterion 10: reference the verdict record, not a quote.
            // trace:BUG-1532 | ai:claude
            let mut hold = merge_hold::typed_hold(
                row.pr,
                merge_hold::HoldReasonKind::Rework,
                &reason,
                row.reviewed_sha.clone(),
            );
            let current = review_verdict::read_recorded_verdict(&project_root, &row.spec_id);
            hold.verdict_ref = Some(merge_hold::VerdictRef::new(
                &row.spec_id,
                Some(row.pr),
                row.reviewed_sha.clone(),
                current.and_then(|v| v.recorded_by),
            ));
            hold.spec = Some(row.spec_id.to_ascii_uppercase());
            merge_hold::write_typed_hold(&project_root, &hold)
                .with_context(|| format!("could not protect PR-{} with a merge hold", row.pr))?;
            let detail = row
                .verdict_summary
                .clone()
                .unwrap_or_else(|| "stranded refusal recovered by sweep".to_string());
            let recovery = format!(
                "address the review findings, move {} back to In Progress, and clear the PR hold only after approval",
                row.spec_id
            );
            if shelve_spec_on_failure(
                &project_root,
                &row.spec_id,
                "reviewer",
                3,
                "verdict:stranded",
                &detail,
                &recovery,
            )?
            .is_some()
            {
                fixed_specs.push(row.spec_id.clone());
            }
        }
        let remaining = run_stranded_sweep(&project_root)?;
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "fixed": fixed_specs,
                    "remaining": remaining.iter().map(|r| &r.spec_id).collect::<Vec<_>>(),
                }))?
            );
        } else {
            println!(
                "{} recovered {} stranded spec(s); {} still require attention",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                fixed_specs.len(),
                remaining.len(),
            );
            for spec in &fixed_specs {
                println!("  {} {}", "→".green(), spec.cyan());
            }
            for row in &remaining {
                println!(
                    "  {} {} (PR-{}) — could not be parked; check manually",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    row.spec_id.cyan(),
                    row.pr
                );
            }
        }
        return Ok(());
    }

    // Acceptance 5: the count, once measured, is recorded durably rather
    // than only printed to a scrollback that will be gone by the time
    // anyone asks how many there were. Best-effort, append-only, and never
    // touches a spec — recording a measurement is not remediation.
    record_stranded_sweep_measurement(&project_root, rows.len());

    if json {
        let arr: Vec<_> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "spec": r.spec_id,
                    "pr": r.pr,
                    "status_not_moved": r.conditions.status_not_moved,
                    "hold_absent": r.conditions.hold_absent,
                    "queue_entry_absent": r.conditions.queue_entry_absent,
                    "verdict_refusing_at_head": r.conditions.verdict_refusing_at_head,
                    "reviewed_sha": r.reviewed_sha,
                    "current_head": r.current_head,
                    "summary": r.verdict_summary,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "count": rows.len(),
                "stranded": arr,
            }))?
        );
        return Ok(());
    }

    if rows.is_empty() {
        println!(
            "{} no stranded specs found",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
        return Ok(());
    }
    println!(
        "{} {} stranded spec(s) — refused at the current head, still Done, unheld, unqueued:",
        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
        rows.len()
    );
    for row in &rows {
        println!(
            "  {} PR-{} — refused at {}",
            row.spec_id.cyan(),
            row.pr,
            row.reviewed_sha
                .as_deref()
                .map(review_verdict::short_sha)
                .unwrap_or("?"),
        );
        if let Some(s) = &row.verdict_summary {
            println!("      {}", s.dimmed());
        }
    }
    println!(
        "  {} remediate with `aida review stranded --fix`",
        "→".dimmed()
    );
    Ok(())
}

/// Best-effort durable log of each real measurement — append-only, never
/// touches a spec. `.aida/stranded-sweep-history.jsonl` is local runtime
/// state (unshared, like `.aida/review-verdicts/`), but it is the record
/// TASK-1307's acceptance 5 asks for: the pre-sweep count is unrecoverable
/// once the underlying PRs are merged or closed, so the first real count
/// this sweep ever produces must not evaporate with scrollback.
// trace:TASK-1307 | ai:claude
pub(crate) fn record_stranded_sweep_measurement(project_root: &std::path::Path, count: usize) {
    let path = project_root
        .join(".aida")
        .join("stranded-sweep-history.jsonl");
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let line = serde_json::json!({
        "measured_at": chrono::Utc::now().to_rfc3339(),
        "count": count,
    });
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{line}");
    }
}

pub(crate) fn guided_review_prompt(spec: &str) -> String {
    format!(
        "Read your AIDA launch context first:\n\
         cat \"$AIDA_AGENT_CONTEXT_FILE\"\n\
         aida show {spec}\n\
         aida review {spec} --no-agent\n\n\
         You are the guided-review advisor for {spec}. Keep this interactive and bounded: \
         inspect the linked review surface, compare the diff to the spec acceptance criteria, \
         surface consequential review forks to the operator with concrete options and a \
         recommendation, and finish by telling the operator whether to approve, request changes, \
         inspect the diff manually, or defer. Do not auto-merge."
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GuidedReviewLaunchRequest {
    pub(crate) role: Option<String>,
    pub(crate) spec: Option<String>,
    pub(crate) prompt: String,
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
}

pub(crate) fn guided_review_launch_request(spec: &str) -> GuidedReviewLaunchRequest {
    GuidedReviewLaunchRequest {
        role: Some("advisor".to_string()),
        // A guided review shell is an advisor reviewing an already-held surface,
        // not a new implementation claim. Keep the spec in the prompt/name only
        // so `agent new` does not run `session_start` for a Done spec.
        // trace:STORY-818 | ai:codex
        spec: None,
        prompt: guided_review_prompt(spec),
        name: Some(format!("guided-review-{}", slugify(spec))),
        description: Some(format!("guided review for {spec}")),
    }
}

pub(crate) fn derisk_prompt(spec: &str) -> String {
    format!(
        "Read your AIDA launch context first:\n\
         cat \"$AIDA_AGENT_CONTEXT_FILE\"\n\
         /aida-derisk {spec}\n\n\
         You are the de-risk advisor for {spec}. Follow the /aida-derisk skill: \
         read the spec and graph, classify decision/blast-radius risk, surface \
         load-bearing forks to the operator, record decisions as ADRs, fold \
         decisions and testable safety gates into acceptance, lint the result, \
         and only write a new execution_mode after explicit operator confirmation. \
         Do not implement the spec."
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeriskLaunchRequest {
    pub(crate) role: Option<String>,
    pub(crate) spec: Option<String>,
    pub(crate) prompt: String,
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
}

// trace:TASK-1235 | ai:codex
pub(crate) fn derisk_launch_request(spec: &str) -> DeriskLaunchRequest {
    DeriskLaunchRequest {
        role: Some("advisor".to_string()),
        // De-risking is an advisor disposition session, not an implementation
        // claim. Keep the target in the prompt/name so the launcher does not
        // acquire an implementer-style spec worktree or mutate status.
        spec: None,
        prompt: derisk_prompt(spec),
        name: Some(format!("derisk-{}", slugify(spec))),
        description: Some(format!("de-risk {spec}")),
    }
}

/// `aida derisk <SPEC>` — vendor-aware launcher for the `/aida-derisk` skill.
/// The Rust command is deliberately thin: it starts an advisor session seeded
/// with the skill prompt and leaves all mode decisions inside the skill's
/// explicit-confirmation workflow.
// trace:TASK-1235 | ai:codex
pub(crate) fn handle_derisk_command(spec: &str) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("aida derisk needs an interactive terminal");
    }
    let vendor = session::resolve_session_vendor();
    let launch = derisk_launch_request(spec);
    match vendor {
        session::HeadlessVendor::Claude => agent_new_claude(
            launch.role,
            launch.spec,
            false,
            None,
            None,
            false,
            AgentContextOptions::new(true, false),
            true,
            false,
            false,
            false,
            AgentPromptOptions::new(Some(launch.prompt), false),
            AgentResumeOptions::new(false, None, true, false),
            AgentDefaultFlagOptions::new(true, Vec::new(), None),
            launch.name,
            launch.description,
            false,
        ),
        session::HeadlessVendor::Codex => agent_new_codex(
            launch.role,
            launch.spec,
            false,
            None,
            false,
            AgentContextOptions::new(true, false),
            true,
            false,
            false,
            false,
            AgentPromptOptions::new(Some(launch.prompt), false),
            AgentResumeOptions::new(false, None, true, false),
            AgentDefaultFlagOptions::new(true, Vec::new(), None),
            launch.name,
            launch.description,
        ),
        session::HeadlessVendor::Agy => anyhow::bail!(
            "aida derisk does not run on agy; set the session vendor to claude or codex \
             (AIDA_HEADLESS_VENDOR=claude|codex, or `[agents] vendor` in agents.toml)."
        ),
    }
}

/// STORY-818: `aida human review <SPEC> --guided` — spawn an interactive
/// advisor shell for review, using the existing `aida agent new` launch-context
/// machinery instead of inventing a second session substrate. The launched
/// agent deliberately has no `--spec` scope: review normally runs while the spec
/// is Done, and a spec-scoped `agent new` means implementation lease/worktree
/// semantics.
// trace:STORY-818 | ai:codex
pub(crate) fn handle_guided_human_review(spec: &str) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("guided review needs an interactive terminal");
    }
    let vendor = session::resolve_session_vendor();
    let launch = guided_review_launch_request(spec);
    match vendor {
        session::HeadlessVendor::Claude => agent_new_claude(
            launch.role,
            launch.spec,
            false,
            None,
            None,
            false,
            AgentContextOptions::new(true, false),
            true,
            false,
            false,
            false,
            AgentPromptOptions::new(Some(launch.prompt), false),
            AgentResumeOptions::new(false, None, true, false),
            AgentDefaultFlagOptions::new(true, Vec::new(), None),
            launch.name,
            launch.description,
            false,
        ),
        session::HeadlessVendor::Codex => agent_new_codex(
            launch.role,
            launch.spec,
            false,
            None,
            false,
            AgentContextOptions::new(true, false),
            true,
            false,
            false,
            false,
            AgentPromptOptions::new(Some(launch.prompt), false),
            AgentResumeOptions::new(false, None, true, false),
            AgentDefaultFlagOptions::new(true, Vec::new(), None),
            launch.name,
            launch.description,
        ),
        session::HeadlessVendor::Agy => anyhow::bail!(
            "guided review does not run on agy; set the session vendor to claude or codex \
             (AIDA_HEADLESS_VENDOR=claude|codex, or `[agents] vendor` in agents.toml)."
        ),
    }
}

/// `aida review record` — write the spec's review verdict as state a gate can
/// read. The recorded commit defaults to the branch tip, which is what makes
/// "has the branch moved since the review?" answerable later.
// trace:BUG-775 | ai:claude
/// BUG-802: the drive root a phase session should write orchestrator
/// handshake artifacts into — the env anchor the spawn/exec paths export,
/// falling back to `find_project_root`. The anchor exists because reviewers
/// routinely check the PR out elsewhere; from inside such a checkout,
/// `find_project_root` resolves to the CHECKOUT (it has its own `.aida/`),
/// and a verdict written there is invisible to the orchestrator. Proven live:
/// three consecutive phase-3 shelves on 2026-08-29, the last with a
/// byte-perfect verdict file in the wrong tree.
// trace:BUG-802 | ai:claude
pub(crate) fn drive_root_or_project_root() -> Result<std::path::PathBuf> {
    if let Some(root) = std::env::var_os("AIDA_DRIVE_ROOT") {
        let p = std::path::PathBuf::from(root);
        if p.is_dir() {
            return Ok(p);
        }
    }
    find_project_root()
}

// BUG-912: phase-3 reviewers may run inside the review worktree while the
// orchestrator polls an explicit verdict-file anchor in the parent project.
// The PR handshake must honor that exact file when present; otherwise prefer
// roots that phase 4 can observe before falling back to the worktree.
// trace:BUG-912 | ai:codex
pub(crate) fn review_pr_handshake_path(
    project_root: &std::path::Path,
    pr_number: u64,
) -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("AIDA_REVIEW_VERDICT_FILE")
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
    {
        return path;
    }
    if let Some(root) = std::env::var_os("AIDA_PROJECT_ROOT")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_dir())
    {
        return review_verdict::verdict_path(&root, &format!("PR-{pr_number}"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(root) = parent_project_root_for_session(&cwd).filter(|p| p.is_dir()) {
            return review_verdict::verdict_path(&root, &format!("PR-{pr_number}"));
        }
    }
    if let Some(root) = std::env::var_os("AIDA_DRIVE_ROOT")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_dir())
    {
        return review_verdict::verdict_path(&root, &format!("PR-{pr_number}"));
    }
    review_verdict::verdict_path(project_root, &format!("PR-{pr_number}"))
}

/// `aida review record` — emit the seat-tagged `ReviewVerdictRecorded` event
/// naming the spec (as `Event.spec`), PR, verdict and reviewed sha. Split out
/// for direct unit testing, same rationale as BUG-1423's `emit_ship_pr_merged`
/// in `pr_cmd.rs`. Best-effort: `events::emit` never fails the command.
// trace:TASK-1450 | ai:claude
pub(crate) fn emit_review_verdict_recorded(
    project_root: &std::path::Path,
    spec_display: &str,
    pr: Option<u32>,
    verdict: String,
    reviewed_sha: Option<String>,
) {
    let mut ev = events::Event::new(
        Some(spec_display.to_string()),
        "",
        events::EventKind::ReviewVerdictRecorded {
            pr,
            verdict,
            reviewed_sha,
        },
    );
    ev.seat = events::active_seat();
    events::emit(project_root, &ev);
}

/// STORY-1436: refusal recording seams that live in this module.
// trace:STORY-1436 | ai:claude
#[cfg(test)]
mod story_1436_gate_held_tests {
    use super::*;

    #[test]
    fn ambiguous_id_is_found_through_context_layers() {
        let amb = aida_core::id_collisions::AmbiguousIdError {
            id: "task-7".into(),
            candidates: vec![],
        };
        let err = anyhow::Error::new(amb)
            .context("loading spec")
            .context("aida edit");
        assert_eq!(
            ambiguous_id_in_chain(&err).map(|a| a.id.as_str()),
            Some("task-7")
        );
        assert!(ambiguous_id_in_chain(&anyhow::anyhow!("other")).is_none());
    }

    #[test]
    fn blocked_by_pickup_is_named_distinctly() {
        use aida_core::pickability::BlockedReason;
        use queue_cmd::{pickup_refusal_gate, QueueFreshPickup};
        let blocked = QueueFreshPickup::Blocked(BlockedReason::PermanentlyBlocked {
            target_spec: "TASK-1".into(),
        });
        assert_eq!(
            pickup_refusal_gate(&blocked),
            events::GATE_BLOCKED_BY_PICKUP
        );
        let human = QueueFreshPickup::Blocked(BlockedReason::HumanOnly);
        assert_eq!(pickup_refusal_gate(&human), events::GATE_QUEUE_PICKUP);
        assert_eq!(
            pickup_refusal_gate(&QueueFreshPickup::AwaitingMerge),
            events::GATE_QUEUE_PICKUP
        );
    }

    #[test]
    fn dry_run_pickup_refusal_is_not_recorded() {
        use aida_core::pickability::BlockedReason;
        let blocked = queue_cmd::QueueFreshPickup::Blocked(BlockedReason::PermanentlyBlocked {
            target_spec: "TASK-1".into(),
        });
        let _on = crate::test_env::EnvVarGuard::unset(events::EVENTS_DISABLE_ENV);
        let preview = tempfile::tempdir().unwrap();
        queue_cmd::record_pickup_refusal(preview.path(), &blocked, "S-1".into(), "r", true);
        assert!(events::read_all(preview.path()).is_empty());
        let real = tempfile::tempdir().unwrap();
        queue_cmd::record_pickup_refusal(real.path(), &blocked, "S-1".into(), "r", false);
        let evs = events::read_all(real.path());
        assert_eq!(evs.len(), 1);
        assert!(matches!(
            &evs[0].kind,
            events::EventKind::GateHeld { gate, .. } if gate == events::GATE_BLOCKED_BY_PICKUP
        ));
    }

    #[test]
    fn merge_hold_clear_floor_refusal_is_recorded_before_the_bail() {
        let src = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}",
            include_str!("lib.rs"),
            include_str!("lib_part1.rs"),
            include_str!("lib_part2.rs"),
            include_str!("lib_part3.rs"),
            include_str!("lib_part4.rs"),
            include_str!("lib_part5.rs"),
            include_str!("lib_part6.rs")
        );
        let clear = src
            .find(concat!("MergeHoldAction::", "Clear { pr, stale }"))
            .expect("clear arm");
        let tail = &src[clear..];
        let rec = tail
            .find(concat!("GATE_MERGE_HOLD_", "CLEAR_FLOOR"))
            .expect("floor refusal recorded");
        let bail = tail.find("anyhow::bail!(refusal)").expect("bail");
        assert!(rec < bail);
    }

    #[test]
    fn closure_hold_reason_names_blockers_and_criteria() {
        let hold = ClosureHold {
            flip: AutoBumpFlip::new(
                "STORY-9".to_string(),
                "abcdef1234".to_string(),
                RequirementStatus::Done,
            ),
            blockers: vec![],
            cycle_members: vec![],
            criteria: vec!["docs updated".to_string()],
        };
        let reason = closure_hold_reason(&hold);
        assert!(reason.contains("abcdef1"), "{reason}");
        assert!(reason.contains("docs updated"), "{reason}");
    }
}

#[cfg(test)]
mod task_1450_review_verdict_event_tests {
    use super::*;

    /// A recorded review verdict must emit a seat-tagged event naming the
    /// spec, PR, verdict and reviewed sha — the reviewer-seat coordination
    /// decision the BUG-1423 feed still missed.
    // trace:TASK-1450 | ai:claude
    #[test]
    fn review_record_emits_seat_tagged_spec_pr_verdict_and_sha() {
        let tmp = tempfile::tempdir().unwrap();
        let _seat = crate::test_env::AmbientGuard::hermetic_with_seat(tmp.path(), "reviewer", &[]);

        emit_review_verdict_recorded(
            tmp.path(),
            "BUG-1450",
            Some(2119),
            "approved".to_string(),
            Some("abc123def".to_string()),
        );

        let events = events::read_all(tmp.path());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].spec.as_deref(), Some("BUG-1450"));
        assert_eq!(events[0].seat.as_deref(), Some("reviewer"));
        assert!(matches!(
            &events[0].kind,
            events::EventKind::ReviewVerdictRecorded { pr, verdict, reviewed_sha }
                if *pr == Some(2119)
                    && verdict == "approved"
                    && reviewed_sha.as_deref() == Some("abc123def")
        ));
    }
}

/// BUG-1536 acceptance criteria 2 + 3 + 5: a test-only, construction-time
/// refusal to let [`handle_review_record_at`] write outside a tempdir. Every
/// filesystem write the recording path makes *from the resolved root* (the
/// review verdict and the merge-hold marker) and the one forge call it can
/// make (`merge_hold::sync_label`, which shells out with the SAME root as its
/// cwd — see `run_forge_cli`) are scoped to this one `project_root`. So
/// refusing here, in test builds only, to operate on a root that is not
/// under the process's temp directory is the belt-and-braces net for the
/// whole call: it holds regardless of which ambient env var a test did or
/// didn't set, because it does not consult any env var at all — it only
/// looks at the resolved root the call was actually given. Compiled out
/// entirely in a non-test build (`cfg(test)`), so it costs nothing and
/// changes no production behavior.
///
/// BUG-1743 amends the claim above. It said "every filesystem write ...
/// including the phase-3 handshake ... are scoped to this one
/// `project_root`", and for the handshake that was never true: it resolves
/// its OWN anchor through [`review_pr_handshake_path`], which by design
/// (BUG-912) prefers ambient `AIDA_REVIEW_VERDICT_FILE` /
/// `AIDA_PROJECT_ROOT` / `AIDA_DRIVE_ROOT` over the root passed in. This
/// guard therefore cannot see the handshake write, and a root-under-`/tmp`
/// check could not have caught the leak anyway, because the root it leaked
/// to was another test's tempdir — also under `/tmp`. That gap is what
/// [`assert_handshake_anchor_is_pinned`] closes.
// trace:BUG-1536 | ai:claude
// trace:BUG-1743 | ai:claude
#[cfg(test)]
pub(crate) fn assert_review_write_root_is_isolated(project_root: &std::path::Path) {
    let tmp = std::env::temp_dir();
    let canon_tmp = tmp.canonicalize().unwrap_or(tmp);
    let canon_root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    assert!(
        canon_root.starts_with(&canon_tmp),
        "BUG-1536: handle_review_record_at was about to write a review \
         verdict / merge-hold / forge label outside a tempdir (root = {}, \
         temp dir = {}). This looks like a test resolved its write root \
         ambiently instead of pinning an explicit tempdir — pass a tempdir \
         path in, not an env var.",
        canon_root.display(),
        canon_tmp.display(),
    );
}

/// BUG-1743: a test-only refusal to let the phase-3 handshake be written into
/// a temp tree this test does not own.
///
/// [`review_pr_handshake_path`] deliberately prefers ambient
/// `AIDA_REVIEW_VERDICT_FILE` / `AIDA_PROJECT_ROOT` / `AIDA_DRIVE_ROOT` over
/// the root the call was given — that is BUG-912's orchestrator anchor and it
/// is correct in production, where those vars name a real repository. Under
/// `cargo test` the same preference is a hazard: `std::env` is
/// process-global, so a sibling test on another thread that pins
/// `AIDA_PROJECT_ROOT` at its own `tempfile` root redirects this call's
/// handshake into that root, and when the sibling's `TempDir` drops, the file
/// this call just wrote and verified ceases to exist. That is precisely the
/// two-faced flake BUG-1743 was filed for: the write failing with `ENOENT`
/// when the sibling had already torn down, and the `is_file()` check finding
/// nothing when it tore down a few syscalls later.
///
/// The discriminator is NOT the path — a foreign tempdir and an owned one are
/// both `/tmp/.tmpXXXXXX`. It is [`test_env::holds_env_lock`]: a test that
/// pointed the anchor somewhere on purpose did so under an `EnvVarsGuard`,
/// which holds `ENV_LOCK` for its whole lifetime and so cannot be racing
/// anyone. A test that resolved a foreign anchor while holding nothing
/// inherited it from a sibling, by accident, and is the bug.
///
/// So: an anchor outside `project_root` is allowed only while this thread
/// holds the env lock. Otherwise fail here, loudly, naming both paths —
/// deterministically at the moment of the mistake, instead of as a 2-in-287
/// CI flake whose message points at a file that no longer exists.
// trace:BUG-1743 | ai:claude
#[cfg(test)]
pub(crate) fn assert_handshake_anchor_is_pinned(
    project_root: &std::path::Path,
    handshake: &std::path::Path,
) {
    let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    // The handshake's own parents may not exist yet (the writer creates
    // them), so canonicalize the root and compare against the un-canonical
    // handshake as well as its existing ancestor.
    let canon_root = canon(project_root);
    let inside = handshake.starts_with(project_root) || handshake.starts_with(&canon_root);
    if inside || test_env::holds_env_lock() {
        return;
    }
    panic!(
        "BUG-1743: the phase-3 handshake was about to be written to {}, which \
         is OUTSIDE this call's project root {}, and this thread does not hold \
         ENV_LOCK. That means an ambient AIDA_REVIEW_VERDICT_FILE / \
         AIDA_PROJECT_ROOT / AIDA_DRIVE_ROOT was inherited from a SIBLING test \
         running in parallel, not set by this one. The handshake would land in \
         that sibling's tempdir and vanish when its TempDir drops — the flake \
         BUG-1743 is about. Fix the TEST, not this guard: hold a \
         `test_env::EnvVarsGuard` pinning those three keys (set the roots to \
         your own tempdir, unset AIDA_REVIEW_VERDICT_FILE) for the whole \
         window, which also serialises you against every sibling setter.",
        handshake.display(),
        canon_root.display(),
    );
}

/// STORY-1405: remove the review-in-progress marker(s) a recorded verdict
/// settles — PR-keyed when `pr` is known, otherwise every marker naming
/// `spec`. Checks the given root and the main clone (where merge surfaces
/// look). Returns the PRs cleared.
// trace:STORY-1405 | ai:claude
pub(crate) fn clear_review_markers_for_verdict(
    project_root: &std::path::Path,
    spec: &str,
    pr: Option<u64>,
) -> Vec<u64> {
    let mut roots = vec![project_root.to_path_buf()];
    let main = main_worktree_root_from(project_root);
    if main != project_root {
        roots.push(main);
    }
    let mut cleared = Vec::new();
    for root in &roots {
        let targets: Vec<u64> = match pr {
            Some(n) => vec![n],
            None => review_marker::list(root)
                .into_iter()
                .filter(|m| {
                    m.spec
                        .as_deref()
                        .is_some_and(|s| s.eq_ignore_ascii_case(spec))
                })
                .map(|m| m.pr)
                .collect(),
        };
        for n in targets {
            if review_marker::clear(root, n) {
                cleared.push(n);
            }
        }
    }
    cleared
}

/// `aida review claim` — STORY-1405's deliberate entry point for a review
/// that runs through no other aida verb (criterion 5d): a reviewer seat reading
/// a diff by hand. No process stays behind to watch, so the claim is TTL-only.
// trace:STORY-1405 | ai:claude
pub(crate) fn handle_review_claim(
    pr: u64,
    sha: Option<&str>,
    spec: Option<&str>,
    ttl_mins: u64,
    release: bool,
) -> Result<()> {
    let project_root = drive_root_or_project_root()?;
    let root = main_worktree_root_from(&project_root);
    if release {
        if review_marker::clear(&root, pr) {
            println!(
                "{} released the review claim on PR-{pr}",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
        } else {
            println!("no review claim on PR-{pr} — nothing to release");
        }
        return Ok(());
    }
    anyhow::ensure!(ttl_mins > 0, "`--ttl-mins` must be at least 1");
    // TASK-1459: an explicit claim has no process to watch, so cap it — an
    // unbounded/mistyped TTL could otherwise hold a PR "under review" for
    // days. Clamp with a note rather than refuse, so a generous-but-honest
    // value (or a scripted default) never hard-fails the claim.
    let (ttl_mins, clamped) = review_marker::clamp_claim_ttl_mins(ttl_mins);
    if clamped {
        eprintln!(
            "  {} `--ttl-mins` is capped at {}m (24h) for an explicit claim — clamped",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            review_marker::MAX_CLAIM_TTL_MINS,
        );
    }
    let head = sha
        .map(str::to_string)
        .or_else(|| std::env::var("AIDA_FROM_PR_HEAD_SHA").ok())
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            pr_cmd::fetch_change_info_via_resolved_forge(
                &project_root,
                pr,
                crate::forge::resolve_open_change_forge_kind(&project_root),
            )
            .ok()
            .map(|info| info.head_oid)
        });
    if let Some(prev) = review_marker::read(&root, pr) {
        if review_marker::is_live(&prev) {
            eprintln!(
                "  {} replacing a live review claim on PR-{pr}: {}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                prev.describe()
            );
        }
    }
    let mut marker = review_marker::Marker::for_this_process(
        pr,
        head.as_deref(),
        spec.map(|s| s.to_ascii_uppercase()).as_deref(),
        &review_recorded_by(&project_root).replace("aida review record", "aida review claim"),
    );
    marker.pid = 0;
    marker.ttl_secs = ttl_mins.saturating_mul(60);
    review_marker::write(&root, &marker)
        .with_context(|| format!("could not mark PR-{pr} as under review"))?;
    println!(
        "{} PR-{pr} is marked under review{} for {ttl_mins}m — merges wait for your verdict",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        head.as_deref()
            .map(|h| format!(" at {}", review_verdict::short_sha(h)))
            .unwrap_or_else(|| " (head unknown — every head is covered)".to_string()),
    );
    println!(
        "  {} `aida review record <SPEC> --verdict … --pr {pr}` clears it; \
         `aida review claim --pr {pr} --release` abandons it.",
        crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed()
    );
    Ok(())
}

/// `aida review record`'s CLI entry point. Resolves the write root exactly
/// ONCE (drive root, falling back to the found project root) and hands it to
/// [`handle_review_record_at`] — the ambient env is read here and nowhere
/// else in the recording path. BUG-1536: the previous shape resolved this
/// root here AND separately, deeper in the call, at the merge-hold write
/// site (preferring `AIDA_PROJECT_ROOT`) — two independent ambient reads that
/// could and did disagree (a seat shell has `AIDA_PROJECT_ROOT` set to the
/// real repo; pointing only `AIDA_DRIVE_ROOT` at a tempdir left the merge
/// hold + forge label call resolving against the real repo). A single
/// resolved root threaded explicitly through the whole call cannot diverge.
// trace:BUG-1536 | ai:claude
pub(crate) fn handle_review_record(
    spec: &str,
    verdict: &str,
    sha: Option<&str>,
    branch: Option<&str>,
    summary: Option<&str>,
    findings: &[String],
    finding_classes: &[String],
    pr: Option<u64>,
) -> Result<()> {
    let project_root = drive_root_or_project_root()?;
    handle_review_record_at(
        project_root,
        spec,
        verdict,
        sha,
        branch,
        summary,
        findings,
        finding_classes,
        pr,
    )
}

/// `aida review classes` — count recorded findings per defect class across
/// the whole verdict corpus (current files, per-sha archives, retained
/// rounds). Read-only.
// trace:STORY-1417 | ai:claude
pub(crate) fn handle_review_classes(since: Option<&str>, json: bool) -> Result<()> {
    let project_root = drive_root_or_project_root()?;
    let since = since.map(queue_cmd::parse_since_arg).transpose()?;
    let dir = project_root.join(".aida").join("review-verdicts");
    let report = review_classes::collect_class_report(&dir, since);
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    if report.classes.is_empty() && report.unknown.is_empty() {
        println!(
            "No classified findings{}.",
            if since.is_some() {
                " in that window"
            } else {
                ""
            }
        );
    } else {
        println!(
            "{:<32} {:>8} {:>9}  {}",
            "CLASS".bold(),
            "FINDINGS".bold(),
            "SUBJECTS".bold(),
            "LAST SEEN".bold()
        );
        for c in &report.classes {
            println!(
                "{:<32} {:>8} {:>9}  {}",
                c.class,
                c.findings,
                c.subjects.len(),
                c.last_seen
                    .as_deref()
                    .map(|s| s.get(..10).unwrap_or(s))
                    .unwrap_or("-")
            );
        }
        for (class, n) in &report.unknown {
            println!("{:<32} {:>8}  (not in the vocabulary)", class, n);
        }
    }
    println!(
        "{} classified, {} unclassified findings.",
        report.classified, report.unclassified
    );
    if report.classified == 0 {
        println!(
            "  {} record a class with `aida review record <SPEC> --finding <TEXT> --finding-class <CLASS>`; older findings stay unclassified.",
            crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed()
        );
    }
    Ok(())
}

/// The core of `aida review record`, taking its write root as an explicit
/// parameter rather than resolving it ambiently. Used directly by tests so a
/// test can pin the root to a tempdir and know — by construction, not by
/// convention — that nothing the call does can escape it. `handle_review_record`
/// is the only caller that resolves `project_root` from the environment.
// trace:BUG-1536 | ai:claude
pub(crate) fn handle_review_record_at(
    project_root: std::path::PathBuf,
    spec: &str,
    verdict: &str,
    sha: Option<&str>,
    branch: Option<&str>,
    summary: Option<&str>,
    findings: &[String],
    finding_classes: &[String],
    pr: Option<u64>,
) -> Result<()> {
    #[cfg(test)]
    assert_review_write_root_is_isolated(&project_root);
    // STORY-1417: resolve classes up front; a bad class is a warning, never a
    // reason to refuse the verdict. trace:STORY-1417 | ai:claude
    let (classes, class_warnings) =
        review_classes::resolve_finding_classes(findings, finding_classes);
    for w in &class_warnings {
        eprintln!(
            "  {} {w}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        );
    }
    let kind = review_verdict::VerdictKind::parse(verdict);
    if kind == review_verdict::VerdictKind::Unknown {
        anyhow::bail!(
            "unrecognised verdict `{verdict}` — use one of: approved, request-changes, rejected"
        );
    }
    let branch = branch
        .map(str::to_string)
        .or_else(|| current_branch_at(&project_root));
    // A from-PR reviewer receives the forge-resolved head in its launch
    // envelope. Prefer it to a checkout's HEAD, which may be stale or may be
    // the drive root rather than the review worktree.
    // trace:BUG-1467 | ai:codex
    let envelope_pr_sha = pr
        .and_then(|_| std::env::var("AIDA_FROM_PR_HEAD_SHA").ok())
        .filter(|s| !s.trim().is_empty());
    let forge_pr_sha = if sha.is_none() && envelope_pr_sha.is_none() {
        match pr {
            // TASK-1455: the verdict's head is read from the PINNED repo and
            // refused if the forge answers for any other repo, so a PR number
            // can never stamp this verdict with another repo's head.
            // trace:TASK-1455 | ai:claude
            Some(n) => Some(
                merge_hold::fetch_pinned_change(
                    &project_root,
                    crate::forge::resolve_open_change_forge_kind(&project_root),
                    n,
                )
                .and_then(|change| {
                    change
                        .ok_or_else(|| "this project has no forge (pure-git)".to_string())?
                        .head_sha
                        .ok_or_else(|| format!("the forge did not return a head SHA for #{n}"))
                })
                .map_err(|e| anyhow::anyhow!(e))
                .with_context(|| {
                    format!(
                        "could not resolve PR {n}'s current head for the verdict; pass `--sha <commit>` explicitly"
                    )
                })?,
            ),
            None => None,
        }
    } else {
        None
    };
    let resolved_sha = sha
        .map(str::to_string)
        .or(envelope_pr_sha)
        .or(forge_pr_sha)
        .or_else(|| resolve_commit_sha(&project_root, branch.as_deref().unwrap_or("HEAD")))
        .or_else(|| resolve_commit_sha(&project_root, "HEAD"));
    if resolved_sha.is_none() {
        anyhow::bail!(
            "no commit could be resolved for this review — pass `--sha <commit>`; an unstamped verdict is unverifiable and will not be recorded"
        );
    }

    // BUG-1802: bind recording identity and enforce policy.
    // trace:BUG-1802 | ai:antigravity
    let recorded_by = review_recorded_by(&project_root);
    // BUG-1918: operator attribution stays behind the human-at-TTY floor
    // (BUG-1693); the outcome is stamped into the attestation below so the
    // ship gate can tell a floor-passed human from an unattested label.
    // trace:BUG-1918 | ai:claude
    let human_at_tty = recorded_by == REVIEW_OPERATOR_RECORDED_BY && review_operator_tty_floor()?;
    if crate::seat_authority::current_seat(&project_root).as_deref() == Some("implementer") {
        anyhow::bail!("refused: implementer seat cannot record a review verdict (self-review)");
    }
    // BUG-1918: an approval is refused when THIS session is bound to
    // authoring the work under review — by a lease it holds or once held, its
    // agent session id, or its agent process — identity that persists across
    // shell calls and lease release, never free-text labels.
    // trace:BUG-1918 | ai:claude
    let recorder_grant = crate::seat_authority::current_grant(&project_root);
    let recorder = review_authority::RecorderIdentity::current(
        &project_root,
        &review_authority::visible_leases(&project_root),
    );
    let approving = kind == review_verdict::VerdictKind::Approved;
    if approving {
        let subjects = review_subject_spec_ids(spec, branch.as_deref());
        let authors = review_recorder_authors(&project_root, &subjects, branch.as_deref());
        if let Some(refusal) = review_author_refusal(&authors, &recorder, !subjects.is_empty()) {
            anyhow::bail!(refusal);
        }
    }
    // BUG-1918: a human who passed the TTY floor leaves a receipt in the
    // user's AIDA home; the ship gate re-reads it instead of trusting the
    // `human_at_tty` flag stored beside the verdict.
    // trace:BUG-1918 | ai:claude
    let receipt_id = if human_at_tty && approving {
        Some(
            review_authority::write_human_receipt(
                resolved_sha.as_deref().unwrap_or_default(),
                spec,
            )
            .context("could not write the human-review receipt")?,
        )
    } else {
        None
    };
    let attestation = serde_json::to_value(review_verdict::RecorderAttestation {
        sha: resolved_sha.clone().unwrap_or_default(),
        seat: recorder_grant.as_ref().map(|g| g.seat.clone()),
        grant_id: recorder_grant.as_ref().map(|g| g.id.clone()),
        human_at_tty,
        identity: recorder.tokens.clone(),
        receipt_id,
        author_check_passed: approving,
    })
    .context("could not serialize the review recorder attestation")?;

    // STORY-1416 criterion 1b: recording a verdict for a PR surfaces the
    // standing marker (its text and who placed it) and any prior verdict
    // BEFORE either is overwritten below. Informational only.
    // trace:STORY-1416 | ai:claude
    if let Some(n) = pr {
        let standing = pr_claim_surface::read_pr_record(&project_root, n, &[spec]);
        pr_claim_surface::print(
            n,
            &pr_claim_surface::lines_for_verdict_write(
                &standing,
                &pr_claim_surface::IncomingVerdict {
                    key: &spec.trim().to_ascii_uppercase(),
                    kind,
                    sha: resolved_sha.as_deref(),
                    recorded_by: &recorded_by,
                },
            ),
        );
    }

    // A refusal is a protection event, not merely metadata. Arm the local
    // merge chokepoint before publishing the verdict; mirroring the label also
    // protects raw forge/UI merges when branch policy consumes it. The hold is
    // intentionally not cleared by a later approval: `merge-hold clear` is the
    // explicit human/advisor release action.
    // trace:BUG-1452 | ai:codex
    if kind.blocks_done() {
        let n = pr.ok_or_else(|| {
            anyhow::anyhow!(
                "a refusing verdict must name `--pr <N>` so AIDA can protect the pull request"
            )
        })?;
        let reason = format!(
            "{} for {} at {}",
            kind.label(),
            spec.to_ascii_uppercase(),
            resolved_sha
                .as_deref()
                .map(review_verdict::short_sha)
                .unwrap_or("unknown")
        );
        // BUG-1536: this used to re-resolve a SEPARATE "protection root" here
        // (preferring `AIDA_PROJECT_ROOT`, ambiently) rather than reusing the
        // one root this whole call was given. That second ambient read is
        // exactly what let a test's `AIDA_DRIVE_ROOT`-only tempdir pin leave
        // the hold, verdict and forge label call resolving against a seat
        // shell's real `AIDA_PROJECT_ROOT`. There is now exactly one root for
        // the whole call — `project_root`, the parameter — so a hold and its
        // label can never target a different tree than the verdict itself.
        // STORY-1397: the hold is typed Rework so it is not mistaken for a
        // recusal or supervision hold. trace:STORY-1397 | ai:codex
        // BUG-1532 criterion 10 / BUG-1562: the hold REFERENCES this verdict
        // record (key, PR, sha, recording seat) and states its release
        // condition, so a later round of the same verdict is followed rather
        // than a frozen quote of this one. trace:BUG-1532 trace:BUG-1562 | ai:claude
        let spec_key = spec.trim().to_ascii_uppercase();
        let mut hold = merge_hold::typed_hold(
            n,
            merge_hold::HoldReasonKind::Rework,
            &reason,
            resolved_sha.clone(),
        );
        hold.verdict_ref = Some(merge_hold::VerdictRef::new(
            &spec_key,
            Some(n),
            resolved_sha.clone(),
            Some(recorded_by.clone()),
        ));
        hold.spec = Some(spec_key.clone());
        hold.release_condition = Some(format!(
            "a fresh APPROVED verdict for {spec_key} recorded at PR #{n}'s current head; then a human ships it"
        ));
        merge_hold::write_typed_hold(&project_root, &hold)
            .with_context(|| format!("could not protect PR-{n} with a merge hold"))?;
        if let Err(err) = merge_hold::sync_label(&project_root, n, true) {
            eprintln!(
                "  {} merge-hold label not applied on PR-{n}: {err} — the local merge chokepoint remains armed; run `aida merge-hold list --fix`",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            );
        }
    }
    // STORY-1417: build + classify + one verified write — the same boundary
    // `record_verdict` uses, with the typed classes layered on before the
    // single durable write (BUG-1571). trace:STORY-1417 | ai:claude
    let path = review_verdict::verdict_path(&project_root, spec);
    let mut verdict_obj = review_verdict::build_verdict_object(
        &project_root,
        &path,
        Some(verdict),
        resolved_sha.as_deref(),
        branch.as_deref(),
        summary,
        findings,
        &recorded_by,
    )
    .with_context(|| "could not write the review verdict")?;
    review_classes::apply_finding_classes(&mut verdict_obj, findings, &classes);
    // trace:BUG-1918 | ai:claude
    verdict_obj.insert(
        review_verdict::ATTESTATION_KEY.to_string(),
        attestation.clone(),
    );
    review_verdict::write_verdict_object(&path, &verdict_obj)
        .with_context(|| "could not write the review verdict")?;
    // PRIN-5 / BUG-1571: `record_verdict` already writes atomically and
    // verifies the bytes landed, but never print a path this process has
    // not itself just confirmed exists on disk.
    // trace:BUG-1571 | ai:claude
    if !path.is_file() {
        anyhow::bail!(
            "the review verdict at {} disappeared immediately after being written — not reporting it as recorded",
            path.display()
        );
    }
    // STORY-1405: the verdict is on disk, so the review is no longer "in
    // progress" — clear the PR's marker (by `--pr`, else any marker naming
    // this spec) so merge surfaces now read the verdict instead.
    // trace:STORY-1405 | ai:claude
    clear_review_markers_for_verdict(&project_root, spec, pr);
    // TASK-1450: a recorded review verdict is a reviewer-seat coordination
    // decision — the other gap BUG-1423's event feed left (that bug closed
    // the merge-path gap; this is the review-path gap). Emitted only after
    // the write above lands, so a failed record never produces a phantom
    // event. Best-effort: `events::emit` never fails the command.
    // trace:TASK-1450 | ai:claude
    emit_review_verdict_recorded(
        &project_root,
        &spec.to_ascii_uppercase(),
        pr.map(|n| n as u32),
        kind.label().to_string(),
        resolved_sha.clone(),
    );
    println!(
        "{} recorded {} for {}{}",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        kind.label().bold(),
        spec.to_ascii_uppercase().cyan(),
        resolved_sha
            .as_deref()
            .map(|s| format!(" against {}", review_verdict::short_sha(s)))
            .unwrap_or_default()
    );
    if kind.blocks_done() {
        let detail = summary
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| kind.label());
        let recovery = format!(
            "address the review findings, move {} back to In Progress, and clear the PR hold only after approval",
            spec.to_ascii_uppercase()
        );
        match shelve_spec_on_failure(
            &project_root,
            spec,
            "reviewer",
            3,
            match kind {
                review_verdict::VerdictKind::RequestChanges => "verdict:request-changes",
                review_verdict::VerdictKind::Rejected => "verdict:rejected",
                _ => unreachable!("blocks_done only covers refusing verdicts"),
            },
            detail,
            &recovery,
        )? {
            Some(_) => println!(
                "  {} {} parked in Needs Attention and surfaced by `aida awaiting`.",
                crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                spec.to_ascii_uppercase()
            ),
            None => anyhow::bail!(
                "recorded the refusal and merge hold, but could not park {} in Needs Attention",
                spec.to_ascii_uppercase()
            ),
        }
        println!(
            "  {} `aida queue done {}` will refuse until the branch moves past that commit.",
            crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
            spec.to_ascii_uppercase()
        );
    }
    println!(
        "  {} {}",
        "record:".dimmed(),
        path.display().to_string().dimmed()
    );

    // BUG-802: with --pr, also write the orchestrator's phase-3 handshake at
    // the orchestrator-visible anchor. The verdict string is the tolerant-parse
    // canonical label so `auto_complete::Verdict::parse` accepts it byte-for-byte.
    if let Some(n) = pr {
        let handshake = review_pr_handshake_path(&project_root, n);
        // trace:BUG-1743 | ai:claude
        #[cfg(test)]
        assert_handshake_anchor_is_pinned(&project_root, &handshake);
        // Read back the canonical record rather than rebuilding provenance.
        // Besides keeping the timestamp byte-identical, this carries the
        // full SHA produced by record_verdict's write-boundary normalization
        // when the caller supplied an abbreviation.
        // trace:BUG-1466 | ai:codex
        // trace:BUG-1516 | ai:codex
        let recorded = review_verdict::read_recorded_verdict(&project_root, spec)
            .ok_or_else(|| anyhow::anyhow!("the verdict was written but could not be read back"))?;
        // BUG-1571: build the full handshake object (base fields + the
        // orchestrator's `mode`/`recorded_at` overlay) in memory and commit
        // it with exactly ONE durable, verified write. The previous code
        // wrote the base record, read it back, patched two fields, and wrote
        // AGAIN — two separate `fs::write`s to the same path, each a window
        // where the artefact could fail to land while the command still
        // walked forward as if it had. `write_verdict_object` verifies the
        // bytes are actually readable back before this function is allowed
        // to claim the handshake exists.
        // trace:BUG-1581 | ai:codex
        // trace:BUG-1571 | ai:claude
        let mut handshake_obj = review_verdict::build_verdict_object(
            &project_root,
            &handshake,
            Some(kind.label()),
            recorded.reviewed_sha.as_deref(),
            recorded.reviewed_branch.as_deref(),
            summary,
            findings,
            recorded.recorded_by.as_deref().unwrap_or(&recorded_by),
        )
        .with_context(|| format!("could not prepare {}", handshake.display()))?;
        // trace:STORY-1417 | ai:claude
        review_classes::apply_finding_classes(&mut handshake_obj, findings, &classes);
        // The two artifacts describe the same act of review, so retain the
        // spec record's timestamp byte-for-byte while preserving any displaced
        // PR-keyed round through the shared writer above.
        // trace:BUG-1918 | ai:claude
        handshake_obj.insert(
            review_verdict::ATTESTATION_KEY.to_string(),
            attestation.clone(),
        );
        handshake_obj.insert(
            "mode".to_string(),
            serde_json::Value::String("orchestrator-phase-3".to_string()),
        );
        if let Some(recorded_at) = recorded.recorded_at.as_deref() {
            handshake_obj.insert(
                "recorded_at".to_string(),
                serde_json::Value::String(recorded_at.to_string()),
            );
        }
        review_verdict::write_verdict_object(&handshake, &handshake_obj).with_context(|| {
            format!(
                "the phase-3 handshake was NOT written to {} — the orchestrator will not see this verdict; re-run `aida review record`",
                handshake.display()
            )
        })?;
        // Belt-and-suspenders: only ever print a path this process has just
        // confirmed exists. `write_verdict_object` already verified the
        // content by reading it back, but a missing/unreadable file at this
        // point must still block the success line rather than merely being
        // ignored. PRIN-5: never print a path that wasn't written.
        // trace:BUG-1571 | ai:claude
        if !handshake.is_file() {
            anyhow::bail!(
                "the phase-3 handshake at {} disappeared immediately after being written — not reporting it as recorded",
                handshake.display()
            );
        }
        println!(
            "  {} {} (phase-3 handshake — the orchestrator reads this to proceed)",
            "handshake:".dimmed(),
            handshake.display().to_string().dimmed()
        );
    }
    Ok(())
}

#[cfg(test)]
mod story_1405_review_marker_tests {
    use super::*;

    fn tempdir_root() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".aida")).unwrap();
        root
    }

    // A reviewer's claim blocks a merge; recording the verdict (with --pr)
    // clears it, so the same merge gate then lets the merge through.
    //
    // BUG-1743: `--pr` also writes the phase-3 handshake, whose anchor is
    // resolved ambiently (`review_pr_handshake_path`), NOT from the root
    // passed in. Unpinned, this test wrote
    // `<real repo>/.aida/review-verdicts/PR-14051.json` into the operator's
    // checkout on every run — a fabricated APPROVED verdict keyed to a PR
    // number, invisible because `.aida/` is gitignored. Pin all three keys to
    // this test's own tempdir; the guard holds `ENV_LOCK` so no sibling can
    // move them mid-test. See `assert_handshake_anchor_is_pinned`.
    // trace:BUG-1743 | ai:claude
    #[test]
    fn recording_the_verdict_clears_the_marker_and_unblocks_merge() {
        let root = tempdir_root();
        let root_str = root.path().to_str().expect("tempdir path is utf-8");
        let _pinned = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_PROJECT_ROOT", Some(root_str)),
            ("AIDA_DRIVE_ROOT", Some(root_str)),
            ("AIDA_REVIEW_VERDICT_FILE", None),
        ]);
        let mut m = review_marker::Marker::for_this_process(
            14051,
            Some("abc1234"),
            Some("BUG-14051"),
            "reviewer seat",
        );
        m.pid = 0;
        m.ttl_secs = review_marker::CLAIM_TTL_SECS;
        review_marker::write(root.path(), &m).unwrap();
        assert!(matches!(
            review_marker::merge_gate(root.path(), 14051, || Some("abc1234".into())),
            review_marker::MergeGate::UnderReview(_)
        ));

        handle_review_record_at(
            root.path().to_path_buf(),
            "BUG-14051",
            "approved",
            Some("abc1234"),
            Some("bug-14051-work"),
            Some("looks right"),
            &[],
            &[],
            Some(14051),
        )
        .expect("recording an approval must succeed");

        assert!(review_marker::read(root.path(), 14051).is_none());
        assert_eq!(
            review_marker::merge_gate(root.path(), 14051, || panic!("no marker, no lookup")),
            review_marker::MergeGate::Clear
        );
    }

    #[test]
    fn verdict_without_pr_clears_markers_naming_the_spec_only() {
        let root = tempdir_root();
        for (pr, spec) in [(1, "TASK-1"), (2, "TASK-2")] {
            let m = review_marker::Marker::for_this_process(pr, Some("abc"), Some(spec), "r");
            review_marker::write(root.path(), &m).unwrap();
        }
        let cleared = clear_review_markers_for_verdict(root.path(), "task-1", None);
        assert_eq!(cleared, vec![1]);
        assert!(review_marker::read(root.path(), 2).is_some());
    }

    // TASK-1459: `aida review <SPEC>` prefers the forge's live PR head over
    // the local `origin/<branch>` tracking ref, falling back to that ref
    // when the forge can't be reached. A repo with no recognized forge
    // remote resolves to `ForgeKind::None` (`PureGitForge`, which errors
    // immediately with no subprocess — see `forge.rs`), so this exercises
    // the fallback deterministically with no `gh`/network dependency.
    #[test]
    fn marker_head_best_effort_falls_back_to_local_ref_without_a_forge() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let git = |args: &[&str]| {
            assert!(std::process::Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .status()
                .expect("git")
                .success());
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "test@example.test"]);
        git(&["config", "user.name", "Test"]);
        std::fs::write(repo.join("f"), "x").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        let head = resolve_commit_sha(repo, "HEAD").expect("HEAD resolves");
        // A local remote-tracking ref, as if fetched, but no `origin` remote
        // URL is configured — `resolve_open_change_forge_kind` degrades to
        // `ForgeKind::None`.
        git(&["update-ref", "refs/remotes/origin/some-branch", "HEAD"]);

        let got = review_marker_head_best_effort(repo, 999, "some-branch");
        assert_eq!(got.as_deref(), Some(head.as_str()));
    }

    #[test]
    fn marker_head_best_effort_is_none_with_no_forge_and_no_local_ref() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["init", "-q"])
            .status()
            .expect("git init")
            .success());
        assert_eq!(
            review_marker_head_best_effort(repo, 999, "no-such-branch"),
            None
        );
    }

    #[test]
    fn awaiting_annotation_is_head_scoped_and_read_only() {
        let root = tempdir_root();
        let m = review_marker::Marker::for_this_process(5, Some("aaa111"), None, "reviewer seat");
        review_marker::write(root.path(), &m).unwrap();
        assert!(review_marker::live_description(root.path(), 5, Some("aaa111")).is_some());
        assert!(review_marker::live_description(root.path(), 5, Some("bbb222")).is_none());
        assert!(review_marker::live_description(root.path(), 6, Some("aaa111")).is_none());
        assert!(review_marker::read(root.path(), 5).is_some());
    }
}

/// Audit identity for a supported seat-written verdict. The launched seat's
/// name is stable and distinguishable from the drain's own metadata writer.
// trace:BUG-1467 | ai:codex
pub(crate) fn review_recorded_by(project_root: &std::path::Path) -> String {
    review_recorded_by_from(
        std::env::var("AIDA_AGENT_NAME").ok().as_deref(),
        std::env::var("AIDA_AGENT_TYPE").ok().as_deref(),
        crate::seat_authority::current_seat(project_root).as_deref(),
    )
}

pub(crate) fn review_recorded_by_from(
    name: Option<&str>,
    kind: Option<&str>,
    role: Option<&str>,
) -> String {
    let role_disp = role.unwrap_or("reviewer");
    match (
        name.map(str::trim).filter(|s| !s.is_empty()),
        kind.map(str::trim).filter(|s| !s.is_empty()),
    ) {
        (Some(name), Some(kind)) => format!("{name} ({kind} {role_disp} seat)"),
        (Some(name), None) => format!("{name} ({role_disp} seat)"),
        (None, Some(kind)) => format!("{kind} {role_disp} seat"),
        (None, None) => REVIEW_OPERATOR_RECORDED_BY.to_string(),
    }
}

/// The `recorded_by` label of a verdict with no agent identity behind it.
pub(crate) const REVIEW_OPERATOR_RECORDED_BY: &str = "aida review record (operator)";

/// BUG-1918 / BUG-1693: the human-at-TTY floor for operator attribution.
/// Returns whether a human was proven present; refuses otherwise. Under
/// `cfg(test)` no TTY exists, so it proves nothing (`false`) — a test-written
/// operator approval is therefore never attested as human.
// trace:BUG-1918 | ai:claude
fn review_operator_tty_floor() -> Result<bool> {
    #[cfg(not(test))]
    {
        crate::seat_authority::require_direct_human()
            .context("aida review record attribution fallback")?;
        Ok(true)
    }
    #[cfg(test)]
    {
        Ok(false)
    }
}

/// BUG-1918: the spec ids a review is ABOUT — the verdict key (unless it is
/// a `PR-N` key) plus any spec id named by the reviewed branch.
// trace:BUG-1918 | ai:claude
pub(crate) fn review_subject_spec_ids(spec: &str, branch: Option<&str>) -> Vec<String> {
    let key = spec.trim().to_ascii_uppercase();
    let mut ids: Vec<String> = Vec::new();
    if !key.is_empty() && !key.starts_with("PR-") {
        ids.push(key);
    }
    if let Some(b) = branch {
        ids.extend(
            pr_ship::extract_spec_ids_from_text(b)
                .into_iter()
                .map(|id| id.to_ascii_uppercase()),
        );
    }
    ids.sort();
    ids.dedup();
    ids
}

/// BUG-1918: the author sessions an approval recorded from `project_root` is
/// checked against — authoring leases (live, or durably recorded when taken)
/// on `subject_ids` or on the reviewed `branch`. With no subject spec
/// resolvable, every live authoring lease counts: non-authorship cannot be
/// established, so the check fails closed.
// trace:BUG-1918 | ai:claude
pub(crate) fn review_recorder_authors(
    project_root: &std::path::Path,
    subject_ids: &[String],
    branch: Option<&str>,
) -> Vec<pr_ship::AuthorSession> {
    let ids: Vec<String> = if subject_ids.is_empty() {
        review_authority::visible_leases(project_root)
            .iter()
            .filter(|l| review_authority::is_authoring_lease(l))
            .map(|l| l.scope.trim().to_string())
            .collect()
    } else {
        subject_ids.to_vec()
    };
    review_authority::author_sessions_at(project_root, &ids, branch)
}

/// BUG-1918: refuse an approval whose recorder shares an identity with one of
/// the work's author sessions — the same lease, the same agent session id,
/// the same agent process, or the shell that took the lease. Identity, never
/// the free-text `recorded_by` label, decides.
// trace:BUG-1918 | ai:claude
pub(crate) fn review_author_refusal(
    authors: &[pr_ship::AuthorSession],
    recorder: &review_authority::RecorderIdentity,
    subject_resolved: bool,
) -> Option<String> {
    let author = authors
        .iter()
        .find(|a| review_authority::identities_overlap(&a.tokens, &recorder.tokens))?;
    Some(if subject_resolved {
        format!(
            "refused: this session is the one that claimed or implemented the work ({}) — a session cannot approve its own work; record the approval from an independent reviewer session",
            author.label
        )
    } else {
        format!(
            "refused: this session holds an authoring lease ({}) and the review's subject could not be resolved, so non-authorship cannot be established — record the approval from an independent reviewer session",
            author.label
        )
    })
}

#[cfg(test)]
mod bug_1467_reviewer_record_tests {
    use super::*;

    #[test]
    fn launched_reviewer_identity_is_distinct_from_the_drain_writer() {
        let identity = review_recorded_by_from(Some("review-pr-1986"), Some("claude"), None);
        assert_eq!(identity, "review-pr-1986 (claude reviewer seat)");
        assert_ne!(identity, "aida drain reviewer");
    }

    #[test]
    fn cold_reviewer_context_names_the_supported_verdict_writer() {
        let guidance = default_role_guidance("reviewer");
        assert!(guidance.contains("aida review record <SPEC> --pr <N>"));
        assert!(guidance.contains("never hand-write verdict JSON"));
    }
}

#[cfg(test)]
mod bug_1452_refusal_aftermath_tests {
    use super::*;
    use aida_core::db::DatabaseBackend;

    /// Criterion 5 asks for the aftermath OF A VERDICT, so the test drives
    /// `handle_review_record` and asserts what the RECORDING PATH did. The
    /// earlier version called `write_hold` and `shelve_spec_on_failure` itself
    /// and then asserted they had run — which proved only that two primitives
    /// work, and would have kept passing if the recording path stopped calling
    /// either one.
    ///
    /// `sha` and `branch` are passed explicitly so the call does not reach the
    /// forge for a head or the checkout for a branch: the forge lookup is
    /// skipped when `sha` is present, and `current_branch_at` only runs when
    /// `branch` is None.
    /// BUG-1743: the `--pr` argument below makes this call write a SECOND
    /// artifact, the phase-3 handshake, and that one does NOT go to the root
    /// passed in — `review_pr_handshake_path` prefers ambient
    /// `AIDA_REVIEW_VERDICT_FILE` / `AIDA_PROJECT_ROOT` / `AIDA_DRIVE_ROOT`,
    /// then the cwd's enclosing project, and only falls back to the explicit
    /// root. That is BUG-912's orchestrator anchor and is correct in
    /// production. This test read it ambiently, which made it write outside
    /// its own tempdir on every single run — measured 40/40 before the pin,
    /// in two shapes:
    ///
    /// - into a SIBLING test's `tempfile` root, whenever a parallel test held
    ///   `AIDA_PROJECT_ROOT` pinned at its own tempdir. The handshake landed
    ///   there, the sibling's `TempDir` dropped, and the file this call had
    ///   just written and read back ceased to exist — the two-faced CI flake
    ///   BUG-1743 was filed for (`ENOENT` at write time when the sibling had
    ///   already torn down, `!is_file()` a few syscalls later when it tore
    ///   down just after).
    /// - into the REAL repository, via the cwd branch, whenever no sibling
    ///   held a pin. `/home/joe/ai/aida/.aida/review-verdicts/PR-1452.json`
    ///   had accumulated three `rounds` of this fixture's payload
    ///   (`BUG-14520` / `abc123` / "the regression is still open") dated
    ///   09-25, 09-27 and 09-30. It went unnoticed because `.aida/` is
    ///   gitignored, so the pollution never showed in `git status`.
    ///
    /// Pinning all three keys under one `EnvVarsGuard` fixes both: the anchor
    /// now resolves to this tempdir, and — because the guard holds `ENV_LOCK`
    /// for its whole lifetime — no sibling can move it mid-test. The guard is
    /// what `assert_handshake_anchor_is_pinned` checks for, and the final
    /// assertion below is the positive half: the handshake is INSIDE `root`.
    // trace:BUG-1452 | ai:claude
    // trace:BUG-1743 | ai:claude
    #[test]
    fn refusal_aftermath_is_parked_held_and_awaiting_visible() {
        let root = tempfile::tempdir().unwrap();
        let root_str = root.path().to_str().expect("tempdir path is utf-8");
        let _pinned = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_PROJECT_ROOT", Some(root_str)),
            ("AIDA_DRIVE_ROOT", Some(root_str)),
            ("AIDA_REVIEW_VERDICT_FILE", None),
        ]);
        let store = root.path().join(".aida-store");
        std::fs::create_dir_all(root.path().join(".aida")).unwrap();
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(
            root.path().join(".aida/config.toml"),
            "mode = \"distributed\"\nstore_path = \".aida-store\"\n",
        )
        .unwrap();

        let backend = aida_core::CachedGitBackend::open(
            &store,
            &aida_core::CachedGitBackend::default_cache_path(&store),
        )
        .unwrap();
        let mut req = aida_core::Requirement::new("Refused work".into(), "desc".into());
        req.spec_id = Some("BUG-14520".into());
        req.status = aida_core::RequirementStatus::Done;
        backend.add_requirement(req).unwrap();
        drop(backend);

        // BUG-1536 removed the env pinning this test used to need, on the
        // grounds that `handle_review_record_at` takes its write root as an
        // explicit parameter and "uses it everywhere". That is true of the
        // hold, the verdict and the forge label call — but NOT of the phase-3
        // handshake `--pr` triggers, which keeps its own ambient resolution
        // order by design (BUG-912). So the pin above is back, and it is not
        // the pre-BUG-1536 pin returning: this one exists to stop the
        // handshake escaping and to hold `ENV_LOCK` against sibling setters,
        // not to steer the hold. See BUG-1743 and the doc comment above.
        // trace:BUG-1743 | ai:claude
        handle_review_record_at(
            root.path().to_path_buf(),
            "BUG-14520",
            "request-changes",
            Some("abc123"),
            Some("bug-14520-work"),
            Some("the regression is still open"),
            &["fix the regression".to_string()],
            &[],
            Some(1452),
        )
        .expect("recording a refusing verdict must succeed");

        // 1. the PR is HELD — written by the recording path, not by this test
        assert!(
            merge_hold::read_hold(root.path(), 1452).is_some(),
            "a refusing verdict must leave a merge-hold on the PR"
        );
        // BUG-1532 criterion 10: the hold REFERENCES the verdict record
        // (key, PR, sha, recording seat) and states its release condition.
        // trace:BUG-1532 | ai:claude
        let hold = merge_hold::read_hold_record(root.path(), 1452).unwrap();
        assert_eq!(hold.reason_kind, merge_hold::HoldReasonKind::Rework);
        let vref = hold
            .verdict_ref
            .as_ref()
            .expect("marker references the verdict");
        assert_eq!(vref.key, "BUG-14520");
        assert_eq!(vref.pr, Some(1452));
        assert_eq!(vref.reviewed_sha.as_deref(), Some("abc123"));
        assert!(vref.recorded_by.is_some());
        assert_eq!(hold.spec.as_deref(), Some("BUG-14520"));
        assert!(hold
            .release_condition
            .as_deref()
            .is_some_and(|c| c.contains("fresh APPROVED verdict for BUG-14520")));

        // 2. the verdict is recorded and blocks done
        let recorded = review_verdict::read_recorded_verdict(root.path(), "PR-1452")
            .or_else(|| review_verdict::read_recorded_verdict(root.path(), "BUG-14520"))
            .expect("the verdict must be readable after recording");
        assert!(
            recorded.kind.blocks_done(),
            "a request-changes verdict must block done"
        );

        // 3. the spec is PARKED, not Done, and visible to `aida awaiting`
        let backend = aida_core::CachedGitBackend::open(
            &store,
            &aida_core::CachedGitBackend::default_cache_path(&store),
        )
        .unwrap();
        let parked = backend
            .get_requirement_by_spec_id("BUG-14520")
            .unwrap()
            .unwrap();
        assert_ne!(
            parked.status,
            aida_core::RequirementStatus::Done,
            "a refused spec must not remain Done — that is the defect this spec is about"
        );
        assert_eq!(parked.status, aida_core::RequirementStatus::NeedsAttention);
        assert!(
            parked.failure_reason.is_some(),
            "failure_reason-backed NeedsAttention is what `aida awaiting` counts as shelved work"
        );

        // 4. BUG-1743: the phase-3 handshake landed INSIDE this test's own
        // tempdir. Asserting the file merely exists would not have caught the
        // defect — it existed, in someone else's temp tree or in the real
        // repository, and `handle_review_record_at` had already read it back
        // and confirmed its bytes. The claim that has to hold is about WHERE.
        // trace:BUG-1743 | ai:claude
        let handshake = review_pr_handshake_path(root.path(), 1452);
        assert!(
            handshake.starts_with(root.path()),
            "the phase-3 handshake anchor must resolve inside this test's own \
             tempdir {}, not {} — an escape here is a file this test cannot \
             keep alive and did not mean to write",
            root.path().display(),
            handshake.display()
        );
        assert!(
            handshake.is_file(),
            "the phase-3 handshake must be on disk at {}",
            handshake.display()
        );
    }

    /// BUG-1536 acceptance criterion 2: driving the recording path with a
    /// pinned tempdir root CANNOT write the merge-hold protection (the exact
    /// artifact the incident leaked) outside that root — even with a bogus PR
    /// number and even with the ambient env set exactly the way the incident
    /// had it: a stand-in "real repo" named by BOTH `AIDA_DRIVE_ROOT` and
    /// `AIDA_PROJECT_ROOT`. Before BUG-1536, `protection_root` preferred
    /// `AIDA_PROJECT_ROOT` over the resolved root passed in here, so this
    /// exact setup sent the hold and the (would-be) forge label call into
    /// `real_root`. Now there is one root for the whole call, so the hold and
    /// the spec-keyed verdict land only in `explicit_root`.
    ///
    /// Deliberately NOT asserted here: the PR-keyed *handshake* file
    /// (`review_pr_handshake_path`) legitimately follows `AIDA_PROJECT_ROOT`
    /// when set — that is a separate, pre-existing, intentional orchestrator
    /// anchor (BUG-912, covered by its own test
    /// `review_record_pr_handshake_honors_explicit_verdict_file`), not part
    /// of the BUG-1536 defect. Asserting `real_root` gets no `.aida/` at all
    /// would be wrong: the handshake write puts one there by design.
    // trace:BUG-1536 | ai:claude
    #[test]
    fn recording_never_escapes_the_explicit_root_even_when_ambient_env_points_elsewhere() {
        let explicit_root = tempfile::tempdir().unwrap();
        let store = explicit_root.path().join(".aida-store");
        std::fs::create_dir_all(explicit_root.path().join(".aida")).unwrap();
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(
            explicit_root.path().join(".aida/config.toml"),
            "mode = \"distributed\"\nstore_path = \".aida-store\"\n",
        )
        .unwrap();
        let backend = aida_core::CachedGitBackend::open(
            &store,
            &aida_core::CachedGitBackend::default_cache_path(&store),
        )
        .unwrap();
        let mut req = aida_core::Requirement::new("Explicit-root work".into(), "desc".into());
        req.spec_id = Some("BUG-99999".into());
        req.status = aida_core::RequirementStatus::Done;
        backend.add_requirement(req).unwrap();
        drop(backend);

        // A stand-in "real repo": set BOTH ambient env vars the way the
        // incident's seat shell had them, to prove the explicit parameter —
        // not fallback ordering between the two — is what decides the
        // protection root.
        let real_root = tempfile::tempdir().unwrap();
        let real_str = real_root.path().to_str().expect("tempdir path is utf-8");
        let _ambient = crate::test_env::EnvVarsGuard::set(&[
            ("AIDA_DRIVE_ROOT", real_str),
            ("AIDA_PROJECT_ROOT", real_str),
        ]);

        // A bogus, unlikely-to-collide-with-anything-real PR number — the
        // exact shape of the incident, where the number happened to match a
        // real merged PR purely by chance.
        handle_review_record_at(
            explicit_root.path().to_path_buf(),
            "BUG-99999",
            "request-changes",
            Some("deadbeef"),
            Some("bug-99999-work"),
            Some("isolation check"),
            &["nothing real".to_string()],
            &[],
            Some(9_999_999),
        )
        .expect("recording against the explicit root must succeed regardless of ambient env");

        assert!(
            merge_hold::read_hold(explicit_root.path(), 9_999_999).is_some(),
            "the hold must land in the explicit root"
        );
        assert!(
            review_verdict::read_recorded_verdict(explicit_root.path(), "BUG-99999").is_some(),
            "the spec-keyed verdict must land in the explicit root"
        );

        assert!(
            merge_hold::read_hold(real_root.path(), 9_999_999).is_none(),
            "no hold may be written to the ambient-pointed root — this is the exact \
             artifact BUG-1536's `protection_root` used to leak there"
        );
        assert!(
            !real_root.path().join(".aida/merge-holds").exists(),
            "the ambient-pointed root must not even gain a `.aida/merge-holds/` directory"
        );
    }
}

/// `aida review verdict <SPEC>` — read the recorded verdict back.
// trace:BUG-775 | ai:claude
// BUG-1508: this used to print the raw verdict with no answer to "does it
// still cover the current head" -- forcing a human to compare shas by hand
// (the exact defect measured against the reviewer queue: 4 of 5 routed
// entries were already-refused-at-the-current-head, and nothing said so).
// The branch checked out here is the ONLY head this process can resolve
// without a forge call, so the actionability line is scoped to it; a
// verdict recorded for a different worktree/branch still prints, just
// without the head comparison. trace:BUG-1508 | ai:claude
pub(crate) fn handle_review_verdict_show(spec: &str, json: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let path = review_verdict::verdict_path(&project_root, spec);
    match review_verdict::read_recorded_verdict(&project_root, spec) {
        Some(v) => {
            let branch = current_branch_at(&project_root);
            let relation =
                verdict_tip_relation(&project_root, branch.as_deref(), v.reviewed_sha.as_deref());
            let actionability = review_verdict::review_actionability(Some(&v), relation);
            if json {
                let body = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".to_string());
                let mut value: serde_json::Value =
                    serde_json::from_str(body.trim()).unwrap_or(serde_json::Value::Null);
                if let Some(obj) = value.as_object_mut() {
                    obj.insert(
                        "actionability".to_string(),
                        serde_json::Value::String(actionability.as_str().to_string()),
                    );
                }
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                println!(
                    "{} {}",
                    format!("{}:", spec.to_ascii_uppercase()).bold(),
                    review_verdict::verdict_notice_line(&v)
                );
                println!("  {} {}", "actionability:".dimmed(), actionability.as_str());
                println!(
                    "  {} {}",
                    "record:".dimmed(),
                    path.display().to_string().dimmed()
                );
            }
            Ok(())
        }
        None => {
            let actionability = review_verdict::ReviewActionability::NeedsReview;
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "verdict": null,
                        "actionability": actionability.as_str(),
                    })
                );
            } else {
                println!(
                    "{} no review verdict recorded for {} — actionability: {}.",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    spec.to_ascii_uppercase(),
                    actionability.as_str()
                );
            }
            Ok(())
        }
    }
}

// trace:TASK-1590 | ai:antigravity
pub(crate) fn handle_review_list(json: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let verdicts = review_verdict::list_active_verdicts(&project_root);

    if verdicts.is_empty() {
        if json {
            println!("[]");
        } else {
            println!(
                "{} No active review verdicts found.",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
            );
        }
        return Ok(());
    }

    let branch = current_branch_at(&project_root);
    let mut entries: Vec<(String, String, String, String, String, colored::Color)> = Vec::new();
    let mut json_out = Vec::new();

    let mut max_spec = 7;
    let mut max_verdict = 7;
    let mut max_action = 13;
    let mut max_sha = 3;

    for (spec, v) in verdicts {
        let relation =
            verdict_tip_relation(&project_root, branch.as_deref(), v.reviewed_sha.as_deref());
        let actionability = review_verdict::review_actionability(Some(&v), relation);
        let spec_upper = spec.to_ascii_uppercase();
        let verdict_str = format!("{:?}", v.kind);
        let action_str = actionability.as_str().to_string();
        let sha_str =
            review_verdict::short_sha(v.reviewed_sha.as_deref().unwrap_or("")).to_string();

        max_spec = max_spec.max(spec_upper.len());
        max_verdict = max_verdict.max(verdict_str.len());
        max_action = max_action.max(action_str.len());
        max_sha = max_sha.max(sha_str.len());

        let color = match actionability {
            review_verdict::ReviewActionability::Resolved => colored::Color::Green,
            review_verdict::ReviewActionability::AwaitingRework => colored::Color::Red,
            review_verdict::ReviewActionability::NeedsReview => colored::Color::Yellow,
        };

        if json {
            json_out.push(serde_json::json!({
                "spec_id": spec_upper,
                "verdict": verdict_str,
                "actionability": action_str,
                "sha": sha_str,
            }));
        } else {
            entries.push((
                spec_upper,
                verdict_str,
                action_str,
                sha_str,
                String::new(),
                color,
            ));
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&json_out)?);
        return Ok(());
    }

    println!(
        "{:<spec_w$}  {:<verdict_w$}  {:<action_w$}  {:<sha_w$}",
        "Spec ID".bold(),
        "Verdict".bold(),
        "Actionability".bold(),
        "SHA".bold(),
        spec_w = max_spec,
        verdict_w = max_verdict,
        action_w = max_action,
        sha_w = max_sha,
    );

    for (spec, verdict, action, sha, _ignored, color) in entries {
        println!(
            "{:<spec_w$}  {:<verdict_w$}  {:<action_w$}  {:<sha_w$}",
            spec,
            verdict,
            action.color(color),
            sha,
            spec_w = max_spec,
            verdict_w = max_verdict,
            action_w = max_action,
            sha_w = max_sha,
        );
    }

    Ok(())
}

/// trace:STORY-67 | ai:claude
pub(crate) fn generate_review_prompt(
    storage: &Storage,
    specs_csv: Option<&str>,
    pr: Option<u64>,
    forge_override: Option<&str>,
    write_path: Option<&str>,
) -> Result<()> {
    let store = storage.load()?;

    // Resolve the spec list. Preference order: --specs explicit,
    // --pr range parse, error if neither.
    // trace:BUG-1434 | ai:claude
    let (spec_ids, header_subtitle, staleness_note): (Vec<String>, String, Option<String>) =
        if let Some(csv) = specs_csv {
            let ids: Vec<String> = csv
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if ids.is_empty() {
                anyhow::bail!("--specs was empty after splitting on commas");
            }
            (ids.clone(), format!("Specs: {}", ids.join(", ")), None)
        } else if let Some(pr_n) = pr {
            let project_root = find_project_root()?;
            let forge = forge_override
                .and_then(ReviewForge::parse)
                .or_else(|| detect_forge_from_origin(&project_root))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "couldn't detect forge from origin URL — pass --forge github|gitlab"
                    )
                })?;
            let (base, head) = pr_base_head(&project_root, forge, pr_n)?;
            let messages = git_log_messages(&project_root, &base, &head)?;
            let mut ids: Vec<String> = Vec::new();
            for msg in &messages {
                for id in extract_spec_ids_from_commit(msg) {
                    if !ids
                        .iter()
                        .any(|existing| existing.eq_ignore_ascii_case(&id))
                    {
                        ids.push(id);
                    }
                }
            }
            if ids.is_empty() {
                anyhow::bail!(
                    "no `(REQ-ID)` trailers found in {}..{} ({} commits inspected)",
                    base,
                    head,
                    messages.len()
                );
            }
            let label = match forge {
                ReviewForge::GitHub => format!("PR #{} — branch `{}`", pr_n, head),
                ReviewForge::GitLab => format!("MR !{} — branch `{}`", pr_n, head),
            };
            let note = stale_base_note(&project_root, &head, &base);
            (ids, label, note)
        } else {
            anyhow::bail!("pass --specs <CSV> or --pr <N>");
        };

    // Compose the markdown.
    let mut out = String::new();
    out.push_str("# Review Prompt\n\n");
    out.push_str(&header_subtitle);
    out.push_str("\n\n");
    if let Some(note) = &staleness_note {
        out.push_str("## Staleness\n\n");
        out.push_str(note);
        out.push('\n');
    }
    out.push_str("## What to verify\n\n");

    let mut missing: Vec<String> = Vec::new();
    for id in &spec_ids {
        // TASK-72 polish: PR/MR pseudo-IDs (e.g., `PR-9`, `MR-42`) aren't
        // real specs in the store — they're forge references that can
        // sneak into the trailer-extracted list. Skip them silently so
        // the review prompt doesn't lead with a misleading
        // "PR-9 — (not found in store)" row before the real spec list.
        // trace:TASK-72 | ai:claude
        if parse_review_scope(id).is_some() {
            continue;
        }
        let req = store
            .requirements
            .iter()
            .find(|r| r.spec_id.as_deref() == Some(id.as_str()))
            .or_else(|| {
                uuid::Uuid::parse_str(id)
                    .ok()
                    .and_then(|u| store.requirements.iter().find(|r| r.id == u))
            });
        let Some(req) = req else {
            out.push_str(&format!("### {}\n\n_(not found in store)_\n\n", id));
            missing.push(id.clone());
            continue;
        };
        out.push_str(&format!(
            "### {} — {}\n\n",
            req.spec_id.as_deref().unwrap_or(id.as_str()),
            req.title
        ));
        match extract_acceptance_section(&req.description) {
            Some(body) => {
                out.push_str(&body);
                out.push_str("\n\n");
            }
            None => {
                out.push_str("_(no `## Acceptance` / `## Verify` section in description — review against the requirement title and description.)_\n\n");
            }
        }
        append_implementer_approach(&mut out, &req.comments);
    }

    out.push_str(review_prompt_test_commands_section());

    out.push_str("## Decide\n\n");
    out.push_str(
        "If every item above passes: approve and merge (`gh pr merge --squash` / \
         `glab mr merge --squash`), then mark each linked req `completed`.\n\n",
    );
    out.push_str(
        "If any item fails: request changes with specifics tied to the spec_id, \
         so the contributor can address them by id (not by paraphrase).\n",
    );

    if let Some(path) = write_path {
        std::fs::write(path, &out)
            .with_context(|| format!("failed to write review prompt to {}", path))?;
        eprintln!(
            "{} review prompt written to {} ({} spec{}{})",
            crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
            path,
            spec_ids.len(),
            if spec_ids.len() == 1 { "" } else { "s" },
            if missing.is_empty() {
                String::new()
            } else {
                format!(", {} missing in store", missing.len())
            },
        );
    } else {
        print!("{}", out);
    }
    Ok(())
}

/// The heading the pickup skill writes and the review prompt reads.
///
/// ONE constant for BOTH SIDES OF A HANDSHAKE THAT SPANS A TEMPLATE AND A
/// PARSER. The writer is `aida-core/templates/skills/aida-pickup.md`, the
/// reader is `extract_implementer_approach` below, and nothing in the type
/// system connects them — edit the heading in the template alone and the
/// reader silently stops matching, leaving reviews quietly missing the
/// approach with no error anywhere. The coupling test in
/// `story_1350_review_approach_tests` reads the template and asserts this
/// exact string appears in it, so the two cannot drift apart in one edit.
// trace:STORY-1350 | ai:claude
pub(crate) const IMPLEMENTER_APPROACH_MARKER: &str = "## Implementer approach";

/// Return the newest durable implementation-intent comment. The exact first
/// heading is the stable marker written by the pickup skill; ordinary comments
/// that merely mention an approach must not leak into the review checklist.
// trace:STORY-1350 | ai:codex
pub(crate) fn extract_implementer_approach(
    comments: &[aida_core::models::Comment],
) -> Option<&str> {
    comments.iter().rev().find_map(|comment| {
        let first_line = comment.content.trim_start().lines().next()?;
        (first_line.trim_end() == IMPLEMENTER_APPROACH_MARKER).then_some(comment.content.trim())
    })
}

// trace:STORY-1350 | ai:codex
/// Emit the recorded approach, or SAY THAT THERE IS NONE.
///
/// Silence here is indistinguishable from "the implementer recorded nothing"
/// and from "the section was dropped", and a reviewer reading a prompt with no
/// approach section cannot tell which. Absent evidence has to look different
/// from good evidence, so the absence is stated rather than left as a gap.
// trace:STORY-1350 | ai:claude
pub(crate) fn append_implementer_approach(
    out: &mut String,
    comments: &[aida_core::models::Comment],
) {
    match extract_implementer_approach(comments) {
        Some(approach) => {
            out.push_str("#### Recorded implementer approach\n\n");
            out.push_str(approach);
            out.push_str("\n\n");
        }
        None => {
            out.push_str("#### Recorded implementer approach\n\n");
            // The marker comes from the constant here too. Spelling it out
            // again would make this the THIRD copy — the prose that TELLS a
            // human what to write, drifting from the parser that reads it, so
            // a rename would leave the system parsing X while instructing
            // people to write Y. The coupling test covers this string.
            out.push_str(&format!(
                "_None recorded._ The implementer did not leave a comment beginning \
                 `{IMPLEMENTER_APPROACH_MARKER}`, so this review has no stated intent \
                 to check the diff against.\n\n"
            ));
        }
    }
}

// trace:BUG-800 | ai:codex
pub(crate) fn review_prompt_test_commands_section() -> &'static str {
    "## Test Commands\n\n\
     When you name focused tests, name the runnable command, not a bare test \
     identifier. For Rust, use `cargo test -p <crate> <test_name>` (or the \
     exact workspace/package command you ran) so the next agent does not \
     pass the test name to the wrong binary.\n\n"
}

/// trace:STORY-67 | ai:claude
pub(crate) fn pr_base_head(
    project_root: &std::path::Path,
    forge: ReviewForge,
    n: u64,
) -> Result<(String, String)> {
    // STORY-1164: route the review-prompt metadata read through the Forge trait
    // instead of hand-rolling `gh pr view` / `glab mr view` here. The provider
    // owns CLI resolution, retry behavior, and GitLab's REST-shaped metadata.
    if let Ok(m) = crate::forge::forge_for_kind(project_root, forge.forge_kind())
        .change_metadata(n, &mut network_retry::NoopSink)
    {
        if !m.base_ref.is_empty() && !m.head_ref.is_empty() {
            return Ok((m.base_ref, m.head_ref));
        }
    }

    // Fallback: pure-git. We don't know the contributor's base branch,
    // so we point at `main` (the most common case in this codebase) and
    // set `head` to the local review branch (`pr-N` / `mr-N`) the user
    // is presumed to have fetched via `aida session start --owns PR-N`
    // (STORY-61). Tell the user.
    eprintln!(
        "{} couldn't resolve PR base/head via {} — falling back to base=main, head={}-{}; \
         pass an explicit base via `git log <base>..<head>` if this is wrong.",
        "Note:".yellow().bold(),
        forge.cli_name(),
        if matches!(forge, ReviewForge::GitHub) {
            "pr"
        } else {
            "mr"
        },
        n
    );
    let head = match forge {
        ReviewForge::GitHub => format!("pr-{}", n),
        ReviewForge::GitLab => format!("mr-{}", n),
    };
    Ok(("main".to_string(), head))
}

/// How many commits `origin/<base>` (the default branch's last-fetched
/// state) is ahead of `head` — i.e. how far the PR's tree trails the
/// branch its checks and findings are compared against. Purely local: it
/// reads whatever `origin/<base>` already points at and never fetches, so
/// it's cheap enough to run on every prompt generation.
///
/// Returns `None` when the count can't be resolved (no such
/// remote-tracking ref locally, `head` not resolvable, git not on PATH,
/// …). Callers must render that as "unknown", never as `0` — a `0` reads
/// as "you're current" while `None` means "we don't know", and conflating
/// them turns absent evidence into false reassurance.
// trace:BUG-1434 | ai:claude
pub(crate) fn commits_behind_default(
    project_root: &std::path::Path,
    head: &str,
    base: &str,
) -> Option<u64> {
    let remote_base = format!("origin/{}", base);
    let range = format!("{}..{}", head, remote_base);
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        // trace:BUG-1622 | ai:claude
        .args(["rev-list", "--count", git_arg_guard::END_OF_OPTIONS, &range])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<u64>()
        .ok()
}

/// `git diff --name-only <a>...<b>` (triple-dot: files changed on `b`
/// since its merge-base with `a`). Best-effort — any git failure yields an
/// empty list rather than an error, since overlap is a nicety layered on
/// top of the commits-behind count, not load-bearing on its own.
// trace:BUG-1434 | ai:claude
pub(crate) fn git_diff_name_only(project_root: &std::path::Path, a: &str, b: &str) -> Vec<String> {
    let range = format!("{}...{}", a, b);
    let Ok(out) = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        // trace:BUG-1622 | ai:claude
        .args([
            "diff",
            "--name-only",
            git_arg_guard::END_OF_OPTIONS,
            &range,
            "--",
        ])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Files the intervening commits (`head..origin/<base>`) touch that the
/// PR's own commits (`base..head`) also touch — the overlap where a stale
/// base is most likely to distort a check or finding (acceptance #2 of
/// this bug). Best-effort like `git_diff_name_only`: any git failure
/// yields an empty overlap rather than an error.
// trace:BUG-1434 | ai:claude
pub(crate) fn stale_base_file_overlap(
    project_root: &std::path::Path,
    head: &str,
    base: &str,
) -> Vec<String> {
    let remote_base = format!("origin/{}", base);
    let intervening = git_diff_name_only(project_root, head, &remote_base);
    let pr_own = git_diff_name_only(project_root, base, head);
    let pr_own_set: std::collections::HashSet<&str> = pr_own.iter().map(|s| s.as_str()).collect();
    let mut overlap: Vec<String> = intervening
        .into_iter()
        .filter(|f| pr_own_set.contains(f.as_str()))
        .collect();
    overlap.sort();
    overlap.dedup();
    overlap
}

/// Render the staleness section for `generate_review_prompt`'s `--pr` path.
/// `None` means "say nothing" — either the head is level with
/// `origin/<base>` (acceptance #5: no extra output when current) or the
/// figure legitimately can't be computed and the caller has already fallen
/// back to a different note. `commits_behind_default` returning `None`
/// (PRIN-5: absent evidence, not a false `0`) still produces `Some` text
/// here, stating "unknown" explicitly rather than omitting the section.
// trace:BUG-1434 | ai:claude
pub(crate) fn stale_base_note(
    project_root: &std::path::Path,
    head: &str,
    base: &str,
) -> Option<String> {
    match commits_behind_default(project_root, head, base) {
        None => Some(format!(
            "PR head is an unknown number of commits behind `{base}` — the local \
             `origin/{base}` ref couldn't be resolved, so staleness could not be computed. \
             Treat the checks and findings below as unverified against the current default \
             branch.\n"
        )),
        Some(0) => None,
        Some(n) => {
            let plural = if n == 1 { "" } else { "s" };
            let mut note = format!(
                "PR head is {n} commit{plural} behind `{base}` — the checks and findings below \
                 were computed against a tree that far behind the default branch.\n"
            );
            let overlap = stale_base_file_overlap(project_root, head, base);
            if !overlap.is_empty() {
                let files = overlap
                    .iter()
                    .map(|f| format!("`{f}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                note.push_str(&format!(
                    "This PR also touches file{} those {n} intervening commit{plural} touched, \
                     the case where findings are most likely distorted: {files}\n",
                    if overlap.len() == 1 { "" } else { "s" }
                ));
            }
            Some(note)
        }
    }
}

/// Run `git log <base>..<head> --pretty=format:%B%n--END--`. Returns
/// each commit message as a separate string. trace:STORY-67 | ai:claude
pub(crate) fn git_log_messages(
    project_root: &std::path::Path,
    base: &str,
    head: &str,
) -> Result<Vec<String>> {
    let range = format!("{}..{}", base, head);
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        // trace:BUG-1622 | ai:claude
        .args([
            "log",
            "--pretty=format:%B%n--END--",
            git_arg_guard::END_OF_OPTIONS,
            &range,
            "--",
        ])
        .output()
        .with_context(|| format!("running git log {}", range))?;
    if !out.status.success() {
        anyhow::bail!(
            "git log {} failed: {}",
            range,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let messages: Vec<String> = stdout
        .split("--END--")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    Ok(messages)
}

// trace:FR-0259 | ai:claude:high
/// BUG-298: find `aida-*` entries under `.claude/{skills,commands,hooks}` that
/// no longer correspond to a template the current binary ships — left behind
/// when a template was renamed/consolidated/retired. Compared by base name, so
/// a flat `aida-x.md` and a folder-form `aida-x/` both resolve to `aida-x`.
/// Symlinks are skipped — the in-repo dogfood `.claude/` is per-file symlinks
/// into the master templates and must never be pruned. trace:BUG-298 | ai:claude
pub(crate) fn detect_obe_aida_scaffold_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    // trace:TASK-1519 | ai:codex
    use std::collections::{HashMap, HashSet};
    const DIRS: [&str; 3] = ["skills", "commands", "hooks"];

    // Expected base names per scaffold dir, from the embedded template set.
    let mut expected: HashMap<&str, HashSet<String>> =
        DIRS.iter().map(|d| (*d, HashSet::new())).collect();
    for key in aida_core::templates::EMBEDDED_TEMPLATES.keys() {
        for dir in DIRS {
            if let Some(rest) = key.strip_prefix(&format!("{dir}/")) {
                let first = rest.split('/').next().unwrap_or(rest);
                let base = std::path::Path::new(first)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(first)
                    .to_string();
                expected.get_mut(dir).unwrap().insert(base);
            }
        }
    }

    let mut obe = Vec::new();
    for dir in DIRS {
        for parent in if dir == "skills" {
            vec![".claude", ".agents"]
        } else {
            vec![".claude"]
        } {
            let d = root.join(parent).join(dir);
            if parent == ".agents"
                && (!root
                    .join(".agents")
                    .symlink_metadata()
                    .is_ok_and(|m| m.file_type().is_dir())
                    || !d.symlink_metadata().is_ok_and(|m| m.file_type().is_dir()))
            {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&d) else {
                continue;
            };
            let exp = &expected[dir];
            for entry in entries.flatten() {
                let path = entry.path();
                // Never touch symlinks (the dogfood per-file symlink layout).
                if path
                    .symlink_metadata()
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false)
                {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if !name.starts_with("aida-") {
                    continue;
                }
                let base = std::path::Path::new(name)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(name)
                    .to_string();
                if !exp.contains(&base) {
                    // Shared portable skills belong to the operator unless we can
                    // prove AIDA generated this exact, unedited regular file.
                    if parent == ".agents" {
                        if !path.is_dir() {
                            continue;
                        }
                        let skill = path.join("SKILL.md");
                        if skill.symlink_metadata().is_err()
                            || skill
                                .symlink_metadata()
                                .is_ok_and(|m| !m.file_type().is_file())
                        {
                            continue;
                        }
                        let Ok(content) = std::fs::read_to_string(&skill) else {
                            continue;
                        };
                        if aida_core::scaffolding::refresh::refresh_disposition(&content)
                            != aida_core::scaffolding::refresh::RefreshDisposition::Pristine
                        {
                            continue;
                        }
                    }
                    obe.push(path);
                }
            }
        }
    }
    obe.sort();
    obe
}

/// Shared BUG-298 / BUG-719 handling: surface (and with `prune`, remove)
/// obsolete `aida-*` skills/commands/hooks this AIDA version no longer ships —
/// e.g. a skill deleted upstream but left behind, or *resurrected*, by an older
/// binary whose embedded template set still has it (BUG-719). Symlinks and
/// non-`aida-` files are never touched. Called by BOTH `scaffold apply` and
/// `scaffold upgrade` so whichever drift-fixing command the user runs cleans
/// the stray. `remove_hint` is the command to suggest when not pruning.
// trace:BUG-719 | ai:claude
pub(crate) fn report_and_prune_obe_scaffold(
    root: &std::path::Path,
    prune: bool,
    dry_run: bool,
    remove_hint: &str,
) {
    let obe = detect_obe_aida_scaffold_files(root);
    if obe.is_empty() {
        return;
    }
    println!();
    if prune && !dry_run {
        println!(
            "{} Pruning {} obsolete aida-* file(s):",
            "🧹".yellow(),
            obe.len()
        );
        for p in &obe {
            let rel = p.strip_prefix(root).unwrap_or(p);
            let removed = if p.is_dir() {
                std::fs::remove_dir_all(p)
            } else {
                std::fs::remove_file(p)
            };
            match removed {
                Ok(()) => println!("  {} {}", "-".red(), rel.display()),
                Err(e) => {
                    eprintln!(
                        "  {} {} (failed: {})",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                        rel.display(),
                        e
                    )
                }
            }
        }
    } else {
        println!(
            "{} {} obsolete aida-* file(s) this AIDA version no longer ships:",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            obe.len()
        );
        for p in &obe {
            println!(
                "  {} {}",
                "·".dimmed(),
                p.strip_prefix(root).unwrap_or(p).display()
            );
        }
        println!(
            "  Remove with: {}{}",
            remove_hint.cyan(),
            if dry_run {
                " (dry-run: not pruned)".dimmed().to_string()
            } else {
                String::new()
            }
        );
    }
}

#[cfg(test)]
#[path = "tests/bug_298_prune_tests.rs"]
mod bug_298_prune_tests;

/// Search requirements for a pattern
#[allow(clippy::too_many_arguments)]
pub(crate) fn grep_requirements(
    storage: &Storage,
    pattern: &str,
    ignore_case: bool,
    extended_regex: bool,
    after_context: usize,
    before_context: usize,
    context: Option<usize>,
    field_filter: Option<&str>,
    status_filter: Option<&str>,
    type_filter: Option<&str>,
    feature_filter: Option<&str>,
    files_with_matches: bool,
    count_only: bool,
    invert_match: bool,
) -> Result<()> {
    use regex::RegexBuilder;

    let store = storage.load_for_read()?;

    // Build the regex pattern
    let regex = if extended_regex {
        RegexBuilder::new(pattern)
            .case_insensitive(ignore_case)
            .build()
            .context("Invalid regex pattern")?
    } else {
        // Escape special regex characters for literal search
        let escaped = regex::escape(pattern);
        RegexBuilder::new(&escaped)
            .case_insensitive(ignore_case)
            .build()
            .context("Invalid pattern")?
    };

    // Parse field filter
    let fields: HashSet<&str> = if let Some(f) = field_filter {
        f.split(',').map(|s| s.trim()).collect()
    } else {
        [
            "title",
            "description",
            "comments",
            "tags",
            "owner",
            "feature",
            "spec_id",
        ]
        .iter()
        .copied()
        .collect()
    };

    // Context lines (C overrides A and B)
    let ctx_before = context.unwrap_or(before_context);
    let ctx_after = context.unwrap_or(after_context);

    let mut total_matches = 0;
    let mut matching_reqs = 0;

    for req in &store.requirements {
        // Apply filters
        if let Some(status_str) = status_filter {
            let req_status = req.status.to_string().to_lowercase();
            if !req_status.contains(&status_str.to_lowercase()) {
                continue;
            }
        }

        if let Some(type_str) = type_filter {
            let req_type = req.req_type.to_string().to_lowercase();
            if !req_type.contains(&type_str.to_lowercase()) {
                continue;
            }
        }

        if let Some(feature_str) = feature_filter {
            if !req
                .feature
                .to_lowercase()
                .contains(&feature_str.to_lowercase())
            {
                continue;
            }
        }

        // Collect matches from all fields
        let mut matches: Vec<GrepMatch> = Vec::new();

        // Search title
        if fields.contains("title") {
            if let Some(m) = search_field(&regex, "title", &req.title, ctx_before, ctx_after) {
                matches.push(m);
            }
        }

        // Search description
        if fields.contains("description") {
            for m in search_multiline_field(
                &regex,
                "description",
                &req.description,
                ctx_before,
                ctx_after,
            ) {
                matches.push(m);
            }
        }

        // Search spec_id
        if fields.contains("spec_id") {
            if let Some(spec_id) = &req.spec_id {
                if let Some(m) = search_field(&regex, "spec_id", spec_id, ctx_before, ctx_after) {
                    matches.push(m);
                }
            }
        }

        // Search owner
        if fields.contains("owner") {
            if let Some(m) = search_field(&regex, "owner", &req.owner, ctx_before, ctx_after) {
                matches.push(m);
            }
        }

        // Search feature
        if fields.contains("feature") {
            if let Some(m) = search_field(&regex, "feature", &req.feature, ctx_before, ctx_after) {
                matches.push(m);
            }
        }

        // Search tags
        if fields.contains("tags") {
            for tag in &req.tags {
                if let Some(m) = search_field(&regex, "tags", tag, ctx_before, ctx_after) {
                    matches.push(m);
                }
            }
        }

        // Search comments
        if fields.contains("comments") {
            for comment in &req.comments {
                for m in search_multiline_field(
                    &regex,
                    &format!("comment:{}", comment.author),
                    &comment.content,
                    ctx_before,
                    ctx_after,
                ) {
                    matches.push(m);
                }
            }
        }

        let has_matches = !matches.is_empty();
        let should_show = if invert_match {
            !has_matches
        } else {
            has_matches
        };

        if should_show {
            matching_reqs += 1;
            let match_count = matches.len();
            total_matches += match_count;

            let id_string = req.id.to_string();
            let spec_id = req.spec_id.as_deref().unwrap_or(&id_string);

            if files_with_matches {
                // Just print the SPEC-ID
                println!("{}", spec_id.cyan());
            } else if count_only {
                // Print count for this requirement
                println!("{}: {}", spec_id.cyan(), match_count);
            } else if invert_match {
                // For invert match, just show SPEC-ID and title
                println!("{}: {}", spec_id.cyan(), req.title);
            } else {
                // Full output with matches
                println!("{}: {}", spec_id.cyan().bold(), req.title);
                for m in matches {
                    print_grep_match(&m);
                }
                println!();
            }
        }
    }

    // Summary
    if !files_with_matches && !count_only {
        if matching_reqs == 0 {
            if invert_match {
                println!("{}", "All requirements matched the pattern.".yellow());
            } else {
                println!("{}", "No matches found.".yellow());
            }
        } else {
            println!(
                "{} match(es) in {} requirement(s)",
                total_matches.to_string().green(),
                matching_reqs.to_string().green()
            );
        }
    }

    Ok(())
}

/// A single grep match result
pub(crate) struct GrepMatch {
    pub(crate) field: String,
    pub(crate) line_num: Option<usize>,
    pub(crate) line: String,
    pub(crate) match_start: usize,
    pub(crate) match_end: usize,
    pub(crate) context_before: Vec<String>,
    pub(crate) context_after: Vec<String>,
}

/// Search a single-line field for matches
pub(crate) fn search_field(
    regex: &regex::Regex,
    field: &str,
    content: &str,
    _ctx_before: usize,
    _ctx_after: usize,
) -> Option<GrepMatch> {
    regex.find(content).map(|m| GrepMatch {
        field: field.to_string(),
        line_num: None,
        line: content.to_string(),
        match_start: m.start(),
        match_end: m.end(),
        context_before: vec![],
        context_after: vec![],
    })
}

/// Search a multiline field for matches
pub(crate) fn search_multiline_field(
    regex: &regex::Regex,
    field: &str,
    content: &str,
    ctx_before: usize,
    ctx_after: usize,
) -> Vec<GrepMatch> {
    let lines: Vec<&str> = content.lines().collect();
    let mut matches = Vec::new();

    for (line_idx, line) in lines.iter().enumerate() {
        if let Some(m) = regex.find(line) {
            // Gather context before
            let start = line_idx.saturating_sub(ctx_before);
            let context_before: Vec<String> = lines[start..line_idx]
                .iter()
                .map(|s| s.to_string())
                .collect();

            // Gather context after
            let end = (line_idx + 1 + ctx_after).min(lines.len());
            let context_after: Vec<String> = lines[line_idx + 1..end]
                .iter()
                .map(|s| s.to_string())
                .collect();

            matches.push(GrepMatch {
                field: field.to_string(),
                line_num: Some(line_idx + 1),
                line: line.to_string(),
                match_start: m.start(),
                match_end: m.end(),
                context_before,
                context_after,
            });
        }
    }

    matches
}

/// Print a grep match with highlighting
pub(crate) fn print_grep_match(m: &GrepMatch) {
    let field_display = if let Some(line_num) = m.line_num {
        format!("[{}:{}]", m.field, line_num)
    } else {
        format!("[{}]", m.field)
    };

    // Print context before
    for ctx_line in &m.context_before {
        println!("  {} {}", field_display.dimmed(), ctx_line.dimmed());
    }

    // Print the matching line with highlighted match
    let before_match = &m.line[..m.match_start];
    let match_text = &m.line[m.match_start..m.match_end];
    let after_match = &m.line[m.match_end..];

    println!(
        "  {} {}{}{}",
        field_display.blue(),
        before_match,
        match_text.red().bold(),
        after_match
    );

    // Print context after
    for ctx_line in &m.context_after {
        println!("  {} {}", field_display.dimmed(), ctx_line.dimmed());
    }
}

/// Resolve the mailbox `from` identity at send time (BUG-1533): the same
/// kind of precedence `current_user_id` uses for the queue, but reordered
/// and widened for *authorship* rather than queue routing — an explicit
/// override, then the launched agent's stable process name
/// (`AIDA_AGENT_NAME`), then the opt-in queue identity (`AIDA_USER`), then
/// the active session-role persona (`AIDA_SESSION_ROLE`) — falling back to
/// the bare shell user only last. Returns which tier resolved so the caller
/// can record it on the message instead of the sender looking silently
/// attributed.
///
/// Deliberately a SEPARATE function from `current_user_id`, not a thin
/// wrapper around it: `current_user_id` is the BUG-89 QUEUE key
/// (`AIDA_USER` first, no agent-name/role tiers, and changing it re-shards
/// which queue a shell sees). Mail authorship must be resolvable without
/// ever requiring a seat to set `AIDA_USER` — a properly-launched agent
/// already has `AIDA_AGENT_NAME` — so adopting a stable mail identity never
/// strands that seat's existing queue entries (BUG-1533 acceptance #6).
// trace:BUG-1533 | ai:claude
pub(crate) fn resolve_mail_sender_identity(
    explicit: Option<&str>,
) -> (String, aida_core::mailbox::SenderSource) {
    let shell_user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .ok();
    aida_core::mailbox::resolve_sender(
        explicit,
        std::env::var("AIDA_AGENT_NAME").ok().as_deref(),
        std::env::var("AIDA_USER").ok().as_deref(),
        std::env::var("AIDA_SESSION_ROLE").ok().as_deref(),
        shell_user.as_deref(),
    )
}

/// The sender's active session role at send time (BUG-1592, AC2): normalized
/// `AIDA_SESSION_ROLE` when it is actually set, `None` when it isn't. This is
/// deliberately NOT `resolve_effective_role`/`effective_role_resolved`, which
/// force an "implementer" default when the env var is absent — a forced
/// default would make every legacy-shaped send look like a resolved
/// "implementer" seat instead of recording that the role was simply unknown.
/// Called alongside [`resolve_mail_sender_identity`] so the envelope records
/// the role next to the agent id when both are known, mirroring how
/// BUG-1533 recorded the id half of "who sent this".
// trace:BUG-1592 | ai:claude
pub(crate) fn resolve_mail_sender_role() -> Option<String> {
    std::env::var("AIDA_SESSION_ROLE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|raw| canonical_role_name(&raw))
}

/// The queue identity for DRAINABLE handoff work (`aida backlog groom`): the
/// draining shell's `USER`, deliberately SKIPPING the agent's `AIDA_USER`
/// mailbox id. An advisor agent grooming on the human's behalf must queue where
/// the human's drain (`aida queue work` / `aida burndown run`, which resolve the
/// queue off the shell `USER`) will actually look — keying it to the agent's own
/// `AIDA_USER` made the groomed batch invisible to the drainer (BUG-605).
/// `--user` still overrides for the explicit case. trace:BUG-605 | ai:claude
pub(crate) fn drain_queue_user_id(user_override: Option<&str>) -> String {
    user_override.map(str::to_string).unwrap_or_else(|| {
        std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "default".to_string())
    })
}

/// Resolve the `aida list --user <raw>` value into a concrete handle: the
/// special token `me` (any casing) maps to `current_user`; every other value is
/// a literal handle passed through unchanged. Pure so the `me` → current-user
/// substitution is unit-testable without touching env vars.
/// trace:STORY-662 | ai:claude
pub(crate) fn resolve_list_user_filter(raw: &str, current_user: &str) -> String {
    if raw.eq_ignore_ascii_case("me") {
        current_user.to_string()
    } else {
        raw.to_string()
    }
}

/// Resolve the friendly node name to record at registration (STORY-652).
///
/// Resolution order:
/// 1. an explicit `--node-name` flag (validated, used as-is);
/// 2. at a TTY with no flag: prompt `Node name [<default>]: ` and accept the
///    default on empty input;
/// 3. non-interactive (no TTY) with no flag: the computed default silently.
///
/// The default is `<host>-<user>-<seq>` (e.g. `imac-joe-1`). The chosen name is
/// validated with the same charset rule as node ids so it stays slug-clean.
/// trace:STORY-652 | ai:claude
pub(crate) fn resolve_node_name(
    flag: Option<&str>,
    hostname: &str,
    user: &str,
    seq: &str,
) -> Result<String> {
    let default = aida_core::node::default_node_name(hostname, user, seq);
    let chosen = match flag {
        Some(f) if !f.trim().is_empty() => f.trim().to_string(),
        _ => {
            if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                use std::io::Write;
                print!("Node name [{}]: ", default);
                std::io::stdout().flush()?;
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                let answer = answer.trim();
                if answer.is_empty() {
                    default.clone()
                } else {
                    answer.to_string()
                }
            } else {
                default.clone()
            }
        }
    };
    if let Err(msg) = aida_core::node::validate_node_id(&chosen) {
        anyhow::bail!("Invalid node name: {}", msg);
    }
    Ok(chosen)
}

/// TASK-618: detect the silent cross-machine queue-collision hazard.
///
/// The distributed queue shards per `user_id`: each writes
/// `registry/queues/<user_id>.yaml` inside the orphan `aida-store` branch.
/// Distinct users (`alice.yaml` / `bob.yaml`) never collide — that's the
/// intended sharding. The hazard is when *two different machines* resolve
/// to the SAME `user_id` and therefore write the SAME file: their
/// concurrent commits produce a merge conflict on the next `aida db sync`
/// rebase (or a rejected non-ff push). This is acute for the BUG-89
/// `"default"` fallback — common in CI/containers where `$USER` /
/// `$AIDA_USER` / `$USERNAME` are all unset, so multiple unconfigured
/// clones silently share one `default.yaml`.
///
/// Per the operator decision on TASK-618 we only WARN (cheap, targets the
/// actual silent case) rather than building conflict-tolerant YAML merges.
/// The warning fires only for the genuinely dangerous shape: the resolved
/// id is the `"default"` fallback AND the existing `default.yaml` already
/// carries at least one entry stamped with a DIFFERENT machine fingerprint
/// than this clone's.
///
/// Pure over its inputs so it's directly unit-testable: pass the resolved
/// `user_id`, this machine's fingerprint, and the iterator of
/// already-on-disk `added_by_machine` values. Returns the foreign
/// fingerprint to name in the warning, or `None` when no warning is due.
/// Entries with no recorded fingerprint (`None` — pre-TASK-618 or non-CLI
/// writers) are ignored: we can't attribute them to a machine, so they
/// never trigger a false alarm.
/// trace:TASK-618 | ai:claude
pub(crate) fn default_queue_collision_fingerprint<'a, I>(
    user_id: &str,
    this_machine: &str,
    existing_fingerprints: I,
) -> Option<String>
where
    I: IntoIterator<Item = Option<&'a str>>,
{
    if user_id != "default" {
        return None;
    }
    existing_fingerprints.into_iter().flatten().find_map(|fp| {
        if !fp.is_empty() && fp != this_machine {
            Some(fp.to_string())
        } else {
            None
        }
    })
}

/// Resolve `--for <role>` / `--all` / active-session-role into the
/// effective queue role filter. Returns `(role_filter, only_unrouted)`:
/// `only_unrouted=true` means filter to entries with no `for_role`
/// (driven by `--for any`); otherwise `role_filter` is `Some(role)` to
/// match `for_role == Some(role)`, or `None` for no role filter.
///
/// `--for X` (non-"any") takes precedence over `--all` — that flag only
/// suppresses the *default* active-role filter, not an explicit override.
/// trace:BUG-87 | ai:claude
pub(crate) fn resolve_queue_role_filter(
    role: Option<&str>,
    all: bool,
    session_role: Option<&str>,
) -> (Option<String>, bool) {
    match role {
        Some(r) if r.eq_ignore_ascii_case("any") => (None, true),
        // TASK-586 / TASK-747: canonicalize the requested role so
        // `--role dialog`→`advisor` and `--role Human`→`human` match the
        // canonical entries written on add. trace:TASK-747 | ai:claude
        Some(r) => (Some(canonical_role_name(r)), false),
        None if all => (None, false),
        None => match session_role {
            Some(s) if !s.is_empty() => (Some(canonical_role_name(s)), false),
            _ => (None, false),
        },
    }
}

/// Predicate: does a queue entry pass the resolved role filter?
/// trace:BUG-87 | ai:claude
pub(crate) fn entry_matches_role_filter(
    for_role: Option<&str>,
    role_filter: Option<&str>,
    only_unrouted: bool,
) -> bool {
    if only_unrouted {
        return for_role.is_none();
    }
    match role_filter {
        Some(r) => for_role == Some(r),
        None => true,
    }
}

/// STORY-333: print a one-line warning if placing `req` at `intended_position`
/// inverts a `BlockedBy` ordering with any already-queued spec. Two shapes:
///
/// - **Forward**: `req` itself is the dependent (has `BlockedBy` edges) and
///   lands ahead of a queued blocker — surface `dependent queued ahead of
///   blocker`.
/// - **Reverse**: `req` is the *blocker* of an already-queued dependent that
///   sits ahead of the position `req` will land at — surface the same
///   message from the dependent's perspective.
///
/// Never refuses (AC8) — staging work ahead of its blocker is sometimes
/// deliberate (e.g. branching from the blocker's branch and building atop
/// it). Caller passes the intended position so we can detect inversion
/// against `i64::MAX` sentinel values (which `queue_add` resolves to
/// "max+1000"). trace:STORY-333 | ai:claude
/// Build only the requirement subset `warn_if_queued_ahead_of_blocker` reads —
/// the spec itself, its `BlockedBy` targets (forward check), and every
/// currently-queued spec (reverse check) — via TARGETED cache-backed lookups,
/// instead of `storage.load()` (a scan of every YAML on the queue write path).
/// The warn helper only does `store.requirements.iter().find(|r| r.id == …)` for
/// exactly these ids, so a store holding their closure is behaviorally
/// identical; a dangling id resolves to `None` here and to a `None` find in the
/// full store alike.
// trace:BUG-634 | ai:claude
pub(crate) fn build_queue_warn_subset(
    backend: &aida_core::CachedGitBackend,
    req: &aida_core::Requirement,
    storage: &Storage,
    user_id: &str,
) -> aida_core::RequirementsStore {
    use aida_core::DatabaseBackend;
    let mut seen: HashSet<Uuid> = HashSet::new();
    seen.insert(req.id);
    let mut requirements: Vec<aida_core::Requirement> = vec![req.clone()];
    let mut want: Vec<Uuid> = Vec::new();
    for rel in &req.relationships {
        if matches!(rel.rel_type, aida_core::RelationshipType::BlockedBy) {
            want.push(rel.target_id);
        }
    }
    if let Ok(entries) = storage.queue_list(user_id, true) {
        for e in entries {
            want.push(e.requirement_id);
        }
    }
    for id in want {
        if seen.insert(id) {
            if let Ok(Some(r)) = backend.get_requirement(&id) {
                requirements.push(r);
            }
        }
    }
    aida_core::RequirementsStore {
        requirements,
        ..Default::default()
    }
}

pub(crate) fn warn_if_queued_ahead_of_blocker(
    req: &aida_core::Requirement,
    intended_position: i64,
    store: &aida_core::RequirementsStore,
    storage: &Storage,
    user_id: &str,
) {
    let raw_entries = match storage.queue_list(user_id, true) {
        Ok(v) => v,
        Err(_) => return,
    };
    let dep_display = |r: &aida_core::Requirement| -> String {
        r.agreed_id
            .as_deref()
            .or(r.spec_id.as_deref())
            .unwrap_or("?")
            .to_string()
    };

    // Forward: this spec has BlockedBy edges pointing at queued blockers
    // that sit at a *later* position than `intended_position`.
    for rel in req
        .relationships
        .iter()
        .filter(|r| matches!(r.rel_type, aida_core::RelationshipType::BlockedBy))
    {
        let Some(target) = store.requirements.iter().find(|r| r.id == rel.target_id) else {
            continue;
        };
        if matches!(target.status, aida_core::RequirementStatus::Completed) {
            continue;
        }
        let Some(blocker_entry) = raw_entries.iter().find(|e| e.requirement_id == target.id) else {
            continue;
        };
        if intended_position < blocker_entry.position {
            eprintln!(
                "  {}  {} queued ahead of {}, which blocks it",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                dep_display(req).bold(),
                dep_display(target).bold(),
            );
        }
    }

    // Reverse: this spec *is* the blocker. Walk queued specs and warn
    // about any whose BlockedBy points at us and whose queued position
    // would land ahead of where this spec is going.
    for entry in &raw_entries {
        if entry.requirement_id == req.id {
            continue;
        }
        let Some(other) = store
            .requirements
            .iter()
            .find(|r| r.id == entry.requirement_id)
        else {
            continue;
        };
        let points_at_us = other.relationships.iter().any(|r| {
            matches!(r.rel_type, aida_core::RelationshipType::BlockedBy) && r.target_id == req.id
        });
        if points_at_us && entry.position < intended_position {
            eprintln!(
                "  {}  {} queued ahead of {}, which blocks it",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                dep_display(other).bold(),
                dep_display(req).bold(),
            );
        }
    }
}

// trace:STORY-0368 | ai:claude
pub(crate) fn effort_display_id(req: &Requirement) -> &str {
    req.agreed_id
        .as_deref()
        .or(req.spec_id.as_deref())
        .unwrap_or("?")
}

pub(crate) fn latest_effort_for_req(
    project_root: &std::path::Path,
    req: &Requirement,
) -> Option<(
    effort_calibration::EffortTouchpoint,
    effort_calibration::EffortBucket,
)> {
    let spec = effort_display_id(req);
    effort_calibration::read_capture(project_root, spec)
        .and_then(|r| r.latest_effort())
        .or_else(|| {
            let tags: Vec<String> = req.tags.iter().cloned().collect();
            [
                effort_calibration::EffortTouchpoint::Review,
                effort_calibration::EffortTouchpoint::Impl,
                effort_calibration::EffortTouchpoint::Plan,
                effort_calibration::EffortTouchpoint::Open,
            ]
            .into_iter()
            .find_map(|t| effort_calibration::effort_from_tags(&tags, t).map(|e| (t, e)))
        })
}

pub(crate) fn print_effort_load_for_requirements<'a>(
    project_root: &std::path::Path,
    title: &str,
    requirements: impl Iterator<Item = &'a Requirement>,
) {
    let mut known = Vec::new();
    let mut unknown = 0usize;
    for req in requirements {
        match latest_effort_for_req(project_root, req) {
            Some((touchpoint, bucket)) => {
                known.push((effort_display_id(req).to_string(), touchpoint, bucket))
            }
            None => unknown += 1,
        }
    }
    let total: u32 = known.iter().map(|(_, _, b)| b.minutes()).sum();
    println!(
        "{}: {} across {} estimated item{} ({} unknown)",
        title.bold(),
        effort_calibration::format_minutes(total).cyan(),
        known.len(),
        if known.len() == 1 { "" } else { "s" },
        unknown
    );
    for (spec, touchpoint, bucket) in known.iter().take(12) {
        println!("  {:<12} {:<6} {}", spec, touchpoint.as_str(), bucket);
    }
    if known.len() > 12 {
        println!("  …and {} more", known.len() - 12);
    }
}

pub(crate) fn queued_requirement_ids(storage: &Storage, user_id: &str) -> Result<HashSet<Uuid>> {
    Ok(storage
        .queue_list(user_id, false)?
        .into_iter()
        .map(|e| e.requirement_id)
        .collect())
}

/// `aida stack {show,list}` (STORY-248). Reads `.aida/stacks.json` —
/// no requirement-store dependency, so dispatches pre-storage like
/// `aida drain status`. Prints "No stacked branches." (exit 0) on an
/// empty graph so a quiet project never errors. `--prune-stale` (show
/// only) drops entries whose branch no longer exists locally or on
/// origin; the prune writes the graph back atomically.
/// trace:STORY-248 | ai:claude
pub(crate) fn handle_stack_command(cmd: &StackCommand) -> Result<()> {
    let project_root = find_main_worktree_root()
        .or_else(|_| std::env::current_dir())
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut graph = stacks::load(&project_root);

    match cmd {
        StackCommand::Show { json, prune_stale } => {
            if *prune_stale {
                let stale: Vec<String> = graph
                    .entries
                    .values()
                    .filter(|e| !branch_exists_anywhere(&project_root, &e.branch))
                    .map(|e| e.branch.clone())
                    .collect();
                if !stale.is_empty() {
                    for s in &stale {
                        stacks::remove(&mut graph, s);
                    }
                    stacks::save(&project_root, &graph)?;
                    if !*json {
                        eprintln!(
                            "  {} pruned {} stale entr{}",
                            crate::glyph(crate::glyphs::Glyph::Check).green(),
                            stale.len(),
                            if stale.len() == 1 { "y" } else { "ies" }
                        );
                    }
                }
            }
            if *json {
                let chains: Vec<Vec<&stacks::StackEntry>> = stacks::chains(&graph);
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({ "chains": chains }))?
                );
                return Ok(());
            }
            if graph.is_empty() {
                println!("No stacked branches.");
                return Ok(());
            }
            for chain in stacks::chains(&graph) {
                // Root parent of the chain is the first entry's parent —
                // typically `main`, sometimes a since-merged branch.
                if let Some(first) = chain.first() {
                    println!("{}", first.parent_branch.dimmed());
                }
                for (depth, entry) in chain.iter().enumerate() {
                    let indent = "  ".repeat(depth + 1);
                    let spec = entry
                        .spec_id
                        .as_deref()
                        .map(|s| format!(" {}", format!("({})", s).dimmed()))
                        .unwrap_or_default();
                    println!("{}└─ {}{}", indent, entry.branch.cyan(), spec);
                }
            }
            Ok(())
        }
        StackCommand::List { json } => {
            let chains = stacks::chains(&graph);
            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({ "chains": chains }))?
                );
                return Ok(());
            }
            if chains.is_empty() {
                println!("No stacked branches.");
                return Ok(());
            }
            for chain in chains {
                let root = chain
                    .first()
                    .map(|e| e.parent_branch.as_str())
                    .unwrap_or("main");
                let rest: Vec<&str> = chain.iter().map(|e| e.branch.as_str()).collect();
                println!("{} → {}", root.dimmed(), rest.join(" → ").cyan());
            }
            Ok(())
        }
    }
}

/// `aida headless tail` — clean tailer for `.aida/headless-logs/<spec>-<lease>.jsonl`
/// (TASK-398). Wraps the right JSONL filtering so the user doesn't have to
/// remember (and debug) the non-obvious jq pipeline that picks text content
/// out of multi-block assistant messages. trace:TASK-398
pub(crate) fn handle_headless_command(cmd: &HeadlessCommand) -> Result<()> {
    match cmd {
        HeadlessCommand::Tail {
            target,
            list,
            with_tools,
            tools_only,
            include_user,
            no_follow,
            since,
        } => {
            let project_root = find_main_worktree_root()
                .or_else(|_| std::env::current_dir())
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            let since_duration = match since {
                Some(s) => Some(headless_tail::parse_since(s)?),
                None => None,
            };
            let color = std::io::IsTerminal::is_terminal(&std::io::stdout())
                && std::env::var_os("NO_COLOR").is_none();
            let opts = headless_tail::TailOptions {
                selector: target.clone(),
                list: *list,
                with_tools: *with_tools,
                tools_only: *tools_only,
                include_user: *include_user,
                follow: !*no_follow,
                since: since_duration,
                color,
            };
            headless_tail::handle_tail(&project_root, &opts)
        }
    }
}

/// `aida worker directives` — list the FIFO of pending directives the
/// `aida-worker` shell function will act on next (TASK-294). Reads
/// `.aida/worker.cmd` only — no requirement-store dependency, so it
/// dispatches pre-storage like `aida drain status`. Prints "No pending
/// directives." (exit 0) when the file is empty or absent so a quiet
/// project never errors. trace:TASK-294 | ai:claude
///
/// `aida worker gc` prunes stale drain orders — spec-targeted drain lines
/// whose target spec is already archived / Completed / Rejected. The worker
/// dispatch stays pre-storage; the gc arm opens the cache-backed backend
/// itself to resolve target-spec statuses.
// trace:BUG-723 | ai:claude
pub(crate) fn handle_worker_command(cmd: &WorkerCommand) -> Result<()> {
    match cmd {
        WorkerCommand::Directives { json } => {
            let project_root = find_main_worktree_root()
                .or_else(|_| std::env::current_dir())
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            let path = worker::worker_cmd_path(&project_root);
            let directives = worker::parse_directives(&path);
            if *json {
                println!("{}", worker::render_json(&directives));
                return Ok(());
            }
            if directives.is_empty() {
                println!("No pending directives.");
            } else {
                print!("{}", worker::render_human(&directives));
            }
            Ok(())
        }
        // trace:BUG-723 | ai:claude
        WorkerCommand::Gc { dry_run } => {
            let project_root = find_main_worktree_root()
                .or_else(|_| std::env::current_dir())
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            run_worker_gc(&project_root, *dry_run)
        }
    }
}

/// `aida worker gc`: prune `drain <SPEC-ID>` directives whose target spec is
/// archived or terminal (Completed / Rejected) from `.aida/worker.cmd`.
// trace:BUG-723 trace:BUG-1670 | ai:claude
pub(crate) fn run_worker_gc(project_root: &std::path::Path, dry_run: bool) -> Result<()> {
    let path = worker::worker_cmd_path(project_root);
    let body = std::fs::read_to_string(&path).unwrap_or_default();
    if worker::parse_directives_from_str(&body).is_empty() {
        println!("No pending directives.");
        return Ok(());
    }
    let store_path = detect_distributed_store_from(project_root).ok_or_else(|| {
        anyhow::anyhow!("no requirement store found — cannot resolve directive target specs")
    })?;
    let backend = advance_backend(&store_path)?;
    // Both view axes wide open: an archived (or deferred) target must
    // still resolve so its directive is classified correctly. Strict: the
    // tolerant read serves an old snapshot while another process writes the
    // cache, and this pass deletes directives. trace:BUG-1670 | ai:claude
    let summaries = backend.list_summaries_strict(&aida_core::ListFilter {
        archive: aida_core::ArchiveFilter::Both,
        defer: aida_core::DeferFilter::Both,
        ..Default::default()
    })?;
    // Dead = the spec still exists AND is archived or terminal
    // (Completed / Rejected) — same predicate as the queue's GC. A
    // directive targeting an unknown spec is LEFT alone (fail-safe:
    // no store row means no evidence the work shipped).
    let mut dead_ids = std::collections::HashSet::new();
    for s in &summaries {
        if s.archived || is_terminal_status_str(&s.status) {
            for id in [s.spec_id.as_deref(), s.agreed_id.as_deref()]
                .into_iter()
                .flatten()
            {
                dead_ids.insert(id.to_ascii_uppercase());
            }
        }
    }
    // Every prune candidate is re-read from its stored object before its
    // directive is dropped; a read error or a missing object keeps it.
    // trace:BUG-1670 | ai:claude
    let is_dead = |spec: &str| {
        dead_ids.contains(&spec.to_ascii_uppercase()) && worker_gc_target_still_dead(&backend, spec)
    };
    let outcome = worker::gc_directives_body(&body, &is_dead);
    if outcome.pruned.is_empty() {
        println!("No stale directives to prune.");
        return Ok(());
    }
    println!(
        "{} stale directive{} (target spec archived / Completed / Rejected):",
        outcome.pruned.len(),
        if outcome.pruned.len() == 1 { "" } else { "s" }
    );
    for d in &outcome.pruned {
        println!("  - {}", d.raw);
    }
    if dry_run {
        println!("Dry run — file unchanged.");
        return Ok(());
    }
    std::fs::write(&path, &outcome.kept_body)?;
    let remaining = worker::parse_directives_from_str(&outcome.kept_body).len();
    println!(
        "Pruned {} directive{}; {} remain{}.",
        outcome.pruned.len(),
        if outcome.pruned.len() == 1 { "" } else { "s" },
        remaining,
        if remaining == 1 { "s" } else { "" }
    );
    Ok(())
}

/// Whether the stored object for a worker directive's target is still
/// archived or terminal. `get_requirement_by_spec_id` reads the spec's YAML,
/// not the cache row, so a spec reopened since any cache snapshot reads as
/// live. An unreadable or missing object is not proof the work shipped.
// trace:BUG-1670 | ai:claude
pub(crate) fn worker_gc_target_still_dead(
    backend: &aida_core::CachedGitBackend,
    spec: &str,
) -> bool {
    use aida_core::db::DatabaseBackend;
    match backend.get_requirement_by_spec_id(spec) {
        Ok(Some(req)) => req.archived || is_terminal_status_str(&format!("{:?}", req.status)),
        _ => false,
    }
}

/// TASK-306: the decision the `--no-human` kickoff gate makes — split from
/// its terminal/stdin I/O so it is unit-testable. `acknowledged` is whether
/// `AIDA_NO_HUMAN_ACKNOWLEDGED=1` is set. STORY-276 dropped `BothUnavailable`
/// — the headless implementer now ships, so `both` is a real, acknowledgeable
/// mode like `reviewer-only`; the gate keys purely off acknowledgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoHumanGate {
    /// Already acknowledged via the env var — proceed with a one-line scope
    /// reminder, no prompt.
    Acknowledged,
    /// Not yet acknowledged — show the scope banner and ask.
    NeedsAck,
}

/// TASK-306 / STORY-276: pure classification for [`no_human_kickoff_gate`].
/// The decision no longer depends on the mode (both modes ship) — only on
/// whether the scope was acknowledged. The *banner wording* still varies by
/// mode; that lives in [`no_human_kickoff_gate`].
pub(crate) fn classify_no_human_gate(acknowledged: bool) -> NoHumanGate {
    if acknowledged {
        NoHumanGate::Acknowledged
    } else {
        NoHumanGate::NeedsAck
    }
}

/// STORY-276: the one-line scope description for a `--no-human` mode — used in
/// the acknowledged-path reminder and as the lead line of the ack banner.
pub(crate) fn no_human_scope_line(mode: auto_complete::NoHumanMode) -> &'static str {
    if mode.wants_headless_implementer() {
        "--no-human=both: the implementer AND reviewer phases run headless — \
         no human at the keyboard. On a design-fork it cannot safely resolve, \
         the implementer punts the spec to Needs Attention instead of guessing."
    } else {
        "--no-human: the reviewer phase runs headless; the implementer phase \
         stays interactive and will pause for you."
    }
}

/// BUG-740: fail fast when an auto-complete drive would launch an interactive
/// implementer from a non-TTY context. The failure used to happen much later,
/// after queue/store/lease/worktree setup, when the child vendor CLI finally
/// reported "stdin is not a terminal". `--no-human=both` is the explicit
/// headless implementer mode; `AIDA_HEADLESS=1` is accepted as the environment
/// opt-in used by agent shells.
// trace:BUG-740 | ai:codex
pub(crate) fn non_tty_interactive_implementer_preflight(
    no_human: Option<auto_complete::NoHumanMode>,
    stdin_is_tty: bool,
    stdout_is_tty: bool,
    aida_headless: bool,
) -> Result<Option<auto_complete::NoHumanMode>> {
    let implementer_is_headless = no_human
        .map(auto_complete::NoHumanMode::wants_headless_implementer)
        .unwrap_or(false);
    if implementer_is_headless || (stdin_is_tty && stdout_is_tty) {
        return Ok(no_human);
    }
    if aida_headless {
        return Ok(Some(auto_complete::NoHumanMode::Both));
    }
    anyhow::bail!(
        "aida do needs a terminal for the interactive implementer — run it from \
         your shell, or add --no-human=both for a headless implementer. \
         (`aida queue work --auto-complete` has the same requirement.)"
    );
}

/// TASK-306: the pre-launch gate for `aida queue work --auto-complete
/// --no-human`, run once per kickoff. It prints a loud scope banner — what
/// runs unattended and what does not — and requires a one-time
/// acknowledgement, either interactively or up front with
/// `AIDA_NO_HUMAN_ACKNOWLEDGED=1` for an unattended run. STORY-276: `both` is
/// now a shipped mode, so it is acknowledged like `reviewer-only` rather than
/// rejected; the banner wording differs by mode. trace:TASK-306, STORY-276
/// TASK-394: machine-wide `--no-human` acknowledgement marker
/// (`~/.aida/no-human-acknowledged`) — persists across every project on the host.
pub(crate) fn no_human_machine_marker() -> Option<std::path::PathBuf> {
    crate::home_dir().map(|h| h.join(".aida").join("no-human-acknowledged"))
}

/// TASK-394: project-scoped marker (`.aida/no-human-acknowledged`) — fresh per
/// project, so the safety prompt returns on a new clone unless re-acked.
pub(crate) fn no_human_project_marker() -> std::path::PathBuf {
    std::path::Path::new(".aida").join("no-human-acknowledged")
}

/// TASK-394: is `--no-human` acknowledged via any channel? Returns the source
/// label for the reminder line. Checked order: env var (existing path) → machine
/// marker → project marker. An overnight loop acks once (the marker) instead of
/// re-exporting the env var per iteration. trace:TASK-394 | ai:claude
pub(crate) fn no_human_ack_source() -> Option<&'static str> {
    if std::env::var("AIDA_NO_HUMAN_ACKNOWLEDGED")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return Some("AIDA_NO_HUMAN_ACKNOWLEDGED");
    }
    if no_human_machine_marker()
        .map(|p| p.exists())
        .unwrap_or(false)
    {
        return Some("~/.aida/no-human-acknowledged");
    }
    if no_human_project_marker().exists() {
        return Some(".aida/no-human-acknowledged");
    }
    None
}

/// TASK-394: `aida no-human acknowledge|revoke|status` — manage the persistent
/// `--no-human` scope-acknowledgement marker so an unattended loop acks once
/// rather than re-exporting AIDA_NO_HUMAN_ACKNOWLEDGED per iteration.
/// trace:TASK-394 | ai:claude
pub(crate) fn handle_no_human_command(cmd: &cli::NoHumanCommand) -> Result<()> {
    let marker_for = |project: bool| -> Result<std::path::PathBuf> {
        if project {
            Ok(no_human_project_marker())
        } else {
            no_human_machine_marker()
                .ok_or_else(|| anyhow::anyhow!("could not resolve home directory for the marker"))
        }
    };
    match cmd {
        cli::NoHumanCommand::Acknowledge { project } => {
            let path = marker_for(*project)?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, "acknowledged\n")
                .with_context(|| format!("writing {}", path.display()))?;
            let scope = if *project {
                "this project"
            } else {
                "this machine"
            };
            println!(
                "{} --no-human acknowledged for {} ({}). Future drains skip the scope prompt.",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                scope,
                path.display()
            );
            if *project {
                println!(
                    "  {}",
                    "Note: .aida/no-human-acknowledged is gitignored runtime state (per-clone)."
                        .dimmed()
                );
            }
            Ok(())
        }
        cli::NoHumanCommand::Revoke { project } => {
            let path = marker_for(*project)?;
            if path.exists() {
                std::fs::remove_file(&path)
                    .with_context(|| format!("removing {}", path.display()))?;
                println!(
                    "{} revoked {} — the scope prompt will return.",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    path.display()
                );
            } else {
                println!("(no acknowledgement marker at {})", path.display());
            }
            Ok(())
        }
        cli::NoHumanCommand::Status => {
            match no_human_ack_source() {
                Some(src) => println!(
                    "{} --no-human is acknowledged (via {}).",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    src
                ),
                None => println!(
                    "{} --no-human is NOT acknowledged — the scope prompt will fire. \
                     Run `aida no-human acknowledge` to persist it.",
                    "○".dimmed()
                ),
            }
            Ok(())
        }
    }
}

pub(crate) fn no_human_kickoff_gate(mode: auto_complete::NoHumanMode) -> Result<()> {
    let ack_source = no_human_ack_source();
    let both = mode.wants_headless_implementer();
    match classify_no_human_gate(ack_source.is_some()) {
        NoHumanGate::Acknowledged => {
            eprintln!(
                "  {} {} (acknowledged via {})",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                no_human_scope_line(mode),
                ack_source.unwrap_or("acknowledgement"),
            );
            Ok(())
        }
        NoHumanGate::NeedsAck => {
            eprintln!();
            if both {
                eprintln!(
                    "{}  --no-human=both runs the implementer AND reviewer phases \
                     headless.",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                );
                eprintln!("   No human is at the keyboard during phase 1 (implementer).");
                eprintln!(
                    "   On a design-fork it cannot safely resolve, the implementer \
                     punts the spec"
                );
                eprintln!(
                    "   to Needs Attention instead of guessing — review punts later \
                     with `aida findings list`."
                );
            } else {
                eprintln!(
                    "{}  --no-human covers the reviewer phase only.",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                );
                eprintln!("   Phase 1 (implementer) still requires interactive input.");
                eprintln!("   The drain will pause at each phase-1 completion until you act.");
                eprintln!(
                    "   {}",
                    "For a fully headless drain use `--no-human=both`.".dimmed()
                );
            }
            eprintln!(
                "   {}",
                "Set AIDA_NO_HUMAN_ACKNOWLEDGED=1 to skip this prompt on an \
                 unattended run,"
                    .dimmed()
            );
            // BUG-703: keep the SPEC-ID out of this user-facing warning — a
            // trace marker in stderr is developer noise to a first-user.
            // trace:TASK-394 | ai:claude
            eprintln!(
                "   {}",
                "or `aida no-human acknowledge` once to persist it across runs \
                 (per machine; --project to scope to this repo)."
                    .dimmed()
            );
            eprintln!();
            // `--no-human` is meant for unattended drains, so a non-terminal
            // stdin is the expected unattended case — but it cannot answer a
            // prompt. Fail loud with the env-var escape rather than blocking
            // forever on a stdin read. trace:TASK-306 | ai:claude
            if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                anyhow::bail!(
                    "--no-human needs a one-time scope acknowledgement, but stdin \
                     is not a terminal. Re-run with AIDA_NO_HUMAN_ACKNOWLEDGED=1 to \
                     confirm you understand its scope."
                );
            }
            if !prompt_yes_no("   Continue? [y/N] ", false)? {
                anyhow::bail!("aborted — --no-human scope not acknowledged");
            }
            Ok(())
        }
    }
}

/// TASK-405: the `--from-pr` entry point — PR-only invocation. Implementation
/// shipped OUTSIDE the orchestrator (a PR is already open for `spec`), so drive
/// the remaining phases (reviewer → CI → merge → pull → build) WITHOUT
/// re-running the implementer. Probes the PR's real state, decides the entry
/// phase (or a clean refusal) via the pure [`drain_resume::from_pr_plan`], and
/// — unless `--dry-run` (the shared `--resume-dry-run` flag) — re-enters at that
/// phase via `run_auto_complete`'s resume-entry seam. Never returns.
///
/// Distinct from `handle_drain_resume`: there is no crashed drain-state file,
/// no PID-liveness gate (the implementer ran elsewhere, possibly by hand), and
/// the entry phase is computed straight from probed PR/spec reality.
/// trace:TASK-405 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_from_pr(
    storage: &Storage,
    user_id: &str,
    scope: &str,
    variant: auto_complete::AutoCompleteVariant,
    dry_run: bool,
    json: bool,
    permission_mode: Option<&str>,
    no_human: Option<auto_complete::NoHumanMode>,
    escalate_mode: auto_complete::EscalateMode,
    steal: bool,
    force_claim: bool,
    allow_stale_base: bool,
    no_auto_rebase: bool,
) -> ! {
    let project_root = match storage.path().parent() {
        Some(p) => p.to_path_buf(),
        None => {
            eprintln!(
                "{} cannot derive project root from the store path",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold()
            );
            std::process::exit(1);
        }
    };
    let (spec, seeded_pr) = match parse_review_scope(scope) {
        Some((forge, n)) => match storage
            .load()
            .map_err(|e| anyhow::anyhow!(e))
            .and_then(|store| resolve_pr_to_spec(&project_root, n as u32, &store))
        {
            Ok(spec) => (spec, Some(n as u32)),
            Err(e) => {
                eprintln!(
                    "{} could not resolve {} to a backing spec: {}",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    format_review_label(forge, n),
                    e
                );
                std::process::exit(1);
            }
        },
        None => (scope.to_string(), None),
    };

    // Probe the world for this spec's PR + per-phase postconditions. Passing
    // `member: None` is exactly the standalone-PR case `probe_resume_facts`
    // already handles (it falls back to a forge lookup by spec when drain-state
    // recorded no PR). trace:TASK-405 | ai:claude
    let (mut facts, branch, pr) = probe_resume_facts(&project_root, storage, &spec, None);
    let pr = seeded_pr.or(pr);
    let branch = if seeded_pr.is_some() && branch.is_none() {
        pr.and_then(|n| pr_head_branch(&project_root, n as u64))
    } else {
        branch
    };
    if let Some(n) = seeded_pr {
        let mut sink = network_retry::StderrSink;
        facts.pr_merged = pr_is_merged_with_sink(&project_root, n, &mut sink).unwrap_or(false);
        facts.branch_exists = true;
    }
    // BUG-1819: `probe_resume_facts` only carries a green bit. Reconcile the
    // provider's terminal state with the exact head it covered so a direct
    // phase-3 entry can inherit the same head-bound proof phase 2 records.
    let forge_kind = crate::forge::resolve_forge_kind(&project_root);
    let mut resume_ci_terminal_sha = None;
    let mut resume_ci_terminal_green = None;
    let mut resume_ci_refusal = None;
    if forge_kind != crate::forge::ForgeKind::None {
        facts.ci_green = false;
        if let (Some(pr), Some(branch)) = (pr, branch.as_deref()) {
            let mut sink = network_retry::StderrSink;
            let expected_head = crate::forge::forge_for_kind(&project_root, forge_kind)
                .change_metadata(pr as u64, &mut sink)
                .ok()
                .map(|metadata| metadata.head_sha);
            let evidence =
                crate::forge::ci_probe_evidence_for_branch(&project_root, forge_kind, branch);
            match verified_from_pr_ci_head(pr, expected_head.as_deref(), &evidence) {
                Ok(head) => {
                    facts.ci_green = true;
                    resume_ci_terminal_sha = Some(head);
                    resume_ci_terminal_green = Some(true);
                }
                Err(failure) => resume_ci_refusal = Some(failure.reason),
            }
        }
    }
    let pr_exists = pr.is_some();

    match drain_resume::from_pr_plan(pr_exists, &facts) {
        drain_resume::FromPrOutcome::RefuseAlreadyCompleted => {
            eprintln!(
                "{} `{}` is already Completed — the merge already promoted it; nothing to drive.",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                spec
            );
            eprintln!(
                "  {} `--from-pr` engages the orchestrator on an OPEN PR — there is no open work here.",
                "→".dimmed()
            );
            std::process::exit(1);
        }
        drain_resume::FromPrOutcome::RefuseNoPr => {
            eprintln!(
                "{} no open PR found for `{}` — nothing to drive with `--from-pr`.",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                spec
            );
            eprintln!(
                "  {} `--from-pr` drives phases 3-6 on a PR shipped outside the orchestrator. \
                 To run the FULL pipeline (implementer first) drop `--from-pr`.",
                "→".dimmed()
            );
            std::process::exit(1);
        }
        drain_resume::FromPrOutcome::RefuseAlreadyMerged => {
            eprintln!(
                "{} {} is already merged — the merge already happened, so there is nothing to drive.",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                pr.map(|n| format!("PR-{n}")).unwrap_or_else(|| "the PR".into())
            );
            eprintln!(
                "  {} run `aida pull` to auto-bump `{}` Done → Completed.",
                "→".dimmed(),
                spec
            );
            std::process::exit(1);
        }
        drain_resume::FromPrOutcome::DriveFrom(probed_start_phase) => {
            // `aida drain resume` pins the failed phase from the newest
            // SpecShelved row; the normal --from-pr path stays reality-based.
            // trace:TASK-1272 | ai:codex
            let recorded_phase = std::env::var("AIDA_DRAIN_RESUME_PHASE")
                .ok()
                .as_deref()
                .and_then(drain_resume::shelved_resume_phase);
            let start_phase = recorded_phase
                .map(|recorded| {
                    drain_resume::reconciled_shelved_phase(recorded, probed_start_phase)
                })
                .unwrap_or(probed_start_phase);
            // Last line of defence, keyed on the FACT not a phase name: a recorded
            // Reviewer outranks a probed Ci through reconciliation and would skip
            // the wait even after `from_pr_plan` demanded CI. See
            // `ci_gated_start_phase` for why a phase-name check is not enough.
            // trace:BUG-1460 trace:TASK-1272 | ai:claude
            let start_phase = drain_resume::ci_gated_start_phase(start_phase, facts.ci_green);
            if start_phase == auto_complete::Phase::Ci {
                if let Some(reason) = resume_ci_refusal.as_deref() {
                    eprintln!(
                        "  {} {reason}; re-running phase 2 before review",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                    );
                }
            }
            println!(
                "{} driving `{}` from phase {} ({}) — implementation shipped outside the \
                 orchestrator (skipping the implementer phase).",
                "↩".cyan().bold(),
                spec,
                start_phase.index(),
                start_phase.slug()
            );
            if let Some(n) = pr {
                println!("  {} seeded PR-{}", "→".dimmed(), n);
            }
            if dry_run {
                println!(
                    "  {} --resume-dry-run — not re-entering. Drop it to drive the PR.",
                    "→".dimmed()
                );
                std::process::exit(0);
            }
            // The implementer ran outside the orchestrator and may hold a stale
            // lease on this scope; release a dead/clean one so the reviewer
            // phase (which resolves PR→spec) doesn't collide. Same guard the
            // resume path applies (BUG-438). trace:TASK-405 | ai:claude
            release_dead_leases_for_resume(&project_root, &spec);
            let resume_entry = Some(ResumeEntry {
                start_phase,
                branch,
                pr,
                head_sha: None,
                from_pr: true,
                ci_terminal_sha: resume_ci_terminal_sha,
                ci_terminal_green: resume_ci_terminal_green,
            });
            let result = run_auto_complete(
                storage,
                user_id,
                &spec,
                variant,
                json,
                permission_mode,
                no_human,
                escalate_mode,
                // A standalone `--from-pr` drive owns its drain-state file.
                true,
                steal,
                force_claim,
                allow_stale_base,
                no_auto_rebase,
                resume_entry,
            );
            // TASK-1054: collapse the failed-phase index to the canonical
            // 0/2/3 process code so a wrapping script can branch on the outcome.
            std::process::exit(result.process_exit_code());
        }
    }
}

/// Entry point for `aida queue work <SPEC> --auto-complete`. Never returns:
/// always terminates the process with an exit code (0 success, 1-6 = the
/// 1-based index of the phase that failed). trace:STORY-246 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_auto_complete(
    storage: &Storage,
    user_id: &str,
    spec: &str,
    variant: auto_complete::AutoCompleteVariant,
    json: bool,
    permission_mode: Option<&str>,
    no_human: Option<auto_complete::NoHumanMode>,
    escalate_mode: auto_complete::EscalateMode,
    // BUG-311: thread the outer `--steal` through so phase 1's
    // `aida queue work` subprocess can clear a dormant lease on this scope.
    steal: bool,
    // TASK-559: thread the outer `--force-claim` through so phase 1 can
    // recover NeedsAttention / ambiguous InProgress specs the same way a
    // direct `aida queue work <SPEC> --force-claim` does.
    force_claim: bool,
    // STORY-281: thread the outer `--allow-stale-base` through so phase 3's
    // pre-flight stale-base check warns-only instead of refusing.
    allow_stale_base: bool,
    // STORY-429: opt out of phase-3 auto-rebase recovery.
    no_auto_rebase: bool,
) -> ! {
    let result = run_auto_complete(
        storage,
        user_id,
        spec,
        variant,
        json,
        permission_mode,
        no_human,
        escalate_mode,
        // STORY-301: a bare single-spec drain owns its drain-state file.
        true,
        steal,
        force_claim,
        allow_stale_base,
        no_auto_rebase,
        // STORY-492: not a resume — start at phase 1.
        None,
    );
    // STORY-493: drain-end mailbox digest for the single-spec drain, matching
    // the batch / batch-chain paths. Best-effort + non-fatal. trace:STORY-493
    if let Ok(root) = find_main_worktree_root() {
        maybe_digest_mailbox_best_effort(&root.join(".aida-store"), "drain-end");
    }
    // TASK-1054: collapse the failed-phase index to the canonical 0/2/3 process
    // code (0 clean, 2 shelved/parked, 3 hard fail) so a wrapping script can
    // branch on the outcome. trace:TASK-1054 | ai:claude
    std::process::exit(result.process_exit_code());
}

// trace:BUG-1120 | ai:codex
pub(crate) fn guarded_execution_mode_for_drain(
    storage: &Storage,
    spec: &str,
) -> Option<aida_core::ExecutionMode> {
    storage
        .load()
        .ok()
        .and_then(|store| store.get_requirement_by_spec_id(spec).cloned())
        .and_then(|req| req.execution_mode)
        .filter(|mode| {
            matches!(
                mode,
                aida_core::ExecutionMode::Guided
                    | aida_core::ExecutionMode::Operator
                    | aida_core::ExecutionMode::Decide
            )
        })
}

// trace:BUG-1120 | ai:codex
pub(crate) fn guarded_execution_mode_drain_message(
    spec: &str,
    mode: aida_core::ExecutionMode,
) -> String {
    format!(
        "skipped {spec} — needs guided/operator session ({mode}); use `aida queue work {spec} --guided` or `aida do {spec}`"
    )
}

// trace:BUG-1574 | ai:claude
/// Whether `req` requires a human at the keyboard and must never be driven,
/// stolen, rebased, or force-pushed by a headless/unattended drain. True
/// when EITHER the literal `keyboard-only` tag is present OR the groomed
/// `execution_mode` is one of the supervised/interactive modes
/// (Guided/Operator/Decide). Both signals are checked — independently —
/// because a spec can be tagged `keyboard-only` while still carrying
/// `execution_mode: drain` (the BUG-1574 incident spec, TASK-1274, was
/// exactly this shape: `execution_mode: drain` + the `keyboard-only` tag).
/// The tag is the human's explicit override and must win regardless of the
/// groomed mode.
pub(crate) fn spec_is_keyboard_only(req: &aida_core::Requirement) -> bool {
    req.tags
        .iter()
        .any(|t| t.eq_ignore_ascii_case("keyboard-only"))
        || matches!(
            req.execution_mode,
            Some(aida_core::ExecutionMode::Guided)
                | Some(aida_core::ExecutionMode::Operator)
                | Some(aida_core::ExecutionMode::Decide)
        )
}

// trace:BUG-1574 | ai:claude
/// AC2: dispatch-time refusal for `aida queue work ... --no-human` — a spec
/// that is keyboard-only ([`spec_is_keyboard_only`]) must never be handed to
/// a headless implementer, even when it groomed to `execution_mode: drain`.
/// Fail-closed: a store that won't load or a spec that can't be resolved
/// also refuses (never treated as "nothing to refuse on").
pub(crate) fn keyboard_only_dispatch_refusal(storage: &Storage, spec: &str) -> Option<String> {
    let Some(store) = storage.load().ok() else {
        return Some(format!(
            "skipped {spec} — could not load the store to verify it is safe for a headless \
             (--no-human) drain; refusing (fail closed)"
        ));
    };
    let Some(req) = store.get_requirement_by_spec_id(spec) else {
        return Some(format!(
            "skipped {spec} — could not be resolved in the store; refusing a headless \
             (--no-human) drain (fail closed)"
        ));
    };
    if spec_is_keyboard_only(req) {
        return Some(format!(
            "skipped {spec} — keyboard-only spec refused for a headless (--no-human) drain; \
             use `aida queue work {spec} --guided` or `aida do {spec}`"
        ));
    }
    None
}

// trace:BUG-1574 | ai:claude
/// A parsed snapshot of `.aida/drain-state.json`'s batch-scoping facts —
/// distinguishes "no drain running" from "a drain IS running with no batch"
/// from "the file exists but didn't parse" (which must fail closed, never
/// collapse to "no batch active" the way `DrainState::read`'s `Option`
/// return does).
pub(crate) enum DrainStateProbe {
    NoActiveDrain,
    Malformed,
    Active {
        batch: Option<String>,
        members: Vec<String>,
    },
}

// trace:BUG-1574 | ai:claude
pub(crate) fn probe_drain_state(project_root: &std::path::Path) -> DrainStateProbe {
    let path = drain_state::drain_state_path(project_root);
    match std::fs::read_to_string(&path) {
        Err(_) => DrainStateProbe::NoActiveDrain,
        Ok(body) => match serde_json::from_str::<drain_state::DrainState>(&body) {
            Err(_) => DrainStateProbe::Malformed,
            Ok(state) => DrainStateProbe::Active {
                batch: state.batch,
                members: state.members.into_iter().map(|m| m.spec).collect(),
            },
        },
    }
}

// trace:BUG-1574 | ai:claude
/// Pure fail-closed decision core for any unattended git-mutating action
/// (steal, rebase, force-push) against `spec`'s branch:
///   - `req: None` (store unreadable OR spec unresolved) → refuse, except for
///     exact synthetic review scopes (`PR-N` / `MR-N`), which deliberately do
///     not exist as requirements.
///   - `spec` is keyboard-only ([`spec_is_keyboard_only`]) → refuse.
///   - [`DrainStateProbe::Malformed`] → refuse (a torn/corrupt
///     `drain-state.json` must never read as "no batch active").
///   - a batch IS active and `spec` is not a declared member → refuse.
/// `None` (no refusal) only when the spec resolves and is not keyboard-only,
/// or the scope is an exact synthetic review scope; the drain must also have
/// no active batch or include `spec` among its members.
pub(crate) fn unattended_git_mutation_refusal_for(
    spec: &str,
    req: Option<&aida_core::Requirement>,
    drain: &DrainStateProbe,
) -> Option<String> {
    // Review sessions own synthetic PR-N / MR-N scopes rather than a
    // requirement. Requiring a store row here makes a clean dead reviewer
    // lease impossible to reclaim with --steal. Exact parsing matters: an
    // ordinary unknown spec must retain the fail-closed behavior.
    // trace:BUG-1820 | ai:codex
    match req {
        Some(req) if spec_is_keyboard_only(req) => {
            return Some(format!(
                "{spec} is keyboard-only (tag or execution_mode) — refusing an unattended \
                 rebase/force-push/steal"
            ));
        }
        Some(_) => {}
        None if parse_review_scope(spec).is_some() => {}
        None => {
            return Some(format!(
                "{spec} could not be resolved (store unreadable or spec unknown) — refusing an \
                 unattended rebase/force-push/steal (fail closed)"
            ));
        }
    }
    match drain {
        DrainStateProbe::NoActiveDrain => None,
        DrainStateProbe::Malformed => Some(format!(
            "drain-state.json exists but could not be parsed — refusing to touch {spec} \
             outside a known batch scope (fail closed)"
        )),
        DrainStateProbe::Active { batch: None, .. } => None,
        DrainStateProbe::Active {
            batch: Some(b),
            members,
        } => {
            if members.iter().any(|m| m == spec) {
                None
            } else {
                Some(format!(
                    "{spec} is not a member of the active batch `{b}` — refusing to touch a \
                     branch outside its declared scope"
                ))
            }
        }
    }
}

// trace:BUG-1574 | ai:claude
/// Walk up from `project_root` and report whether `.aida/config.toml` exists
/// anywhere in the ancestor chain — WITHOUT caring whether it parses or
/// declares a resolvable store (that is [`detect_distributed_store_from`]'s
/// job). This is the narrow signal the fail-open carve-out needs: "is this
/// even an AIDA-managed project root at all" — a bare git fixture (a unit
/// test, a non-AIDA repo) has no `.aida/` anywhere and is a clean no-op; a
/// real AIDA project always has one, so from there on a failure to resolve
/// the store/spec is a genuine problem, not an absence, and must fail
/// closed (refuse), never silently read as "nothing to check".
pub(crate) fn config_toml_exists_upward(project_root: &std::path::Path) -> bool {
    config_toml_exists_upward_with_roots(project_root, &aida_core::store_locate::real_temp_roots())
}

/// [`config_toml_exists_upward`], parameterized on the temp roots to guard
/// against.
///
/// BUG-1598: must agree with `detect_distributed_store_from` on where the
/// walk-up stops. Without this guard, a stray `.aida/config.toml` sitting
/// directly in a temp root makes this function report `true` (an
/// AIDA-managed project root exists) while the now-guarded
/// `detect_distributed_store_from` correctly refuses to resolve a store
/// from it — `unattended_git_mutation_refusal` then falls through PAST its
/// only fail-open carve-out (this function returning `false`) and fails
/// CLOSED (refuses the mutation) for a project that, from
/// `unattended_git_mutation_refusal`'s point of view, has no AIDA config at
/// all. Factored out as `_with_roots` so a test can exercise the guard
/// against a fake root without mutating `TMPDIR` or touching the real,
/// shared system temp dir.
// trace:BUG-1598 | ai:claude
pub(crate) fn config_toml_exists_upward_with_roots(
    project_root: &std::path::Path,
    temp_roots: &[std::path::PathBuf],
) -> bool {
    // Canonicalize the root set ONCE, before the loop — not on every
    // ancestor level.
    let canonical_roots = aida_core::store_locate::canonicalize_roots(temp_roots);
    let mut current = project_root;
    loop {
        if aida_core::store_locate::is_in_canonical_roots(current, &canonical_roots) {
            return false;
        }
        if current.join(".aida").join("config.toml").is_file() {
            return true;
        }
        match current.parent() {
            Some(p) => current = p,
            None => return false,
        }
    }
}

// trace:BUG-1574 | ai:claude
/// Pure: does `store` already carry an OPEN (not Completed/Rejected) finding
/// for this exact `(spec, branch)` pair? Matches on the tag triple a filed
/// refusal carries: `kind:headless-refusal` + `from-implementer:<spec>` +
/// (`branch:<branch>` when `branch` is known). Factored out so "must never
/// file duplicates" is testable against an in-memory store, without a real
/// `aida add`/`aida list` round trip.
pub(crate) fn has_open_refusal_finding(
    store: &aida_core::RequirementsStore,
    spec: &str,
    branch: Option<&str>,
) -> bool {
    let from_tag = format!("from-implementer:{spec}");
    let branch_tag = branch.map(|b| format!("branch:{b}"));
    store.requirements.iter().any(|r| {
        !matches!(
            r.status,
            aida_core::RequirementStatus::Completed | aida_core::RequirementStatus::Rejected
        ) && r.tags.contains("kind:headless-refusal")
            && r.tags.contains(&from_tag)
            && branch_tag.as_ref().is_none_or(|bt| r.tags.contains(bt))
    })
}

// trace:BUG-1574 | ai:claude
/// AC3: best-effort — file a finding (the existing `aida findings` tag
/// convention: a Draft Task tagged `from-implementer:<spec>`) recording that
/// a headless action was refused, so the refusal is visible on
/// `aida findings list` / `aida awaiting` — not just eprintln'd where only
/// the refusing process's own log carries it. This is what lets the OTHER
/// session (whose branch almost got moved) find out what nearly happened.
/// Dedupes via [`has_open_refusal_finding`] first — a drain or `--watch`
/// loop that keeps hitting the same guard (the common case: a stacked child
/// stays outside the batch on every pass until someone acts) must file ONE
/// finding, not one per pass. `branch` — when known — lands in both the
/// title and a `branch:<name>` tag, so which physical branch nearly moved is
/// visible without opening the description. A failure to file (or to check
/// for dupes) is swallowed — filing must never crash the caller.
pub(crate) fn record_headless_refusal_finding(
    project_root: &std::path::Path,
    store: Option<&aida_core::RequirementsStore>,
    spec: &str,
    action: &str,
    branch: Option<&str>,
    reason: &str,
) {
    if let Some(store) = store {
        if has_open_refusal_finding(store, spec, branch) {
            return;
        }
    }
    let title = match branch {
        Some(b) => format!("Headless {action} refused for {spec} (branch {b})"),
        None => format!("Headless {action} refused for {spec}"),
    };
    let mut tags = format!("from-implementer:{spec},severity:notice,kind:headless-refusal");
    if let Some(b) = branch {
        tags.push_str(&format!(",branch:{b}"));
    }
    let _ = std::process::Command::new(aida_exe_path())
        .current_dir(project_root)
        .args([
            "add",
            "--title",
            &title,
            "--description",
            reason,
            "--type",
            "task",
            "--status",
            "draft",
            "--tags",
            &tags,
        ])
        .output_retrying_etxtbsy();
}

// trace:BUG-1574 | ai:claude
/// Impure wrapper: loads the store + probes `.aida/drain-state.json`, then
/// delegates to the pure [`unattended_git_mutation_refusal_for`] core. This
/// is what call sites (the `--steal` scope-conflict loop, the stack-aware
/// promotion rebase/force-push, the phase-3 auto-rebase) use. `action` is a
/// short human phrase ("steal", "rebase/force-push") and `branch` — when
/// known — is the specific branch about to be touched; both land in the
/// finding filed on refusal.
///
/// The ONLY fail-open carve-out: no `.aida/config.toml` anywhere upward from
/// `project_root` ([`config_toml_exists_upward`]) — not an AIDA-managed
/// project root at all (a bare git fixture in a unit test, a legacy repo),
/// so there is nothing to scope/tag-check against. From the moment a config
/// IS found, this fails closed on everything else: a store the config
/// declares but that can't be resolved/loaded, or a spec that can't be
/// found within it, both refuse — that is the real BUG-1574 failure mode (a
/// corrupted/unreadable store, or a since-deleted spec), and must never
/// silently read as "nothing to check". Deliberately bypasses
/// [`load_store_for_lookup`]'s legacy-YAML fallback entirely: this check
/// requires the distributed store the config declares, so a legacy store
/// sitting beside it must not satisfy it. (Until BUG-1732 that fallback also
/// resolved off the process CWD rather than `project_root`, which is the
/// hazard this bypass was originally written against; the fallback is now
/// project-scoped, so only the stricter requirement remains.)
pub(crate) fn unattended_git_mutation_refusal(
    project_root: &std::path::Path,
    spec: &str,
    action: &str,
    branch: Option<&str>,
) -> Option<String> {
    if !config_toml_exists_upward(project_root) {
        return None;
    }
    let store: Option<aida_core::RequirementsStore> = detect_distributed_store_from(project_root)
        .and_then(|store_path| aida_core::GitBackend::new(&store_path).ok())
        .and_then(|backend| aida_core::DatabaseBackend::load(&backend).ok());
    let req = store
        .as_ref()
        .and_then(|s| s.get_requirement_by_spec_id(spec));
    let drain = probe_drain_state(project_root);
    let reason = unattended_git_mutation_refusal_for(spec, req, &drain)?;
    record_headless_refusal_finding(project_root, store.as_ref(), spec, action, branch, &reason);
    Some(reason)
}

#[cfg(test)]
mod bug_1574_unattended_git_mutation_tests {
    use super::*;

    fn req_with(tags: &[&str], mode: Option<aida_core::ExecutionMode>) -> aida_core::Requirement {
        let mut r = aida_core::Requirement::new("t".into(), "d".into());
        for t in tags {
            r.tags.insert(t.to_string());
        }
        r.execution_mode = mode;
        r
    }

    fn active(batch: Option<&str>, members: &[&str]) -> DrainStateProbe {
        DrainStateProbe::Active {
            batch: batch.map(|s| s.to_string()),
            members: members.iter().map(|s| s.to_string()).collect(),
        }
    }

    // ── fail-closed: missing req (store load failure OR unresolved spec) ────

    #[test]
    fn refuses_when_req_is_none() {
        let reason =
            unattended_git_mutation_refusal_for("TASK-1", None, &DrainStateProbe::NoActiveDrain);
        assert!(reason.is_some());
        assert!(reason.unwrap().contains("fail closed"));
    }

    // BUG-1820: reviewer leases use synthetic scopes, so a missing
    // requirement is expected for both forge spellings. The exact parser
    // keeps unknown and malformed ids on the fail-closed path.
    // trace:BUG-1820 | ai:codex
    #[test]
    fn allows_synthetic_pr_and_mr_scopes_without_requirement_rows() {
        for scope in ["PR-26", "MR-26", "pr-26", "mr-26"] {
            assert!(
                unattended_git_mutation_refusal_for(scope, None, &DrainStateProbe::NoActiveDrain)
                    .is_none(),
                "synthetic review scope {scope} must not require a requirement row"
            );
        }
    }

    #[test]
    fn malformed_review_like_scopes_still_fail_closed() {
        for scope in ["PR-x", "MR-", "PR-26-extra", "REVIEW-26"] {
            let reason =
                unattended_git_mutation_refusal_for(scope, None, &DrainStateProbe::NoActiveDrain);
            assert!(
                reason.is_some_and(|reason| reason.contains("fail closed")),
                "review-like scope {scope} must not bypass requirement lookup"
            );
        }
    }

    #[test]
    fn synthetic_review_scope_still_refuses_malformed_drain_state() {
        let reason =
            unattended_git_mutation_refusal_for("MR-26", None, &DrainStateProbe::Malformed);
        assert!(reason.is_some_and(|reason| reason.contains("fail closed")));
    }

    #[test]
    fn synthetic_review_scope_outside_active_batch_still_refuses() {
        let drain = active(Some("night-0920"), &["STORY-207"]);
        let reason = unattended_git_mutation_refusal_for("PR-26", None, &drain);
        assert!(reason.is_some_and(|reason| reason.contains("night-0920")));
    }

    // ── keyboard-only: tag wins even when execution_mode is drain ──────────

    #[test]
    fn refuses_on_keyboard_only_tag_even_with_drain_mode() {
        let req = req_with(&["keyboard-only"], Some(aida_core::ExecutionMode::Drain));
        let reason = unattended_git_mutation_refusal_for(
            "TASK-1274",
            Some(&req),
            &DrainStateProbe::NoActiveDrain,
        );
        assert!(
            reason.is_some(),
            "the tag alone must refuse, mode notwithstanding"
        );
        assert!(reason.unwrap().contains("keyboard-only"));
    }

    #[test]
    fn refuses_on_guided_mode_without_the_tag() {
        let req = req_with(&[], Some(aida_core::ExecutionMode::Guided));
        assert!(unattended_git_mutation_refusal_for(
            "TASK-2",
            Some(&req),
            &DrainStateProbe::NoActiveDrain
        )
        .is_some());
    }

    // ── malformed drain-state.json: fail closed, never "no batch" ──────────

    #[test]
    fn refuses_on_malformed_drain_state() {
        let req = req_with(&[], Some(aida_core::ExecutionMode::Drain));
        let reason =
            unattended_git_mutation_refusal_for("TASK-3", Some(&req), &DrainStateProbe::Malformed);
        assert!(reason.is_some());
        assert!(reason.unwrap().contains("fail closed"));
    }

    // ── batch membership ─────────────────────────────────────────────────

    #[test]
    fn refuses_spec_outside_active_batch() {
        let req = req_with(&[], Some(aida_core::ExecutionMode::Drain));
        let drain = active(Some("night-0920"), &["STORY-1", "STORY-2"]);
        let reason = unattended_git_mutation_refusal_for("TASK-1274", Some(&req), &drain);
        assert!(reason.is_some());
        assert!(reason.unwrap().contains("night-0920"));
    }

    // ── controls: the allowed path ──────────────────────────────────────

    #[test]
    fn allows_drain_mode_member_of_active_batch() {
        let req = req_with(&[], Some(aida_core::ExecutionMode::Drain));
        let drain = active(Some("night-0920"), &["TASK-1274", "STORY-2"]);
        assert!(unattended_git_mutation_refusal_for("TASK-1274", Some(&req), &drain).is_none());
    }

    #[test]
    fn allows_no_execution_mode_no_tag_when_no_batch_active() {
        let req = req_with(&[], None);
        assert!(unattended_git_mutation_refusal_for(
            "TASK-1274",
            Some(&req),
            &DrainStateProbe::NoActiveDrain
        )
        .is_none());
    }

    #[test]
    fn allows_unlisted_spec_when_active_drain_has_no_batch() {
        // A single-spec / next-n drain writes drain-state.json with
        // `batch: None` — never refuse on membership in that case.
        let req = req_with(&[], Some(aida_core::ExecutionMode::Drain));
        let drain = active(None, &[]);
        assert!(unattended_git_mutation_refusal_for("TASK-1274", Some(&req), &drain).is_none());
    }

    // ── probe_drain_state: distinguishes absent vs malformed on disk ──────

    #[test]
    fn probe_distinguishes_absent_from_malformed_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            probe_drain_state(dir.path()),
            DrainStateProbe::NoActiveDrain
        ));
        let path = drain_state::drain_state_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ this is not valid json").unwrap();
        assert!(matches!(
            probe_drain_state(dir.path()),
            DrainStateProbe::Malformed
        ));
    }

    // ── AC2: keyboard_only_dispatch_refusal (the --no-human dispatch gate) ──

    fn storage_with(
        spec: &str,
        tags: &[&str],
        mode: Option<aida_core::ExecutionMode>,
    ) -> (tempfile::TempDir, Storage) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().join("requirements.yaml"));
        let mut store = aida_core::RequirementsStore::new();
        let mut req = req_with(tags, mode);
        req.spec_id = Some(spec.to_string());
        store.requirements.push(req);
        storage.save(&store).unwrap();
        (dir, storage)
    }

    #[test]
    fn dispatch_refuses_keyboard_only_tag() {
        let (_dir, storage) = storage_with(
            "TASK-1274",
            &["keyboard-only"],
            Some(aida_core::ExecutionMode::Drain),
        );
        let reason = keyboard_only_dispatch_refusal(&storage, "TASK-1274");
        assert!(reason.is_some());
        assert!(reason.unwrap().contains("keyboard-only"));
    }

    #[test]
    fn dispatch_refuses_guided_mode() {
        let (_dir, storage) = storage_with("TASK-2", &[], Some(aida_core::ExecutionMode::Guided));
        assert!(keyboard_only_dispatch_refusal(&storage, "TASK-2").is_some());
    }

    #[test]
    fn dispatch_allows_plain_drain_spec() {
        let (_dir, storage) = storage_with("TASK-3", &[], Some(aida_core::ExecutionMode::Drain));
        assert!(keyboard_only_dispatch_refusal(&storage, "TASK-3").is_none());
    }

    // ── AC2, end-to-end through the real dispatch fn (not just the helper) ──

    #[test]
    fn ac2_run_auto_complete_refuses_keyboard_only_tag_under_no_human() {
        // The tag-under-drain-mode shape TASK-1274 actually had — must be
        // refused at dispatch under `--no-human`, before any queueing/I-O
        // past the guard.
        let (_dir, storage) = storage_with(
            "TASK-1274",
            &["keyboard-only"],
            Some(aida_core::ExecutionMode::Drain),
        );
        let result = run_auto_complete(
            &storage,
            "test-user",
            "TASK-1274",
            auto_complete::AutoCompleteVariant::Full,
            false,
            None,
            Some(auto_complete::NoHumanMode::Both),
            auto_complete::EscalateMode::Blocks,
            true,
            false,
            false,
            false,
            false,
            None,
        );
        assert_eq!(result.failed_phase, Some(auto_complete::Phase::Implementer));
    }

    // ── the wrapper's two carve-out cases ────────────────────────────────

    #[test]
    fn wrapper_allows_when_no_config_toml_exists_upward() {
        // A bare fixture with no `.aida/` anywhere — not an AIDA-managed
        // project root at all, so there is nothing to scope/tag-check
        // against. The ONLY fail-open case.
        let dir = tempfile::tempdir().unwrap();
        assert!(unattended_git_mutation_refusal(dir.path(), "TASK-1274", "test", None).is_none());
    }

    /// BUG-1598: `config_toml_exists_upward` must agree with
    /// `detect_distributed_store_from` on where the walk-up stops — a stray
    /// `.aida/config.toml` sitting directly in a temp root must NOT make
    /// this function report `true` (which would push
    /// `unattended_git_mutation_refusal` past its only fail-open carve-out
    /// and refuse a mutation for a project that has no real AIDA config at
    /// all). A FAKE temp root (a plain tempdir, injected — never `TMPDIR`,
    /// never the real shared `/tmp`) holds a planted `.aida/config.toml`
    /// directly at its own root; a fixture one level under it must not see
    /// it.
    // trace:BUG-1598 | ai:claude
    #[test]
    fn config_toml_exists_upward_never_adopts_a_temp_root() {
        let fake_temp_root = tempfile::tempdir().unwrap();
        let roots = vec![fake_temp_root.path().to_path_buf()];

        std::fs::create_dir_all(fake_temp_root.path().join(".aida")).unwrap();
        std::fs::write(
            fake_temp_root.path().join(".aida").join("config.toml"),
            "store_path = \".aida-store\"\n",
        )
        .unwrap();

        let nested = fake_temp_root.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();

        assert!(
            !config_toml_exists_upward_with_roots(&nested, &roots),
            "must never report a temp root's ambient .aida/config.toml as an AIDA project root"
        );

        // Sanity check: WITHOUT the guard (empty roots list), the same
        // fixture DOES report true — proving the guard, not some other
        // difference, is what suppresses the false positive above.
        assert!(
            config_toml_exists_upward_with_roots(&nested, &[]),
            "fixture must be adoptable when nothing is guarded, or this test proves nothing"
        );
    }

    #[test]
    fn wrapper_refuses_when_config_declares_an_unresolvable_store() {
        // A REAL AIDA project root (config.toml present) whose declared
        // store can't be resolved (deleted, corrupted, wrong path) must
        // fail closed — never silently read as "nothing to check".
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
        std::fs::write(
            dir.path().join(".aida").join("config.toml"),
            "store_path = \"nonexistent-store\"\n",
        )
        .unwrap();
        let reason = unattended_git_mutation_refusal(dir.path(), "TASK-1274", "test", None);
        assert!(
            reason.is_some(),
            "a declared-but-unresolvable store must refuse"
        );
        assert!(reason.unwrap().contains("fail closed"));
    }

    #[test]
    fn wrapper_allows_synthetic_review_scopes_with_an_unresolvable_store() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
        std::fs::write(
            dir.path().join(".aida").join("config.toml"),
            "store_path = \"nonexistent-store\"\n",
        )
        .unwrap();

        for scope in ["PR-26", "MR-26"] {
            assert!(
                unattended_git_mutation_refusal(dir.path(), scope, "steal", None).is_none(),
                "synthetic review scope {scope} must not depend on store lookup"
            );
        }
    }

    // ── dedupe: a second refusal for the same (spec, branch) is a no-op ────

    #[test]
    fn dedupe_second_refusal_sees_the_first_findings_open_dupe() {
        let mut store = aida_core::RequirementsStore::new();
        // Nothing filed yet — the first refusal would file.
        assert!(!has_open_refusal_finding(
            &store,
            "TASK-1274",
            Some("task-1274-work")
        ));

        // Simulate the FIRST refusal's finding landing in the store.
        let mut finding = aida_core::Requirement::new(
            "Headless rebase/force-push refused for TASK-1274 (branch task-1274-work)".to_string(),
            "reason".to_string(),
        );
        finding.status = aida_core::RequirementStatus::Draft;
        finding
            .tags
            .insert("from-implementer:TASK-1274".to_string());
        finding.tags.insert("kind:headless-refusal".to_string());
        finding.tags.insert("branch:task-1274-work".to_string());
        store.requirements.push(finding);

        // A SECOND refusal for the same (spec, branch) must see it and
        // skip — never file a duplicate.
        assert!(has_open_refusal_finding(
            &store,
            "TASK-1274",
            Some("task-1274-work")
        ));
    }

    #[test]
    fn dedupe_ignores_closed_findings_and_other_branches() {
        let mut store = aida_core::RequirementsStore::new();
        let mut closed = aida_core::Requirement::new("t".to_string(), "d".to_string());
        closed.status = aida_core::RequirementStatus::Completed;
        closed.tags.insert("from-implementer:TASK-1274".to_string());
        closed.tags.insert("kind:headless-refusal".to_string());
        closed.tags.insert("branch:task-1274-work".to_string());
        store.requirements.push(closed);
        assert!(
            !has_open_refusal_finding(&store, "TASK-1274", Some("task-1274-work")),
            "a Completed/Rejected finding must not suppress a new refusal"
        );

        let mut other_branch = aida_core::Requirement::new("t".to_string(), "d".to_string());
        other_branch.status = aida_core::RequirementStatus::Draft;
        other_branch
            .tags
            .insert("from-implementer:TASK-1274".to_string());
        other_branch
            .tags
            .insert("kind:headless-refusal".to_string());
        other_branch.tags.insert("branch:other-branch".to_string());
        store.requirements.push(other_branch);
        assert!(
            !has_open_refusal_finding(&store, "TASK-1274", Some("task-1274-work")),
            "a finding for a DIFFERENT branch must not suppress this one"
        );
    }
}

/// STORY-265 slice 3: execute the `--with-plan` PLAN PRELUDE for one spec —
/// the plan phase that runs before the auto-complete drain's phase 1. Reuses
/// slice 2's `aida queue work <spec> --plan-only` planning session (headless
/// when the drain runs a headless implementer) and slice 1's
/// `aida plan promote <spec>` Approved→Planned transition, in that order
/// (decided by [`auto_complete::plan_prelude_steps`]). The drain itself is
/// entered unchanged after this returns `Ok`; the Phase enum is untouched (the
/// prelude is NOT a renumbered phase). Each step shells out to the current
/// `aida` binary; a non-zero exit aborts the prelude so the caller skips the
/// drain. trace:STORY-265 | ai:claude
pub(crate) fn run_plan_prelude(spec: &str, headless_implementer: bool, json: bool) -> Result<()> {
    use auto_complete::PlanPreludeStep;
    let exe = aida_exe_path();
    let steps = auto_complete::plan_prelude_steps(spec, true, headless_implementer);
    if !json {
        eprintln!();
        eprintln!(
            "{} {} {}",
            "📝".bold(),
            format!("plan prelude: {spec}").bold(),
            "(plan phase → promote → drain)".dimmed()
        );
    }
    for step in steps {
        match step {
            PlanPreludeStep::PlanSession { spec: s, headless } => {
                let mut args: Vec<String> = vec![
                    "queue".into(),
                    "work".into(),
                    s.clone(),
                    "--plan-only".into(),
                ];
                if headless {
                    // Mirror the phase-1 headless implementer launch onto the
                    // plan session so a `--no-human=both` drain plans headless
                    // too. trace:STORY-265 | ai:claude
                    args.push("--no-human".into());
                }
                let status = std::process::Command::new(&exe)
                    .args(&args)
                    .status_retrying_etxtbsy()
                    .with_context(|| format!("could not launch the plan session for {s}"))?;
                if !status.success() {
                    anyhow::bail!(
                        "the plan session exited with status {} — no plan file was produced",
                        status.code().unwrap_or(-1)
                    );
                }
            }
            PlanPreludeStep::Promote { spec: s } => {
                let status = std::process::Command::new(&exe)
                    .args(["plan", "promote", &s])
                    .status_retrying_etxtbsy()
                    .with_context(|| format!("could not run `aida plan promote {s}`"))?;
                if !status.success() {
                    anyhow::bail!(
                        "`aida plan promote {s}` exited with status {} — the spec was not \
                         promoted to Planned (does the plan file's `Specs:` header list {s}?)",
                        status.code().unwrap_or(-1)
                    );
                }
            }
        }
    }
    Ok(())
}

/// Run one `--auto-complete` orchestration and return its result *without*
/// exiting the process — so a batch drain (TASK-285) can chain runs. Handles
/// the preflight queue, the real driver, and the TASK-266 telemetry record.
/// Hard environment errors (project root unresolvable, preflight queue-add
/// failed) still exit the process directly — they would recur identically on
/// every batch iteration. trace:STORY-246, TASK-285 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_auto_complete(
    storage: &Storage,
    user_id: &str,
    spec: &str,
    variant: auto_complete::AutoCompleteVariant,
    json: bool,
    permission_mode: Option<&str>,
    no_human: Option<auto_complete::NoHumanMode>,
    // STORY-306: how an advisor escalation is handled — `Blocks` (default) or
    // `Defaults`. Only meaningful under `--no-human=both`.
    escalate_mode: auto_complete::EscalateMode,
    // STORY-301: `true` for a standalone single-spec drain — this run creates
    // the `.aida/drain-state.json` file and clears it on return. `false` for a
    // batch / nextN member — the batch orchestrator owns the file; this run
    // only announces it is the current spec.
    owns_drain_state: bool,
    // BUG-311: pass-through of the outer `--steal` flag — appended to the
    // phase-1 `aida queue work` subprocess so it can clear a dormant lease
    // on this scope. Without this thread-through the outer flag silently
    // dropped on the floor and the canned "pass --steal" message recurred.
    // trace:BUG-311 | ai:claude
    steal: bool,
    // TASK-559: pass-through of the outer `--force-claim` flag, appended to
    // phase-1 `aida queue work` subprocesses just like `--steal`.
    force_claim: bool,
    // STORY-281: pass-through of the outer `--allow-stale-base` flag. When
    // true, phase 3's pre-flight check warns-only instead of refusing on a
    // stale-base + file-overlap; also propagated to the reviewer subprocess.
    // trace:STORY-281 | ai:claude
    allow_stale_base: bool,
    // STORY-429: pass-through of the outer `--no-auto-rebase` flag. When
    // false, fully-headless phase 3 can attempt one clean PR rebase before
    // falling back to STORY-281 refusal.
    no_auto_rebase: bool,
    // STORY-492: `None` for a normal drain (start at phase 1, run everything).
    // `Some` for a `--resume-drain` re-entry — carries the reconciled
    // `start_phase` (skip earlier phases) plus the branch + PR to seed into the
    // driver so the resumed phases have the context the skipped phases would
    // have discovered. trace:STORY-492 | ai:claude
    resume: Option<ResumeEntry>,
) -> auto_complete::OrchestrationResult {
    // BUG-657: a spec that is already terminal (Completed / Rejected) is a clean
    // NO-OP — return BEFORE touching the queue or spawning anything. Driving it
    // would queue it, spawn an implementer that exits 1 ("nothing to implement"),
    // and auto-draft a phantom failure BUG — the BUG-638 → BUG-644..649 incident
    // (6 identical drafts in 6 minutes). A `--resume` / `--from-pr` re-entry
    // (`resume.is_some()`) is exempt: it legitimately re-drives a spec that
    // reached Completed mid-pipeline (the merge promoted it; the BUG-241 reconcile
    // treats that as an out-of-band success). The pure orchestrator carries the
    // same guard via `PhaseDriver::terminal_status` for defense-in-depth + unit
    // testing; this earlier check is what keeps the queue side-effect-free.
    // trace:BUG-657 | ai:claude
    if resume.is_none() {
        if let Some(mode) = guarded_execution_mode_for_drain(storage, spec) {
            eprintln!("{}", guarded_execution_mode_drain_message(spec, mode));
            return auto_complete::OrchestrationResult::failed(auto_complete::Phase::Implementer);
        }
        // BUG-1574 AC2: a spec tagged keyboard-only must be refused at
        // dispatch under a headless (`--no-human`) drain even when it
        // groomed to `execution_mode: drain` — the tag above wins. This is
        // in addition to the execution_mode-only check above, which already
        // covers Guided/Operator/Decide unconditionally; this one covers the
        // tag specifically for the headless path. trace:BUG-1574 | ai:claude
        if no_human.is_some() {
            if let Some(reason) = keyboard_only_dispatch_refusal(storage, spec) {
                eprintln!("{reason}");
                return auto_complete::OrchestrationResult::failed(
                    auto_complete::Phase::Implementer,
                );
            }
        }
        if let Ok(root) = find_main_worktree_root() {
            let terminal = match spec_status(&root, spec) {
                Some(RequirementStatus::Completed) => Some("Completed"),
                Some(RequirementStatus::Rejected) => Some("Rejected"),
                _ => None,
            };
            if let Some(status) = terminal {
                return auto_complete::finish_noop(spec, status, json, &std::time::Instant::now());
            }
        }
    }
    // Preflight: `aida queue work <spec>` can only pick up a spec that's
    // queued for the implementer. Queue it if it isn't — so a fresh
    // `aida add` flows straight into `--auto-complete`.
    if let Err(e) = ensure_queued_for_implementer(storage, user_id, spec) {
        eprintln!(
            "{} {}",
            crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
            e
        );
        std::process::exit(1);
    }
    // STORY-265 slice 3: the `--with-plan` PLAN PRELUDE. When `--with-plan` is
    // set (propagated as `AIDA_WITH_PLAN=1` by the dispatch arm), run a plan
    // session + Approved→Planned promote BEFORE the phase-1 status bump and the
    // drain. This is a prelude, NOT a renumbered phase — the 6-phase drain
    // below is entered unchanged, so the Phase enum's index-as-exit-code
    // contract is preserved. The prelude runs once per drained spec (single /
    // batch / nextN all route through here). A prelude failure aborts this
    // spec's run before any work is bumped/leased. trace:STORY-265 | ai:claude
    if std::env::var("AIDA_WITH_PLAN").is_ok() {
        let headless_implementer = no_human
            .map(auto_complete::NoHumanMode::wants_headless_implementer)
            .unwrap_or(false);
        if let Err(e) = run_plan_prelude(spec, headless_implementer, json) {
            eprintln!(
                "{} plan prelude failed for {}: {} — the drain did not start \
                 (re-run, or drop --with-plan to implement without a plan phase)",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                spec,
                e
            );
            return auto_complete::OrchestrationResult::failed(auto_complete::Phase::Implementer);
        }
    }
    let lifecycle_skip = match resolve_lifecycle_skip(storage, spec) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "{} {}",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                e
            );
            std::process::exit(1);
        }
    };

    let project_root = match find_main_worktree_root() {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "{} could not resolve the project root: {}",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                e
            );
            std::process::exit(1);
        }
    };

    // STORY-263 / TASK-306 / STORY-276: a one-line per-run reminder of the
    // headless scope. The loud scope banner + acknowledgement already fired
    // once at kickoff (`no_human_kickoff_gate`); this keeps the scope visible
    // in a batch drain's per-member scrollback. The wording follows the mode:
    // `both` runs phase 1 headless (with the punt safety net), `reviewer-only`
    // leaves it interactive. trace:STORY-263, TASK-306, STORY-276 | ai:claude
    if let Some(mode) = no_human {
        if !json {
            if mode.wants_headless_implementer() {
                eprintln!(
                    "  {} headless implementer + reviewer — phase 1 punts to \
                     Needs Attention on a design-fork it cannot resolve",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
                );
            } else {
                eprintln!(
                    "  {} headless reviewer phase — phase 1 (implementer) stays \
                     interactive and will pause for you",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
                );
            }
        }
    }

    // BUG-233 / TASK-336: mint this orchestrator run's corroboration UUID and
    // persist it into the drain-state file. A phase child carrying
    // `AIDA_AUTO_COMPLETE_TOKEN=<uuid>` corroborates orchestrator-mode against
    // `DrainState::run_uuid` + a `pid_is_alive` check on the recorded
    // orchestrator PID. Before TASK-336 this lived in a sidecar
    // `.aida/orchestrator-runs/<uuid>` marker file; that file has been removed
    // since the drain-state file already records every other field the check
    // needs. trace:BUG-233 trace:TASK-336 | ai:claude
    //
    // ADR-7/ADR-10: resolve this run's autonomy ONCE into a typed value (the
    // single in-process source of truth) rather than re-reading `AIDA_ZEN` as a
    // bare env bool. `for_auto_complete_run` unifies the `--no-human` mode and
    // the `--zen` intent token; it preserves the BUG-237 guarantee that a
    // leaked `AIDA_ZEN=1` (no token) is not recorded as a zen run. The env var
    // stays as the cross-process transport to phase children / skill templates.
    // trace:ADR-7 trace:ADR-10 trace:BUG-237 | ai:claude
    let autonomy = AutonomyMode::for_auto_complete_run(no_human);
    let run_token = uuid::Uuid::now_v7().to_string();

    // ADR-10: build the driver FIRST, carrying the resolved-once typed
    // `autonomy` alongside `no_human`, so the drain-state stamping below reads
    // the carried field (`driver.is_zen_run()`) rather than re-deriving zen-ness
    // from a bare `AIDA_ZEN` env read. The env var stays as the cross-process
    // transport to phase children / skill templates. trace:ADR-10 | ai:claude
    let mut driver = RealPhaseDriver::new(
        project_root.clone(),
        spec.to_string(),
        user_id.to_string(),
        permission_mode.map(|s| s.to_string()),
        json,
        no_human,
        autonomy,
        run_token.clone(),
        steal,
        force_claim,
        allow_stale_base,
        no_auto_rebase,
        lifecycle_skip,
        variant,
    );

    // trace:TASK-1603 | ai:codex
    // Publish ownership before changing status or launching any phase child.
    // This is authority state, not optional telemetry: a missing/failed write
    // would make the first child apply standalone lease rules to our own bump.
    let owns_drain_state =
        owns_drain_state && std::env::var_os("AIDA_PIPELINED_BATCH_CHILD").is_none();
    // Retain the prior status for TASK-133's lease-less failure recovery.
    let phase1_bump = match prepare_registered_auto_complete_phase1(
        storage,
        &project_root,
        spec,
        &run_token,
        driver.is_zen_run(),
        owns_drain_state,
    ) {
        Ok(bump) => bump,
        Err(e) => {
            eprintln!("could not prepare orchestrator ownership/status for {spec}: {e} — no phase launched");
            return auto_complete::OrchestrationResult::failed(auto_complete::Phase::Implementer);
        }
    };
    // STORY-492: a resume re-entry seeds the driver with the branch + PR the
    // skipped phases would otherwise have discovered, so the resumed phases
    // (CI / reviewer / merge / …) have the context they need.
    let start_phase = match &resume {
        Some(r) => {
            driver.seed_resume_state(
                r.branch.clone(),
                r.pr,
                r.head_sha.clone(),
                r.ci_terminal_sha.clone(),
                r.ci_terminal_green,
            );
            driver.from_pr = r.from_pr;
            r.start_phase
        }
        None => auto_complete::Phase::Implementer,
    };
    let started_at = chrono::Utc::now();
    // TASK-136: a batch / nextN member (`owns_drain_state == false`) shelves a
    // still-inconclusive phase-1 verify so the drain advances; a standalone
    // single-spec drain keeps the Inconclusive pause. The two are exact
    // inverses, so `batch` derives directly from `owns_drain_state`.
    let batch = !owns_drain_state;
    // TASK-827: solo mode as a max-discretion safe-backlog POSTURE. When solo is
    // active (`presence::current_solo`), fold it into this spec's escalate
    // behaviour per-spec: SAFE work proceeds on the defensible default
    // (ProceedOnDefault → Defaults — maximum discretion), KEYSTONE/architecture
    // work parks for the human (ParkForHuman → Blocks — never ship keystone
    // unattended). Reuses the existing escalate mechanism + park path; when solo
    // is inactive the posture is `Inactive` and `escalate_mode` is UNCHANGED, so
    // non-solo behaviour is untouched. Only meaningful under `--no-human=both`
    // where the advisor escalation tier runs. trace:TASK-827 | ai:claude
    let escalate_mode = {
        // The advisor escalation tier (where `escalate_mode` is consulted) only
        // runs under `--no-human=both`; gate the posture there so a non-headless
        // solo drain neither prints a misleading banner nor flips a no-op flag.
        let advisor_tier_runs = no_human == Some(auto_complete::NoHumanMode::Both);
        let solo_active = advisor_tier_runs && presence::current_solo(chrono::Utc::now());
        let is_keystone = solo_active && solo_spec_is_keystone(storage, spec);
        let posture = presence::resolve_solo_posture(solo_active, is_keystone);
        if posture.is_active() {
            if !json {
                if matches!(posture, presence::SoloPosture::ParkForHuman) {
                    eprintln!(
                        "  {} solo posture: working safe backlog, parking keystone for human ({} classified keystone — parks on a design-fork)",
                        crate::glyph(crate::glyphs::Glyph::Robot).bold(),
                        spec
                    );
                } else {
                    eprintln!(
                        "  {} solo posture: working safe backlog, parking keystone for human ({} is safe — proceeds on the defensible default)",
                        crate::glyph(crate::glyphs::Glyph::Robot).bold(),
                        spec
                    );
                }
            }
            auto_complete::EscalateMode::from_flags(posture.escalate_defaults())
        } else {
            escalate_mode
        }
    };
    let result = auto_complete::orchestrate_with_resume(
        &mut driver,
        spec,
        variant,
        json,
        escalate_mode,
        lifecycle_skip,
        batch,
        start_phase,
    );
    let completed_at = chrono::Utc::now();

    // TASK-133: compensate the pre-spawn phase-1 status bump. If phase 1
    // failed *and* the implementer child never recorded a lease, no work
    // happened — restore the captured prior status (clearing the spurious
    // shelve `failure_reason`) so the spec is cleanly re-queueable rather than
    // stranded InProgress→NeedsAttention behind a transient spawn/contention
    // error. A lease-acquired or later-phase failure leaves real work to
    // triage and is left shelved. trace:TASK-133 | ai:claude
    if let Some((display_id, prior)) = &phase1_bump {
        let lease_acquired = driver.implementer_lease.is_some();
        if auto_complete::should_compensate_phase1_bump(true, lease_acquired, result.failed_phase) {
            // BUG-1134: a lease-less phase-1 failure usually means no work happened —
            // but it can be a worktree-reuse collision AFTER a prior attempt already
            // opened a PR. Restoring to un-started would orphan that PR and mislead
            // with "no work was stranded". Detect a definitive open PR first; an
            // absent/flaky forge falls back to the safe restore. trace:BUG-1134
            let pr_lookup = crate::forge::forge_for(&project_root)
                .change_for_spec(spec)
                .unwrap_or(crate::forge::ChangeLookup::NoChange);
            match auto_complete::classify_phase1_failure_recovery(&pr_lookup) {
                auto_complete::Phase1FailureRecovery::PreserveOpenPr => {
                    if let crate::forge::ChangeLookup::Found(pr) = &pr_lookup {
                        eprintln!(
                            "  {} phase-1 failed, but {} ALREADY HAS AN OPEN PR (#{}) — the \
                             work shipped (a worktree-reuse collision, not lost work). Not \
                             restoring to un-started; resume with `aida queue work {} --resume` \
                             or review it: {}",
                            "⚠".yellow(),
                            display_id,
                            pr.id,
                            display_id,
                            pr.url
                        );
                    }
                    // Leave the status as phase-1 left it — the open PR is real work;
                    // restoring to a clean un-started status would orphan it.
                }
                auto_complete::Phase1FailureRecovery::RestoreUnstarted => {
                    // BUG-1638: report the restore only when it happened.
                    // trace:BUG-1638 | ai:claude
                    match restore_phase1_status_on_lease_failure(&project_root, spec, prior) {
                        // BUG-1647: the refusal honours --json like the
                        // restore does (BUG-1651: pinned through
                        // `phase1_restore_report_line`).
                        // trace:BUG-1647 trace:BUG-1651 | ai:claude
                        Ok(outcome) => {
                            if let Some(line) =
                                phase1_restore_report_line(&outcome, display_id, prior, json)
                            {
                                eprintln!("{line}");
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "  {} could not restore {}'s status after a lease-less phase-1 \
                                 failure: {} — reset it manually with `aida edit {} --status {}`",
                                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                                display_id,
                                e,
                                display_id,
                                format!("{prior:?}").to_lowercase(),
                            );
                        }
                    }
                }
            }
        }
    }

    // trace:TASK-1603 | ai:codex
    // All this run's children have been reaped. Revoke its token only;
    // concurrent members must retain authority through their later phases.
    drain_state::clear_run(&project_root, &run_token);

    // TASK-266: log the run to `~/.aida/auto-complete.jsonl` and, on a phase
    // failure, auto-draft a Draft BUG so the friction surfaces back to the
    // project instead of dying in scrollback. Best-effort — never blocks the
    // exit. trace:TASK-266 | ai:claude
    // STORY-301: stamp this member's terminal state — completed / failed plus
    // the PR a phase discovered — into the drain-state file. For a batch drain
    // this leaves a member-by-member trail; for a single drain the file is
    // cleared just below, so this update only matters mid-run.
    drain_state::set_member_outcome(
        &project_root,
        spec,
        result.exit_code == 0,
        auto_complete::PhaseDriver::hint_context(&driver).pr_number,
    );
    write_pipelined_child_result_sidecar(&result);

    record_auto_complete_run(
        &driver,
        &project_root,
        spec,
        variant,
        &result,
        started_at,
        completed_at,
        json,
        lifecycle_skip,
    );

    // STORY-301: a single-spec drain owns the file — clear it now the run is
    // over. A clean exit removes the file; only a crash leaves it behind for
    // `aida drain status` to flag as a stale drain. trace:STORY-301
    //
    // BUG-438: but a *resume* that FAILED at a phase must KEEP the drain-state,
    // or the operator can't `--resume-drain` again after fixing the blocker —
    // the checkpoint would be consumed on failure. A resume that succeeded
    // clears it (the work is done); a non-resume drain clears as before.
    // trace:BUG-438 | ai:claude
    if owns_drain_state {
        if should_clear_drain_state(owns_drain_state, resume.is_some(), result.exit_code) {
            let _ = drain_state::DrainState::clear(&project_root);
        } else if !json {
            eprintln!(
                "  {} keeping drain-state — fix the blocker, then re-run \
                 `aida queue work --auto-complete --resume-drain`",
                crate::glyph(crate::glyphs::Glyph::Info).cyan()
            );
        }
    }

    // trace:TASK-1184 | ai:codex
    let _ = agent_registry::gc_dead_agents(&project_root, false, None);

    result
}

// Child-process batch pipelining preserves the existing per-spec engine while
// passing the in-process outcome back to the parent scheduler.
// trace:STORY-1091 trace:ADR-28 | ai:codex
pub(crate) fn write_pipelined_child_result_sidecar(result: &auto_complete::OrchestrationResult) {
    let Some(path) = std::env::var_os("AIDA_PIPELINED_RESULT_PATH") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let escalation = result.escalation.as_ref().map(|e| {
        serde_json::json!({
            "kind": match e.kind {
                auto_complete::EscalationKind::MergeDecision => "merge-decision",
                auto_complete::EscalationKind::DesignFork => "design-fork",
                auto_complete::EscalationKind::SupervisedMerge => "supervised-merge",
            },
            "reason": e.reason,
        })
    });
    let value = serde_json::json!({
        "exit_code": result.exit_code,
        "failed_phase": result.failed_phase.map(|p| p.index()),
        "punt_reason": result.punt_reason,
        "shipped_spec_id": result.shipped_spec_id,
        "escalation": escalation,
        "inconclusive_reason": result.inconclusive_reason,
        "shelved": result.shelved_reason.is_some(),
        "held_reason": result.held_reason,
    });
    if let Ok(bytes) = serde_json::to_vec_pretty(&value) {
        let _ = std::fs::write(path, bytes);
    }
}

// trace:STORY-1091 trace:ADR-28 | ai:codex
pub(crate) fn read_pipelined_child_result_sidecar(
    path: &std::path::Path,
) -> Option<auto_complete::OrchestrationResult> {
    let bytes = std::fs::read(path).ok()?;
    let _ = std::fs::remove_file(path);
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let exit_code = value
        .get("exit_code")
        .and_then(|v| v.as_i64())
        .and_then(|v| i32::try_from(v).ok())?;
    let failed_phase = value
        .get("failed_phase")
        .and_then(|v| v.as_i64())
        .and_then(|v| i32::try_from(v).ok())
        .and_then(auto_complete::Phase::from_index);
    let escalation = value.get("escalation").and_then(|v| {
        let kind = match v.get("kind").and_then(|k| k.as_str())? {
            "merge-decision" => auto_complete::EscalationKind::MergeDecision,
            "design-fork" => auto_complete::EscalationKind::DesignFork,
            "supervised-merge" => auto_complete::EscalationKind::SupervisedMerge,
            _ => return None,
        };
        Some(auto_complete::EscalationSummary {
            kind,
            reason: v
                .get("reason")
                .and_then(|r| r.as_str())
                .unwrap_or("pipelined child escalated")
                .to_string(),
        })
    });
    let shelved_reason = value
        .get("shelved")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        .then(|| aida_core::FailureReason {
            phase: failed_phase
                .unwrap_or(auto_complete::Phase::Ci)
                .slug()
                .to_string(),
            phase_index: failed_phase
                .unwrap_or(auto_complete::Phase::Ci)
                .index()
                .try_into()
                .unwrap_or(2),
            kind: "pipelined-child-shelved".to_string(),
            detail: "pipelined child parked this spec".to_string(),
            recovery_hint: Some(
                "inspect the child drain output and `aida findings list`".to_string(),
            ),
            shelved_by: Some("orchestrator".to_string()),
            shelved_at: chrono::Utc::now(),
        });
    Some(auto_complete::OrchestrationResult {
        exit_code,
        failed_phase,
        failure: None,
        phase_durations: Vec::new(),
        total_ms: 0,
        punt_reason: value
            .get("punt_reason")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        shipped_spec_id: value
            .get("shipped_spec_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        escalation,
        inconclusive_reason: value
            .get("inconclusive_reason")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        shelved_reason,
        held_reason: value
            .get("held_reason")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    })
}

/// BUG-438: should a finished single-spec drain clear its `drain-state.json`?
/// Clear when this run owns the file AND it is not a *failed resume* — a resume
/// that failed keeps the checkpoint so the operator can fix the blocker and
/// `--resume-drain` again, rather than the checkpoint being consumed on
/// failure. A successful resume (work done) and any non-resume drain clear as
/// before. Pure so the rule is unit-pinned. trace:BUG-438 | ai:claude
pub(crate) fn should_clear_drain_state(
    owns_drain_state: bool,
    is_resume: bool,
    exit_code: i32,
) -> bool {
    owns_drain_state && !(is_resume && exit_code != 0)
}

#[cfg(test)]
#[path = "tests/drain_state_clear_tests.rs"]
mod drain_state_clear_tests;

/// Resolve the queued members of a `batch:NAME` tag in pickup order, filtered
/// to the active role. Terminal items (Completed/Rejected) are dropped — they
/// are already shipped. Returns `(entry, display_id, title, status)` tuples.
/// Shared by the plain `--batch` head-pickup path and the TASK-285
/// `--batch --auto-complete` drain. trace:TASK-229, TASK-285 | ai:claude
pub(crate) fn resolve_batch_members(
    storage: &Storage,
    user_id: &str,
    batch_name: &str,
    role: Option<&str>,
) -> Result<
    Vec<(
        aida_core::QueueEntry,
        String,
        String,
        aida_core::RequirementStatus,
    )>,
> {
    let project_root = find_project_root().ok();
    resolve_batch_members_with_context(
        storage,
        user_id,
        batch_name,
        role,
        project_root.as_deref(),
        &mut std::io::stderr(),
    )
}

/// Return queued members carrying `batch:NAME` that the autonomous drain will
/// not pick up, preserving the same reason label shown by queue diagnostics.
// trace:BUG-1422 | ai:codex
pub(crate) fn resolve_batch_ineligible_members(
    storage: &Storage,
    user_id: &str,
    batch_name: &str,
    role: Option<&str>,
) -> Result<Vec<events::IneligibleBatchMember>> {
    let store = storage.load()?;
    let want = format!("batch:{batch_name}");
    let session_role = std::env::var("AIDA_SESSION_ROLE").ok();
    let (role_filter, _only_unrouted) =
        resolve_queue_role_filter(role, false, session_role.as_deref());
    let entries = queue_role_fallback::queue_list_with_role_fallback(
        storage,
        user_id,
        role_filter.as_deref(),
        false,
    )?;
    let mut ineligible = Vec::new();
    for entry in entries {
        if !entry_matches_role_filter(entry.for_role.as_deref(), role_filter.as_deref(), false) {
            continue;
        }
        let Some(req) = storage.resolve_queued_requirement(&entry.requirement_id)? else {
            continue;
        };
        if !req.tags.iter().any(|tag| tag.eq_ignore_ascii_case(&want)) {
            continue;
        }
        if let Some(reason) = queue_cmd::queue_fresh_pickup_reason_label(
            &queue_cmd::queue_drain_pickup_policy(&req, &store, false, storage.path().parent()),
        ) {
            ineligible.push(events::IneligibleBatchMember {
                spec: req.display_id(),
                reason,
            });
        }
    }
    Ok(ineligible)
}

/// TASK-1456 (follow-up to BUG-1515): the `next N` / bare `--auto-complete`
/// drain's candidate scan (`auto_complete_head_candidates`) only asks "is
/// this Approved/Planned" — `Done` was never a drivable head status, refused
/// or not, so it is silently absent from that filter rather than reported.
/// A queue that is ENTIRELY Done specs carrying a still-live review refusal
/// therefore hit the generic "no drivable items in the queue — nothing to
/// drive" idle message with the refusals never named — the exact "relaunches
/// every few minutes and does nothing" symptom BUG-1515 measured, just
/// outside the `--batch` path BUG-1422 already covers via
/// `resolve_batch_ineligible_members`.
///
/// Scans the SAME role-routed queue `auto_complete_head_candidates_with_roles`
/// reads for this drain and returns the display_id + recovery hint
/// (`queue_fresh_pickup_reason_label`) for every member whose
/// `queue_drain_pickup_policy` resolves to `AwaitingRework` specifically —
/// PRIN-5: this reports ONLY the verdicts it can positively confirm are
/// outstanding-and-at-the-tip (`done_spec_outstanding_refusal`'s own
/// contract), never a guess about a Done spec whose verdict state it
/// couldn't read.
// trace:TASK-1456 | ai:claude
/// TASK-1456: the human-readable idle-drain message naming every all-refused
/// candidate and its recovery route — a pure formatter (no `eprintln!`
/// inside it) so the exact wording is assertable directly, without spinning
/// up a drain.
// trace:TASK-1456 | ai:claude
pub(crate) fn next_n_rework_idle_message(
    rework_needed: &[events::IneligibleBatchMember],
) -> String {
    let rendered = rework_needed
        .iter()
        .map(|member| format!("{} ({})", member.spec, member.reason))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "queue has {} item{} but none are eligible — every candidate needs \
         rework, not a fresh drive: {}",
        rework_needed.len(),
        if rework_needed.len() == 1 { "" } else { "s" },
        rendered,
    )
}

pub(crate) fn resolve_queue_rework_needed(
    storage: &Storage,
    user_id: &str,
    role_override: Option<&str>,
) -> Result<Vec<events::IneligibleBatchMember>> {
    let store = storage.load()?;
    let effective_role = queue_cmd::effective_auto_complete_role(role_override);
    let role_filter = Some(effective_role);
    let entries = queue_role_fallback::queue_list_with_role_fallback(
        storage,
        user_id,
        role_filter.as_deref(),
        false,
    )?;
    let mut rework = Vec::new();
    for entry in entries {
        if !entry_matches_role_filter(entry.for_role.as_deref(), role_filter.as_deref(), false) {
            continue;
        }
        let Some(req) = storage.resolve_queued_requirement(&entry.requirement_id)? else {
            continue;
        };
        let policy =
            queue_cmd::queue_drain_pickup_policy(&req, &store, false, storage.path().parent());
        if !matches!(policy, queue_cmd::QueueFreshPickup::AwaitingRework) {
            continue;
        }
        if let Some(reason) = queue_cmd::queue_fresh_pickup_reason_label(&policy) {
            rework.push(events::IneligibleBatchMember {
                spec: req.display_id(),
                reason,
            });
        }
    }
    Ok(rework)
}

/// Injectable shell around batch resolution so the missing-object diagnostic
/// and event contract can be regression-tested without changing process cwd.
// trace:BUG-1264 | ai:codex
pub(crate) fn resolve_batch_members_with_context(
    storage: &Storage,
    user_id: &str,
    batch_name: &str,
    role: Option<&str>,
    project_root: Option<&std::path::Path>,
    diagnostic: &mut dyn std::io::Write,
) -> Result<
    Vec<(
        aida_core::QueueEntry,
        String,
        String,
        aida_core::RequirementStatus,
    )>,
> {
    let store = storage.load()?;
    let want = format!("batch:{}", batch_name);
    let session_role = std::env::var("AIDA_SESSION_ROLE").ok();
    let (role_filter, _only_unrouted) =
        resolve_queue_role_filter(role, false, session_role.as_deref());
    // Batch drains must see the same cross-user role-routed queue as ordinary
    // pickup. Otherwise an item filed in a person's queue for `implementer`
    // disappears when the drain identity is the synthetic role user.
    // trace:BUG-1264 | ai:codex
    let entries = queue_role_fallback::queue_list_with_role_fallback(
        storage,
        user_id,
        role_filter.as_deref(),
        false,
    )?;
    let mut members: Vec<(
        aida_core::QueueEntry,
        String,
        String,
        aida_core::RequirementStatus,
    )> = Vec::new();
    for entry in entries {
        if !entry_matches_role_filter(entry.for_role.as_deref(), role_filter.as_deref(), false) {
            continue;
        }
        // Resolve each queue UUID through the targeted backend path instead of
        // assuming the full-store projection contains an object at a path
        // derived from its current display id. Merge-gate aliases can leave the
        // authoritative YAML under the node-qualified/origin id.
        // trace:BUG-1264 | ai:codex
        let Some(req) = storage.resolve_queued_requirement(&entry.requirement_id)? else {
            let member = entry.requirement_id.to_string();
            let reason = "requirement id does not resolve to a stored object";
            let _ = writeln!(
                diagnostic,
                "  {} batch:{} — skipping unresolvable member {}: {}",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                batch_name,
                member,
                reason,
            );
            if let Some(root) = project_root {
                let (_, run_uuid) = drain_state::current_context(root);
                events::emit(
                    root,
                    &events::Event::new(
                        Some(member),
                        run_uuid,
                        events::EventKind::SpecSkipped {
                            reason: reason.to_string(),
                        },
                    ),
                );
            }
            continue;
        };
        if !req.tags.iter().any(|t| t.eq_ignore_ascii_case(&want)) {
            continue;
        }
        // BUG-1017: batch pickup shares the same fresh-pickup policy as
        // queue next, list, and explicit dry-runs. Done remains visible as
        // awaiting-merge work but is not re-drained from scratch. BUG-1120:
        // headless drains also skip guided/operator/decide members; those
        // modes require the keyboard path.
        if let Some(reason_label) = queue_cmd::queue_fresh_pickup_reason_label(
            &queue_cmd::queue_drain_pickup_policy(&req, &store, false, project_root),
        ) {
            eprintln!(
                "  {} batch:{} — skipping un-pickable member {} ({})",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                batch_name,
                req.display_id(),
                reason_label,
            );
            continue;
        }
        members.push((
            entry,
            req.display_id(),
            req.title.clone(),
            req.status.clone(),
        ));
    }
    Ok(members)
}
