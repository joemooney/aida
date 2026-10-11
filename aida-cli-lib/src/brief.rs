//! Agent brief create/list/read/ack and pending-brief bookkeeping (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

pub(crate) fn create_agent_brief(
    project_root: &std::path::Path,
    store: &RequirementsStore,
    agent: &str,
    spec: &str,
    note: Option<&str>,
    depends_on: Option<&str>,
    authorized_by: Option<&str>,
) -> Result<std::path::PathBuf> {
    let agent = validate_brief_agent(agent)?;
    // TASK-1468: a brief never routes work at a guessed spec.
    // trace:TASK-1468 | ai:claude
    let req = store
        .get_requirement_unambiguous(spec)?
        .ok_or_else(|| not_found::requirement_not_found(spec, None))?;
    let spec_id = req.spec_id.as_deref().unwrap_or(spec);
    let depends_on = normalize_brief_dependency(store, depends_on)?;
    ensure_brief_dependency_is_acyclic(project_root, agent, spec_id, depends_on.as_deref())?;
    let generated_at = chrono::Utc::now().format("%Y-%m-%dT%H%M%SZ").to_string();
    let brief_dir = project_root.join(".aida").join("agent-briefs").join(agent);
    std::fs::create_dir_all(&brief_dir)
        .with_context(|| format!("failed to create {}", brief_dir.display()))?;
    let path = brief_dir.join(format!("{spec_id}-{generated_at}.md"));
    let body = render_agent_brief(
        project_root,
        agent,
        spec_id,
        req,
        store,
        &generated_at,
        note,
        depends_on.as_deref(),
        authorized_by,
    );
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| {
            format!(
                "brief already exists or cannot be written: {}",
                path.display()
            )
        })?;
    std::io::Write::write_all(&mut file, body.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

/// STORY-569: does the agent's mailbox already hold a PENDING brief for this
/// spec? An ack renames the file to `*.md.acked`, so a pending brief is a
/// plain `.md` whose name starts with `<SPEC>-` (the [`create_agent_brief`]
/// naming scheme). Keeps the zen-finish handoff idempotent — the finish gate
/// can be consulted more than once per checkpoint without double-filing.
// trace:STORY-569 | ai:claude
pub(crate) fn pending_brief_exists(
    project_root: &std::path::Path,
    agent: &str,
    spec_id: &str,
) -> bool {
    let dir = project_root.join(".aida").join("agent-briefs").join(agent);
    let prefix = format!("{}-", spec_id.to_ascii_uppercase());
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        let name = e.file_name().to_string_lossy().to_string();
        name.to_ascii_uppercase().starts_with(&prefix) && name.ends_with(".md")
    })
}

/// STORY-569: the clean-finish build→review handoff. When a standalone
/// `--zen` session finishes with an open PR on its lease branch, file a
/// pickup brief to the advisor's mailbox (`.aida/agent-briefs/<agent>/`)
/// carrying the spec id, the PR number/url, and the pointer to review
/// against the spec's resolved-design block — so the reviewing session
/// learns about the PR through the substrate, never an operator relay.
///
/// Returns `Ok(None)` when there is nothing to do: the handoff is disabled
/// (`[zen] review_brief_agent = ""`), the lease has no open PR (or the
/// forge is unreachable — fail open, a notify must never block a finish),
/// or a pending brief for the spec already sits in the mailbox
/// (idempotent). Reuses the existing brief writer + `.pending` sentinel;
/// nothing reinvented. trace:STORY-569 | ai:claude
pub(crate) fn file_zen_review_brief(
    project_root: &std::path::Path,
    lease: &SessionLease,
) -> Result<Option<(String, std::path::PathBuf)>> {
    let Some(agent) = read_zen_review_brief_agent(project_root) else {
        return Ok(None);
    };
    let spec_id = lease.scope.trim().to_string();
    if spec_id.is_empty() {
        return Ok(None);
    }
    let crate::forge::ChangeLookup::Found(change) =
        change_lookup_for_branch(project_root, &lease.branch)
    else {
        return Ok(None);
    };
    if pending_brief_exists(project_root, &agent, &spec_id) {
        return Ok(None);
    }
    let store = load_store_for_lookup(project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "no requirement store reachable from {}",
            project_root.display()
        )
    })?;
    let note = format!(
        "PR #{} is ready for review: {}\n\
         Review the diff against {}'s acceptance criteria and resolved-design block \
         (`aida show {}`), then ack this brief.",
        change.id, change.url, spec_id, spec_id
    );
    let path = create_agent_brief(
        project_root,
        &store,
        &agent,
        &spec_id,
        Some(&note),
        None,
        None,
    )?;
    // Urgent-path sentinel so the advisor's `aida status` surfaces it
    // without waiting on a mailbox poll (the `--notify` mechanic).
    add_pending_brief(project_root, &agent, &path)?;
    Ok(Some((agent, path)))
}

pub(crate) fn validate_brief_agent(agent: &str) -> Result<&str> {
    let agent = agent.trim();
    if agent.is_empty() {
        anyhow::bail!("agent name cannot be empty");
    }
    if !agent
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        anyhow::bail!("agent name must contain only ASCII letters, digits, '.', '_' or '-'");
    }
    Ok(agent)
}

