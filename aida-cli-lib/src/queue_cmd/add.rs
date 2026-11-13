// trace:TASK-3426 | ai:codex
use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueueDestinationDetails {
    pub(crate) identity: String,
    pub(crate) queue: String,
    pub(crate) routed_role: String,
    pub(crate) observe_command: String,
    pub(crate) pickup_command: String,
}

pub(crate) fn shell_arg(raw: &str) -> String {
    if raw
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '/' | '='))
    {
        raw.to_string()
    } else {
        format!("'{}'", raw.replace('\'', "'\\''"))
    }
}

// trace:STORY-1002 | ai:codex
pub(crate) fn queue_destination_details(
    spec_id: &str,
    user_id: &str,
    queue_kind: &str,
    role: Option<&str>,
    pickup_available: bool,
) -> QueueDestinationDetails {
    let routed_role = match role.filter(|r| !r.trim().is_empty()) {
        Some(role) if role.eq_ignore_ascii_case("all") => "all".to_string(),
        Some(role) => canonical_role_name(role),
        None => "any".to_string(),
    };
    let observe_role = shell_arg(&routed_role);
    let observe_command = if queue_kind == "global-role" {
        format!("aida queue list --global --for {observe_role}")
    } else if routed_role == "all" {
        format!("aida queue list --user {} --all", shell_arg(user_id))
    } else {
        format!(
            "aida queue list --user {} --for {observe_role}",
            shell_arg(user_id)
        )
    };
    let pickup_command = if pickup_available {
        let mut cmd = format!("aida queue work {}", shell_arg(spec_id));
        if queue_kind != "global-role" {
            cmd.push_str(&format!(" --user {}", shell_arg(user_id)));
        }
        if routed_role != "any" && routed_role != "all" {
            cmd.push_str(&format!(" --role {}", shell_arg(&routed_role)));
        }
        cmd
    } else {
        "n/a (entry removed from queue)".to_string()
    };
    QueueDestinationDetails {
        identity: user_id.to_string(),
        queue: queue_kind.to_string(),
        routed_role,
        observe_command,
        pickup_command,
    }
}

pub(crate) fn role_is_stakeholder_only(role: &str) -> bool {
    matches!(canonical_role_name(role).as_str(), "guest" | "requester")
}

// trace:TASK-1261 | ai:codex
/// Launch guidance for least-privilege stakeholder personas. Kept here beside
/// the persona classifier so every launcher presents the same capability
/// envelope.
pub(crate) fn stakeholder_persona_guidance(role: &str) -> Option<&'static str> {
    match canonical_role_name(role).as_str() {
        "guest" => Some(
            "You are a least-privilege stakeholder persona, not a build seat. You may inspect the project with `aida why`, `aida list`, `aida status`, `aida graph`, `aida search`, and `aida digest`. Mutating, approval, queue, status-edit, merge, lease, and worktree operations are refused; the gate's hint directs you to ask an advisor. You do not hold a spec lease or implementation worktree.",
        ),
        "requester" => Some(
            "You are a least-privilege stakeholder persona, not a build seat. You may inspect the project with `aida why`, `aida list`, `aida status`, `aida graph`, `aida search`, and `aida digest`. You may also file intake with `aida add --type change-request|bug|user`; it always lands Draft and is tagged `intake:requester` for an advisor's grooming pass. Other mutating, approval, queue, status-edit, merge, lease, and worktree operations are refused; the gate's hint directs you to file a Draft request or ask an advisor. Nothing you file is worked until an advisor approves and routes it.",
        ),
        _ => None,
    }
}

// trace:STORY-1002 | ai:codex
pub(crate) fn queue_mutation_destination_json(
    action: &str,
    spec_id: &str,
    title: Option<&str>,
    details: &QueueDestinationDetails,
) -> serde_json::Value {
    serde_json::json!({
        "action": action,
        "spec_id": spec_id,
        "title": title,
        "destination": {
            "identity": details.identity,
            "queue": details.queue,
            "routed_role": details.routed_role,
            "observe_command": details.observe_command,
            "pickup_command": details.pickup_command,
        }
    })
}

// trace:STORY-1002 | ai:codex
pub(crate) fn queue_mutation_destination_human(
    summary: &str,
    details: &QueueDestinationDetails,
) -> String {
    [
        summary.to_string(),
        "  Destination:".to_string(),
        format!("    identity: {}", details.identity),
        format!("    queue: {}", details.queue),
        format!("    routed role: {}", details.routed_role),
        format!("    observe: {}", details.observe_command),
        format!("    pickup: {}", details.pickup_command),
    ]
    .join("\n")
}

