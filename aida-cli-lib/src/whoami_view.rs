//! `aida whoami`: one compact view of who this session is — its mail handle,
//! node, unread mail, the spec it is on, its goal and its worktree.
//!
//! Every line is read from local state only (env, registry rows, lease files,
//! the mailbox files, the queue YAML and a read-only cache lookup); nothing
//! here touches the network or marks mail seen. The handle and the mail
//! counts come from the same resolver and the same mail-channel collector the
//! per-turn mail notice uses, so the handle printed here is the address
//! `aida mailbox send` delivers to.
// trace:STORY-B6B1278890774EF993D2EBC327862470-098 | ai:claude

use std::path::Path;

use chrono::{DateTime, Utc};
use colored::Colorize;
use serde::Serialize;

use crate::MailIdentityFacts;

/// How long whoami's mail read may take. Shorter than the per-turn notice's
/// passive deadline so the whole view stays inside its 300 ms budget; a read
/// that runs out marks the mail line partial rather than stalling.
pub(crate) const MAIL_READ_BUDGET: std::time::Duration = std::time::Duration::from_millis(180);

/// The command that reads (and acknowledges) the mail the view counts.
pub(crate) const MAIL_READ_COMMAND: &str = "aida mailbox inbox";

#[derive(Debug, Clone, Serialize)]
pub(crate) struct WhoamiView {
    /// The address this session's mail is delivered to.
    pub handle: String,
    pub role: String,
    pub agent_type: String,
    /// The harness's own session id (Claude, Codex, agy), shortened.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The AIDA lease id (`AIDA_SESSION_ID`), a different namespace from the
    /// harness session id; shown only when no harness id is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<NodeIdentity>,
    pub mail: MailLine,
    pub working: Working,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal: Option<GoalLine>,
    pub worktree: WorktreeLine,
}

/// This clone's node: canonical id, its current label, and whether it holds
/// HOME (authoritative) allocation or is a work node allocating locally.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct NodeIdentity {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub authoritative: bool,
}