pub(crate) fn render_agent_brief(
    project_root: &std::path::Path,
    agent: &str,
    spec_id: &str,
    req: &Requirement,
    store: &RequirementsStore,
    generated_at: &str,
    note: Option<&str>,
    depends_on: Option<&str>,
    authorized_by: Option<&str>,
) -> String {
    let generated_by = brief_generated_by();
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("spec_id: {}\n", yaml_scalar(spec_id)));
    out.push_str(&format!("agent: {}\n", yaml_scalar(agent)));
    out.push_str(&format!("generated_at: {}\n", yaml_scalar(generated_at)));
    out.push_str(&format!("generated_by: {}\n", yaml_scalar(&generated_by)));
    if let Some(note) = note {
        out.push_str("note: |-\n");
        out.push_str(&indent_block(note, "  "));
    }
    if let Some(depends_on) = depends_on {
        out.push_str(&format!("depends_on: {}\n", yaml_scalar(depends_on)));
    }
    // STORY-711 slice 2: the authorizing advisor's id, carried into the
    // receiving agent's role-context snapshot at launch so the automatic
    // advisor-lock gate can read it at commit time. trace:TASK-1140 | ai:claude
    if let Some(authorized_by) = authorized_by.filter(|s| !s.trim().is_empty()) {
        out.push_str(&format!("authorized_by: {}\n", yaml_scalar(authorized_by)));
    }
    out.push_str("status: pending\n");
    out.push_str("---\n\n");

    out.push_str(&format!(
        "## Routing\n\nThis brief is for: {agent}. Generated {generated_at} from the AIDA spec graph.\n\n"
    ));
    out.push_str("## Optional preamble\n\n");
    if let Some(note) = note.filter(|n| !n.trim().is_empty()) {
        out.push_str(note);
        out.push_str("\n\n");
    } else {
        out.push_str("_No operator note provided._\n\n");
    }
    out.push_str("## Spec\n\n");
    out.push_str(&format!("- ID: {spec_id}\n"));
    out.push_str(&format!("- Title: {}\n", req.title));
    out.push_str(&format!("- Status: {}\n", req.status));
    out.push_str(&format!("- Type: {}\n", req.req_type));
    out.push_str(&format!("- Priority: {}\n", req.priority));
    out.push_str(&format!(
        "- Tags: {}\n\n",
        sorted_tags(&req.tags).unwrap_or_else(|| "none".to_string())
    ));
    out.push_str(&req.description);
    out.push_str("\n\n");

    out.push_str("## Composes with\n\n");
    let relations = brief_relationship_lines(req, store);
    if relations.is_empty() {
        out.push_str("_No direct relationships recorded._\n\n");
    } else {
        for line in relations {
            out.push_str("- ");
            out.push_str(&line);
            out.push('\n');
        }
        out.push('\n');
    }

    out.push_str("## Discipline\n\n");
    out.push_str("- Start with AGENTS.md for Codex-oriented project discipline.\n");
    out.push_str(&format!(
        "- Agent setup convention: docs/agents/{agent}-mcp-setup.md if present.\n\n"
    ));

    // BUG-583: the Setup block must reference the TARGET PROJECT's location
    // (resolved at runtime from the invocation's project root), never the AIDA
    // binary's compiled-in source-repo path. A cold vendor agent following these
    // steps literally must stay inside its own project, not `cd` into wherever
    // aida happened to be built. The sibling worktree is derived from the
    // project root's basename + its parent dir; both are anchored to the project
    // being briefed, not the binary. trace:BUG-583
    let project_name = project_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");
    let project_root_display = project_root.display();
    let worktree_path_for = |branch: &str| -> std::path::PathBuf {
        project_root
            .parent()
            .map(|parent| parent.join(format!("{project_name}-{branch}")))
            .unwrap_or_else(|| project_root.join(format!("../{project_name}-{branch}")))
    };

    // BUG-1525: never emit a fresh-start `git worktree add -b <new> origin/main`
    // when the spec already has a branch (local or `origin/<branch>`) carrying
    // its work — that starts a second lineage with none of the reviewed
    // commits, silently, because the fresh branch name doesn't collide with
    // anything. Resolve what's actually there first. trace:BUG-1525 | ai:claude
    let target = resolve_brief_branch_target(project_root, spec_id);
    out.push_str("## Setup\n\n");
    match &target {
        BriefBranchTarget::Ambiguous { candidates } => {
            out.push_str(&format!(
                "_Cannot determine this spec's branch automatically — {} branches reference {spec_id} ({}). \
                 Check `aida show {spec_id}` for the branch its open PR (if any) actually points at, then \
                 `cd` into that branch's existing worktree, or `git worktree add <path> <branch>` to attach \
                 one — do NOT create a new branch off `origin/main`._\n\n",
                candidates.len(),
                candidates.join(", ")
            ));
            out.push_str("```bash\n");
            out.push_str(&format!("cd {project_root_display}\n"));
            out.push_str("git fetch origin main\n");
            out.push_str("# resolve the branch above by hand before attaching a worktree\n");
            out.push_str("```\n\n");
        }
        BriefBranchTarget::ExistingWorktree { branch, worktree } => {
            let worktree_display = worktree.display();
            out.push_str(&format!(
                "This spec already has branch `{branch}` checked out in an existing worktree — \
                 continue it, don't start a second lineage.\n\n"
            ));
            out.push_str("```bash\n");
            out.push_str(&format!("cd {worktree_display}\n"));
            out.push_str("git fetch origin main\n");
            out.push_str(
                "# realign if diverged from main before continuing:\n\
                 #   git rebase origin/main   (or: git merge origin/main)\n",
            );
            out.push_str(&format!(
                "aida session start --owns {spec_id} --branch {branch} --path {worktree_display} --reuse-branch\n"
            ));
            out.push_str("```\n\n");
        }
        BriefBranchTarget::ExistingBranch {
            branch,
            remote_only,
        } => {
            let worktree_dir = worktree_path_for(branch);
            let worktree_display = worktree_dir.display();
            out.push_str(&format!(
                "This spec already has branch `{branch}` with commits ahead of the default branch — \
                 continue it, don't start a fresh one.\n\n"
            ));
            out.push_str("```bash\n");
            out.push_str(&format!("cd {project_root_display}\n"));
            out.push_str("git fetch origin main\n");
            if *remote_only {
                out.push_str(&format!(
                    "git worktree add {worktree_display} -B {branch} origin/{branch}\n"
                ));
            } else {
                out.push_str(&format!("git worktree add {worktree_display} {branch}\n"));
            }
            out.push_str(&format!("cd {worktree_display}\n"));
            out.push_str(
                "# realign if diverged from main before continuing:\n\
                 #   git rebase origin/main   (or: git merge origin/main)\n",
            );
            out.push_str(&format!(
                "aida session start --owns {spec_id} --branch {branch} --path {worktree_display} --reuse-branch\n"
            ));
            out.push_str("```\n\n");
        }
        BriefBranchTarget::Fresh { branch } => {
            let worktree_dir = worktree_path_for(branch);
            let worktree_display = worktree_dir.display();
            out.push_str("```bash\n");
            out.push_str(&format!("cd {project_root_display}\n"));
            out.push_str("git fetch origin main\n");
            out.push_str(&format!(
                "git worktree add {worktree_display} -b {branch} origin/main\n"
            ));
            out.push_str(&format!("cd {worktree_display}\n"));
            // BUG-331: no `.aida-store` symlink needed — sibling worktrees now
            // resolve the canonical store at the main worktree via
            // git-common-dir. trace:BUG-331
            out.push_str(&format!(
                "aida session start --owns {spec_id} --branch {branch} --path {worktree_display} --reuse-branch\n"
            ));
            out.push_str("```\n\n");
        }
    }

    out.push_str("## Trailer reminder\n\n");
    out.push_str(&format!(
        "Use a trailing-parens spec trailer in the commit subject: `({spec_id})` for a single-spec ship, or include every shipped spec in the same trailing parens.\n"
    ));
    out
}