// trace:STORY-1002 | ai:codex
pub(crate) fn print_queue_mutation_destination(
    action: &str,
    spec_id: &str,
    title: Option<&str>,
    summary: &str,
    details: &QueueDestinationDetails,
) -> Result<()> {
    if output_format_is_json() {
        println!(
            "{}",
            crate::cache_output::json_pretty(&queue_mutation_destination_json(
                action, spec_id, title, details
            ))?
        );
        return Ok(());
    }
    if matches!(output_format_override(), Some(OutputFormat::Toon)) {
        println!("{}", crate::toon::scalar("action", action));
        println!("{}", crate::toon::scalar("spec_id", spec_id));
        if let Some(title) = title {
            println!("{}", crate::toon::scalar("title", title));
        }
        println!("{}", crate::toon::scalar("identity", &details.identity));
        println!("{}", crate::toon::scalar("queue", &details.queue));
        println!(
            "{}",
            crate::toon::scalar("routed_role", &details.routed_role)
        );
        println!(
            "{}",
            crate::toon::scalar("observe_command", &details.observe_command)
        );
        println!(
            "{}",
            crate::toon::scalar("pickup_command", &details.pickup_command)
        );
        return Ok(());
    }

    println!("{}", queue_mutation_destination_human(summary, details));
    Ok(())
}

