//! `aida team` command cluster (STORY-647 team / RBAC management).
//!
//! The team roster view (`aida team`), the per-user role roster writes
//! (`set-role` / `unset-role`), and the caller's effective-role lookup
//! (`my-role`). Extracted verbatim from `main.rs` (SPIKE-78); no behavior
//! change. The RBAC *gate* (`enforce_team_gate`) is shared with the queue /
//! merge-gate / drain / integrate paths and stays in `main.rs`, reached via
//! `crate::`. The `team` model / roster store lives in the `crate::team`
//! module (and `aida_core::team`).

use anyhow::Result;
use colored::Colorize;

use crate::*;

/// `aida team` — the roster of every node/clone sharing this store, joined with
/// the `coordination/` claims each currently holds.
// trace:STORY-640 | ai:claude
pub(crate) fn handle_team_command(store_path: &std::path::Path, json: bool) -> Result<()> {
    let our_clone = team::our_clone_path(store_path);
    let members = team::build_team_view(store_path, &our_clone);
    // STORY-646: per-user roles from the RBAC roster (`registry/team.toml`).
    let roles = team::roles_by_user(store_path);
    let now = chrono::Utc::now();

    if json {
        let nodes: Vec<serde_json::Value> = members
            .iter()
            .map(|m| {
                serde_json::json!({
                    "node_id": m.entry.id,
                    // STORY-652: friendly name + owner string (backfilled for
                    // pre-STORY-652 rows). trace:STORY-652
                    "name": m.entry.display_name(),
                    "owner": m.entry.owner(),
                    "host": m.entry.hostname,
                    "email": m.entry.email,
                    "clone_path": m.entry.clone_path
                        .as_ref()
                        .map(|p| p.display().to_string()),
                    "registered": m.entry.registered.to_rfc3339(),
                    "active_claims": m.active_claims,
                    "is_self": m.is_self,
                })
            })
            .collect();
        let role_rows: Vec<serde_json::Value> = roles
            .iter()
            .map(|(user, role)| serde_json::json!({ "user": user, "role": role }))
            .collect();
        let out = serde_json::json!({ "nodes": nodes, "roles": role_rows });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    if members.is_empty() {
        println!("{}", "Team roster".bold());
        println!();
        println!(
            "  No nodes registered yet. Claim one with {} (or {} for a full bootstrap).",
            "aida node acquire".cyan(),
            "aida init".cyan()
        );
        return Ok(());
    }

    println!("{}", "Team roster".bold());
    println!();
    // STORY-652: friendly name column (stored name, else backfilled default).
    println!(
        "{:<8} {:<20} {:<14} {:<22} {:<12} claims",
        "node", "name", "host", "email", "registered"
    );
    println!("{}", "─".repeat(86));
    for m in &members {
        let marker = if m.is_self { " (you)" } else { "" };
        let node_label = format!("{}{}", m.entry.id, marker);
        let name = m.entry.display_name();
        let email = m.entry.email.as_deref().unwrap_or("-");
        let registered = m
            .entry
            .registered
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d")
            .to_string();
        let claims = if m.active_claims.is_empty() {
            "-".dimmed().to_string()
        } else {
            m.active_claims.join(", ").yellow().to_string()
        };
        println!(
            "{:<8} {:<20} {:<14} {:<22} {:<12} {}",
            truncate(&node_label, 8),
            truncate(&name, 20),
            truncate(&m.entry.hostname, 14),
            truncate(email, 22),
            registered,
            claims,
        );
        if let Some(p) = &m.entry.clone_path {
            println!("  {}", p.display().to_string().dimmed());
        }
    }
    let _ = now; // reserved for a future last-seen column
    println!();
    let n = members.len();
    if n == 1 {
        println!(
            "  {} node registered. {}",
            n,
            "Solo store — add a teammate with their own clone + `aida node acquire`.".dimmed()
        );
    } else {
        println!("  {} nodes registered.", n.to_string().bold());
    }

    // TASK-845: the person view — one row per CANONICAL person, collapsing a
    // human's several clones/owner-strings (case-variants via TASK-951, plus the
    // operator-curated alias map) into a single member. Surfaced only when the
    // person count differs from the node count (i.e. some collapse happened) so
    // the table stays uncluttered on a one-node-per-person store.
    let people = aida_core::team::build_team_members(store_path);
    if people.len() < members.len() {
        println!();
        println!("{}", "People".bold());
        println!();
        for p in &people {
            let hosts = if p.hosts.is_empty() {
                "-".dimmed().to_string()
            } else {
                p.hosts.join(", ")
            };
            let role_suffix = p
                .role
                .as_deref()
                .map(|r| format!("  [{}]", r.cyan()))
                .unwrap_or_default();
            println!(
                "  {:<24} {}{}",
                p.display_label.bold(),
                hosts.dimmed(),
                role_suffix
            );
        }
        println!();
        println!(
            "  {}",
            "One row per person — clones/aliases of the same human are collapsed. \
             Link aliases with `aida identity link <a> <b>`."
                .dimmed()
        );
    }

    // STORY-646: per-user role roster (RBAC guardrail). Only shown when at
    // least one role is recorded — a fresh store has none and stays uncluttered.
    if !roles.is_empty() {
        println!();
        println!("{}", "Roles".bold());
        println!();
        let me = current_user_id(None);
        for (user, role) in &roles {
            let you = if *user == me { " (you)" } else { "" };
            println!("  {:<20} {}{}", user, role.cyan(), you.dimmed());
        }
        println!();
        println!(
            "  {}",
            "Roles are a guardrail, not security — anyone with store push access can edit \
             directly. Manage with `aida team set-role`."
                .dimmed()
        );
    }
    Ok(())
}

/// STORY-646: the canonical role names a `set-role` write is allowed to record —
/// the core roles plus any role files installed under `~/.aida/roles/` (or the
/// project roles dir). Validating against this catches a typo'd role before it
/// lands in the durable roster. Best-effort: an unreadable roles dir still
/// admits the core roles.
// trace:STORY-646 | ai:claude
fn known_role_names() -> std::collections::BTreeSet<String> {
    let mut names: std::collections::BTreeSet<String> = ["advisor", "implementer", HUMAN_ROUTE]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if let Ok(root) = find_project_root() {
        if let Ok(roles) = list_roles(&root) {
            for r in roles {
                names.insert(canonical_role_name(&r.name));
            }
        }
    }
    names
}

/// `aida team set-role <user> --role <role>` (STORY-646). Validate the role
/// against the known set, write `registry/team.toml` with a CAS push, and print
/// the guardrail-not-security caveat once.
// trace:STORY-646 | ai:claude
pub(crate) fn handle_team_set_role(
    store_path: &std::path::Path,
    user: &str,
    role: &str,
) -> Result<()> {
    let canonical = canonical_role_name(role.trim());
    if canonical.is_empty() {
        anyhow::bail!("a role name is required (e.g. --role advisor)");
    }
    let known = known_role_names();
    if !known.contains(&canonical) {
        anyhow::bail!(
            "unknown role `{}`. Known roles: {}. (Install more with `aida role scaffold`.)",
            role,
            known.iter().cloned().collect::<Vec<_>>().join(", ")
        );
    }

    team::set_role_cas(store_path, user, &canonical)?;

    println!(
        "{} set team role: {} = {}",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        user.bold(),
        canonical.cyan()
    );
    // The caveat, surfaced on every write (cheap, and the honest model matters).
    println!();
    println!(
        "  {}",
        "Guardrail, not security: this records team structure and stops accidental \
         role-violating edits via the CLI, but anyone with push access to the store can \
         still edit any spec directly with raw git. It is NOT an access-control boundary."
            .dimmed()
    );
    Ok(())
}

/// `aida team unset-role <user>` (STORY-654): remove a member entry from the
/// roster (CAS push), to clean stray / duplicate keys. Friendly no-op if the
/// user isn't present.
// trace:STORY-654 | ai:claude
pub(crate) fn handle_team_unset_role(store_path: &std::path::Path, user: &str) -> Result<()> {
    let removed = team::unset_role_cas(store_path, user)?;
    if removed {
        println!(
            "{} removed team role entry: {}",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            user.bold()
        );
    } else {
        println!("No roster entry for {} — nothing to remove.", user.bold());
    }
    Ok(())
}

/// `aida team my-role` (STORY-646): show the caller's effective role and where
/// it resolved from (roster / env / default).
// trace:STORY-646 | ai:claude
pub(crate) fn handle_team_my_role(store_path: &std::path::Path, json: bool) -> Result<()> {
    let user = current_user_id(None);
    let (role, source) = team::effective_role_for_user(store_path, &user);
    let source_str = match source {
        team::RoleSource::Roster => "roster",
        team::RoleSource::Env => "env (AIDA_SESSION_ROLE)",
        team::RoleSource::Default => "default",
    };
    if json {
        let out = serde_json::json!({
            "user": user,
            "role": role,
            "source": match source {
                team::RoleSource::Roster => "roster",
                team::RoleSource::Env => "env",
                team::RoleSource::Default => "default",
            },
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    println!(
        "{} {} (from {})",
        format!("{}:", user).dimmed(),
        role.cyan().bold(),
        source_str
    );
    Ok(())
}