/// BUG-1525: what the brief's Setup block should tell the implementer to do
/// about branches, derived from what already exists rather than assumed
/// fresh.
// trace:BUG-1525 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BriefBranchTarget {
    /// No branch anywhere references this spec — safe to start fresh off the
    /// default branch, using AIDA's `<spec>-work` naming convention (the
    /// shape the drain itself creates, per BUG-1525).
    Fresh { branch: String },
    /// Exactly one existing branch was found, and it's already checked out
    /// in an existing worktree — `cd` into it, don't create a second one.
    ExistingWorktree {
        branch: String,
        worktree: std::path::PathBuf,
    },
    /// Exactly one existing branch was found but no worktree currently has
    /// it checked out — attach a worktree to that branch (`remote_only`
    /// picks `-B <branch> origin/<branch>` over a plain local checkout).
    ExistingBranch { branch: String, remote_only: bool },
    /// More than one branch plausibly belongs to this spec — refuse to
    /// guess which one is live; say so explicitly rather than emit a
    /// confident fresh-start recipe.
    Ambiguous { candidates: Vec<String> },
}

/// Git-only scan (no `gh`/forge call — matches the pattern in
/// `collect_git_linkage_opts`) for a branch that already carries this spec's
/// work: any local or `origin/<branch>` ref whose name references the spec
/// id (`branch_name_references_spec`), excluding the `aida-store` orphan
/// branch. Used to decide whether the brief's Setup block should reuse an
/// existing lineage instead of starting a fresh one off `origin/main`.
// trace:BUG-1525 | ai:claude
pub(crate) fn resolve_brief_branch_target(
    project_root: &std::path::Path,
    spec_id: &str,
) -> BriefBranchTarget {
    let fresh_branch = format!("{}-work", spec_id.trim().to_ascii_lowercase());

    let git = |args: &[&str]| -> Option<String> {
        std::process::Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
    };

    let mut candidates: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if let Some(out) = git(&["for-each-ref", "--format=%(refname:short)", "refs/heads/"]) {
        for line in out.lines() {
            let name = line.trim();
            if !name.is_empty()
                && branch_name_references_spec(name, spec_id)
                && !is_orphan_store_branch(name)
            {
                candidates.insert(name.to_string());
            }
        }
    }
    if let Some(out) = git(&[
        "for-each-ref",
        "--format=%(refname:short)",
        "refs/remotes/origin/",
    ]) {
        for line in out.lines() {
            let Some(short) = line.trim().strip_prefix("origin/") else {
                continue;
            };
            if short.is_empty() || short == "HEAD" {
                continue;
            }
            if branch_name_references_spec(short, spec_id) && !is_orphan_store_branch(short) {
                candidates.insert(short.to_string());
            }
        }
    }

    if candidates.is_empty() {
        return BriefBranchTarget::Fresh {
            branch: fresh_branch,
        };
    }
    if candidates.len() > 1 {
        return BriefBranchTarget::Ambiguous {
            candidates: candidates.into_iter().collect(),
        };
    }
    let branch = candidates.into_iter().next().unwrap();

    for wt in list_worktrees(project_root) {
        if wt.branch.as_deref() == Some(branch.as_str()) {
            return BriefBranchTarget::ExistingWorktree {
                branch,
                worktree: wt.path,
            };
        }
    }

    let local_exists = git(&[
        "rev-parse",
        "--verify",
        "--quiet",
        &format!("refs/heads/{branch}"),
    ])
    .is_some();
    BriefBranchTarget::ExistingBranch {
        branch,
        remote_only: !local_exists,
    }
}