// trace:TASK-3426 | ai:codex
pub(super) fn handle_queue_command(
    cmd: &QueueCommand,
    storage: &Storage,
    store_path: &std::path::Path,
) -> Result<()> {
    let get_user = |user: &Option<String>| -> String { current_user_id(user.as_deref()) };
    match cmd {
        QueueCommand::Add {
            id,
            top,
            bottom: _,
            user,
            note,
            r#for,
            scope,
            for_session,
            no_scope,
            global,
            force,
        } => {
            if r#for
                .as_deref()
                .map(role_is_stakeholder_only)
                .unwrap_or(false)
            {
                anyhow::bail!(
                    "`{}` is a least-privilege stakeholder role, not a build-loop queue target. Route intake to advisor instead.",
                    r#for.as_deref().unwrap_or_default()
                );
            }
            // TASK-647 (ADR-3): queue-for-work is an advisor-authority act —
            // it commits a spec to the execution pipeline. A non-advisor,
            // non-TTY caller (headless agent, drain capture) is refused; they
            // file drafts and let the advisor triage + queue. Internal Rust
            // callers (orchestrator) use `storage.queue_add()` directly and
            // bypass this CLI gate. trace:TASK-647 | ai:claude
            //
            // BUG-631: scope the gate to DISPATCH-for-execution targets only.
            // Routing a draft `--for advisor` (or `--for human`/`--for reviewer`)
            // is a REQUEST for review/triage — open to any role, since it does
            // not dispatch execution and the advisor/reviewer still decides.
            // Only execution-dispatch routes (implementer, unknown/custom roles,
            // and the unrouted `--for any`/no-`--for` cases) stay gated.
            // trace:BUG-631 | ai:claude
            // Dispatch is routing, not disposition. Product, advisor, and
            // integrator seats (plus a live orchestrator) may do it without
            // acquiring advisor authority. trace:STORY-1353 | ai:codex
            if for_target_requires_dispatch_authority(r#for.as_deref())
                && !crate::gate::role_authority::has_dispatch_authority()
            {
                anyhow::bail!(
                    "queuing work for execution needs dispatch authority (product, advisor, or \
                     integrator role, or a live orchestrator). File the spec for advisor triage \
                     instead, or ask a dispatch-capable seat to route it. (Routing for review — \
                     `--for advisor`/`--for human`/`--for reviewer` — needs no dispatch authority.){}",
                    roleless_recovery_sentence()
                );
            }
            // BUG-498: queuing work is advisor-style — nudge the operator to
            // seat the advisor role if they're acting via an env prefix.
            maybe_hint_advisor_seat();
            // BUG-634: avoid a full-store scan (`storage.load()` parses every
            // YAML) on the queue write path. For the distributed (directory)
            // store, open a cache-backed backend for targeted single-spec
            // lookups; the deprecated centralized (.db) path keeps the full
            // load. trace:BUG-634 | ai:claude
            let targeted_backend: Option<aida_core::CachedGitBackend> = if store_path.is_dir() {
                let cache_path = aida_core::CachedGitBackend::default_cache_path(store_path);
                aida_core::CachedGitBackend::open(store_path, &cache_path).ok()
            } else {
                None
            };

            // Default routing: when no --for is given but the active session
            // has a role (AIDA_SESSION_ROLE), route to that role automatically.
            // Without this, `queue add X` produced an unrouted entry that
            // `queue next` (filtered to active role by default) wouldn't show
            // — surprising "queue is empty" right after queueing something.
            // Pass `--for any` to keep the unrouted behavior explicitly.
            // trace:BUG-18 | ai:claude
            let session_role_for_routing = std::env::var("AIDA_SESSION_ROLE")
                .ok()
                .filter(|s| !s.is_empty());
            let r#for: Option<String> = match r#for.as_deref() {
                Some("any") => None,
                // TASK-586 / TASK-747: canonicalize the route target on add so
                // `dialog`→`advisor` and `Human`/`HUMAN`→`human` route
                // consistently and surface together downstream.
                Some(role) => Some(canonical_role_name(role)),
                // No --for: doer roles queue into their own lane, but the
                // dispatch seats (advisor, human) queue work FOR the doers —
                // their unflagged adds route to implementer. Without this, an
                // advisor's plain `queue add` produces an entry every
                // implementer drain skips under the role-mismatch filter, and
                // the spec silently never drains (an explicit
                // `--for advisor` still routes to advisor and is skipped —
                // that's the intended meaning of explicit routing).
                // trace:BUG-862 | ai:claude
                None => session_role_for_routing
                    .as_deref()
                    .map(canonical_role_name)
                    .map(|role| match role.as_str() {
                        "advisor" | "human" => "implementer".to_string(),
                        _ => role,
                    }),
            };
            let user_id = queue_role_fallback::coordination_role_queue_user(
                user.is_some(),
                r#for.as_deref(),
                session_role_for_routing.as_deref(),
            )
            .unwrap_or_else(|| get_user(user));

            // TASK-1150: distinct-user identity guard. Adding to a queue keyed
            // by a genuinely-different user id than this shell's identity (e.g.
            // `--user user-b` from a `user-a` shell — not just a case variant)
            // silently crosses identities. Surface it (warn by default, refuse
            // when the operator opts in). No-op on the common same-identity add.
            //
            // BUG-900: advisor/human dispatch writes with no explicit `--user`
            // are intentionally keyed to `role:<target>`; skip the personal
            // identity guard for that derived shared role queue.
            // trace:TASK-1150 | ai:claude
            // trace:BUG-900 | ai:codex
            if !user_id.starts_with("role:") {
                identity_guard::enforce(&current_user_id(None), &user_id, "queue add")?;
            }

            // STORY-57: default scope routing. When adding inside a session
            // worktree without --scope or --no-scope, fill `for_scope` with
            // the active lease's scope so concurrent sessions sharing a
            // role don't see each other's work. --for-session is more
            // specific than --scope and overrides it for filtering, but we
            // keep both fields in the entry — the consumer side ANDs them.
            // trace:STORY-57 | ai:claude
            let active_lease_for_routing: Option<SessionLease> =
                std::env::current_dir().ok().and_then(|cwd| {
                    let project_root = storage
                        .path()
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| std::path::PathBuf::from("."));
                    active_lease_for_cwd(&project_root, &cwd)
                });
            let for_scope_routing: Option<String> = queue_add_for_scope_routing(
                *no_scope,
                scope.as_deref(),
                for_session.as_deref(),
                active_lease_for_routing.as_ref(),
            );
            let for_session_routing: Option<String> = for_session.clone();

            // Resolve requirement ID + the blocker-warning graph subset. The
            // distributed path resolves both via targeted lookups; the
            // centralized path falls back to a single full load. trace:BUG-634
            let (req_owned, warn_store): (aida_core::Requirement, aida_core::RequirementsStore) =
                if let Some(backend) = targeted_backend.as_ref() {
                    let resolved = if let Ok(uuid) = uuid::Uuid::parse_str(id) {
                        backend.get_requirement(&uuid)?
                    } else {
                        backend.get_requirement_unambiguous(id)? // trace:TASK-1468 | ai:claude
                    }
                    .ok_or_else(|| not_found::requirement_not_found(id, Some(storage.path())))?;
                    let warn_store = build_queue_warn_subset(backend, &resolved, storage, &user_id);
                    (resolved, warn_store)
                } else {
                    let store = storage.load()?;
                    let resolved = if let Ok(uuid) = uuid::Uuid::parse_str(id) {
                        store.requirements.iter().find(|r| r.id == uuid).cloned()
                    } else {
                        store.get_requirement_unambiguous(id)?.cloned() // trace:TASK-1468 | ai:claude
                    }
                    .ok_or_else(|| not_found::requirement_not_found(id, Some(storage.path())))?;
                    (resolved, store)
                };
            let req = &req_owned;

            // TASK-45: refuse to queue a Completed/Rejected req unless
            // --force. The intermediate state ("Completed item in
            // queue") is harmless but confuses agents and races with
            // bulk reclassification. The error message points at the
            // escape hatch. trace:TASK-45 | ai:claude
            if is_terminal_status(&req.status) && !*force {
                // BUG-81: surface the short id when one's been assigned.
                // trace:BUG-81 | ai:claude
                let display_id = req
                    .agreed_id
                    .as_deref()
                    .or(req.spec_id.as_deref())
                    .unwrap_or("?");
                // BUG-671: override flag on the FIRST line so agent mode (which
                // shows only the first line as the error summary) surfaces it.
                // trace:BUG-671 | ai:claude
                anyhow::bail!(
                    "{} is {} — re-queueing closed work is usually a mistake; pass --force \
                     to override.\n  Otherwise, file a new requirement that supersedes {}.",
                    display_id,
                    req.status,
                    display_id
                );
            }
            if let Some(reason) = queue_add_hidden_view_reason(req) {
                let display_id = req
                    .agreed_id
                    .as_deref()
                    .or(req.spec_id.as_deref())
                    .unwrap_or("?");
                anyhow::bail!(
                    "{} is {} — parked specs do not appear in the default queue. \
                     Clear that state first (`aida {} {}`), then queue it.",
                    display_id,
                    reason,
                    if reason == "deferred" {
                        "undefer"
                    } else {
                        "unarchive"
                    },
                    display_id
                );
            }

            let position = if *top {
                let entries = storage.queue_list(&user_id, true)?;
                entries.first().map(|e| e.position - 1000).unwrap_or(1000)
            } else {
                i64::MAX // sentinel: queue_add auto-assigns max+1000
            };

            let spec_id = req.spec_id.as_deref().unwrap_or("???");
            // BUG-81: short id for user-facing prints; `spec_id` stays
            // canonical for the activity-log key (consistency with edit/
            // show/comment events). trace:BUG-81 | ai:claude
            let display_id = req
                .agreed_id
                .as_deref()
                .or(req.spec_id.as_deref())
                .unwrap_or("???");

            // --global routes to ~/.aida/queue/<role>.yaml. The role comes
            // from --for, falling back to the active role. Refuse silently
            // if neither is set — global queues only make sense role-scoped.
            // trace:FR-1-012 | ai:claude
            if *global {
                let role = r#for
                    .clone()
                    .or_else(|| {
                        std::env::var("AIDA_SESSION_ROLE")
                            .ok()
                            .filter(|s| !s.is_empty())
                    })
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                        "--global requires --for <role> or an active role (AIDA_SESSION_ROLE). \
                         The global queue is keyed by role."
                    )
                    })?;
                let project_root = storage
                    .path()
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                let project_root = project_root.canonicalize().unwrap_or(project_root);
                let project_name = global_queue::project_name_for(&project_root);
                let position = if *top {
                    // trace:BUG-1682 | ai:codex
                    let existing = global_queue::load(&role)?;
                    existing.first().map(|e| e.position - 1000).unwrap_or(1000)
                } else {
                    i64::MAX
                };
                // Resolve i64::MAX to actual max+1000 inline (the local queue
                // path delegates this to the backend; we do it here ourselves).
                let position = if position == i64::MAX {
                    // trace:BUG-1682 | ai:codex
                    let existing = global_queue::load(&role)?;
                    existing.iter().map(|e| e.position).max().unwrap_or(0) + 1000
                } else {
                    position
                };
                let gentry = global_queue::GlobalQueueEntry {
                    requirement_id: req.id,
                    project_root,
                    project_name: project_name.clone(),
                    spec_id: req.spec_id.clone(),
                    // BUG-83: cache agreed_id too so the global queue
                    // can render the short id once one's been assigned.
                    // trace:BUG-83 | ai:claude
                    agreed_id: req.agreed_id.clone(),
                    title: Some(req.title.clone()),
                    position,
                    added_by: user_id.clone(),
                    added_at: chrono::Utc::now(),
                    note: note.clone(),
                    for_role: role.clone(),
                };
                global_queue::add(&role, gentry)?;
                // BUG-65: bump role activity on queue-add so statusline
                // tracks "started caring about this spec" alongside the
                // existing edit/show/comment events.
                // trace:BUG-65 | ai:claude
                record_role_activity(spec_id, "queue-add");
                let summary = format!(
                    "{} Added {} ({}) to {} {}",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    display_id.bold(),
                    req.title,
                    "global queue".cyan(),
                    format!("[role:{}, origin:{}]", role, project_name).dimmed()
                );
                let details = queue_destination_details(
                    display_id,
                    &format!("role:{role}"),
                    "global-role",
                    Some(&role),
                    true,
                );
                print_queue_mutation_destination(
                    "add",
                    display_id,
                    Some(&req.title),
                    &summary,
                    &details,
                )?;
                return Ok(());
            }

            // STORY-333: warn — never refuse — if this placement puts
            // the new entry ahead of a queued blocker. Operator may have
            // a reason (e.g. branching from the blocker's branch
            // intentionally), so the warning is informational only.
            // trace:STORY-333 | ai:claude
            warn_if_queued_ahead_of_blocker(req, position, &warn_store, storage, &user_id);

            // TASK-618: warn on the silent cross-machine collision hazard.
            // When the queue user_id is the BUG-89 "default" fallback and a
            // remote default.yaml already carries entries from a DIFFERENT
            // machine, two clones are about to write the same orphan-branch
            // file → merge conflict on the next sync rebase. Stamp this
            // clone's fingerprint so the next add can make the same check.
            // trace:TASK-618 | ai:claude
            let this_machine = hostname();
            if let Ok(existing) = storage.queue_list(&user_id, true) {
                if let Some(other) = default_queue_collision_fingerprint(
                    &user_id,
                    &this_machine,
                    existing.iter().map(|e| e.added_by_machine.as_deref()),
                ) {
                    eprintln!(
                        "{} This queue is using the shared '{}' user id and already has \
                         entries from another machine ('{}'). Two machines writing the \
                         same queue can collide on the next sync. Set a distinct user id \
                         per machine (export AIDA_USER=<name>) to keep queues separate.",
                        "warning:".yellow().bold(),
                        "default".bold(),
                        other.bold(),
                    );
                }
            }

            let entry = aida_core::QueueEntry {
                user_id: user_id.clone(),
                requirement_id: req.id,
                position,
                added_by: user_id.clone(),
                note: note.clone(),
                added_at: chrono::Utc::now(),
                for_role: r#for.clone(),
                for_scope: for_scope_routing.clone(),
                for_session: for_session_routing.clone(),
                added_by_machine: Some(this_machine),
            };
            storage.queue_add(entry)?;
            // BUG-65: bump role activity on queue-add so statusline tracks
            // "started caring about this spec" alongside edit/show/comment.
            // trace:BUG-65 | ai:claude
            record_role_activity(spec_id, "queue-add");

            // trace:STORY-57 | ai:claude
            let mut routing_parts: Vec<String> = Vec::new();
            if let Some(r) = &r#for {
                routing_parts.push(format!("for:{}", r));
            }
            if let Some(s) = &for_scope_routing {
                routing_parts.push(format!("@{}", s));
            }
            if let Some(s) = &for_session_routing {
                routing_parts.push(format!("session:{}", &s[..s.len().min(8)]));
            }
            let routing = if routing_parts.is_empty() {
                format!(" [queue:{}]", user_id).cyan().to_string()
            } else {
                format!(" [queue:{} {}]", user_id, routing_parts.join(" "))
                    .cyan()
                    .to_string()
            };
            let summary = format!(
                "{} Added {} ({}) to queue{}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                display_id.bold(),
                req.title,
                routing
            );
            let details =
                queue_destination_details(display_id, &user_id, "local", r#for.as_deref(), true);
            print_queue_mutation_destination(
                "add",
                display_id,
                Some(&req.title),
                &summary,
                &details,
            )?;
        }
        _ => unreachable!("dispatcher selected another queue subcommand"),
    }
    Ok(())
}