impl NodeIdentity {
    /// The node's role as lower-case prose, kept apart from any label so an
    /// operator label such as `WORK` can never be read as the role.
    pub(crate) fn role_prose(&self) -> &'static str {
        if self.authoritative {
            "home node, authoritative allocation"
        } else {
            "work node, node-local allocation"
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct MailLine {
    pub unread: usize,
    pub shared_unread: usize,
    pub urgent: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared_scope: Option<String>,
    /// Some mail source could not be read; the counts may be low.
    pub partial: bool,
    pub read_command: &'static str,
}

impl From<crate::awaiting_you::MailChannel> for MailLine {
    fn from(c: crate::awaiting_you::MailChannel) -> Self {
        Self {
            // Mail to the session's role is its direct mail, counted with
            // its own rather than as shared noise.
            // trace:BUG-B6B1278890774EF993D2EBC327862470-040 | ai:claude
            unread: c.unread + c.role_unread,
            shared_unread: c.shared_unread,
            urgent: c.urgent,
            shared_scope: c.shared_scope,
            partial: c.partial,
            read_command: MAIL_READ_COMMAND,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum Working {
    /// This session holds a lease (or registry claim) on `spec`. `lease` is
    /// `None` for a registry claim; `age_secs` is the lease's age, or for a
    /// claim the age of the session row that names it.
    Spec {
        spec: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        lease: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        age_secs: Option<i64>,
    },
    /// Nothing claimed; `next` is the head of this role's queue, if any.
    Idle {
        #[serde(skip_serializing_if = "Option::is_none")]
        next: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GoalLine {
    /// The operator's standing goal, shown in the advisor seat only.
    // trace:STORY-2243 | ai:claude
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advisor_goal: Option<String>,
    /// The active mandate's one-line status (time left, principal, gates).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mandate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct WorktreeLine {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// The handle `facts` resolve to: the verified session handle, else the user
/// — the same choice the mail channel makes for its own-inbox headline.
pub(crate) fn resolved_handle(facts: &MailIdentityFacts) -> String {
    facts
        .own_handle()
        .map(str::to_string)
        .unwrap_or_else(|| facts.user().to_string())
}

/// What this process knows about its session besides its mail facts: the
/// registry row the session lookup selected, the AIDA lease id, the seat
/// purpose and the mandate state. [`collect`] reads them from the process;
/// tests supply fixtures.
pub(crate) struct SessionInputs {
    pub entry: Option<crate::agent_registry::AgentRegistryEntry>,
    pub lease_id: Option<String>,
    pub purpose: Option<String>,
    pub mandate: crate::gate::mandate::MandateState,
    /// The queue identity (`current_user_id`).
    pub user: String,
}

impl SessionInputs {
    fn observed() -> Self {
        Self {
            entry: crate::agent_handle::current_session_entry_for_cwd(),
            lease_id: env_nonempty("AIDA_SESSION_ID"),
            purpose: env_nonempty("AIDA_SESSION_PURPOSE"),
            mandate: crate::gate::mandate::current_state(),
            user: crate::current_user_id(None),
        }
    }
}

/// Gather the view. `main_root` is the main checkout (registry, leases,
/// store, cache); `worktree` is the checkout this process runs in.
pub(crate) fn collect(
    main_root: &Path,
    worktree: &Path,
    facts: &MailIdentityFacts,
    now: DateTime<Utc>,
) -> WhoamiView {
    collect_with(main_root, worktree, facts, SessionInputs::observed(), now)
}

/// [`collect`] over explicit session inputs.
pub(crate) fn collect_with(
    main_root: &Path,
    worktree: &Path,
    facts: &MailIdentityFacts,
    inputs: SessionInputs,
    now: DateTime<Utc>,
) -> WhoamiView {
    let store = main_root.join(".aida-store");
    let aliases = aida_core::node_alias::NodeAliasMap::load(&store);
    let SessionInputs {
        entry,
        lease_id,
        purpose,
        mandate,
        user,
    } = inputs;

    let role = facts
        .role()
        .map(str::to_string)
        .or_else(|| entry.as_ref().and_then(|e| e.role.clone()))
        .map(|r| crate::canonical_role_name(&r))
        .unwrap_or_else(|| "implementer".to_string());
    let agent_type = entry
        .as_ref()
        .map(|e| e.agent_type.clone())
        .unwrap_or_else(crate::agent_registry::detect_agent_type);
    let session_id: Option<String> = entry
        .as_ref()
        .and_then(|e| e.native_session_id.clone().or(e.claude_session_id.clone()))
        .map(|s| s.chars().take(8).collect());
    // The lease id is a different namespace; label it, and only when no
    // harness id names the session.
    let shown_lease_id = session_id.is_none().then(|| lease_id.clone()).flatten();

    let limits = aida_core::mailbox_logical::ReadLimits::with_deadline(
        std::time::Instant::now() + MAIL_READ_BUDGET,
    );
    let mail = crate::collect_mail_channel(main_root, None, facts, &limits).into();

    let canonical = aida_core::node_alias::canonical_ids();
    let working = match current_work(
        main_root,
        worktree,
        lease_id.as_deref(),
        entry.as_ref(),
        now,
    ) {
        Some(Working::Spec {
            spec,
            lease,
            age_secs,
        }) => Working::Spec {
            spec: aliases.display(&spec, canonical),
            lease,
            age_secs,
        },
        _ => Working::Idle {
            next: queue_head(main_root, &user, &role, &aliases),
        },
    };

    let goal = goal_line(&mandate, purpose, now, || {
        crate::mandate_gates::describe(Some(main_root))
    });
    // trace:STORY-2243 | ai:claude
    let goal = with_advisor_goal(
        goal,
        crate::goal_cmd::goal_advisor::whoami_goal_text(&store, &role),
    );

    WhoamiView {
        handle: resolved_handle(facts),
        role,
        agent_type,
        session_id,
        lease_id: shown_lease_id,
        node: this_node(&store, &aliases),
        mail,
        working,
        goal,
        worktree: WorktreeLine {
            path: worktree.display().to_string(),
            branch: head_branch(worktree),
        },
    }
}

/// The branch `worktree` has checked out, read from its `HEAD` file rather
/// than by running git: no subprocess, and it works in a contained seat clone
/// where AIDA refuses to run git. A detached HEAD reads as `detached <sha>`.
pub(crate) fn head_branch(worktree: &Path) -> Option<String> {
    let dotgit = worktree.join(".git");
    let gitdir = if dotgit.is_dir() {
        dotgit
    } else {
        // A linked worktree's `.git` is a `gitdir: <path>` pointer file.
        let text = std::fs::read_to_string(&dotgit).ok()?;
        let target = std::path::PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
        worktree.join(target)
    };
    let head = std::fs::read_to_string(gitdir.join("HEAD")).ok()?;
    let head = head.trim();
    match head.strip_prefix("ref: refs/heads/") {
        Some(branch) => Some(branch.to_string()),
        None if !head.is_empty() => Some(format!(
            "detached {}",
            head.chars().take(8).collect::<String>()
        )),
        None => None,
    }
}

/// This clone's node identity, or `None` when the clone has no node binding.
pub(crate) fn this_node(
    store: &Path,
    aliases: &aida_core::node_alias::NodeAliasMap,
) -> Option<NodeIdentity> {
    let authoritative = aida_core::allocation_authority::resolve(store)
        .ok()?
        .authoritative;
    let id = if authoritative {
        aida_core::NodeConfig::load(&store.join(".aida").join("node.toml"))
            .ok()?
            .node_id
    } else {
        aida_core::work_binding::load(store).ok()??.node_id
    };
    Some(NodeIdentity {
        label: aliases.current_label(&id).map(str::to_string),
        id,
        authoritative,
    })
}

/// The spec this session is on, unaliased: the lease named by `lease_id`
/// (`AIDA_SESSION_ID`), else the lease whose worktree holds `worktree`, each
/// aged from its start; else the spec the session's registry row claims,
/// aged from that row's start (rows record no separate claim time).
fn current_work(
    main_root: &Path,
    worktree: &Path,
    lease_id: Option<&str>,
    entry: Option<&crate::agent_registry::AgentRegistryEntry>,
    now: DateTime<Utc>,
) -> Option<Working> {
    let age = |since: DateTime<Utc>| Some((now - since).num_seconds().max(0));
    let leases = crate::list_leases(main_root);
    let by_id = lease_id.and_then(|sid| leases.iter().find(|l| l.id == sid).cloned());
    if let Some(lease) = by_id.or_else(|| crate::active_lease_for_cwd(main_root, worktree)) {
        return Some(Working::Spec {
            age_secs: age(lease.started_at),
            spec: lease.scope,
            lease: Some(lease.id),
        });
    }
    let entry = entry?;
    Some(Working::Spec {
        spec: entry.current_spec.clone()?,
        lease: None,
        age_secs: age(entry.started_at),
    })
}

/// The goal line: the active mandate's status line (with the human gates
/// `gates` describes) and the seat purpose; `None` when neither is set.
pub(crate) fn goal_line(
    mandate: &crate::gate::mandate::MandateState,
    purpose: Option<String>,
    now: DateTime<Utc>,
    gates: impl FnOnce() -> String,
) -> Option<GoalLine> {
    let mandate = match mandate {
        crate::gate::mandate::MandateState::Active(_) => {
            crate::mandate::status_line(mandate, now, &gates())
        }
        _ => None,
    };
    (mandate.is_some() || purpose.is_some()).then_some(GoalLine {
        advisor_goal: None,
        mandate,
        purpose,
    })
}

/// [`goal_line`] with the advisor's standing goal added (`advisor_goal` is
/// only ever `Some` in the advisor seat).
// trace:STORY-2243 | ai:claude
pub(crate) fn with_advisor_goal(
    goal: Option<GoalLine>,
    advisor_goal: Option<String>,
) -> Option<GoalLine> {
    match (goal, advisor_goal) {
        (Some(mut g), a) => {
            g.advisor_goal = a;
            Some(g)
        }
        (None, Some(a)) => Some(GoalLine {
            advisor_goal: Some(a),
            mandate: None,
            purpose: None,
        }),
        (None, None) => None,
    }
}

/// The next item for `role` as `<ID> <title>`, from the same source and in
/// the same order as `aida queue next`: `user`'s own queue, then the entries
/// routed to `role` from every other queue file (the `role:<role>` pool
/// included). Items whose spec is closed (completed, rejected, superseded)
/// are skipped. An unrouted item counts as the implementer's. Ids and titles
/// come from a read-only cache lookup; an item the cache does not know is
/// skipped. `None` when nothing is left.
pub(crate) fn queue_head(
    main_root: &Path,
    user: &str,
    role: &str,
    aliases: &aida_core::node_alias::NodeAliasMap,
) -> Option<String> {
    let storage = aida_core::Storage::new(main_root.join(".aida-store"));
    let entries = crate::queue_role_fallback::queue_list_with_role_fallback(
        &storage,
        user,
        Some(role),
        /* include_completed */ false,
    )
    .ok()?;
    let conn = rusqlite::Connection::open_with_flags(
        main_root.join(".aida/cache.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()?;
    entries
        .iter()
        .filter(|e| {
            e.for_role
                .as_deref()
                .map(crate::canonical_role_name)
                .as_deref()
                .unwrap_or("implementer")
                == role
        })
        .find_map(|e| {
            let (id, title, status): (String, String, String) = conn
                .query_row(
                    "SELECT COALESCE(agreed_id, spec_id, ''), title, status \
                     FROM requirements_cache WHERE id = ?1",
                    [e.requirement_id.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .ok()?;
            (!id.is_empty() && !crate::is_terminal_status_str(&status)).then(|| {
                format!(
                    "{} {}",
                    aliases.display(&id, aida_core::node_alias::canonical_ids()),
                    title
                )
            })
        })
}

/// `2m`, `3h05m`, `2d4h` — a compact age.
pub(crate) fn format_age(secs: i64) -> String {
    let m = secs / 60;
    match m {
        m if m < 1 => format!("{secs}s"),
        m if m < 60 => format!("{m}m"),
        m if m < 60 * 24 => format!("{}h{:02}m", m / 60, m % 60),
        m => format!("{}d{}h", m / (60 * 24), (m / 60) % 24),
    }
}

impl WhoamiView {
    fn header(&self) -> String {
        let mut parts = vec![
            self.handle.clone(),
            self.role.clone(),
            self.agent_type.clone(),
        ];
        if let Some(s) = &self.session_id {
            parts.push(format!("session {s}"));
        } else if let Some(l) = &self.lease_id {
            parts.push(format!("lease {l}"));
        }
        parts.join(" · ")
    }

    fn node_text(&self) -> Option<String> {
        self.node.as_ref().map(|n| {
            let name = match &n.label {
                Some(label) => format!("{label} ({})", n.id),
                None => n.id.clone(),
            };
            format!("{name} · {}", n.role_prose())
        })
    }

    fn mail_text(&self) -> String {
        let m = &self.mail;
        let mut s = format!(
            "{} unread · {} shared · {} urgent",
            m.unread, m.shared_unread, m.urgent
        );
        if m.partial {
            s.push_str(" · partial view (counts may be low)");
        }
        s
    }

    fn working_text(&self) -> String {
        match &self.working {
            Working::Spec {
                spec,
                lease,
                age_secs,
            } => {
                let kind = if lease.is_some() { "lease" } else { "claim" };
                match age_secs {
                    Some(age) => format!("{spec} ({kind} {})", format_age(*age)),
                    None => format!("{spec} ({kind})"),
                }
            }
            Working::Idle { next: Some(next) } => format!("idle · next: {next}"),
            Working::Idle { next: None } => "idle".to_string(),
        }
    }

    /// The advisor seat prints its purpose on its own `purpose:` line, so
    /// `goal:` carries the operator's goal. trace:STORY-2243 | ai:claude
    fn purpose_on_own_line(&self) -> bool {
        self.role == "advisor"
    }

    fn goal_text(&self) -> Option<String> {
        let g = self.goal.as_ref()?;
        let inline_purpose = (!self.purpose_on_own_line())
            .then_some(())
            .and(g.purpose.as_ref());
        let parts: Vec<String> = g
            .advisor_goal
            .iter()
            .cloned()
            .chain(g.mandate.iter().cloned())
            .chain(inline_purpose.map(|p| format!("purpose: {p}")))
            .collect();
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    fn purpose_text(&self) -> Option<&str> {
        self.purpose_on_own_line()
            .then(|| self.goal.as_ref()?.purpose.as_deref())
            .flatten()
    }

    fn worktree_text(&self) -> String {
        match &self.worktree.branch {
            Some(b) => format!("{} ({b})", self.worktree.path),
            None => self.worktree.path.clone(),
        }
    }

    /// The view in the output mode `agent` selects: flat toon lines for an
    /// agent (piped) reader, else the human block.
    pub(crate) fn render(&self, agent: bool) -> String {
        if agent {
            self.render_toon()
        } else {
            self.render_human()
        }
    }

    /// The compact human block.
    pub(crate) fn render_human(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("{}\n", self.header().bold()));
        if let Some(node) = self.node_text() {
            out.push_str(&format!("node:     {node}\n"));
        }
        out.push_str(&format!(
            "mail:     {}        {}\n",
            self.mail_text(),
            format!("→ {MAIL_READ_COMMAND}").dimmed()
        ));
        out.push_str(&format!("working:  {}\n", self.working_text()));
        if let Some(goal) = self.goal_text() {
            out.push_str(&format!("goal:     {goal}\n"));
        }
        if let Some(purpose) = self.purpose_text() {
            out.push_str(&format!("purpose:  {purpose}\n"));
        }
        out.push_str(&format!("worktree: {}\n", self.worktree_text()));
        out
    }

    /// Flat `key: value` lines for a piped (agent) reader.
    pub(crate) fn render_toon(&self) -> String {
        let mut lines = vec![
            format!("handle: {}", self.handle),
            format!("role: {}", self.role),
            format!("agent_type: {}", self.agent_type),
            format!("session: {}", self.session_id.as_deref().unwrap_or("-")),
        ];
        if let Some(l) = &self.lease_id {
            lines.push(format!("lease: {l}"));
        }
        if let Some(node) = self.node_text() {
            lines.push(format!("node: {node}"));
        }
        lines.push(format!("mail: {} → {MAIL_READ_COMMAND}", self.mail_text()));
        lines.push(format!("working: {}", self.working_text()));
        if let Some(goal) = self.goal_text() {
            lines.push(format!("goal: {goal}"));
        }
        if let Some(purpose) = self.purpose_text() {
            lines.push(format!("purpose: {purpose}"));
        }
        lines.push(format!("worktree: {}", self.worktree_text()));
        let mut out = lines.join("\n");
        out.push('\n');
        out
    }
}

#[cfg(test)]
#[path = "tests/story_b6b_098_whoami_tests.rs"]
mod tests;