pub(crate) fn brief_generated_by() -> String {
    std::env::var("AIDA_SESSION_ID")
        .or_else(|_| std::env::var("CLAUDE_CODE_SESSION_ID"))
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "unknown".to_string())
}

pub(crate) fn yaml_scalar(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '/'))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "''"))
    }
}

pub(crate) fn indent_block(value: &str, prefix: &str) -> String {
    if value.is_empty() {
        return format!("{prefix}\n");
    }
    value
        .lines()
        .map(|line| format!("{prefix}{line}\n"))
        .collect()
}

pub(crate) fn sorted_tags(tags: &HashSet<String>) -> Option<String> {
    if tags.is_empty() {
        return None;
    }
    let mut tags = tags.iter().cloned().collect::<Vec<_>>();
    tags.sort();
    Some(tags.join(", "))
}

pub(crate) fn brief_relationship_lines(
    req: &Requirement,
    store: &RequirementsStore,
) -> Vec<String> {
    let mut lines = Vec::new();
    for rel in &req.relationships {
        if let Some(target) = store.get_requirement_by_id(&rel.target_id) {
            let target_id = target
                .spec_id
                .as_deref()
                .or(target.agreed_id.as_deref())
                .unwrap_or("<no-spec-id>");
            lines.push(format!(
                "{}: {} — {}",
                rel.rel_type, target_id, target.title
            ));
        }
    }
    for other in &store.requirements {
        for rel in &other.relationships {
            if rel.target_id == req.id {
                let other_id = other
                    .spec_id
                    .as_deref()
                    .or(other.agreed_id.as_deref())
                    .unwrap_or("<no-spec-id>");
                lines.push(format!(
                    "referenced by {}: {} — {}",
                    rel.rel_type, other_id, other.title
                ));
            }
        }
    }
    lines.sort();
    lines.dedup();
    lines
}

// trace:TASK-714
#[derive(Debug, Clone, PartialEq, Eq, ts_rs_forge::TS)]
pub(crate) struct BriefListEntry {
    pub(crate) path: std::path::PathBuf,
    pub(crate) spec_id: String,
    pub(crate) agent: String,
    pub(crate) generated_at: String,
    pub(crate) depends_on: Option<String>,
    pub(crate) acked: bool,
    /// STORY-711 slice 2: the authorizing advisor's id, when the brief was
    /// created with `--authorized-by`.
    // trace:TASK-1140 | ai:claude
    pub(crate) authorized_by: Option<String>,
}

pub(crate) fn list_agent_briefs(
    project_root: &std::path::Path,
    for_agent: Option<&str>,
    include_acked: bool,
) -> Result<()> {
    if let Some(agent) = for_agent {
        validate_brief_agent(agent)?;
    }
    let entries = collect_agent_briefs(project_root, for_agent, include_acked)?;
    if entries.is_empty() {
        println!("No briefs found.");
        return Ok(());
    }
    for entry in entries {
        let status = if entry.acked { "acked" } else { "pending" };
        // STORY-711 slice 2: surface the authorizing advisor when present.
        // trace:TASK-1140 | ai:claude
        match entry.authorized_by.as_deref() {
            Some(by) => println!(
                "{}  {}  {}  {}  {}  authorized_by={}",
                entry.generated_at,
                entry.agent,
                entry.spec_id,
                status,
                entry.path.display(),
                by
            ),
            None => println!(
                "{}  {}  {}  {}  {}",
                entry.generated_at,
                entry.agent,
                entry.spec_id,
                status,
                entry.path.display()
            ),
        }
    }
    Ok(())
}

pub(crate) fn collect_agent_briefs(
    project_root: &std::path::Path,
    for_agent: Option<&str>,
    include_acked: bool,
) -> Result<Vec<BriefListEntry>> {
    // Internal scans (notification banners, dependency walks, drain pickups)
    // resolve a bare agent-type name-class and must stay silent; only an
    // explicit user `--for-agent <target>` should surface ambiguity.
    collect_agent_briefs_inner(project_root, for_agent, include_acked, true)
}

