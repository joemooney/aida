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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
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
            unread: c.unread,
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
    /// This session holds a lease (or registry claim) on `spec`.
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

/// Gather the view. `main_root` is the main checkout (registry, leases,
/// store, cache); `worktree` is the checkout this process runs in.
pub(crate) fn collect(
    main_root: &Path,
    worktree: &Path,
    facts: &MailIdentityFacts,
    now: DateTime<Utc>,
) -> WhoamiView {
    let store = main_root.join(".aida-store");
    let aliases = aida_core::node_alias::NodeAliasMap::load(&store);
    let entry = crate::agent_handle::current_session_entry_for_cwd();

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
    let session_id = entry
        .as_ref()
        .and_then(|e| e.native_session_id.clone().or(e.claude_session_id.clone()))
        .map(|s| s.chars().take(8).collect())
        .or_else(|| env_nonempty("AIDA_SESSION_ID"));

    let limits = aida_core::mailbox_logical::ReadLimits::with_deadline(
        std::time::Instant::now() + MAIL_READ_BUDGET,
    );
    let mail = crate::collect_mail_channel(main_root, None, facts, &limits).into();

    let canonical = aida_core::node_alias::canonical_ids();
    let working = match current_work(main_root, worktree, now) {
        Some((spec, lease, age_secs)) => Working::Spec {
            spec: aliases.display(&spec, canonical),
            lease,
            age_secs,
        },
        None => match entry.as_ref().and_then(|e| e.current_spec.clone()) {
            Some(spec) => Working::Spec {
                spec: aliases.display(&spec, canonical),
                lease: None,
                age_secs: None,
            },
            None => Working::Idle {
                next: queue_head(main_root, &role, &aliases),
            },
        },
    };

    let state = crate::gate::mandate::current_state();
    let mandate = match &state {
        crate::gate::mandate::MandateState::Active(_) => crate::mandate::status_line(
            &state,
            now,
            &crate::mandate_gates::describe(Some(main_root)),
        ),
        _ => None,
    };
    let purpose = env_nonempty("AIDA_SESSION_PURPOSE");
    let goal = (mandate.is_some() || purpose.is_some()).then_some(GoalLine { mandate, purpose });

    WhoamiView {
        handle: resolved_handle(facts),
        role,
        agent_type,
        session_id,
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
        None if !head.is_empty() => Some(format!("detached {}", &head[..head.len().min(8)])),
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

/// The spec this session holds a lease on: the lease named by
/// `AIDA_SESSION_ID`, else the lease whose worktree holds `worktree`.
fn current_work(
    main_root: &Path,
    worktree: &Path,
    now: DateTime<Utc>,
) -> Option<(String, Option<String>, Option<i64>)> {
    let leases = crate::list_leases(main_root);
    let by_id = env_nonempty("AIDA_SESSION_ID")
        .and_then(|sid| leases.iter().find(|l| l.id == sid).cloned());
    let lease = by_id.or_else(|| crate::active_lease_for_cwd(main_root, worktree))?;
    let age = (now - lease.started_at).num_seconds().max(0);
    Some((lease.scope, Some(lease.id), Some(age)))
}

/// The head of `role`'s queue for this user: `<ID> <title>`, resolved from
/// the queue YAML and a read-only cache lookup. `None` on any miss.
fn queue_head(
    main_root: &Path,
    role: &str,
    aliases: &aida_core::node_alias::NodeAliasMap,
) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Row {
        requirement_id: String,
        #[serde(default)]
        position: i64,
        #[serde(default)]
        for_role: Option<String>,
    }
    let store = main_root.join(".aida-store");
    let user = aida_core::db::resolve_queue_user(&store, &crate::current_user_id(None));
    let text =
        std::fs::read_to_string(store.join("registry/queues").join(format!("{user}.yaml"))).ok()?;
    let rows: Vec<Row> = serde_yaml::from_str(&text).ok()?;
    let head = rows
        .into_iter()
        .filter(|r| r.for_role.as_deref().unwrap_or("implementer") == role)
        .min_by_key(|r| r.position)?;
    let cache = main_root.join(".aida/cache.db");
    let conn =
        rusqlite::Connection::open_with_flags(&cache, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()?;
    let (id, title): (String, String) = conn
        .query_row(
            "SELECT COALESCE(agreed_id, spec_id, ''), title \
             FROM requirements_cache WHERE id = ?1",
            [head.requirement_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok()?;
    (!id.is_empty()).then(|| {
        format!(
            "{} {}",
            aliases.display(&id, aida_core::node_alias::canonical_ids()),
            title
        )
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
            } => match (lease, age_secs) {
                (Some(_), Some(age)) => format!("{spec} (lease {})", format_age(*age)),
                _ => format!("{spec} (claim)"),
            },
            Working::Idle { next: Some(next) } => format!("idle · next: {next}"),
            Working::Idle { next: None } => "idle".to_string(),
        }
    }

    fn goal_text(&self) -> Option<String> {
        let g = self.goal.as_ref()?;
        let parts: Vec<String> = g
            .mandate
            .iter()
            .cloned()
            .chain(g.purpose.iter().map(|p| format!("purpose: {p}")))
            .collect();
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    fn worktree_text(&self) -> String {
        match &self.worktree.branch {
            Some(b) => format!("{} ({b})", self.worktree.path),
            None => self.worktree.path.clone(),
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
        if let Some(node) = self.node_text() {
            lines.push(format!("node: {node}"));
        }
        lines.push(format!("mail: {} → {MAIL_READ_COMMAND}", self.mail_text()));
        lines.push(format!("working: {}", self.working_text()));
        if let Some(goal) = self.goal_text() {
            lines.push(format!("goal: {goal}"));
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
