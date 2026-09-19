use anyhow::{bail, Result};

use aida_core::{
    get_type_protocol, resolve_protocol, seed_missing_type_protocols, Requirement, RequirementType,
    RequirementsStore, Storage,
};

use crate::cli::ProtocolCommand;

const TYPES: &[&str] = &["spike", "bug", "story", "task", "decision", "doc"];

// trace:STORY-1221 | ai:codex
pub(crate) fn handle_protocol_command(cmd: &ProtocolCommand, storage: &Storage) -> Result<()> {
    let mut store = storage.load()?;
    match cmd {
        ProtocolCommand::List => {
            for kind in TYPES {
                if let Some(protocol) = get_type_protocol(&store, kind) {
                    println!("{:<10} {}", kind, protocol.meta_id);
                }
            }
        }
        ProtocolCommand::Show { req_type, lane } => {
            let Some(protocol) = resolve_protocol(&store, req_type, lane.as_deref()) else {
                bail!("no type protocol found for `{req_type}`; run `aida protocol list`");
            };
            if lane.is_some() && protocol.lane_protocol.is_none() {
                bail!(
                    "no lane protocol found for `{}`",
                    lane.as_deref().unwrap_or_default()
                );
            }
            print!("{}", protocol.render());
        }
        ProtocolCommand::Seed => {
            let seeded = seed_missing_type_protocols(&mut store);
            if seeded > 0 {
                storage.save(&store)?;
            }
            println!("seeded {seeded} missing type protocol(s)");
        }
    }
    Ok(())
}

pub(crate) fn seed_missing_protocols(storage: &Storage) -> Result<usize> {
    let mut store = storage.load()?;
    let seeded = seed_missing_type_protocols(&mut store);
    if seeded > 0 {
        storage.save(&store)?;
    }
    Ok(seeded)
}

pub(crate) fn resolved_protocol_for_requirement(
    store: &RequirementsStore,
    req: &Requirement,
) -> Option<aida_core::ResolvedProtocol> {
    let lane = if crate::presence::is_keystone_class(
        &req.req_type.to_string(),
        req.tags.iter().map(String::as_str),
    ) {
        Some("keystone")
    } else if req.req_type == RequirementType::Spike {
        Some("research")
    } else if req.req_type == RequirementType::Doc {
        Some("docs")
    } else {
        None
    };
    resolve_protocol(store, &req.req_type.to_string(), lane)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProtocolPickupSurface {
    QueueWork,
    Do,
    WorktreeEnter,
}

/// Return the protocol block injected by each interactive pickup surface.
/// Queue work also uses this for headless phases; `aida do` intentionally lets
/// the child queue/zen driver perform the one headless injection.
// trace:STORY-1221 trace:TASK-1278 | ai:codex
pub(crate) fn pickup_protocol_block(
    store: &RequirementsStore,
    req: &Requirement,
    surface: ProtocolPickupSurface,
    headless: bool,
    reviewer: bool,
) -> Option<String> {
    let inject = match surface {
        ProtocolPickupSurface::QueueWork => headless || !reviewer,
        ProtocolPickupSurface::Do => !headless,
        ProtocolPickupSurface::WorktreeEnter => true,
    };
    inject
        .then(|| resolved_protocol_for_requirement(store, req))
        .flatten()
        .map(|protocol| protocol.pickup_block())
}

/// Resolve the per-turn reminder only when the current worktree is covered by
/// an active lease. Removing the lease on `queue done`/release makes this
/// return `None` without relying on lifecycle status or a stale environment.
// trace:STORY-1221 | ai:codex
pub(crate) fn leased_protocol_notice<'a>(
    store: &RequirementsStore,
    current_worktree: &std::path::Path,
    leases: impl IntoIterator<Item = (&'a std::path::Path, &'a str)>,
) -> Option<String> {
    let scope = leases
        .into_iter()
        .find(|(worktree, _)| current_worktree.starts_with(worktree))
        .map(|(_, scope)| scope)?;
    let req = store.requirements.iter().find(|req| {
        req.display_id().eq_ignore_ascii_case(scope)
            || req
                .spec_id
                .as_deref()
                .is_some_and(|id| id.eq_ignore_ascii_case(scope))
    })?;
    resolved_protocol_for_requirement(store, req)
        .map(|protocol| protocol.type_protocol.notice_line())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aida_core::{seed_missing_type_protocols, Requirement};

    fn protocol_fixture() -> (RequirementsStore, Requirement) {
        let mut store = RequirementsStore::new();
        seed_missing_type_protocols(&mut store);
        let mut req = Requirement::new("Protocol fixture".into(), "test".into());
        req.req_type = RequirementType::Bug;
        req.spec_id = Some("BUG-42".into());
        req.agreed_id = Some("BUG-42".into());
        (store, req)
    }

    #[test]
    fn protocol_injection_covers_queue_work_do_and_worktree_enter() {
        let (store, req) = protocol_fixture();
        for surface in [
            ProtocolPickupSurface::QueueWork,
            ProtocolPickupSurface::Do,
            ProtocolPickupSurface::WorktreeEnter,
        ] {
            let block = pickup_protocol_block(&store, &req, surface, false, false)
                .unwrap_or_else(|| panic!("{surface:?} omitted its protocol"));
            assert!(block.contains("Resolved protocol"), "{surface:?}: {block}");
            assert!(block.contains("META-"), "{surface:?}: {block}");
        }

        assert!(
            pickup_protocol_block(&store, &req, ProtocolPickupSurface::QueueWork, true, true)
                .is_some()
        );
        assert!(
            pickup_protocol_block(&store, &req, ProtocolPickupSurface::Do, true, false).is_none()
        );
    }

    #[test]
    fn protocol_notice_tracks_current_worktree_lease_and_disappears_after_release() {
        let (mut store, req) = protocol_fixture();
        store.requirements.push(req);
        let current = std::path::Path::new("/tmp/project-worktrees/bug-42");
        let other = std::path::Path::new("/tmp/project-worktrees/bug-99");

        assert!(leased_protocol_notice(&store, current, [(other, "BUG-42")]).is_none());
        let notice = leased_protocol_notice(&store, current, [(current, "BUG-42")])
            .expect("current worktree lease should inject the protocol notice");
        assert!(notice.contains("protocol: bug"), "{notice}");

        let released: [(&std::path::Path, &str); 0] = [];
        assert!(leased_protocol_notice(&store, current, released).is_none());
    }
}