// BUG-569: the ambiguity warning from `resolve_brief_directories` is only a real
// user error when the caller explicitly targeted an agent. Internal callers that
// pass a bare agent-TYPE name-class (e.g. `detect_agent_type()` → "antigravity")
// to scan that type's briefs were inheriting the warning and printing spurious
// "agent target 'antigravity' is ambiguous" noise on every advisor edit/comment
// when 2+ agents of that type were registered. Route those through
// `warn_on_ambiguity = false`. trace:BUG-569 | ai:claude
pub(crate) fn collect_agent_briefs_inner(
    project_root: &std::path::Path,
    for_agent: Option<&str>,
    include_acked: bool,
    warn_on_ambiguity: bool,
) -> Result<Vec<BriefListEntry>> {
    let root = project_root.join(".aida").join("agent-briefs");
    if !root.exists() {
        return Ok(Vec::new());
    }

    let allowed_dirs: Option<Vec<String>> = if let Some(target) = for_agent {
        let (dirs, warning) =
            resolve_brief_dirs_with_optional_warning(project_root, target, warn_on_ambiguity);
        if let Some(warn) = warning {
            eprintln!("{}", warn);
        }
        Some(dirs)
    } else {
        None
    };

    let mut entries = Vec::new();
    for agent_dir in
        std::fs::read_dir(&root).with_context(|| format!("failed to read {}", root.display()))?
    {
        let agent_dir = agent_dir?;
        if !agent_dir.file_type()?.is_dir() {
            continue;
        }
        let agent = agent_dir.file_name().to_string_lossy().to_string();
        if let Some(ref allowed) = allowed_dirs {
            if !allowed.iter().any(|d| d.eq_ignore_ascii_case(&agent)) {
                continue;
            }
        }
        for file in std::fs::read_dir(agent_dir.path())? {
            let file = file?;
            if !file.file_type()?.is_file() {
                continue;
            }
            let path = file.path();
            let name = file.file_name().to_string_lossy().to_string();
            let acked = name.ends_with(".acked");
            if acked && !include_acked {
                continue;
            }
            if !(name.ends_with(".md") || acked) {
                continue;
            }
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            let spec_id = frontmatter_value(&content, "spec_id")
                .unwrap_or_else(|| spec_id_from_brief_filename(&name).unwrap_or_default());
            let generated_at = frontmatter_value(&content, "generated_at").unwrap_or_default();
            let depends_on = frontmatter_value(&content, "depends_on");
            // trace:TASK-1140 | ai:claude
            let authorized_by = frontmatter_value(&content, "authorized_by");
            entries.push(BriefListEntry {
                path,
                spec_id,
                agent: agent.clone(),
                generated_at,
                depends_on,
                acked,
                authorized_by,
            });
        }
    }
    sort_brief_entries_topologically(&mut entries)?;
    Ok(entries)
}

/// BUG-569: resolve brief directories for `target`, dropping the ambiguity
/// warning entirely when `warn_on_ambiguity` is false. The warning is only a
/// real user error when the caller explicitly targeted an agent (e.g.
/// `aida brief list --for-agent <target>`); internal scans that pass a bare
/// agent-TYPE name-class must stay silent. Split out so the suppression
/// decision is unit-testable without capturing stderr. trace:BUG-569 | ai:claude
pub(crate) fn resolve_brief_dirs_with_optional_warning(
    project_root: &std::path::Path,
    target: &str,
    warn_on_ambiguity: bool,
) -> (Vec<String>, Option<String>) {
    let (dirs, warning) = agent_registry::resolve_brief_directories(project_root, target);
    let warning = if warn_on_ambiguity { warning } else { None };
    (dirs, warning)
}

pub(crate) fn normalize_brief_dependency(
    store: &RequirementsStore,
    depends_on: Option<&str>,
) -> Result<Option<String>> {
    let Some(depends_on) = depends_on.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    // trace:TASK-1468 | ai:claude
    let req = store
        .get_requirement_unambiguous(depends_on)?
        .ok_or_else(|| not_found::requirement_not_found(depends_on, None))?;
    Ok(Some(
        req.spec_id.as_deref().unwrap_or(depends_on).to_string(),
    ))
}

pub(crate) fn ensure_brief_dependency_is_acyclic(
    project_root: &std::path::Path,
    agent: &str,
    spec_id: &str,
    depends_on: Option<&str>,
) -> Result<()> {
    let Some(depends_on) = depends_on else {
        return Ok(());
    };
    if depends_on == spec_id {
        anyhow::bail!("brief dependency cycle: {spec_id} cannot depend on itself");
    }

    // BUG-569: internal dependency walk — stay silent on type-class ambiguity.
    let mut deps = collect_agent_briefs_inner(project_root, Some(agent), true, false)?
        .into_iter()
        .filter_map(|entry| entry.depends_on.map(|dep| (entry.spec_id, dep)))
        .collect::<std::collections::HashMap<_, _>>();
    deps.insert(spec_id.to_string(), depends_on.to_string());

    let mut cursor = depends_on;
    let mut seen = std::collections::HashSet::new();
    while let Some(next) = deps.get(cursor).map(String::as_str) {
        if next == spec_id {
            anyhow::bail!(
                "brief dependency cycle: adding {spec_id} --depends-on {depends_on} would create a cycle"
            );
        }
        if !seen.insert(next.to_string()) {
            anyhow::bail!("brief dependency cycle detected involving {next}");
        }
        cursor = next;
    }
    Ok(())
}

pub(crate) fn brief_entry_depends_on(
    deps: &std::collections::HashMap<String, String>,
    start: &str,
    target: &str,
) -> bool {
    let mut cursor = start;
    let mut seen = std::collections::HashSet::new();
    while let Some(next) = deps.get(cursor).map(String::as_str) {
        if next == target {
            return true;
        }
        if !seen.insert(next.to_string()) {
            return false;
        }
        cursor = next;
    }
    false
}

pub(crate) fn sort_brief_entries_topologically(entries: &mut [BriefListEntry]) -> Result<()> {
    let deps = entries
        .iter()
        .filter_map(|entry| {
            entry
                .depends_on
                .as_ref()
                .map(|depends_on| (entry.spec_id.clone(), depends_on.clone()))
        })
        .collect::<std::collections::HashMap<_, _>>();

    entries.sort_by(|a, b| {
        if a.agent == b.agent {
            if brief_entry_depends_on(&deps, &a.spec_id, &b.spec_id) {
                return std::cmp::Ordering::Greater;
            }
            if brief_entry_depends_on(&deps, &b.spec_id, &a.spec_id) {
                return std::cmp::Ordering::Less;
            }
        }
        a.agent
            .cmp(&b.agent)
            .then(a.generated_at.cmp(&b.generated_at))
            .then(a.path.cmp(&b.path))
    });
    Ok(())
}

/// BUG-378: substrate-as-bouncer for agent scratchpad drift.
///
/// When an agent is about to declare a spec Done/Completed, scan the brief
/// surface for unacked briefs targeting THIS agent's type and print a loud
/// banner to stderr naming each file path. The agent — about to loop on its
/// own internal scratchpad and re-render "all done" — gets told by the
/// substrate that new work is queued before it gets a chance to exit.
///
/// Gating rules (kept narrow on purpose — false positives would teach agents
/// to ignore the banner):
/// - Agent type detected as `claude`, `codex`, or `antigravity` only. The
///   `"other"` fallback (raw shell, untagged caller) does NOT scan — would
///   noise up every interactive `aida queue done` run by a human.
/// - Only briefs matching the running agent's type are listed. A pending
///   `antigravity` brief never fires when Codex is running.
/// - Banner goes to stderr in bold red so it survives stdout piping and
///   stands out in a wall of green check marks. trace:BUG-378 | ai:claude
pub(crate) fn warn_pending_briefs_for_running_agent(project_root: &std::path::Path) {
    let agent_type = agent_registry::detect_agent_type();
    let Some(lines) = pending_brief_banner_lines(project_root, &agent_type) else {
        return;
    };
    for line in lines {
        eprintln!("{}", line);
    }
}

/// Pure-function core of [`warn_pending_briefs_for_running_agent`] —
/// returns the banner lines (already styled with ANSI red/bold) or `None`
/// when the gate stays silent. Split out so unit tests can assert directly
/// on the rendered output without capturing stderr. trace:BUG-378 | ai:claude
pub(crate) fn pending_brief_banner_lines(
    project_root: &std::path::Path,
    agent_type: &str,
) -> Option<Vec<String>> {
    if !matches!(agent_type, "claude" | "codex" | "antigravity") {
        return None;
    }
    // BUG-569: bare agent-type scan — stay silent on type-class ambiguity.
    let entries = collect_agent_briefs_inner(project_root, Some(agent_type), false, false).ok()?;
    if entries.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(entries.len() + 4);
    out.push(String::new());
    out.push(
        format!(
            "{} NEW BRIEF(S) PENDING for agent `{}` — read before exiting:",
            crate::glyph(crate::glyphs::Glyph::Warning),
            agent_type
        )
        .red()
        .bold()
        .to_string(),
    );
    for entry in &entries {
        out.push(format!("    {}", entry.path.display()).red().to_string());
    }
    out.push(format!(
        "{} {}",
        "  Run:".red(),
        format!("aida brief list --for-agent {}", agent_type)
            .red()
            .bold()
    ));
    out.push(
        "  Your internal task.md / scratchpad is NOT ground truth — \
         poll the brief surface before declaring work complete."
            .red()
            .to_string(),
    );
    out.push(String::new());
    Some(out)
}

pub(crate) fn frontmatter_value(content: &str, key: &str) -> Option<String> {
    let mut lines = content.lines();
    if lines.next()? != "---" {
        return None;
    }
    let prefix = format!("{key}:");
    for line in lines {
        if line == "---" {
            return None;
        }
        if let Some(raw) = line.strip_prefix(&prefix) {
            return Some(raw.trim().trim_matches('\'').to_string());
        }
    }
    None
}

pub(crate) fn spec_id_from_brief_filename(name: &str) -> Option<String> {
    let name = name.strip_suffix(".acked").unwrap_or(name);
    let name = name.strip_suffix(".md").unwrap_or(name);
    let (spec, _) = name.rsplit_once('-')?;
    Some(spec.to_string())
}

/// TASK-502: the `.pending` sentinel for an agent — one urgent (unacked,
/// `--notify`'d) brief path per line, project-relative. Lives in the agent's
/// brief dir so `ack`, which only has the brief path, can find it from the
/// brief's parent without needing the project root or agent name.
pub(crate) fn pending_briefs_path(
    project_root: &std::path::Path,
    agent: &str,
) -> std::path::PathBuf {
    project_root
        .join(".aida")
        .join("agent-briefs")
        .join(agent)
        .join(".pending")
}

/// Read the current `.pending` entries (project-relative brief paths), trimming
/// blanks. Missing file → empty list.
pub(crate) fn read_pending_briefs(pending_path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(pending_path)
        .map(|s| {
            s.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Rewrite `.pending` from a list — delete the file when the list is empty so
/// an empty inbox leaves no sentinel behind.
pub(crate) fn write_pending_briefs(
    pending_path: &std::path::Path,
    entries: &[String],
) -> Result<()> {
    if entries.is_empty() {
        if pending_path.exists() {
            std::fs::remove_file(pending_path)
                .with_context(|| format!("failed to remove empty {}", pending_path.display()))?;
        }
        return Ok(());
    }
    if let Some(parent) = pending_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(pending_path, format!("{}\n", entries.join("\n")))
        .with_context(|| format!("failed to write {}", pending_path.display()))?;
    Ok(())
}

/// Add a brief to the agent's `.pending` sentinel, idempotently (re-notifying
/// the same brief does not duplicate the entry). Stored project-relative.
pub(crate) fn add_pending_brief(
    project_root: &std::path::Path,
    agent: &str,
    brief_path: &std::path::Path,
) -> Result<()> {
    let pending_path = pending_briefs_path(project_root, agent);
    // Store the project-relative key with forward slashes regardless of OS so
    // the `.pending` sentinel is portable and matches the `.aida/agent-briefs/`
    // convention used everywhere else. `Path::display()` emits `\` on Windows,
    // which broke task_492_brief_tests on the cross-platform runner.
    // trace:BUG-466 | ai:claude
    // trace:BUG-1648 | ai:claude — shared with the plan-path writer.
    let rel = plan_rel_path(brief_path, project_root);
    let mut entries = read_pending_briefs(&pending_path);
    if !entries.iter().any(|e| e == &rel) {
        entries.push(rel);
    }
    write_pending_briefs(&pending_path, &entries)
}

/// Remove a brief from its agent's `.pending` sentinel (called on ack). The
/// agent dir is the brief's parent, so we don't need the project root. Matches
/// either the bare brief name or any path entry ending in it, so an
/// absolute-vs-relative mismatch doesn't strand the entry.
pub(crate) fn clear_pending_brief(brief_path: &std::path::Path) {
    let Some(dir) = brief_path.parent() else {
        return;
    };
    let pending_path = dir.join(".pending");
    if !pending_path.exists() {
        return;
    }
    let Some(name) = brief_path.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let kept: Vec<String> = read_pending_briefs(&pending_path)
        .into_iter()
        .filter(|e| {
            // Drop the entry whose final path component is this brief's file
            // name (handles project-relative vs absolute storage).
            std::path::Path::new(e).file_name().and_then(|n| n.to_str()) != Some(name)
        })
        .collect();
    let _ = write_pending_briefs(&pending_path, &kept);
}

/// TASK-502: scan `.aida/agent-briefs/*/.pending` and return `(agent, count)`
/// for every agent with at least one urgent (notify'd, unacked) brief. When
/// `AIDA_AGENT_NAME` resolves to a known brief-agent, narrow to that one (the
/// "current agent's identity" case); otherwise report all agents.
pub(crate) fn collect_pending_brief_counts(project_root: &std::path::Path) -> Vec<(String, usize)> {
    let briefs_root = project_root.join(".aida").join("agent-briefs");
    let Ok(entries) = std::fs::read_dir(&briefs_root) else {
        return Vec::new();
    };
    // Derive the current brief-agent from the stable session name
    // (e.g. "claude-3f2a" → "claude"), if any.
    let current_agent = std::env::var("AIDA_AGENT_NAME").ok().and_then(|name| {
        ["claude", "codex", "antigravity"]
            .into_iter()
            .find(|a| name == *a || name.starts_with(&format!("{a}-")))
            .map(str::to_string)
    });
    let mut out: Vec<(String, usize)> = Vec::new();
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let Some(agent) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if let Some(cur) = &current_agent {
            if &agent != cur {
                continue;
            }
        }
        let count = read_pending_briefs(&entry.path().join(".pending")).len();
        if count > 0 {
            out.push((agent, count));
        }
    }
    out.sort();
    out
}

pub(crate) fn ack_agent_brief(brief_file: &std::path::Path) -> Result<()> {
    let path = brief_file;
    if !path.exists() {
        anyhow::bail!("brief file not found: {}", path.display());
    }
    if path.extension().and_then(|e| e.to_str()) == Some("acked") {
        println!("Already acknowledged: {}", path.display());
        return Ok(());
    }
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("invalid brief file path: {}", path.display()))?;
    let acked_path = path.with_file_name(format!("{file_name}.acked"));
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read brief {}", path.display()))?;
    let content = if content.contains("\nstatus: pending\n") {
        content.replacen("\nstatus: pending\n", "\nstatus: acked\n", 1)
    } else {
        content
    };
    std::fs::write(path, content)
        .with_context(|| format!("failed to update brief status in {}", path.display()))?;
    std::fs::rename(path, &acked_path).with_context(|| {
        format!(
            "failed to acknowledge brief {} -> {}",
            path.display(),
            acked_path.display()
        )
    })?;
    // TASK-502: drop this brief from the agent's `.pending` sentinel (no-op if
    // it was never --notify'd). Uses the pre-rename path's file name.
    clear_pending_brief(path);
    println!("Acknowledged: {}", acked_path.display());
    Ok(())
}

pub(crate) fn read_agent_brief(
    project_root: &std::path::Path,
    brief_file: &str,
    latest: bool,
) -> Result<()> {
    let resolved_path = if latest {
        let agent = validate_brief_agent(brief_file)?;
        let (dirs, warning) = agent_registry::resolve_brief_directories(project_root, agent);
        if let Some(warn) = warning {
            eprintln!("{}", warn);
        }
        let chosen_agent = if dirs.is_empty() {
            agent.to_string()
        } else {
            dirs[0].clone()
        };
        // BUG-569: the explicit-target warning already fired above via
        // resolve_brief_directories; this re-scan stays silent to avoid a
        // double warning.
        let entries = collect_agent_briefs_inner(project_root, Some(&chosen_agent), false, false)?;
        let last_entry = entries.last().ok_or_else(|| {
            anyhow::anyhow!(
                "Error: no pending briefs found for agent \"{}\". Use 'aida brief list' to view available briefs.",
                chosen_agent
            )
        })?;
        last_entry.path.clone()
    } else {
        let raw_path = std::path::Path::new(brief_file);
        if raw_path.exists() {
            raw_path.to_path_buf()
        } else {
            let relative_path = project_root.join(brief_file);
            if relative_path.exists() {
                relative_path
            } else if let Some((agent, filename)) = brief_file.split_once('/') {
                let agent = validate_brief_agent(agent)?;
                let (dirs, warning) =
                    agent_registry::resolve_brief_directories(project_root, agent);
                if let Some(warn) = warning {
                    eprintln!("{}", warn);
                }
                let chosen_agent = if dirs.is_empty() {
                    agent.to_string()
                } else {
                    dirs[0].clone()
                };
                let base_path = project_root
                    .join(".aida")
                    .join("agent-briefs")
                    .join(&chosen_agent)
                    .join(filename);
                if base_path.exists() {
                    base_path
                } else {
                    let md_path = base_path.with_extension("md");
                    if md_path.exists() {
                        md_path
                    } else {
                        let acked_path = base_path.with_extension("acked");
                        if acked_path.exists() {
                            acked_path
                        } else {
                            let filename_str = filename.to_string();
                            if filename_str.ends_with(".md") {
                                let without_ext = &filename_str[..filename_str.len() - 3];
                                let acked =
                                    base_path.with_file_name(format!("{}.acked", without_ext));
                                if acked.exists() {
                                    acked
                                } else {
                                    anyhow::bail!(
                                        "Error: brief not found at \"{}\". Use 'aida brief list' to view available briefs.",
                                        brief_file
                                    );
                                }
                            } else if filename_str.ends_with(".acked") {
                                let without_ext = &filename_str[..filename_str.len() - 6];
                                let md = base_path.with_file_name(format!("{}.md", without_ext));
                                if md.exists() {
                                    md
                                } else {
                                    anyhow::bail!(
                                        "Error: brief not found at \"{}\". Use 'aida brief list' to view available briefs.",
                                        brief_file
                                    );
                                }
                            } else {
                                anyhow::bail!(
                                    "Error: brief not found at \"{}\". Use 'aida brief list' to view available briefs.",
                                    brief_file
                                );
                            }
                        }
                    }
                }
            } else {
                anyhow::bail!(
                    "Error: brief not found at \"{}\". Use 'aida brief list' to view available briefs.",
                    brief_file
                );
            }
        }
    };

    let body = std::fs::read_to_string(&resolved_path)
        .with_context(|| format!("failed to read brief at {}", resolved_path.display()))?;
    print!(
        "{}",
        render_agent_brief_read(project_root, &resolved_path, &body)?
    );
    Ok(())
}

pub(crate) fn render_agent_brief_read(
    project_root: &std::path::Path,
    path: &std::path::Path,
    body: &str,
) -> Result<String> {
    let Some(depends_on) = frontmatter_value(body, "depends_on") else {
        return Ok(body.to_string());
    };
    let agent = frontmatter_value(body, "agent")
        .or_else(|| {
            path.parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    if agent.is_empty() {
        return Ok(body.to_string());
    }
    // BUG-569: internal render scan — stay silent on type-class ambiguity.
    let pending = collect_agent_briefs_inner(project_root, Some(&agent), false, false)?;
    if pending.iter().any(|entry| {
        entry.spec_id == depends_on
            && entry.path != path
            && entry.path.with_extension("md") != path.with_extension("md")
    }) {
        return Ok(format!("Blocked by: {depends_on}\n\n{body}"));
    }
    Ok(body.to_string())
}

#[cfg(test)]
#[path = "tests/task_492_brief_tests.rs"]
mod task_492_brief_tests;
